//! Symmetric per-coordinate int8 quantization over unit-normalized rows.
//!
//! Calibration fixes one positive finite f32 scale per coordinate, `s_j = max_abs_j / 127`, computed once in f32.
//! Encoding widens `x_j` and `s_j` to f64, divides, rounds ties to even, clamps to `[-127, 127]`, and counts every clamp; `-128` is never produced.
//! Scoring sums `(s_j * s_j) * (c_query_j * c_doc_j)` in f64 in increasing coordinate order with the integer product formed in i32, so the same scales and codes yield the same f64 everywhere.

use sha2::{Digest, Sha256};

use super::codec::{self, RowLayout, RowRejection};
use super::score::BLOCK_ROWS;

/// The recipe every persisted code was produced under; a new recipe is a new variant, never a reinterpretation of old bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarRecipe {
    SymmetricInt8V1,
}

impl ScalarRecipe {
    pub const fn id(self) -> &'static str {
        match self {
            Self::SymmetricInt8V1 => "scalar-int8-symmetric.v1",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "scalar-int8-symmetric.v1" => Some(Self::SymmetricInt8V1),
            _ => None,
        }
    }
}

pub const CODE_MAX: i8 = 127;
pub const CODE_MIN: i8 = -127;

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum CalibrationRejection {
    #[error("the layout admits no generation: {0}")]
    Layout(RowRejection),
    #[error("no calibration rows were supplied")]
    NoRows,
    #[error("calibration row {index}: {rejection}")]
    Row {
        index: usize,
        rejection: RowRejection,
    },
    #[error("coordinate {coordinate} has a nonzero maximum whose scale underflows to zero")]
    ScaleUnderflow { coordinate: usize },
}

/// Why persisted scale or code bytes are not a member of the recipe.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum ScalarBytesRejection {
    #[error("{bytes} bytes is not a whole number of f32 words")]
    TruncatedWord { bytes: usize },
    #[error("{actual} coordinates were supplied, not {expected}")]
    Dimension { expected: u32, actual: usize },
    #[error("scale {coordinate} is not a positive finite number")]
    NotPositiveFinite { coordinate: usize },
    #[error("code {coordinate} is the reserved value -128")]
    ReservedCode { coordinate: usize },
}

/// One positive finite f32 per coordinate.
#[derive(Clone, PartialEq)]
pub struct Scales {
    scales: Vec<f32>,
    /// `f64(s_j) * f64(s_j)`, the weight [`weighted_dot`] applies at coordinate `j`; squaring a widened f32 is exact in f64.
    weights: Vec<f64>,
}

impl std::fmt::Debug for Scales {
    /// Scales derive from row content and stay out of diagnostics.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scales")
            .field("dimension", &self.scales.len())
            .finish()
    }
}

impl Scales {
    /// Subnormal positive scales are admitted: their squares stay normal in f64 and every quotient stays finite.
    pub fn from_values(scales: Vec<f32>, dimension: u32) -> Result<Self, ScalarBytesRejection> {
        if scales.len() != dimension as usize {
            return Err(ScalarBytesRejection::Dimension {
                expected: dimension,
                actual: scales.len(),
            });
        }
        if let Some(coordinate) = scales
            .iter()
            .position(|scale| !(scale.is_finite() && *scale > 0.0))
        {
            return Err(ScalarBytesRejection::NotPositiveFinite { coordinate });
        }
        Ok(Self::weighted(scales))
    }

    fn weighted(scales: Vec<f32>) -> Self {
        let weights = scales
            .iter()
            .map(|scale| f64::from(*scale) * f64::from(*scale))
            .collect();
        Self { scales, weights }
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.scales
    }

    /// Four little-endian bytes per scale, the same word encoding as a row.
    pub fn encode(&self) -> Vec<u8> {
        codec::encode(&self.scales)
    }

    pub fn decode(bytes: &[u8], dimension: u32) -> Result<Self, ScalarBytesRejection> {
        let values = codec::decode_words(bytes).map_err(|rejection| match rejection {
            RowRejection::TruncatedWord { bytes } => ScalarBytesRejection::TruncatedWord { bytes },
            other => unreachable!("decode_words rejects only a trailing partial word: {other}"),
        })?;
        if values.len() != dimension as usize {
            return Err(ScalarBytesRejection::Dimension {
                expected: dimension,
                actual: values.len(),
            });
        }
        Self::from_values(values.collect(), dimension)
    }

    /// SHA-256 of the encoded scales, for binding a calibration to the generation that carries it.
    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(self.encode()).into()
    }
}

/// The recipe, the rows it saw, and the digest of the scales it produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibrationIdentity {
    pub recipe: ScalarRecipe,
    pub calibrated_rows: u64,
    pub scales_digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Calibration {
    pub scales: Scales,
    pub identity: CalibrationIdentity,
}

/// `max_abs_j / 127` is computed once in f32 so the stored scale is the single correctly rounded quotient; an all-zero coordinate takes scale 1.
///
/// # Errors
///
/// A layout that is not a generation predicate, no rows, a row outside `layout`, or a nonzero coordinate whose scale rounds to zero.
pub fn calibrate<'a>(
    layout: &RowLayout,
    rows: impl IntoIterator<Item = &'a [f32]>,
) -> Result<Calibration, CalibrationRejection> {
    let mut calibrator = Calibrator::new(layout)?;
    for row in rows {
        calibrator.push(row)?;
    }
    calibrator.finish()
}

/// [`calibrate`] one row at a time, so a caller streaming rows from a file holds one row, not all of them.
pub struct Calibrator {
    layout: RowLayout,
    max_abs: Vec<f32>,
    count: u64,
}

impl Calibrator {
    /// # Errors
    ///
    /// A layout that is not a generation predicate.
    pub fn new(layout: &RowLayout) -> Result<Self, CalibrationRejection> {
        layout.check().map_err(CalibrationRejection::Layout)?;
        Ok(Self {
            layout: *layout,
            max_abs: vec![0.0f32; layout.dimension as usize],
            count: 0,
        })
    }

    /// # Errors
    ///
    /// A row outside the layout, reported at its index among the rows pushed.
    pub fn push(&mut self, row: &[f32]) -> Result<(), CalibrationRejection> {
        let index = usize::try_from(self.count).unwrap_or(usize::MAX);
        codec::validate(row, &self.layout)
            .map_err(|rejection| CalibrationRejection::Row { index, rejection })?;
        self.push_validated(row);
        Ok(())
    }

    /// [`Self::push`] for a `row` that [`codec::validate`] has accepted under this layout.
    pub fn push_validated(&mut self, row: &[f32]) {
        debug_assert_eq!(row.len(), self.max_abs.len());
        for (max, value) in self.max_abs.iter_mut().zip(row) {
            *max = max.max(value.abs());
        }
        self.count += 1;
    }

    /// # Errors
    ///
    /// No rows pushed, or a nonzero coordinate whose scale rounds to zero.
    pub fn finish(self) -> Result<Calibration, CalibrationRejection> {
        if self.count == 0 {
            return Err(CalibrationRejection::NoRows);
        }
        let mut scales = Vec::with_capacity(self.max_abs.len());
        for (coordinate, max) in self.max_abs.into_iter().enumerate() {
            let scale = if max == 0.0 { 1.0 } else { max / 127.0 };
            if scale == 0.0 {
                return Err(CalibrationRejection::ScaleUnderflow { coordinate });
            }
            scales.push(scale);
        }
        let scales = Scales::weighted(scales);
        let identity = CalibrationIdentity {
            recipe: ScalarRecipe::SymmetricInt8V1,
            calibrated_rows: self.count,
            scales_digest: scales.digest(),
        };
        Ok(Calibration { scales, identity })
    }
}

/// The codes of one row and how many coordinates were clamped into range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub codes: Vec<i8>,
    pub clipped: u32,
}

/// Each quotient is formed in f64 from the f32 value and scale, rounded ties to even, then clamped; the scale is never changed to fit the value.
/// Scales of another dimension than the layout are a wiring error and panic, like unequal code lengths in [`weighted_dot`].
///
/// # Errors
///
/// A layout that is not a generation predicate, or a row outside it.
pub fn encode(layout: &RowLayout, scales: &Scales, row: &[f32]) -> Result<Encoded, RowRejection> {
    let mut codes = Vec::new();
    let clipped = encode_into(layout, scales, row, &mut codes)?;
    Ok(Encoded { codes, clipped })
}

/// [`encode`] that refills `codes` with one code per coordinate, for buffer reuse across rows, and returns the number of clamped coordinates.
///
/// # Errors
///
/// The same as [`encode`].
pub fn encode_into(
    layout: &RowLayout,
    scales: &Scales,
    row: &[f32],
    codes: &mut Vec<i8>,
) -> Result<u32, RowRejection> {
    layout.check()?;
    codec::validate(row, layout)?;
    Ok(encode_validated_into(layout, scales, row, codes))
}

/// [`encode_into`] for a `row` that [`codec::validate`] has accepted under `layout`.
pub fn encode_validated_into(
    layout: &RowLayout,
    scales: &Scales,
    row: &[f32],
    codes: &mut Vec<i8>,
) -> u32 {
    debug_assert_eq!(row.len(), layout.dimension as usize);
    assert_eq!(
        scales.scales.len(),
        layout.dimension as usize,
        "scales of one calibration match the layout"
    );
    codes.clear();
    codes.resize(row.len(), 0);
    quotient_codes(row, &scales.scales, codes)
}

fn quotient_codes(row: &[f32], scales: &[f32], codes: &mut [i8]) -> u32 {
    let mut clipped = 0u32;
    for ((code, value), scale) in codes.iter_mut().zip(row).zip(scales) {
        let rounded = (f64::from(*value) / f64::from(*scale)).round_ties_even();
        let clamped = rounded.clamp(f64::from(CODE_MIN), f64::from(CODE_MAX));
        clipped += u32::from(clamped != rounded);
        // A finite value over a positive finite scale is a finite quotient, and the clamp bounds it to i8, so the cast is exact.
        *code = clamped as i8;
    }
    clipped
}

/// Coordinates [`Encoder`] codes together; a block whose products are all clear of every rounding midpoint stays on the product path.
const ENCODE_BLOCK: usize = 32;

/// A product farther than this from its nearest integer lies within `2^-14` of a midpoint between two integers, where the product may round otherwise than the quotient.
const NEAR_HALF: f32 = 0.5 - 1.0 / 16384.0;

/// Adding and subtracting `1.5 * 2^23` rounds an f32 of magnitude at most `2^22` to an integer, ties to even, in the default rounding mode.
const ROUNDING_BIAS: f32 = 12_582_912.0;

/// Checks stored codes against [`encode_validated_into`] for many rows under one set of scales.
///
/// Each coordinate is multiplied by `r = 1 / s` rounded to f32 once per encoder. With `r` normal, `p = v * r` in f32 lies within `2^-22.9 * |v / s|` of `v / s`, or below `2^-126` with `v / s` when the product leaves the normal range, where both round to zero.
/// The exact quotient of two f32 values is a midpoint between two integers or at least `2^-26` from every such midpoint, so the f64 quotient of [`encode_validated_into`] rounds as the exact one does.
/// Up to 128 in magnitude the product lies within `2^-15.9` of the quotient, so a product farther than `2^-14` from every midpoint rounds as the quotient does, and a product past 128 clamps to the code the quotient clamps to; a block holding a product near a midpoint takes the f64 quotient.
pub struct Encoder<'a> {
    scales: &'a Scales,
    /// Empty when some reciprocal is not a normal f32; every coordinate then takes the f64 quotient.
    reciprocals: Vec<f32>,
}

impl<'a> Encoder<'a> {
    pub fn new(scales: &'a Scales) -> Self {
        let reciprocals: Vec<f32> = scales.scales.iter().map(|scale| 1.0 / scale).collect();
        let reciprocals = if reciprocals.iter().all(|reciprocal| reciprocal.is_normal()) {
            reciprocals
        } else {
            Vec::new()
        };
        Self {
            scales,
            reciprocals,
        }
    }

    /// Whether `stored` holds, byte for byte, the codes [`encode_validated_into`] writes for a `row` that [`codec::validate`] has accepted under `layout`.
    pub fn matches(&self, layout: &RowLayout, row: &[f32], stored: &[u8]) -> bool {
        debug_assert_eq!(row.len(), layout.dimension as usize);
        assert_eq!(
            self.scales.scales.len(),
            row.len(),
            "scales of one calibration match the layout"
        );
        if stored.len() != row.len() {
            return false;
        }
        let quotient_differ = |start: usize, stored: &[u8]| {
            let mut codes = [0i8; ENCODE_BLOCK];
            let codes = &mut codes[..stored.len()];
            let end = start + stored.len();
            quotient_codes(&row[start..end], &self.scales.scales[start..end], codes);
            differ_bits(codes, stored)
        };
        if self.reciprocals.is_empty() {
            return (0..row.len())
                .step_by(ENCODE_BLOCK)
                .zip(stored.chunks(ENCODE_BLOCK))
                .fold(0, |differ, (start, stored)| {
                    differ | quotient_differ(start, stored)
                })
                == 0;
        }
        let (values, _) = row.as_chunks::<ENCODE_BLOCK>();
        let (reciprocals, _) = self.reciprocals.as_chunks::<ENCODE_BLOCK>();
        let (blocks, tail) = stored.as_chunks::<ENCODE_BLOCK>();
        let mut differ = 0u8;
        for (index, ((values, reciprocals), stored)) in
            values.iter().zip(reciprocals).zip(blocks).enumerate()
        {
            differ |= match product_differ(values, reciprocals, stored) {
                Some(block) => block,
                None => quotient_differ(index * ENCODE_BLOCK, stored),
            };
        }
        differ |= quotient_differ(row.len() - tail.len(), tail);
        differ == 0
    }
}

/// Nonzero when some code differs from its stored byte; the fold reads every byte, so it vectorizes.
fn differ_bits(codes: &[i8], stored: &[u8]) -> u8 {
    codes
        .iter()
        .zip(stored)
        .fold(0, |differ, (code, byte)| differ | (*code as u8 ^ byte))
}

/// The bits [`differ_bits`] folds for the codes of `values` from their products; `None` when some product lies within `2^-14` of a midpoint between two integers.
fn product_differ(
    values: &[f32; ENCODE_BLOCK],
    reciprocals: &[f32; ENCODE_BLOCK],
    stored: &[u8; ENCODE_BLOCK],
) -> Option<u8> {
    let mut near = false;
    let mut differ = 0u8;
    for ((value, reciprocal), byte) in values.iter().zip(reciprocals).zip(stored) {
        // A finite value times a normal reciprocal is never NaN. Up to `2^22` in magnitude the bias rounds the product exactly and its distance to the rounded integer is exact; past that, the product and the quotient both code as 127 or -127 whichever path the block takes.
        let product = value * reciprocal;
        let rounded = (product + ROUNDING_BIAS) - ROUNDING_BIAS;
        near |= (product - rounded).abs() > NEAR_HALF;
        let code = rounded.max(f32::from(CODE_MIN)).min(f32::from(CODE_MAX)) as i32 as i8;
        differ |= code as u8 ^ byte;
    }
    (!near).then_some(differ)
}

/// One byte per code, two's complement.
pub fn encode_codes(codes: &[i8]) -> Vec<u8> {
    codes.iter().map(|code| *code as u8).collect()
}

/// Rejects the reserved code `-128` so a corrupt byte cannot widen into a product outside the recipe's range.
pub fn decode_codes(bytes: &[u8], dimension: u32) -> Result<Vec<i8>, ScalarBytesRejection> {
    let codes: Vec<i8> = bytes.iter().map(|byte| *byte as i8).collect();
    check_codes(&codes, dimension)?;
    Ok(codes)
}

/// The row of codes [`decode_codes`] admits: `dimension` codes, none of them the reserved `-128`.
pub fn check_codes(codes: &[i8], dimension: u32) -> Result<(), ScalarBytesRejection> {
    if codes.len() != dimension as usize {
        return Err(ScalarBytesRejection::Dimension {
            expected: dimension,
            actual: codes.len(),
        });
    }
    if !codes
        .iter()
        .fold(false, |reserved, code| reserved | (*code == i8::MIN))
    {
        return Ok(());
    }
    let coordinate = codes
        .iter()
        .position(|code| *code == i8::MIN)
        .expect("the pass found a reserved code");
    Err(ScalarBytesRejection::ReservedCode { coordinate })
}

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum QueryRefusal {
    #[error("the query is not a member of the generation: {0}")]
    Row(#[from] RowRejection),
    /// Every coordinate rounds to code zero under the layer's scales, so every quantized score is zero.
    #[error("every coordinate of the query encodes to code zero")]
    ZeroCodes,
    /// The scales belong to a calibration of another dimension than the layout, a wiring fault of the generation rather than of the query.
    #[error("the scales have {actual} coordinates, not the layout's {expected}")]
    ScalesDimension { expected: u32, actual: usize },
}

/// A query encoded under one layer's scales. Its documents are that layer's codes, because scoring weights each product by those scales squared.
#[derive(Clone, PartialEq)]
pub struct QuantizedQuery<'s> {
    scales: &'s Scales,
    codes: Vec<i8>,
}

impl std::fmt::Debug for QuantizedQuery<'_> {
    /// Codes derive from the query embedding, so diagnostics show only the dimension.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuantizedQuery")
            .field("dimension", &self.codes.len())
            .finish()
    }
}

impl<'s> QuantizedQuery<'s> {
    /// Validates `query` against `layout` and encodes it under `scales` exactly as a stored row is encoded, clipping included.
    pub fn new(
        layout: &RowLayout,
        scales: &'s Scales,
        query: &[f32],
    ) -> Result<Self, QueryRefusal> {
        layout.check()?;
        if scales.scales.len() != layout.dimension as usize {
            return Err(QueryRefusal::ScalesDimension {
                expected: layout.dimension,
                actual: scales.scales.len(),
            });
        }
        let Encoded { codes, .. } = encode(layout, scales, query)?;
        if codes.iter().all(|code| *code == 0) {
            return Err(QueryRefusal::ZeroCodes);
        }
        Ok(Self { scales, codes })
    }

    /// The query's codes, derived from its embedding, for the scan that scores them.
    pub fn codes(&self) -> &[i8] {
        &self.codes
    }

    /// [`Self::score`] of eight rows at once; lane `i` holds the score of `docs[i]`, bit for bit.
    pub fn score_block(&self, docs: &[&[i8]; BLOCK_ROWS]) -> [f64; BLOCK_ROWS] {
        weighted_dot_block(self.scales, &self.codes, docs)
    }

    /// [`weighted_dot`] of the query's codes with `doc`, a row [`decode_codes`] produced from the same layer, so its length is the dimension and no code is `-128`.
    pub fn score(&self, doc: &[i8]) -> f64 {
        weighted_dot(self.scales, &self.codes, doc)
    }

    /// Every term [`Self::score`] can add, tabulated once so a scan over many rows looks each term up.
    ///
    /// The table holds `2048 * dimension` bytes. Building it forms `256 * dimension` terms, as many as scoring 256 rows with [`Self::score`] forms.
    pub fn term_table(&self) -> TermTable {
        let mut terms = vec![0.0f64; self.codes.len() * CODE_VALUES];
        for ((weight, q), entries) in self
            .scales
            .weights
            .iter()
            .zip(&self.codes)
            .zip(terms.as_chunks_mut::<CODE_VALUES>().0)
        {
            let q = f64::from(*q);
            for (entry, code) in entries.iter_mut().zip(&CODE_AS_F64) {
                // `q * code` is the integer product, exact in f64; adding `+0.0` turns a `-0.0` product into the `+0.0` that widening the zero i32 product gives.
                let product = q * *code + 0.0;
                *entry = *weight * product;
            }
        }
        TermTable {
            dimension: self.codes.len(),
            terms,
        }
    }
}

/// Entries per coordinate in a [`TermTable`], one for each byte a code can occupy.
const CODE_VALUES: usize = 256;

const CODE_AS_F64: [f64; CODE_VALUES] = {
    let mut values = [0.0f64; CODE_VALUES];
    let mut byte = 0;
    while byte < CODE_VALUES {
        values[byte] = byte as u8 as i8 as f64;
        byte += 1;
    }
    values
};

/// Coordinates a [`TermTable`] scan adds per pass over its rows; their terms occupy 32 KiB, sized for the L1 data cache.
const TABLE_CHUNK: usize = 16;

/// Rows a [`TermTable`] scan carries through every chunk before it moves on, so each chunk's terms are loaded once per this many rows.
const TABLE_ROWS: usize = 512;

pub struct TermTable {
    dimension: usize,
    terms: Vec<f64>,
}

impl std::fmt::Debug for TermTable {
    /// Terms derive from the query embedding, so diagnostics show only the dimension.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TermTable")
            .field("dimension", &self.dimension)
            .finish()
    }
}

impl TermTable {
    /// Scores `out.len()` rows stored back to back in `codes`, writing [`QuantizedQuery::score`] of row `i` to `out[i]`.
    ///
    /// Each row sums its terms from `+0.0` in increasing coordinate order. Rows advance together one chunk of coordinates at a time, which keeps that chunk's terms cached; the order within each row is unchanged.
    ///
    /// # Panics
    ///
    /// When `codes` is not `out.len()` rows of the table's dimension.
    pub fn score_rows(&self, codes: &[i8], out: &mut [f64]) {
        let dimension = self.dimension;
        assert_eq!(
            Some(codes.len()),
            out.len().checked_mul(dimension),
            "codes of one calibration have one length"
        );
        debug_assert!(
            !codes.contains(&i8::MIN),
            "the reserved code -128 never reaches scoring"
        );
        let chunked = dimension / TABLE_CHUNK * TABLE_CHUNK;
        let (chunks, tail) = self.terms.split_at(chunked * CODE_VALUES);
        let rows_per_tile = TABLE_ROWS * dimension;
        for (tile, sums) in codes.chunks(rows_per_tile).zip(out.chunks_mut(TABLE_ROWS)) {
            sums.fill(0.0);
            for (index, chunk) in chunks
                .as_chunks::<{ TABLE_CHUNK * CODE_VALUES }>()
                .0
                .iter()
                .enumerate()
            {
                let start = index * TABLE_CHUNK;
                for (row, sum) in tile.chunks_exact(dimension).zip(sums.iter_mut()) {
                    let bytes: &[i8; TABLE_CHUNK] = row[start..start + TABLE_CHUNK]
                        .try_into()
                        .expect("a chunk lies inside the row");
                    let mut acc = *sum;
                    for quad in 0..TABLE_CHUNK / 4 {
                        let word =
                            u32::from_le_bytes(std::array::from_fn(|i| bytes[4 * quad + i] as u8));
                        let (low, high) = (word as u16, (word >> 16) as u16);
                        let terms = 4 * quad * CODE_VALUES;
                        acc += chunk[terms + usize::from(low as u8)];
                        acc += chunk[terms + CODE_VALUES + usize::from(low >> 8)];
                        acc += chunk[terms + 2 * CODE_VALUES + usize::from(high as u8)];
                        acc += chunk[terms + 3 * CODE_VALUES + usize::from(high >> 8)];
                    }
                    *sum = acc;
                }
            }
            for (row, sum) in tile.chunks_exact(dimension).zip(sums.iter_mut()) {
                let mut acc = *sum;
                for (terms, code) in tail
                    .as_chunks::<CODE_VALUES>()
                    .0
                    .iter()
                    .zip(&row[chunked..])
                {
                    acc += terms[usize::from(*code as u8)];
                }
                *sum = acc;
            }
        }
    }
}

/// `sum_j (s_j * s_j) * (c_query_j * c_doc_j)` in f64, increasing coordinate order, starting at `+0.0`; the integer product is formed in i32 and lies in `[-16129, 16129]`.
///
/// Inputs must not contain the reserved `-128`: it widens to a product outside the recipe's
/// range, so debug assertions reject it before scoring.
///
/// Panics on unequal lengths so a shape error can never become a silently truncated score.
pub fn weighted_dot(scales: &Scales, query: &[i8], doc: &[i8]) -> f64 {
    assert_eq!(
        scales.weights.len(),
        query.len(),
        "codes of one calibration have one length"
    );
    assert_eq!(
        query.len(),
        doc.len(),
        "codes of one calibration have one length"
    );
    let mut sum = 0.0f64;
    for ((weight, q), d) in scales.weights.iter().zip(query).zip(doc) {
        debug_assert!(
            *q != i8::MIN && *d != i8::MIN,
            "the reserved code -128 never reaches scoring"
        );
        let product = i32::from(*q) * i32::from(*d);
        let term = *weight * f64::from(product);
        sum += term;
    }
    sum
}

/// [`weighted_dot`] of `query` with each of eight rows, as eight independent accumulation chains. Each lane accumulates the same terms from `+0.0` in increasing coordinate order, so lane `i` equals `weighted_dot(scales, query, docs[i])` bit for bit.
///
/// Panics when the scale weights or any row of `docs` differ in length from `query`, enforcing full-vector scoring.
pub fn weighted_dot_block(
    scales: &Scales,
    query: &[i8],
    docs: &[&[i8]; BLOCK_ROWS],
) -> [f64; BLOCK_ROWS] {
    assert_eq!(
        scales.weights.len(),
        query.len(),
        "codes of one calibration have one length"
    );
    for doc in docs {
        assert_eq!(
            query.len(),
            doc.len(),
            "codes of one calibration have one length"
        );
    }
    let mut sums = [0.0f64; BLOCK_ROWS];
    for (coordinate, (weight, q)) in scales.weights.iter().zip(query).enumerate() {
        let q = i32::from(*q);
        for (sum, doc) in sums.iter_mut().zip(docs) {
            let d = doc[coordinate];
            debug_assert!(
                q != i32::from(i8::MIN) && d != i8::MIN,
                "the reserved code -128 never reaches scoring"
            );
            let product = q * i32::from(d);
            *sum += *weight * f64::from(product);
        }
    }
    sums
}

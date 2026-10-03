//! Symmetric per-coordinate int8 quantization over unit-normalized rows.
//!
//! Calibration fixes one positive finite f32 scale per coordinate, `s_j = max_abs_j / 127`, computed once in f32.
//! Encoding widens `x_j` and `s_j` to f64, divides, rounds ties to even, clamps to `[-127, 127]`, and counts every clamp; `-128` is never produced.
//! Scoring sums `(s_j * s_j) * (c_query_j * c_doc_j)` in f64 in increasing coordinate order with the integer product formed in i32, so the same scales and codes yield the same f64 everywhere.

use sha2::{Digest, Sha256};

use super::codec::{self, RowLayout, RowRejection};

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
        Ok(Self { scales })
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
        let scales = Scales { scales };
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
/// Up to 128 in magnitude the product lies within `2^-15.9` of the quotient, so a product farther than `2^-14` from every midpoint rounds as the quotient does, and a product past 128 clamps as the quotient does; a block holding a product near a midpoint takes the f64 quotient.
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
        let mut codes = [0i8; ENCODE_BLOCK];
        let mut differ = 0u8;
        for (start, stored) in (0..row.len()).step_by(ENCODE_BLOCK).zip(stored.chunks(ENCODE_BLOCK)) {
            let end = start + stored.len();
            let codes = &mut codes[..stored.len()];
            let values = &row[start..end];
            let product = <&[f32; ENCODE_BLOCK]>::try_from(values).ok().zip(
                self.reciprocals
                    .get(start..end)
                    .and_then(|reciprocals| <&[f32; ENCODE_BLOCK]>::try_from(reciprocals).ok()),
            );
            let clear = product.is_some_and(|(values, reciprocals)| {
                product_codes(values, reciprocals, codes.try_into().expect("a whole block"))
            });
            if !clear {
                quotient_codes(values, &self.scales.scales[start..end], codes);
            }
            // The fold reads every byte of the block, so the comparison vectorizes.
            differ = codes
                .iter()
                .zip(stored)
                .fold(differ, |differ, (code, byte)| differ | (*code as u8 ^ byte));
        }
        differ == 0
    }
}

/// `false` when some product lies within `2^-14` of a midpoint between two integers.
fn product_codes(
    values: &[f32; ENCODE_BLOCK],
    reciprocals: &[f32; ENCODE_BLOCK],
    codes: &mut [i8; ENCODE_BLOCK],
) -> bool {
    let mut near = false;
    for ((code, value), reciprocal) in codes.iter_mut().zip(values).zip(reciprocals) {
        // A finite value times a normal reciprocal is never NaN. Past 128 every product and quotient clamps alike, and clamping first keeps the bias exact.
        let product = (value * reciprocal).max(-128.0).min(128.0);
        let rounded = (product + ROUNDING_BIAS) - ROUNDING_BIAS;
        // Within one of its rounded integer, the product's distance to it is exact.
        near |= (product - rounded).abs() > NEAR_HALF;
        *code = rounded.max(f32::from(CODE_MIN)).min(f32::from(CODE_MAX)) as i32 as i8;
    }
    !near
}

/// One byte per code, two's complement.
pub fn encode_codes(codes: &[i8]) -> Vec<u8> {
    codes.iter().map(|code| *code as u8).collect()
}

/// Rejects the reserved code `-128` so a corrupt byte cannot widen into a product outside the recipe's range.
pub fn decode_codes(bytes: &[u8], dimension: u32) -> Result<Vec<i8>, ScalarBytesRejection> {
    if bytes.len() != dimension as usize {
        return Err(ScalarBytesRejection::Dimension {
            expected: dimension,
            actual: bytes.len(),
        });
    }
    let codes: Vec<i8> = bytes.iter().map(|byte| *byte as i8).collect();
    if let Some(coordinate) = codes.iter().position(|code| *code == i8::MIN) {
        return Err(ScalarBytesRejection::ReservedCode { coordinate });
    }
    Ok(codes)
}

/// `sum_j (s_j * s_j) * (c_query_j * c_doc_j)` in f64, increasing coordinate order, starting at `+0.0`; the integer product is formed in i32 and lies in `[-16129, 16129]`.
///
/// Inputs must not contain the reserved `-128`: it widens to a product outside the recipe's
/// range, so debug assertions reject it before scoring.
///
/// Panics on unequal lengths so a shape error can never become a silently truncated score.
pub fn weighted_dot(scales: &Scales, query: &[i8], doc: &[i8]) -> f64 {
    assert_eq!(
        scales.scales.len(),
        query.len(),
        "codes of one calibration have one length"
    );
    assert_eq!(
        query.len(),
        doc.len(),
        "codes of one calibration have one length"
    );
    let mut sum = 0.0f64;
    for ((scale, q), d) in scales.scales.iter().zip(query).zip(doc) {
        debug_assert!(
            *q != i8::MIN && *d != i8::MIN,
            "the reserved code -128 never reaches scoring"
        );
        let weight = f64::from(*scale) * f64::from(*scale);
        let product = i32::from(*q) * i32::from(*d);
        let term = weight * f64::from(product);
        sum += term;
    }
    sum
}

//! Little-endian f32 decoding preserves every bit, including signed zero.
//! The original-row artifact is one fixed header followed by the rows in writer order; identical rows under an identical layout encode to identical bytes.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    InnerProduct,
}

impl Metric {
    pub const fn code(self) -> u8 {
        match self {
            Self::InnerProduct => 1,
        }
    }

    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::InnerProduct),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::InnerProduct => "inner_product",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "inner_product" => Some(Self::InnerProduct),
            _ => None,
        }
    }
}

/// The shape and normalization predicate every row of one generation satisfies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowLayout {
    pub dimension: u32,
    pub metric: Metric,
    /// Greatest `|‖x‖₂ − 1|` a row may have; the norm is computed in f64 from the row's f32 coordinates in increasing coordinate order.
    pub unit_norm_tolerance: f64,
}

impl RowLayout {
    /// A tolerance that is negative or not finite admits no row or every row, so neither is a generation predicate.
    pub fn check(&self) -> Result<(), RowRejection> {
        if self.unit_norm_tolerance.is_finite() && self.unit_norm_tolerance >= 0.0 {
            Ok(())
        } else {
            Err(RowRejection::Tolerance {
                tolerance: self.unit_norm_tolerance,
            })
        }
    }
}

/// Rejections name shape and magnitude, never coordinate values.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum RowRejection {
    #[error("{bytes} bytes is not a whole number of f32 words")]
    TruncatedWord { bytes: usize },
    #[error("the row has {actual} coordinates, not {expected}")]
    Dimension { expected: u32, actual: usize },
    #[error("coordinate {coordinate} is not finite")]
    NonFinite { coordinate: usize },
    #[error("the row has zero norm")]
    ZeroNorm,
    #[error("the row's norm {norm} is outside the generation's unit-norm tolerance")]
    Normalization { norm: f64 },
    #[error("the unit-norm tolerance {tolerance} is not a finite non-negative number")]
    Tolerance { tolerance: f64 },
}

impl RowRejection {
    /// The stable word `ProjectionError::InvalidVector` carries.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::TruncatedWord { .. } => "truncated",
            Self::Dimension { .. } => "dimension",
            Self::NonFinite { .. } => "nonfinite",
            Self::ZeroNorm => "zero_norm",
            Self::Normalization { .. } => "normalization",
            Self::Tolerance { .. } => "tolerance",
        }
    }
}

pub fn encode(row: &[f32]) -> Vec<u8> {
    row.iter().flat_map(|value| value.to_le_bytes()).collect()
}

/// Checks truncation eagerly but decodes lazily, so callers can reject the word count before allocating.
pub(super) fn decode_words(
    bytes: &[u8],
) -> Result<impl ExactSizeIterator<Item = f32> + '_, RowRejection> {
    let (words, rest) = bytes.as_chunks::<4>();
    if !rest.is_empty() {
        return Err(RowRejection::TruncatedWord { bytes: bytes.len() });
    }
    Ok(words.iter().map(|word| f32::from_le_bytes(*word)))
}

/// Truncation and dimension are checked before any coordinate is read; the norm is not, so a stored row can be read where the generation's tolerance is unknown.
pub fn decode_shape(bytes: &[u8], dimension: u32) -> Result<Vec<f32>, RowRejection> {
    let row = decode_length(bytes, dimension)?;
    check_finite(&row)?;
    Ok(row)
}

/// Checks truncation and dimension; callers validate finiteness and norm with [`validate_from_sum`] when a block carries the row's sum of squares.
pub fn decode_length(bytes: &[u8], dimension: u32) -> Result<Vec<f32>, RowRejection> {
    let mut row = Vec::new();
    decode_length_into(bytes, dimension, &mut row)?;
    Ok(row)
}

/// [`decode_length`] into a reused buffer, so a walk decodes every row without allocating for it.
pub fn decode_length_into(
    bytes: &[u8],
    dimension: u32,
    into: &mut Vec<f32>,
) -> Result<(), RowRejection> {
    let words = decode_words(bytes)?;
    check_dimension(words.len(), dimension)?;
    into.clear();
    into.extend(words);
    Ok(())
}

pub fn decode(bytes: &[u8], layout: &RowLayout) -> Result<Vec<f32>, RowRejection> {
    layout.check()?;
    let row = decode_shape(bytes, layout.dimension)?;
    check_norm(&row, layout.unit_norm_tolerance)?;
    Ok(row)
}

pub fn validate_shape(row: &[f32], dimension: u32) -> Result<(), RowRejection> {
    check_dimension(row.len(), dimension)?;
    check_finite(row)
}

pub fn validate_length(row: &[f32], dimension: u32) -> Result<(), RowRejection> {
    check_dimension(row.len(), dimension)
}

/// A non-finite coordinate makes the sum of squares non-finite.
pub fn validate(row: &[f32], layout: &RowLayout) -> Result<(), RowRejection> {
    if layout.check().is_ok()
        && row.len() == layout.dimension as usize
        && surely_unit(row, layout.unit_norm_tolerance)
    {
        return Ok(());
    }
    validate_from_sum(row, layout, sum_of_squares(row))
}

/// Lanes [`surely_unit`] accumulates squares in.
const SUM_LANES: usize = 8;

/// Whether the in-order sum of squares of `row` passes the norm check, decided from a sum over [`SUM_LANES`] lanes; `false` leaves the decision to the in-order sum.
///
/// Each square of an f32 is exact in f64, so every summation order of the `n` squares lies within relative `γ = (n - 1)u / (1 - (n - 1)u)` of their exact sum, `u = 2^-53`, and the in-order sum lies within relative `4nu` of the lane sum, a margin that also covers rounding the interval's ends.
/// The rounded square root and the subtraction are monotone, so the sums the norm check admits form an interval; when both ends of the lane sum's interval pass, the in-order sum passes too.
fn surely_unit(row: &[f32], tolerance: f64) -> bool {
    let mut lanes = [0.0f64; SUM_LANES];
    let (blocks, tail) = row.as_chunks::<SUM_LANES>();
    for block in blocks {
        for (lane, value) in lanes.iter_mut().zip(block) {
            let widened = f64::from(*value);
            *lane += widened * widened;
        }
    }
    let mut sum = lanes.iter().sum::<f64>();
    for value in tail {
        let widened = f64::from(*value);
        sum += widened * widened;
    }
    let margin = 2.0 * row.len() as f64 * f64::EPSILON;
    sum.is_finite()
        && sum > 0.0
        && margin < 0.5
        && [sum * (1.0 - margin), sum * (1.0 + margin)]
            .into_iter()
            .all(|end| check_norm_sum(end, tolerance).is_ok())
}

/// Validates a row whose sum of squares was accumulated as [`validate`] accumulates it: from `+0.0`, in increasing coordinate order.
///
/// A `Vec<f32>` cannot hold enough coordinates for finite `f32` squares to overflow `f64`. A
/// non-finite sum identifies a non-finite coordinate, and the scalar scan names the first one.
pub fn validate_from_sum(
    row: &[f32],
    layout: &RowLayout,
    sum_of_squares: f64,
) -> Result<(), RowRejection> {
    layout.check()?;
    check_dimension(row.len(), layout.dimension)?;
    if !sum_of_squares.is_finite() {
        check_finite(row)?;
    }
    check_norm_sum(sum_of_squares, layout.unit_norm_tolerance)
}

fn check_dimension(actual: usize, expected: u32) -> Result<(), RowRejection> {
    if actual == expected as usize {
        Ok(())
    } else {
        Err(RowRejection::Dimension { expected, actual })
    }
}

/// Checked in increasing coordinate order, so the rejection names the earliest failing coordinate.
fn check_finite(row: &[f32]) -> Result<(), RowRejection> {
    match row.iter().position(|value| !value.is_finite()) {
        Some(coordinate) => Err(RowRejection::NonFinite { coordinate }),
        None => Ok(()),
    }
}

/// The accumulator starts at `+0.0` and each square is added in coordinate order; `Iterator::sum` is not used because its initial value is not part of the contract.
fn check_norm(row: &[f32], tolerance: f64) -> Result<(), RowRejection> {
    check_norm_sum(sum_of_squares(row), tolerance)
}

/// `sum_of_squares` guarantees accumulation from `+0.0` in coordinate order.
fn sum_of_squares(row: &[f32]) -> f64 {
    let mut sum_of_squares = 0.0f64;
    for value in row {
        let widened = f64::from(*value);
        sum_of_squares += widened * widened;
    }
    sum_of_squares
}

/// The admission test is written as the contract's inclusive bound so a NaN sum is refused rather than admitted by a false `>` comparison.
fn check_norm_sum(sum_of_squares: f64, tolerance: f64) -> Result<(), RowRejection> {
    if sum_of_squares == 0.0 {
        return Err(RowRejection::ZeroNorm);
    }
    let norm = sum_of_squares.sqrt();
    if (norm - 1.0).abs() <= tolerance {
        Ok(())
    } else {
        Err(RowRejection::Normalization { norm })
    }
}

pub const ARTIFACT_MAGIC: [u8; 8] = *b"EIDF32R\0";
pub const ARTIFACT_VERSION: u16 = 1;
/// Magic, version, metric code, one reserved zero byte, dimension, and row count.
pub const ARTIFACT_HEADER_BYTES: usize = 8 + 2 + 1 + 1 + 4 + 8;

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum ArtifactRejection {
    #[error("the artifact is {bytes} bytes, shorter than its {ARTIFACT_HEADER_BYTES}-byte header")]
    ShortHeader { bytes: usize },
    #[error("the artifact magic is not the original-row magic")]
    Magic,
    #[error("artifact version {version} is not {ARTIFACT_VERSION}")]
    Version { version: u16 },
    #[error("the reserved header byte is {reserved}, not zero")]
    Reserved { reserved: u8 },
    #[error("metric code {code} is not the layout's {expected:?}")]
    Metric { code: u8, expected: Metric },
    #[error("a layout with dimension zero has no rows")]
    ZeroDimension,
    #[error("the layout admits no generation: {0}")]
    Layout(RowRejection),
    #[error("the artifact declares dimension {declared}, not the layout's {expected}")]
    Dimension { declared: u32, expected: u32 },
    #[error("the artifact declares {declared} rows but carries {bytes} row bytes")]
    RowBytes { declared: u64, bytes: usize },
    #[error("row {index}: {rejection}")]
    Row {
        index: usize,
        rejection: RowRejection,
    },
}

#[derive(Clone, PartialEq)]
pub struct OriginalRows {
    pub layout: RowLayout,
    pub rows: Vec<Vec<f32>>,
}

impl fmt::Debug for OriginalRows {
    /// Rows are embedding content and stay out of diagnostics.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OriginalRows")
            .field("layout", &self.layout)
            .field("rows", &self.rows.len())
            .finish()
    }
}

/// Every row is validated before encoding, so no artifact carries a row the decoder would refuse.
pub fn encode_rows<'a>(
    layout: &RowLayout,
    rows: impl IntoIterator<Item = &'a [f32]>,
) -> Result<Vec<u8>, ArtifactRejection> {
    if layout.dimension == 0 {
        return Err(ArtifactRejection::ZeroDimension);
    }
    layout.check().map_err(ArtifactRejection::Layout)?;
    let rows = rows.into_iter();
    let mut bytes = Vec::with_capacity(
        ARTIFACT_HEADER_BYTES + rows.size_hint().0 * layout.dimension as usize * 4,
    );
    bytes.extend_from_slice(&ARTIFACT_MAGIC);
    bytes.extend_from_slice(&ARTIFACT_VERSION.to_le_bytes());
    bytes.push(layout.metric.code());
    bytes.push(0);
    bytes.extend_from_slice(&layout.dimension.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    let mut count = 0u64;
    for (index, row) in rows.enumerate() {
        validate(row, layout).map_err(|rejection| ArtifactRejection::Row { index, rejection })?;
        for value in row {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        count += 1;
    }
    bytes[ARTIFACT_HEADER_BYTES - 8..ARTIFACT_HEADER_BYTES].copy_from_slice(&count.to_le_bytes());
    Ok(bytes)
}

/// The row count an artifact header declares, after the magic, version, metric, reserved byte, and dimension are checked against `layout`; `header` is the artifact's first [`ARTIFACT_HEADER_BYTES`] bytes or fewer.
///
/// # Errors
///
/// The same header rejections [`decode_rows`] makes.
pub fn decode_header(header: &[u8], layout: &RowLayout) -> Result<u64, ArtifactRejection> {
    if layout.dimension == 0 {
        return Err(ArtifactRejection::ZeroDimension);
    }
    layout.check().map_err(ArtifactRejection::Layout)?;
    if header.len() < ARTIFACT_HEADER_BYTES {
        return Err(ArtifactRejection::ShortHeader {
            bytes: header.len(),
        });
    }
    if header[..8] != ARTIFACT_MAGIC {
        return Err(ArtifactRejection::Magic);
    }
    let version = u16::from_le_bytes([header[8], header[9]]);
    if version != ARTIFACT_VERSION {
        return Err(ArtifactRejection::Version { version });
    }
    if Metric::from_code(header[10]) != Some(layout.metric) {
        return Err(ArtifactRejection::Metric {
            code: header[10],
            expected: layout.metric,
        });
    }
    if header[11] != 0 {
        return Err(ArtifactRejection::Reserved {
            reserved: header[11],
        });
    }
    let declared = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
    if declared != layout.dimension {
        return Err(ArtifactRejection::Dimension {
            declared,
            expected: layout.dimension,
        });
    }
    Ok(u64::from_le_bytes(
        header[16..24].try_into().expect("eight header bytes"),
    ))
}

pub const fn row_bytes(dimension: u32) -> u64 {
    dimension as u64 * 4
}

/// Rows follow the header with no padding, so row `index` starts `index` whole rows past it.
pub const fn row_offset(index: u64, dimension: u32) -> u64 {
    ARTIFACT_HEADER_BYTES as u64 + index * row_bytes(dimension)
}

/// Checks that the `body_bytes` after the header hold exactly the `declared` rows of `layout`.
///
/// # Errors
///
/// [`ArtifactRejection::RowBytes`] when they hold more or fewer bytes.
pub fn check_body(
    declared: u64,
    body_bytes: u64,
    layout: &RowLayout,
) -> Result<(), ArtifactRejection> {
    if declared.checked_mul(row_bytes(layout.dimension)) == Some(body_bytes) {
        Ok(())
    } else {
        Err(ArtifactRejection::RowBytes {
            declared,
            bytes: usize::try_from(body_bytes).unwrap_or(usize::MAX),
        })
    }
}

/// Decodes and validates artifact rows one at a time into one reused buffer, so a walk over every row allocates once.
pub struct RowDecoder {
    layout: RowLayout,
    row: Vec<f32>,
}

impl RowDecoder {
    pub fn new(layout: &RowLayout) -> Self {
        Self {
            layout: *layout,
            row: Vec::with_capacity(layout.dimension as usize),
        }
    }

    /// The coordinates of artifact row `index`, decoded from its `bytes` and validated under the layout.
    ///
    /// # Errors
    ///
    /// [`ArtifactRejection::Row`] at `index` for a row outside the layout.
    pub fn decode(&mut self, index: usize, bytes: &[u8]) -> Result<&[f32], ArtifactRejection> {
        decode_length_into(bytes, self.layout.dimension, &mut self.row)
            .and_then(|()| validate(&self.row, &self.layout))
            .map_err(|rejection| ArtifactRejection::Row { index, rejection })?;
        Ok(&self.row)
    }
}

/// The header, metric, dimension, and byte count are checked before any row is read.
pub fn decode_rows(bytes: &[u8], layout: &RowLayout) -> Result<OriginalRows, ArtifactRejection> {
    let row_count = decode_header(bytes, layout)?;
    let body = &bytes[ARTIFACT_HEADER_BYTES..];
    check_body(row_count, body.len() as u64, layout)?;
    let mut decoder = RowDecoder::new(layout);
    let rows = body
        .chunks_exact(row_bytes(layout.dimension) as usize)
        .enumerate()
        .map(|(index, chunk)| decoder.decode(index, chunk).map(<[f32]>::to_vec))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(OriginalRows {
        layout: *layout,
        rows,
    })
}

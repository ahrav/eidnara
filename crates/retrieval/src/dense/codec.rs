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
    let words = decode_words(bytes)?;
    check_dimension(words.len(), dimension)?;
    let row: Vec<f32> = words.collect();
    check_finite(&row)?;
    Ok(row)
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

pub fn validate(row: &[f32], layout: &RowLayout) -> Result<(), RowRejection> {
    layout.check()?;
    validate_shape(row, layout.dimension)?;
    check_norm(row, layout.unit_norm_tolerance)
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
    let mut sum_of_squares = 0.0f64;
    for value in row {
        let widened = f64::from(*value);
        sum_of_squares += widened * widened;
    }
    if sum_of_squares == 0.0 {
        return Err(RowRejection::ZeroNorm);
    }
    let norm = sum_of_squares.sqrt();
    if (norm - 1.0).abs() > tolerance {
        return Err(RowRejection::Normalization { norm });
    }
    Ok(())
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

/// The header, metric, dimension, and byte count are checked before any row is read.
pub fn decode_rows(bytes: &[u8], layout: &RowLayout) -> Result<OriginalRows, ArtifactRejection> {
    if layout.dimension == 0 {
        return Err(ArtifactRejection::ZeroDimension);
    }
    layout.check().map_err(ArtifactRejection::Layout)?;
    if bytes.len() < ARTIFACT_HEADER_BYTES {
        return Err(ArtifactRejection::ShortHeader { bytes: bytes.len() });
    }
    let (header, body) = bytes.split_at(ARTIFACT_HEADER_BYTES);
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
    let row_count = u64::from_le_bytes(header[16..24].try_into().expect("eight header bytes"));
    let row_bytes = u64::from(layout.dimension) * 4;
    let mismatch = ArtifactRejection::RowBytes {
        declared: row_count,
        bytes: body.len(),
    };
    if row_count.checked_mul(row_bytes) != u64::try_from(body.len()).ok() {
        return Err(mismatch);
    }
    let rows = body
        .chunks_exact(usize::try_from(row_bytes).map_err(|_| mismatch)?)
        .enumerate()
        .map(|(index, chunk)| {
            decode(chunk, layout).map_err(|rejection| ArtifactRejection::Row { index, rejection })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(OriginalRows {
        layout: *layout,
        rows,
    })
}

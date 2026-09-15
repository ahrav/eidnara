//! Little-endian f32 decoding preserves every bit, including signed zero.

/// Rows are unit-normalized, so the inner product is the cosine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    InnerProduct,
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

/// Truncation and dimension are checked before any coordinate is read; the norm is not, so a stored row can be read where the generation's tolerance is unknown.
pub fn decode_shape(bytes: &[u8], dimension: u32) -> Result<Vec<f32>, RowRejection> {
    let (words, rest) = bytes.as_chunks::<4>();
    if !rest.is_empty() {
        return Err(RowRejection::TruncatedWord { bytes: bytes.len() });
    }
    check_dimension(words.len(), dimension)?;
    let row: Vec<f32> = words.iter().map(|word| f32::from_le_bytes(*word)).collect();
    check_finite(&row)?;
    Ok(row)
}

pub fn decode(bytes: &[u8], layout: &RowLayout) -> Result<Vec<f32>, RowRejection> {
    let row = decode_shape(bytes, layout.dimension)?;
    check_norm(&row, layout.unit_norm_tolerance)?;
    Ok(row)
}

pub fn validate_shape(row: &[f32], dimension: u32) -> Result<(), RowRejection> {
    check_dimension(row.len(), dimension)?;
    check_finite(row)
}

pub fn validate(row: &[f32], layout: &RowLayout) -> Result<(), RowRejection> {
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

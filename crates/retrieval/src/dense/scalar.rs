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
        let values = codec::decode_words(bytes)
            .map_err(|_| ScalarBytesRejection::TruncatedWord { bytes: bytes.len() })?;
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
    layout.check().map_err(CalibrationRejection::Layout)?;
    let dimension = layout.dimension as usize;
    let mut max_abs = vec![0.0f32; dimension];
    let mut count = 0u64;
    for (index, row) in rows.into_iter().enumerate() {
        codec::validate(row, layout)
            .map_err(|rejection| CalibrationRejection::Row { index, rejection })?;
        for (max, value) in max_abs.iter_mut().zip(row) {
            *max = max.max(value.abs());
        }
        count += 1;
    }
    if count == 0 {
        return Err(CalibrationRejection::NoRows);
    }
    let mut scales = Vec::with_capacity(dimension);
    for (coordinate, max) in max_abs.into_iter().enumerate() {
        let scale = if max == 0.0 { 1.0 } else { max / 127.0 };
        if scale == 0.0 {
            return Err(CalibrationRejection::ScaleUnderflow { coordinate });
        }
        scales.push(scale);
    }
    let scales = Scales { scales };
    let identity = CalibrationIdentity {
        recipe: ScalarRecipe::SymmetricInt8V1,
        calibrated_rows: count,
        scales_digest: scales.digest(),
    };
    Ok(Calibration { scales, identity })
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
    layout.check()?;
    codec::validate(row, layout)?;
    assert_eq!(
        scales.scales.len(),
        layout.dimension as usize,
        "scales of one calibration match the layout"
    );
    let mut codes = Vec::with_capacity(row.len());
    let mut clipped = 0u32;
    for (value, scale) in row.iter().zip(&scales.scales) {
        let rounded = (f64::from(*value) / f64::from(*scale)).round_ties_even();
        let code = rounded.clamp(f64::from(CODE_MIN), f64::from(CODE_MAX));
        if code != rounded {
            clipped += 1;
        }
        // A finite value over a positive finite scale is a finite quotient, and the clamp bounds it to i8, so the cast is exact.
        codes.push(code as i8);
    }
    Ok(Encoded { codes, clipped })
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
        let weight = f64::from(*scale) * f64::from(*scale);
        let product = i32::from(*q) * i32::from(*d);
        let term = weight * f64::from(product);
        sum += term;
    }
    sum
}

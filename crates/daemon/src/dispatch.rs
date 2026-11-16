use std::fmt;
use std::io::{self, Write};
use std::sync::Arc;

use host_runtime::wire::ByteCharge;
use serde_json::{Map, Value};

use crate::staged_body::{BoundedSink, Sink, StagedBody};

/// Maximum body size shared with host frame admission.
pub const MAX_WIRE_BODY_BYTES: usize = host_runtime::MAX_FRAME_BODY_LEN as usize;

pub struct PreparedOutput {
    source: PreparedSource,
    /// The output retains shared ownership of resident charges for request bytes held by
    /// `source`.
    held: Vec<Arc<[ByteCharge]>>,
    /// Work the request owner runs once these bytes are encoded and handed to the wire writer.
    after_response: Option<AfterResponse>,
    #[cfg(test)]
    encoded_for_test: Arc<std::sync::OnceLock<Vec<u8>>>,
}

/// Work deferred until a response's bytes are encoded and handed to the wire writer. Dropping it unrun is the outcome for a response that never settled, so its owned state releases with it.
pub struct AfterResponse(Box<dyn FnOnce() + Send + Sync>);

impl AfterResponse {
    pub fn new(work: impl FnOnce() + Send + Sync + 'static) -> Self {
        Self(Box::new(work))
    }

    pub fn run(self) {
        (self.0)();
    }
}

/// Request charges and after-response work stay with the original response when a clone shares its source.
impl Clone for PreparedOutput {
    fn clone(&self) -> Self {
        Self {
            source: self.source.clone(),
            held: Vec::new(),
            after_response: None,
            #[cfg(test)]
            encoded_for_test: Arc::clone(&self.encoded_for_test),
        }
    }
}

#[derive(Clone)]
enum PreparedSource {
    Json(Arc<Value>),
    Exact(Arc<Vec<u8>>),
    Transform(Arc<TransformSegments>),
    Emit(Arc<dyn Emit>),
}

/// A typed body serialized on demand: once into a counter for measurement, once into the reserved destination.
pub trait Emit: Send + Sync {
    fn emit(&self, destination: &mut dyn Write) -> Result<(), PreparedOutputError>;
}

/// `TransformSegments` stores the encoded envelope around its prepared operations.
struct TransformSegments {
    /// `envelope` holds the encoded envelope without the operations value, which belongs at
    /// `split`, right after the `"operations":` key.
    envelope: Vec<u8>,
    split: usize,
    operations: Vec<RecipeSegment>,
}

/// One recipe operation as prepared bytes: a keep is one exact token; an insert wraps whole-message
/// segments that are measured once and written without re-serialization.
#[derive(Debug)]
pub enum RecipeSegment {
    Keep(PreparedSegment),
    Insert(Vec<PreparedSegment>),
}

const INSERT_PREFIX: &[u8] = br#"{"op":"insert","values":["#;
const INSERT_SUFFIX: &[u8] = b"]}";

/// Staging bytes for encoding a transform envelope; a larger envelope grows past them.
const ENVELOPE_BYTES: usize = 4 * 1024;

#[derive(Clone)]
enum PreparedSegmentSource {
    Exact(Arc<[u8]>),
    Served(crate::transform::ServedMessage),
    /// Serialized into the destination at write time; the length was measured when the value was
    /// encoded, so no intermediate buffer is built for it.
    Value(Arc<Value>),
}

#[derive(Clone)]
pub struct PreparedSegment {
    source: PreparedSegmentSource,
    measured_len: usize,
}

impl PreparedSegment {
    pub fn exact(bytes: Arc<[u8]>) -> Self {
        let measured_len = bytes.len();
        Self {
            source: PreparedSegmentSource::Exact(bytes),
            measured_len,
        }
    }

    pub(crate) fn served(message: crate::transform::ServedMessage) -> Self {
        let measured_len = message.canonical_bytes().len();
        Self {
            source: PreparedSegmentSource::Served(message),
            measured_len,
        }
    }

    /// `measured_len` is the value's canonical JSON length; the writer checks it on output.
    pub fn value(value: Arc<Value>, measured_len: usize) -> Self {
        Self {
            source: PreparedSegmentSource::Value(value),
            measured_len,
        }
    }

    /// Creates a segment with caller-supplied `measured_len` for length-check tests.
    #[doc(hidden)]
    pub fn inconsistent_for_test(bytes: Arc<[u8]>, measured_len: usize) -> Self {
        Self {
            source: PreparedSegmentSource::Exact(bytes),
            measured_len,
        }
    }

    fn fits_recipe_depth(&self) -> bool {
        let limit = crate::edit_recipe::MAX_JSON_NESTING - 4;
        match &self.source {
            PreparedSegmentSource::Value(value) => containers_within(value, limit),
            PreparedSegmentSource::Exact(bytes) => encoded_depth_within(bytes, limit),
            PreparedSegmentSource::Served(message) => message.nesting_depth() <= limit,
        }
    }

    fn write_staged<S: Sink>(&self, body: &mut StagedBody<S>) -> io::Result<()> {
        match &self.source {
            PreparedSegmentSource::Exact(bytes) => body.bytes(bytes),
            PreparedSegmentSource::Served(message) => body.bytes(message.canonical_bytes()),
            PreparedSegmentSource::Value(value) => body.value(value),
        }
    }

    fn write_to<W: Write>(&self, destination: &mut W) -> Result<(), PreparedOutputError> {
        match &self.source {
            PreparedSegmentSource::Exact(bytes) => destination.write_all(bytes)?,
            PreparedSegmentSource::Served(message) => {
                destination.write_all(message.canonical_bytes())?
            }
            PreparedSegmentSource::Value(value) => serde_json::to_writer(destination, value)
                .map_err(|error| {
                    if error.is_io() {
                        PreparedOutputError::Write(error.into())
                    } else {
                        PreparedOutputError::Serialize(error)
                    }
                })?,
        }
        Ok(())
    }
}

/// Whether `value` nests at most `limit` container levels, the root container being level one.
/// For any `limit` up to `MAX_JSON_NESTING` this is `validate_json_nesting(value)` succeeding with
/// at most `limit`, found by a recursion that allocates nothing.
fn containers_within(value: &Value, limit: usize) -> bool {
    match value {
        Value::Array(items) => {
            limit > 0 && items.iter().all(|item| containers_within(item, limit - 1))
        }
        Value::Object(fields) => {
            limit > 0
                && fields
                    .values()
                    .all(|item| containers_within(item, limit - 1))
        }
        _ => true,
    }
}

fn encoded_depth_within(bytes: &[u8], limit: usize) -> bool {
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for &byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > limit {
                        return false;
                    }
                }
                b']' | b'}' => {
                    let Some(parent) = depth.checked_sub(1) else {
                        return false;
                    };
                    depth = parent;
                }
                _ => {}
            }
        }
    }
    depth == 0 && !quoted
}

impl fmt::Debug for PreparedSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = match &self.source {
            PreparedSegmentSource::Exact(_) => "exact",
            PreparedSegmentSource::Served(_) => "served",
            PreparedSegmentSource::Value(_) => "value",
        };
        f.debug_struct("PreparedSegment")
            .field("source", &source)
            .field("measured_len", &self.measured_len)
            .finish()
    }
}

impl PreparedOutput {
    fn from_source(source: PreparedSource) -> Self {
        Self {
            source,
            held: Vec::new(),
            after_response: None,
            #[cfg(test)]
            encoded_for_test: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// JSON measurement does not retain encoded bytes because it precedes host memory reservation.
    pub fn json(value: Value) -> Self {
        Self::from_source(PreparedSource::Json(Arc::new(value)))
    }

    /// A typed body that serializes directly, with no intermediate `Value` tree.
    pub fn emit(body: Arc<dyn Emit>) -> Self {
        Self::from_source(PreparedSource::Emit(body))
    }

    /// Retains existing encoded bytes without copying them into an output body.
    pub fn cached_bytes(bytes: Vec<u8>) -> Self {
        Self::from_source(PreparedSource::Exact(Arc::new(bytes)))
    }

    /// The envelope carries `"operations": null` where the prepared operations are spliced in.
    pub fn transform_recipe(
        envelope: Value,
        operations: Vec<RecipeSegment>,
    ) -> Result<Self, PreparedOutputError> {
        // An envelope too deep is refused as too deep before its shape is judged.
        let invalid = |envelope: &Value| {
            if containers_within(envelope, crate::edit_recipe::MAX_JSON_NESTING) {
                PreparedOutputError::InvalidTransformEnvelope
            } else {
                PreparedOutputError::RecipeNestingTooDeep
            }
        };
        let envelope = match envelope {
            Value::Object(fields) if fields.get("operations") == Some(&Value::Null) => fields,
            other => return Err(invalid(&other)),
        };
        if operations.iter().any(|operation| match operation {
            RecipeSegment::Keep(_) => false,
            RecipeSegment::Insert(values) => values.iter().any(|value| !value.fits_recipe_depth()),
        }) {
            return Err(PreparedOutputError::RecipeNestingTooDeep);
        }
        // `encode_envelope` refuses an envelope nested too deep while it encodes it.
        let (encoded, split) = encode_envelope(&envelope)?;
        Ok(Self::from_source(PreparedSource::Transform(Arc::new(
            TransformSegments {
                envelope: encoded,
                split,
                operations,
            },
        ))))
    }

    /// The kernel-routes bench reads the JSON body to reject a cell whose route did not answer `available`.
    #[cfg(feature = "test-support")]
    pub fn json_for_test(&self) -> Option<&Value> {
        match &self.source {
            PreparedSource::Json(value) => Some(value),
            PreparedSource::Exact(_) | PreparedSource::Transform(_) | PreparedSource::Emit(_) => {
                None
            }
        }
    }

    /// Transform and exact-length bodies are immutable shared sources whose bytes are fixed at preparation, so the transport can serialize them into the frame at publication.
    pub fn serializes_directly(&self) -> bool {
        matches!(
            self.source,
            PreparedSource::Exact(_) | PreparedSource::Transform(_)
        )
    }

    /// Writes this body, measured at `len` bytes, into `destination`; any other length is an error.
    ///
    /// An exact body is one write. Every other body is encoded through a staging buffer that
    /// reaches `destination` in buffer-sized writes.
    pub fn write_measured(
        &self,
        len: usize,
        destination: &mut dyn Write,
    ) -> Result<usize, PreparedOutputError> {
        match &self.source {
            PreparedSource::Exact(bytes) => {
                let source = MeasuredSource::Exact(bytes);
                MeasuredOutput { source, len }.write_to(&mut { destination })
            }
            PreparedSource::Json(value) => write_staged_body(len, destination, |body| {
                body.value(value).map_err(PreparedOutputError::Write)
            }),
            PreparedSource::Transform(segments) => write_staged_body(len, destination, |body| {
                write_transform_envelope(segments, body, |body, segment| {
                    segment
                        .write_staged(body)
                        .map_err(PreparedOutputError::Write)
                })
            }),
            PreparedSource::Emit(emitted) => {
                write_staged_body(len, destination, |body| emitted.emit(body))
            }
        }
    }

    /// Measures this immutable source exactly before output reservation.
    ///
    /// JSON measurement does not retain encoded bytes because it precedes the host's resident-byte reservation.
    pub fn measure(&self) -> Result<MeasuredOutput<'_>, PreparedOutputError> {
        #[cfg(feature = "test-support")]
        guard_calls::MEASURED.with(|calls| calls.set(calls.get() + 1));
        let (source, len) = match &self.source {
            PreparedSource::Json(value) => {
                let len = measure_json(value)?;
                (MeasuredSource::Json(value), len)
            }
            PreparedSource::Exact(bytes) => {
                let len = checked_body_len([bytes.len()])?;
                (MeasuredSource::Exact(bytes), len)
            }
            PreparedSource::Transform(segments) => {
                let len = measure_transform(segments)?;
                (MeasuredSource::Transform(segments), len)
            }
            PreparedSource::Emit(body) => {
                let mut counter = CountingWriter::default();
                let result = body.emit(&mut counter);
                let counter = finish_count(counter, result)?;
                (MeasuredSource::Emit(body.as_ref()), counter.len)
            }
        };
        Ok(MeasuredOutput { source, len })
    }
}

#[cfg(test)]
impl std::ops::Deref for PreparedOutput {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.encoded_for_test
            .get_or_init(|| {
                let measured = self.measure().expect("test response must measure");
                let mut bytes = Vec::with_capacity(measured.len());
                measured
                    .write_to(&mut bytes)
                    .expect("test response must serialize");
                bytes
            })
            .as_slice()
    }
}

#[cfg(test)]
impl AsRef<[u8]> for PreparedOutput {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

#[cfg(test)]
impl PartialEq<Vec<u8>> for PreparedOutput {
    fn eq(&self, other: &Vec<u8>) -> bool {
        &**self == other.as_slice()
    }
}

impl fmt::Debug for PreparedOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.source {
            PreparedSource::Json(_) => "json",
            PreparedSource::Exact(_) => "exact",
            PreparedSource::Transform(_) => "transform",
            PreparedSource::Emit(_) => "emit",
        };
        f.debug_struct("PreparedOutput")
            .field("kind", &kind)
            .finish()
    }
}

pub enum PreparedOutcome {
    Response(PreparedOutput),
    Error { code: String, message: String },
    Streamed,
}

impl PreparedOutcome {
    /// A response output retains `charges` for its whole lifetime, so request bytes its source
    /// shares stay accounted for. Any other outcome drops them at once.
    pub fn holding(self, charges: Arc<[ByteCharge]>) -> Self {
        match self {
            Self::Response(mut output) => {
                if !charges.is_empty() {
                    output.held.push(charges);
                }
                Self::Response(output)
            }
            other => other,
        }
    }

    /// Attaches `after` to a response; any other outcome drops it unrun.
    pub fn after_response(self, after: Option<AfterResponse>) -> Self {
        match (self, after) {
            (Self::Response(mut output), Some(after)) => {
                output.after_response = Some(after);
                Self::Response(output)
            }
            (other, _) => other,
        }
    }

    /// Detaches the after-response work, which the request owner runs once the response settles.
    pub fn take_after_response(&mut self) -> Option<AfterResponse> {
        match self {
            Self::Response(output) => output.after_response.take(),
            _ => None,
        }
    }
}

impl fmt::Debug for PreparedOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Response(body) => f.debug_tuple("Response").field(body).finish(),
            Self::Error { code, message } => f
                .debug_struct("Error")
                .field("code_len", &code.len())
                .field("message_len", &message.len())
                .finish(),
            Self::Streamed => f.write_str("Streamed"),
        }
    }
}

pub struct MeasuredOutput<'a> {
    source: MeasuredSource<'a>,
    len: usize,
}

enum MeasuredSource<'a> {
    Json(&'a Value),
    Exact(&'a [u8]),
    Transform(&'a TransformSegments),
    Emit(&'a dyn Emit),
}

impl MeasuredOutput<'_> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Writes into a caller-reserved destination and verifies exact length.
    pub fn write_to<W: Write>(&self, destination: &mut W) -> Result<usize, PreparedOutputError> {
        count_write();
        let mut destination = BoundedWriter::new(destination, self.len);
        match &self.source {
            MeasuredSource::Json(value) => {
                // `write_to` unwraps serde's wrapped I/O errors so callers can distinguish destination failures from serialization failures.
                serde_json::to_writer(&mut destination, value).map_err(|error| {
                    if error.is_io() {
                        PreparedOutputError::Write(error.into())
                    } else {
                        PreparedOutputError::Serialize(error)
                    }
                })?;
            }
            MeasuredSource::Exact(bytes) => destination.write_all(bytes)?,
            MeasuredSource::Transform(segments) => write_transform(segments, &mut destination)?,
            MeasuredSource::Emit(body) => body.emit(&mut destination)?,
        }
        let written = destination.written();
        if written != self.len {
            return Err(PreparedOutputError::LengthMismatch {
                measured: self.len,
                written,
            });
        }
        Ok(written)
    }
}

/// Writes a body of `len` measured bytes through a staging buffer into `destination`; any other
/// length is an error.
fn write_staged_body(
    len: usize,
    destination: &mut dyn Write,
    write: impl FnOnce(&mut StagedBody<BoundedSink<'_>>) -> Result<(), PreparedOutputError>,
) -> Result<usize, PreparedOutputError> {
    count_write();
    let mut body = StagedBody::new(BoundedSink::new(destination, len), len);
    write(&mut body)?;
    let written = body.finish()?.written();
    if written != len {
        return Err(PreparedOutputError::LengthMismatch {
            measured: len,
            written,
        });
    }
    Ok(written)
}

/// Counts one body write for `guard_calls` and runs its write hook.
fn count_write() {
    #[cfg(feature = "test-support")]
    guard_calls::WRITTEN.with(|calls| calls.set(calls.get() + 1));
    #[cfg(feature = "test-support")]
    guard_calls::ON_WRITE.with(|hook| {
        if let Some(hook) = &*hook.borrow() {
            hook();
        }
    });
}

/// Per-thread guard call counts; a test reads them to observe how many times
/// a body was measured and written instead of trusting the caller's structure.
#[cfg(feature = "test-support")]
pub mod guard_calls {
    use std::cell::{Cell, RefCell};

    thread_local! {
        pub(super) static MEASURED: Cell<usize> = const { Cell::new(0) };
        pub(super) static WRITTEN: Cell<usize> = const { Cell::new(0) };
        pub(super) static ON_WRITE: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    }

    pub fn reset() {
        MEASURED.with(|calls| calls.set(0));
        WRITTEN.with(|calls| calls.set(0));
        ON_WRITE.with(|hook| hook.borrow_mut().take());
    }

    /// Runs `hook` on this thread at the start of every write until `reset`,
    /// so a test can end a budget while a body is being written.
    pub fn on_write(hook: impl Fn() + 'static) {
        ON_WRITE.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    }

    /// `(measured, written)` on this thread since the last `reset`.
    pub fn counts() -> (usize, usize) {
        (MEASURED.with(Cell::get), WRITTEN.with(Cell::get))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PreparedOutputError {
    #[error("prepared body length {len} exceeds wire cap {max}")]
    BodyTooLarge { len: usize, max: usize },
    #[error("prepared body length overflowed")]
    LengthOverflow,
    #[error("transform envelope must contain a null operations field")]
    InvalidTransformEnvelope,
    #[error("transform recipe exceeds the JSON nesting limit")]
    RecipeNestingTooDeep,
    #[error("prepared JSON serialization failed: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("prepared body write failed: {0}")]
    Write(#[from] io::Error),
    #[error("prepared body length mismatch: measured {measured}, wrote {written}")]
    LengthMismatch { measured: usize, written: usize },
}

fn checked_body_len(
    lengths: impl IntoIterator<Item = usize>,
) -> Result<usize, PreparedOutputError> {
    let mut total = 0usize;
    for len in lengths {
        total = total
            .checked_add(len)
            .ok_or(PreparedOutputError::LengthOverflow)?;
    }
    if total > MAX_WIRE_BODY_BYTES {
        return Err(PreparedOutputError::BodyTooLarge {
            len: total,
            max: MAX_WIRE_BODY_BYTES,
        });
    }
    Ok(total)
}

/// Measures a typed value's exact encoded length without retaining the bytes.
pub(crate) fn measure_serialize<T: serde::Serialize + ?Sized>(
    value: &T,
) -> Result<usize, PreparedOutputError> {
    let mut writer = CountingWriter::default();
    let result = serde_json::to_writer(&mut writer, value).map_err(PreparedOutputError::Serialize);
    let writer = finish_count(writer, result)?;
    Ok(writer.len)
}

/// Measures a JSON value's exact encoded length without retaining the bytes.
///
/// A structural count equals the serializer's byte count. A value past `MAX_WIRE_BODY_BYTES`
/// is measured by the serializer, which reports `BodyTooLarge` or `LengthOverflow`.
pub(crate) fn measure_json(value: &Value) -> Result<usize, PreparedOutputError> {
    match compact_json_len(value) {
        Some(len) if len <= MAX_WIRE_BODY_BYTES => Ok(len),
        _ => measure_serialize(value),
    }
}

/// Returns the byte length of `serde_json::to_vec(value)` when the count fits in `usize`.
fn compact_json_len(value: &Value) -> Option<usize> {
    match value {
        Value::Null => Some(4),
        Value::Bool(true) => Some(4),
        Value::Bool(false) => Some(5),
        Value::Number(number) => Some(compact_number_len(number)),
        Value::String(text) => Some(crate::json_string::json_string_len(text)),
        Value::Array(items) => items
            .iter()
            .try_fold(items.len().saturating_sub(1) + 2, |len, item| {
                len.checked_add(compact_json_len(item)?)
            }),
        Value::Object(fields) => {
            fields
                .iter()
                .try_fold(fields.len().saturating_sub(1) + 2, |len, (key, item)| {
                    len.checked_add(crate::json_string::json_string_len(key) + 1)?
                        .checked_add(compact_json_len(item)?)
                })
        }
    }
}

fn compact_number_len(number: &serde_json::Number) -> usize {
    let digits = |value: u64| value.checked_ilog10().map_or(1, |log| log as usize + 1);
    if let Some(value) = number.as_u64() {
        digits(value)
    } else if let Some(value) = number.as_i64() {
        1 + digits(value.unsigned_abs())
    } else {
        let mut writer = CountingWriter::default();
        let _ = serde_json::to_writer(&mut writer, number);
        writer.len
    }
}

fn finish_count(
    counter: CountingWriter,
    result: Result<(), PreparedOutputError>,
) -> Result<CountingWriter, PreparedOutputError> {
    match counter.failure {
        Some(CountFailure::Overflow) => Err(PreparedOutputError::LengthOverflow),
        Some(CountFailure::TooLarge(len)) => Err(PreparedOutputError::BodyTooLarge {
            len,
            max: MAX_WIRE_BODY_BYTES,
        }),
        None => {
            result?;
            Ok(counter)
        }
    }
}

fn measure_transform(segments: &TransformSegments) -> Result<usize, PreparedOutputError> {
    let mut counter = CountingWriter::default();
    let result = write_transform_envelope(segments, &mut counter, |counter, segment| {
        counter.add_len(segment.measured_len)
    });
    finish_count(counter, result).map(|counter| counter.len)
}

fn write_transform<W: Write>(
    segments: &TransformSegments,
    destination: &mut W,
) -> Result<(), PreparedOutputError> {
    write_transform_envelope(segments, destination, |destination, segment| {
        segment.write_to(destination)
    })
}

fn write_transform_envelope<W: Write>(
    segments: &TransformSegments,
    destination: &mut W,
    mut write_segment: impl FnMut(&mut W, &PreparedSegment) -> Result<(), PreparedOutputError>,
) -> Result<(), PreparedOutputError> {
    let (head, tail) = segments.envelope.split_at(segments.split);
    destination.write_all(head)?;
    destination.write_all(b"[")?;
    for (operation_index, operation) in segments.operations.iter().enumerate() {
        if operation_index > 0 {
            destination.write_all(b",")?;
        }
        match operation {
            RecipeSegment::Keep(segment) => write_segment(destination, segment)?,
            RecipeSegment::Insert(values) => {
                destination.write_all(INSERT_PREFIX)?;
                for (value_index, value) in values.iter().enumerate() {
                    if value_index > 0 {
                        destination.write_all(b",")?;
                    }
                    write_segment(destination, value)?;
                }
                destination.write_all(INSERT_SUFFIX)?;
            }
        }
    }
    destination.write_all(b"]")?;
    destination.write_all(tail)?;
    Ok(())
}

/// The split at `operations` lets `write_transform_envelope` splice prepared operations into the serialized envelope.
fn encode_envelope(envelope: &Map<String, Value>) -> Result<(Vec<u8>, usize), PreparedOutputError> {
    let mut encoded = StagedBody::new(Vec::new(), ENVELOPE_BYTES);
    let mut split = 0;
    encoded.byte(b'{')?;
    for (index, (key, value)) in envelope.iter().enumerate() {
        if index > 0 {
            encoded.byte(b',')?;
        }
        encoded.string(key)?;
        encoded.byte(b':')?;
        if key == "operations" {
            split = encoded.written();
        } else if !encoded.value_within(value, crate::edit_recipe::MAX_JSON_NESTING - 1)? {
            return Err(PreparedOutputError::RecipeNestingTooDeep);
        }
    }
    encoded.byte(b'}')?;
    Ok((encoded.into_bytes()?, split))
}

#[derive(Debug, Clone, Copy)]
enum CountFailure {
    Overflow,
    TooLarge(usize),
}

/// Serialization stops when encoded output exceeds `MAX_WIRE_BODY_BYTES`.
#[derive(Default)]
struct CountingWriter {
    len: usize,
    failure: Option<CountFailure>,
}

impl CountingWriter {
    fn add_len(&mut self, len: usize) -> Result<(), PreparedOutputError> {
        let Some(next) = self.len.checked_add(len) else {
            self.failure = Some(CountFailure::Overflow);
            return Err(PreparedOutputError::LengthOverflow);
        };
        if next > MAX_WIRE_BODY_BYTES {
            self.failure = Some(CountFailure::TooLarge(next));
            return Err(PreparedOutputError::BodyTooLarge {
                len: next,
                max: MAX_WIRE_BODY_BYTES,
            });
        }
        self.len = next;
        Ok(())
    }
}

impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.add_len(bytes.len())
            .map_err(|error| io::Error::new(io::ErrorKind::FileTooLarge, error.to_string()))?;
        Ok(bytes.len())
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.write(bytes).map(drop)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct BoundedWriter<'a, W> {
    inner: &'a mut W,
    max_len: usize,
    written: usize,
}

impl<'a, W> BoundedWriter<'a, W> {
    fn new(inner: &'a mut W, max_len: usize) -> Self {
        Self {
            inner,
            max_len,
            written: 0,
        }
    }

    fn written(&self) -> usize {
        self.written
    }

    /// Refuses a write of `len` bytes that would pass the measured length.
    fn admit(&self, len: usize) -> io::Result<()> {
        if len > self.max_len - self.written {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "prepared body exceeded measured length",
            ));
        }
        Ok(())
    }
}

impl<W: Write> Write for BoundedWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.admit(bytes.len())?;
        let written = self.inner.write(bytes)?;
        if written > bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "destination reported an invalid write length",
            ));
        }
        self.written += written;
        Ok(written)
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.admit(bytes.len())?;
        self.inner.write_all(bytes)?;
        self.written += bytes.len();
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_store::{BlockKind, HarnessMeta, ProviderExtras, WireBlock, WireMessage};
    use serde_json::json;

    #[test]
    fn a_json_value_over_the_limit_is_refused_at_the_serializer_length() {
        let value = Value::String("\u{1}".repeat(MAX_WIRE_BODY_BYTES / 6 + 1));
        let mut counter = CountingWriter::default();
        let result =
            serde_json::to_writer(&mut counter, &value).map_err(PreparedOutputError::Serialize);
        let expected = finish_count(counter, result).map(|counter| counter.len);
        assert!(matches!(
            expected,
            Err(PreparedOutputError::BodyTooLarge { .. })
        ));
        assert_eq!(
            format!("{:?}", measure_json(&value)),
            format!("{expected:?}")
        );
    }

    fn json_value() -> impl proptest::strategy::Strategy<Value = Value> {
        use proptest::prelude::*;
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<u64>().prop_map(Value::from),
            any::<i64>().prop_map(Value::from),
            any::<f64>().prop_map(Value::from),
            Just(Value::from(-0.0)),
            "(\\PC|[\\x00-\\x1f\"\\\\\u{7f}\u{2028}])*".prop_map(Value::from),
        ];
        leaf.prop_recursive(4, 64, 8, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
                proptest::collection::btree_map("(\\PC|[\\x00-\\x1f\"\\\\])*", inner, 0..8)
                    .prop_map(|fields| Value::Object(fields.into_iter().collect())),
            ]
        })
    }

    proptest::proptest! {
        /// The tree count is the serializer's count for every value under the cap.
        #[test]
        fn measured_json_length_is_the_serialized_length(value in json_value()) {
            proptest::prop_assert_eq!(compact_json_len(&value), Some(serde_json::to_vec(&value).unwrap().len()));
            proptest::prop_assert_eq!(measure_json(&value).unwrap(), serde_json::to_vec(&value).unwrap().len());
        }
    }

    #[test]
    fn measured_json_length_counts_integer_extremes_and_float_spellings() {
        use serde_json::json;
        for value in [
            json!(0),
            json!(9),
            json!(10),
            json!(u64::MAX),
            json!(i64::MIN),
            json!(-1),
            json!(1.0),
            json!(-0.0),
            json!(1e300),
            json!(5e-324),
            json!(0.1),
            json!([]),
            json!({}),
            json!([[], {}, null, true, false]),
            json!({"k\n": "v\u{1}"}),
        ] {
            assert_eq!(
                measure_json(&value).unwrap(),
                serde_json::to_vec(&value).unwrap().len(),
                "{value}"
            );
        }
    }

    /// A served message whose tool input nests `levels` arrays inside the block's own containers.
    fn served_with_nested_input(levels: usize) -> PreparedSegment {
        let mut input = serde_json::json!("leaf [brackets] {braces} \\\"quotes\\\"");
        for _ in 0..levels {
            input = serde_json::json!([input]);
        }
        let message = WireMessage::from_parts(
            "assistant",
            vec![WireBlock::bare(BlockKind::ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                input,
                provider_executed: false,
            })],
            None,
            ProviderExtras::new(),
            HarnessMeta::default(),
        );
        PreparedSegment::served(crate::transform::served_message_for_test(message))
    }

    #[test]
    fn served_segment_depth_agrees_with_the_byte_scan_at_the_recipe_limit() {
        let limit = crate::edit_recipe::MAX_JSON_NESTING - 4;
        let mut seen_both = (false, false);
        for levels in 0..=limit {
            let segment = served_with_nested_input(levels);
            let PreparedSegmentSource::Served(message) = &segment.source else {
                unreachable!("served segments keep their message");
            };
            let scanned = encoded_depth_within(message.canonical_bytes(), limit);
            assert_eq!(
                segment.fits_recipe_depth(),
                scanned,
                "{levels} nested levels, measured depth {}",
                message.nesting_depth()
            );
            match scanned {
                true => seen_both.0 = true,
                false => seen_both.1 = true,
            }
        }
        assert_eq!(seen_both, (true, true), "the sweep crosses the limit");
    }

    #[test]
    fn served_segment_past_the_recipe_limit_is_refused() {
        let limit = crate::edit_recipe::MAX_JSON_NESTING - 4;
        let envelope = || serde_json::json!({"status": "ok", "operations": null});
        let fitting = served_with_nested_input(limit.saturating_sub(4));
        assert!(fitting.fits_recipe_depth());
        PreparedOutput::transform_recipe(envelope(), vec![RecipeSegment::Insert(vec![fitting])])
            .expect("a served message within the limit is accepted");
        let too_deep = served_with_nested_input(limit);
        assert!(matches!(
            PreparedOutput::transform_recipe(
                envelope(),
                vec![RecipeSegment::Insert(vec![too_deep])]
            ),
            Err(PreparedOutputError::RecipeNestingTooDeep)
        ));
    }

    fn serialized_len(value: &Value) -> usize {
        serde_json::to_vec(value)
            .expect("a JSON value serializes")
            .len()
    }

    fn assert_measured_as_serialized(value: &Value) {
        let expected = serialized_len(value);
        assert_eq!(compact_json_len(value), Some(expected), "{value}");
        assert_eq!(measure_json(value).unwrap(), expected, "{value}");
    }

    #[test]
    fn structural_length_equals_the_serialized_length_for_every_escape_and_scalar() {
        let every_ascii_byte: String = (0u8..=0x7f).map(char::from).collect();
        let mut values = vec![
            json!(null),
            json!(true),
            json!(false),
            json!(""),
            json!([]),
            json!({}),
            json!(every_ascii_byte),
            json!("\u{e9}\u{e8}\u{ea} \u{2028}\u{2029} \u{1f600} \u{7f}"),
            json!({"\u{0}key\"\\": ["\u{1f}", "\u{8}\u{9}\u{a}\u{b}\u{c}\u{d}"]}),
            json!([
                0,
                1,
                -1,
                u64::MAX,
                i64::MIN,
                0.0,
                -0.0,
                1.5,
                -2.25e-7,
                1e21,
                1e300
            ]),
            json!([f64::MAX, f64::MIN_POSITIVE, 5e-324, 123_456_789.012_345_68]),
            json!({"info": {"id": "m1", "time": {"created": 1_700_000_000_001_u64}}, "parts": [{"type": "text", "text": "x".repeat(5 * 1024)}]}),
        ];
        // These cases exercise escape counting around 64-byte chunk boundaries.
        for len in [0, 1, 63, 64, 65, 127, 128, 129, 200] {
            for offset in [0, 31, 62, 63, 64, 65, 127, 128] {
                for escape in ['"', '\\', '\n', '\u{1}', '\u{1f}'] {
                    let mut text = "a".repeat(len);
                    if offset <= len {
                        text.insert(offset, escape);
                    }
                    values.push(json!(text));
                }
            }
        }
        for value in &values {
            assert_measured_as_serialized(value);
        }
    }

    fn escaped_json_value() -> impl proptest::strategy::Strategy<Value = Value> {
        use proptest::prelude::*;
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::from),
            any::<u64>().prop_map(Value::from),
            any::<f64>().prop_map(|number| json!(number)),
            any::<String>().prop_map(Value::String),
            "[\\x00-\\x7f]{0,150}".prop_map(Value::String),
        ];
        leaf.prop_recursive(4, 48, 6, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
                proptest::collection::btree_map(any::<String>(), inner, 0..6)
                    .prop_map(|fields| Value::Object(fields.into_iter().collect())),
            ]
        })
    }

    proptest::proptest! {
        #[test]
        fn structural_length_equals_the_serialized_length(value in escaped_json_value()) {
            proptest::prop_assert_eq!(compact_json_len(&value), Some(serialized_len(&value)));
            proptest::prop_assert_eq!(measure_json(&value).unwrap(), serialized_len(&value));
        }
    }

    #[test]
    fn the_cap_boundary_measures_as_the_serializer_measures() {
        let at_cap = Value::String("x".repeat(MAX_WIRE_BODY_BYTES - 2));
        assert_eq!(measure_json(&at_cap).unwrap(), MAX_WIRE_BODY_BYTES);
        assert_eq!(measure_serialize(&at_cap).unwrap(), MAX_WIRE_BODY_BYTES);

        let past_cap = Value::String("x".repeat(MAX_WIRE_BODY_BYTES - 1));
        assert_eq!(compact_json_len(&past_cap), Some(MAX_WIRE_BODY_BYTES + 1));
        let measured = measure_json(&past_cap);
        let serialized = measure_serialize(&past_cap);
        assert!(
            matches!(
                (&measured, &serialized),
                (
                    Err(PreparedOutputError::BodyTooLarge { len, max: MAX_WIRE_BODY_BYTES }),
                    Err(PreparedOutputError::BodyTooLarge { len: serialized_len, .. }),
                ) if len == serialized_len && *len > MAX_WIRE_BODY_BYTES
            ),
            "{measured:?} against {serialized:?}"
        );
    }

    /// Records the bytes and the number of `write` calls a direct destination receives.
    #[derive(Default)]
    struct RecordingDestination {
        bytes: Vec<u8>,
        writes: usize,
    }

    impl Write for RecordingDestination {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A transform recipe whose inserted values serialize as many small escaped fragments.
    fn fragmented_transform() -> PreparedOutput {
        let values = (0..64)
            .map(|index| {
                let text = format!("line {index}\n\t\"quoted\" \\path\n").repeat(8);
                let value = json!({"role": "tool", "content": [{"type": "text", "text": text}]});
                let len = serialized_len(&value);
                PreparedSegment::value(Arc::new(value), len)
            })
            .collect();
        PreparedOutput::transform_recipe(
            json!({"status": "ok", "operations": null, "timings": {"total_ms": 1.5}}),
            vec![
                RecipeSegment::Keep(PreparedSegment::exact(Arc::from(
                    &br#"{"op":"keep","count":2}"#[..],
                ))),
                RecipeSegment::Insert(values),
            ],
        )
        .expect("the recipe is valid")
    }

    /// The bytes the owned path writes for `output`, with the number of writes its serializer issues.
    fn owned_fragments(output: &PreparedOutput) -> RecordingDestination {
        let mut owned = RecordingDestination::default();
        output
            .measure()
            .expect("the body measures")
            .write_to(&mut owned)
            .expect("the body writes");
        owned
    }

    /// A direct destination receives a fragmented transform body in staging-sized writes carrying the owned path's bytes, and an exact body in one write.
    #[test]
    fn a_direct_body_reaches_its_destination_in_staged_writes_with_the_owned_bytes() {
        let output = fragmented_transform();
        let owned = owned_fragments(&output);
        let len = owned.bytes.len();
        assert!(
            len > 2 * crate::staged_body::STAGE_BYTES,
            "the body spans several staging buffers"
        );

        let mut direct = RecordingDestination::default();
        assert_eq!(output.write_measured(len, &mut direct).unwrap(), len);
        assert_eq!(direct.bytes, owned.bytes);
        let staged_bound = 2 * len.div_ceil(crate::staged_body::STAGE_BYTES);
        assert!(
            direct.writes <= staged_bound && owned.writes > 8 * staged_bound,
            "{} staged writes and {} serializer writes for {len} bytes",
            direct.writes,
            owned.writes
        );

        let exact = PreparedOutput::cached_bytes(owned.bytes.clone());
        let mut direct = RecordingDestination::default();
        assert_eq!(exact.write_measured(len, &mut direct).unwrap(), len);
        assert_eq!((direct.bytes, direct.writes), (owned.bytes, 1));
    }

    /// A staged body written at another length is refused, and a short length reaches the destination as at most that many bytes of the body.
    #[test]
    fn a_staged_body_written_at_another_length_is_refused() {
        let output = fragmented_transform();
        let owned = owned_fragments(&output).bytes;
        let len = owned.len();

        let mut short = RecordingDestination::default();
        assert!(matches!(
            output.write_measured(len - 1, &mut short),
            Err(PreparedOutputError::Write(_))
        ));
        assert!(short.bytes.len() < len && owned.starts_with(&short.bytes));

        let mut long = RecordingDestination::default();
        assert!(matches!(
            output.write_measured(len + 1, &mut long),
            Err(PreparedOutputError::LengthMismatch { measured, written })
                if measured == len + 1 && written == len
        ));
    }

    fn nested(levels: usize, leaf: Value) -> Value {
        (0..levels).fold(leaf, |inner, level| {
            if level % 2 == 0 {
                json!([1, inner, "x"])
            } else {
                json!({"a": null, "b": inner})
            }
        })
    }

    /// The recursive container count agrees with `validate_json_nesting` on both sides of every limit, through the error at the nesting maximum.
    #[test]
    fn the_container_count_agrees_with_the_nesting_validator() {
        let max = crate::edit_recipe::MAX_JSON_NESTING;
        for levels in [
            0,
            1,
            2,
            3,
            7,
            max - 5,
            max - 4,
            max - 3,
            max - 1,
            max,
            max + 1,
            max + 2,
        ] {
            for leaf in [json!(null), json!([]), json!({}), json!("s")] {
                let value = nested(levels, leaf);
                let validated = crate::edit_recipe::validate_json_nesting(&value);
                let limits = [
                    0,
                    1,
                    2,
                    3,
                    levels.saturating_sub(1),
                    levels,
                    levels + 1,
                    max - 4,
                    max,
                ];
                for limit in limits.into_iter().filter(|limit| *limit <= max) {
                    assert_eq!(
                        containers_within(&value, limit),
                        validated.as_ref().is_ok_and(|depth| *depth <= limit),
                        "{levels} levels against limit {limit}"
                    );
                }
            }
        }
    }

    proptest::proptest! {
        #[test]
        fn the_container_count_agrees_with_the_validator_on_any_value(value in json_value(), limit in 0usize..8) {
            let validated = crate::edit_recipe::validate_json_nesting(&value);
            proptest::prop_assert_eq!(
                containers_within(&value, limit),
                validated.as_ref().is_ok_and(|depth| *depth <= limit)
            );
        }
    }

    /// A transform body equals `serde_json` of the whole response wherever `operations` sorts among the envelope's keys, including an envelope larger than its staging buffer.
    #[test]
    fn a_transform_body_is_the_serialized_response_wherever_its_operations_sort() {
        let note = "note \"quoted\"\n".repeat(2 * ENVELOPE_BYTES / 10);
        for envelope in [
            json!({"operations": null}),
            json!({"operations": null, "z": 1}),
            json!({"a": "x", "operations": null}),
            json!({"a": note, "operations": null, "z": [1.5, -2, {"k": note}]}),
        ] {
            let operations = vec![
                RecipeSegment::Keep(PreparedSegment::exact(Arc::from(
                    &br#"{"count":2,"op":"keep"}"#[..],
                ))),
                RecipeSegment::Insert(vec![PreparedSegment::value(
                    Arc::new(json!({"role": "user", "text": "hi\n"})),
                    serialized_len(&json!({"role": "user", "text": "hi\n"})),
                )]),
            ];
            let mut whole = envelope.clone();
            whole["operations"] = json!([
                {"op": "keep", "count": 2},
                {"op": "insert", "values": [{"role": "user", "text": "hi\n"}]},
            ]);
            let expected = serde_json::to_vec(&whole).unwrap();
            let output = PreparedOutput::transform_recipe(envelope, operations).unwrap();
            let measured = output.measure().unwrap();
            assert_eq!(measured.len(), expected.len());
            let mut owned = Vec::new();
            measured.write_to(&mut owned).unwrap();
            assert_eq!(owned, expected);
            let mut direct = Vec::new();
            assert_eq!(
                output.write_measured(expected.len(), &mut direct).unwrap(),
                expected.len()
            );
            assert_eq!(direct, expected);
        }
    }

    /// An envelope nested past the limit is refused as too deep whatever its shape, and a shallow envelope without a null `operations` is refused as invalid.
    #[test]
    fn an_envelope_is_refused_as_too_deep_before_its_shape_is_judged() {
        let max = crate::edit_recipe::MAX_JSON_NESTING;
        let refusal = |envelope: Value| {
            PreparedOutput::transform_recipe(envelope, Vec::new())
                .err()
                .map(|error| error.to_string())
        };
        let too_deep = PreparedOutputError::RecipeNestingTooDeep.to_string();
        let invalid = PreparedOutputError::InvalidTransformEnvelope.to_string();
        let fits = nested(max - 1, json!(null));
        let deep = nested(max, json!(null));
        assert_eq!(
            refusal(json!({"operations": null, "z": fits.clone()})),
            None
        );
        assert_eq!(
            refusal(json!({"operations": null, "z": deep.clone()})),
            Some(too_deep.clone())
        );
        assert_eq!(
            refusal(json!({"operations": [], "z": deep.clone()})),
            Some(too_deep.clone())
        );
        assert_eq!(refusal(nested(max + 1, json!(null))), Some(too_deep));
        assert_eq!(
            refusal(json!({"operations": [], "z": fits})),
            Some(invalid.clone())
        );
        assert_eq!(refusal(json!({"z": 1})), Some(invalid.clone()));
        assert_eq!(refusal(json!([null])), Some(invalid));
    }
}

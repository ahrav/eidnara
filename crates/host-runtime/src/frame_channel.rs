//! The module defines a private complete-frame channel boundary between the connection engine and a transport.
//!
//! The contract is directional: a cloneable [`FrameSender`] admits complete
//! outbound frames in FIFO order against one logical writer, and the
//! single-owner receive side yields complete, structurally validated
//! inbound frames. Direct producers fill bounded transport spans through a
//! cursor and commit one exact length. Receive bytes are visible only through
//! a lexical [`ReceiveLease`]; contiguous consumers use the explicit
//! copying adapter before entering asynchronous work.

use std::io;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use tokio::sync::mpsc;

use crate::wire::{AdmissionClass, EnvelopeHeader, FrameType};
use tokio::time::{Duration, Instant, timeout_at};
use tokio_util::sync::CancellationToken;

use crate::wire::MAX_BODY_LEN;

#[cfg(test)]
pub(crate) mod contract_tests;

/// ReadClose identifies why a generation is retired without another frame.
#[derive(Debug)]
#[allow(dead_code)]
pub enum ReadClose {
    /// CleanEof reports a clean close at a frame boundary before any byte of the next frame.
    CleanEof,
    /// Corrupt reports structural stream corruption or a read-deadline expiry.
    Corrupt(&'static str),
    /// The generation or host was cancelled while reading.
    Cancelled,
    /// A resource wait (ingress budget) outlasted its deadline: the peer
    /// and the transport are healthy, so retirement is clean backpressure,
    /// not a structural fault.
    Overloaded,
    Io(std::io::Error),
    /// RejectedDrainFailed reports failed realignment after a rejected frame.
    RejectedDrainFailed,
}

/// one place.
///
pub(crate) fn validate_inbound_header(header: EnvelopeHeader) -> Result<(), ReadClose> {
    if header.len > MAX_BODY_LEN {
        return Err(ReadClose::Corrupt("body over interoperability cap"));
    }
    if header.ty.is_pure_header()
        && (header.flags.is_binary()
            || header.flags.is_last()
            || header.flags.admission_class() != Some(AdmissionClass::Normal))
    {
        return Err(ReadClose::Corrupt("invalid pure-header flags"));
    }
    if !matches!(
        header.ty,
        FrameType::Request | FrameType::Cancel | FrameType::Pong | FrameType::Goodbye
    ) {
        return Err(ReadClose::Corrupt("role-invalid frame type"));
    }
    Ok(())
}

///
/// Direct/leased paths leave this at zero. Flattening adapters add exactly one
/// for each body they copy into owned semantic storage.
#[derive(Clone, Default)]
pub struct CopyCounter(Arc<AtomicU64>);

impl CopyCounter {
    pub fn copies(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    pub(crate) fn record_copy(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// The `Rc` marker makes this view `!Send`.
///
/// The callback can return only values that do not borrow the leased bytes.
///
/// ```compile_fail
/// use host_runtime::frame_channel::ReceiveLease;
/// fn require_send<T: Send>(_: T) {}
/// let bytes = [1u8, 2, 3];
/// require_send(ReceiveLease::contiguous(&bytes));
/// ```
///
/// ```compile_fail
/// use host_runtime::frame_channel::ReceiveLease;
/// fn require_static<T: 'static>(_: T) {}
/// let bytes = [1u8, 2, 3];
/// require_static(ReceiveLease::contiguous(&bytes));
/// ```
pub struct ReceiveLease<'lease> {
    first: &'lease [u8],
    second: Option<&'lease [u8]>,
    _not_send: PhantomData<Rc<()>>,
}

impl<'lease> ReceiveLease<'lease> {
    pub fn contiguous(bytes: &'lease [u8]) -> Self {
        Self::segmented(bytes, None)
    }

    pub fn segmented(first: &'lease [u8], second: Option<&'lease [u8]>) -> Self {
        Self {
            first,
            second,
            _not_send: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.first
            .len()
            .saturating_add(self.second.map_or(0, <[u8]>::len))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn segment_count(&self) -> usize {
        usize::from(!self.first.is_empty())
            + usize::from(self.second.is_some_and(|s| !s.is_empty()))
    }

    pub fn segment(&self, index: usize) -> Option<&[u8]> {
        match (index, self.first.is_empty()) {
            (0, false) => Some(self.first),
            (0, true) => self.second.filter(|segment| !segment.is_empty()),
            (1, false) => self.second.filter(|segment| !segment.is_empty()),
            _ => None,
        }
    }

    pub fn contiguous_bytes(&self) -> Option<&[u8]> {
        self.second.is_none().then_some(self.first)
    }

    /// Explicit contiguous-body adapter. One call records one body copy even
    /// when the body is empty.
    pub fn to_owned(&self, counter: &CopyCounter) -> Vec<u8> {
        let mut body = Vec::with_capacity(self.len());
        body.extend_from_slice(self.first);
        if let Some(second) = self.second {
            body.extend_from_slice(second);
        }
        counter.record_copy();
        body
    }
}

/// One admitted inbound frame. Body bytes can only be observed through
/// [`InboundFrame::with_lease`] or moved/copied through [`InboundFrame::into_owned`].
pub struct InboundFrame {
    pub header: EnvelopeHeader,
    body: Vec<u8>,
    charge: crate::wire::ByteCharge,
    copies: CopyCounter,
}

impl InboundFrame {
    pub(crate) fn owned(
        header: EnvelopeHeader,
        body: Vec<u8>,
        charge: crate::wire::ByteCharge,
        copies: CopyCounter,
    ) -> Self {
        Self {
            header,
            body,
            charge,
            copies,
        }
    }

    /// `CopyCounter` excludes copies made when an adapter flattens wrapped bodies into owned storage outside this module.
    pub(crate) fn copy_counter(&self) -> CopyCounter {
        self.copies.clone()
    }

    pub fn body_len(&self) -> usize {
        self.body.len()
    }

    /// `with_lease` confines transport-byte decoding to a non-escaping lexical scope.
    pub fn with_lease<T>(&self, decode: impl for<'lease> FnOnce(ReceiveLease<'lease>) -> T) -> T {
        decode(ReceiveLease::contiguous(&self.body))
    }

    /// `InboundFrame::into_owned` moves the body without copying.
    pub fn into_owned(self) -> OwnedInboundFrame {
        let Self {
            header,
            body,
            charge,
            copies: _,
        } = self;
        OwnedInboundFrame {
            header,
            body,
            charge,
        }
    }
}

/// Asynchronous handlers receive owned semantic input only.
pub struct OwnedInboundFrame {
    pub header: EnvelopeHeader,
    pub body: Vec<u8>,
    pub charge: crate::wire::ByteCharge,
}

pub struct RejectedFrame {
    pub corr: u64,
}

pub enum InboundEvent {
    Frame(InboundFrame),
    Rejected(RejectedFrame),
}

pub(crate) type DirectSerializer =
    Box<dyn FnOnce(&mut dyn io::Write) -> io::Result<()> + Send + 'static>;

pub struct DirectFrame {
    header: [u8; crate::wire::HEADER_LEN],
    body_len: usize,
    serializer: DirectSerializer,
}

impl DirectFrame {
    pub(crate) fn new(
        header: EnvelopeHeader,
        body_len: usize,
        serializer: DirectSerializer,
    ) -> Self {
        Self {
            header: header.encode(),
            body_len,
            serializer,
        }
    }

    pub(crate) const fn header(&self) -> [u8; crate::wire::HEADER_LEN] {
        self.header
    }

    pub(crate) const fn body_len(&self) -> usize {
        self.body_len
    }

    pub(crate) fn serialize(self, writer: &mut dyn io::Write) -> io::Result<()> {
        (self.serializer)(writer)
    }
}

/// `OutboundFrame` queues one encoded frame for the single logical writer.
pub struct OutboundFrame {
    pub bytes: Vec<u8>,
    /// `tail` follows `bytes` when encoding avoids a prepend copy.
    pub tail: Vec<u8>,
    pub(crate) direct: Option<DirectFrame>,
    pub charge: crate::wire::ByteCharge,
    /// `written` runs after every frame byte reaches local egress.
    pub written: Option<Box<dyn FnOnce(Instant) + Send>>,
}

const QUEUED: u8 = 0;
const CANCELLED: u8 = 1;
const PUBLISHED: u8 = 2;
pub(crate) const COMPLETE: u8 = 3;

pub(crate) struct QueuedOutboundFrame {
    pub(crate) frame: OutboundFrame,
    pub(crate) state: Arc<AtomicU8>,
    on_publish: Option<Box<dyn FnOnce() + Send>>,
}

impl QueuedOutboundFrame {
    pub(crate) fn begin_publication(&mut self) -> bool {
        if self
            .state
            .compare_exchange(QUEUED, PUBLISHED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        if let Some(on_publish) = self.on_publish.take() {
            on_publish();
        }
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    NotSent,
    PossibleSend,
}

#[derive(Clone)]
pub struct FrameSendTicket {
    state: Arc<AtomicU8>,
}

impl FrameSendTicket {
    pub fn cancel(&self) -> SendOutcome {
        match self
            .state
            .compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => SendOutcome::NotSent,
            Err(_) => SendOutcome::PossibleSend,
        }
    }
}

#[derive(Clone)]
pub struct FrameSender {
    tx: mpsc::Sender<QueuedOutboundFrame>,
    retired: CancellationToken,
    generation: CancellationToken,
    discard: CancellationToken,
    finish: CancellationToken,
    admission_timeout: Duration,
}

impl FrameSender {
    pub fn finish(&self) {
        self.finish.cancel();
    }

    pub fn discard(&self) {
        self.discard.cancel();
    }

    /// Admission adapter for callers that do not need a ticket.
    pub async fn send(&self, frame: OutboundFrame) -> Result<(), WriterGone> {
        self.send_before(frame, self.admission_deadline()).await
    }

    pub fn admission_deadline(&self) -> Instant {
        Instant::now() + self.admission_timeout
    }

    /// Admission adapter for callers that do not need a ticket.
    pub async fn send_before(
        &self,
        frame: OutboundFrame,
        deadline: Instant,
    ) -> Result<(), WriterGone> {
        self.send_ticket_before(frame, deadline, None)
            .await
            .map(drop)
    }

    pub async fn send_ticket_before(
        &self,
        frame: OutboundFrame,
        deadline: Instant,
        on_publish: Option<Box<dyn FnOnce() + Send>>,
    ) -> Result<FrameSendTicket, WriterGone> {
        let state = Arc::new(AtomicU8::new(QUEUED));
        let queued = QueuedOutboundFrame {
            frame,
            state: Arc::clone(&state),
            on_publish,
        };
        tokio::select! {
            biased;
            () = self.retired.cancelled() => Err(WriterGone),
            sent = timeout_at(deadline, self.tx.send(queued)) => match sent {
                Ok(sent) => sent
                    .map(|()| FrameSendTicket { state })
                    .map_err(|_| WriterGone),
                Err(_) => {
                    self.retired.cancel();
                    self.generation.cancel();
                    Err(WriterGone)
                }
            },
        }
    }

    pub fn is_retired(&self) -> bool {
        self.retired.is_cancelled()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriterGone;

pub(crate) struct SenderQueue {
    rx: mpsc::Receiver<QueuedOutboundFrame>,
    pub retired: CancellationToken,
    pub discard: CancellationToken,
    pub finish: CancellationToken,
}

impl SenderQueue {
    pub(crate) async fn recv(&mut self) -> Option<QueuedOutboundFrame> {
        self.rx.recv().await
    }

    pub(crate) fn try_recv(&mut self) -> Result<QueuedOutboundFrame, mpsc::error::TryRecvError> {
        self.rx.try_recv()
    }
}

pub(crate) fn frame_sender(
    queue_frames: usize,
    generation: CancellationToken,
    admission_timeout: Duration,
) -> (FrameSender, SenderQueue) {
    let (tx, rx) = mpsc::channel::<QueuedOutboundFrame>(queue_frames);
    let retired = CancellationToken::new();
    let discard = CancellationToken::new();
    let finish = CancellationToken::new();
    let sender = FrameSender {
        tx,
        retired: retired.clone(),
        generation: generation.clone(),
        discard: discard.clone(),
        finish: finish.clone(),
        admission_timeout,
    };
    let queue = SenderQueue {
        rx,
        retired,
        discard,
        finish,
    };
    (sender, queue)
}

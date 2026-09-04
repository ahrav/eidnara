//! The module defines a private complete-frame channel boundary between the connection engine and a transport.
//!
//! The contract is directional: a cloneable [`FrameSender`] admits complete
//! outbound frames in FIFO order against one logical writer, and the
//! single-owner receive side yields complete, structurally validated
//! inbound frames. Receive bytes are visible only through a lexical
//! [`ReceiveLease`]; consumers that need owned bytes use the explicit copying
//! adapter before entering asynchronous work.

use std::io;
use std::marker::PhantomData;
use std::rc::Rc;

use tokio::sync::mpsc;

use crate::wire::{AdmissionClass, EnvelopeHeader, FrameType};
use tokio::time::{Duration, Instant, timeout_at};
use tokio_util::sync::CancellationToken;

use crate::wire::MAX_BODY_LEN;

#[cfg(test)]
pub(crate) mod contract_tests;

/// ReadClose identifies why a generation is retired without another frame.
#[derive(Debug)]
pub enum ReadClose {
    /// CleanEof reports a clean close at a frame boundary before any byte of the next frame.
    CleanEof,
    /// Corrupt reports structural stream corruption, a transport fault, or a read-deadline expiry.
    Corrupt(&'static str),
    /// The read side was cancelled; the writer may still be draining.
    Cancelled,
    /// A resource wait (ingress budget) outlasted its deadline: the peer
    /// and the transport are healthy, so retirement is clean backpressure,
    /// not a structural fault.
    Overloaded,
}

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
    bytes: &'lease [u8],
    _not_send: PhantomData<Rc<()>>,
}

impl<'lease> ReceiveLease<'lease> {
    pub fn contiguous(bytes: &'lease [u8]) -> Self {
        Self {
            bytes,
            _not_send: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes
    }

    /// Explicit owned-body adapter for consumers that outlive the lease.
    pub fn to_owned(&self) -> Vec<u8> {
        self.bytes.to_vec()
    }
}

/// One admitted inbound frame. Body bytes can only be observed through
/// [`InboundFrame::with_lease`] or moved through [`InboundFrame::into_owned`].
pub struct InboundFrame {
    pub header: EnvelopeHeader,
    body: Vec<u8>,
    charge: crate::wire::ByteCharge,
}

impl InboundFrame {
    pub(crate) fn owned(
        header: EnvelopeHeader,
        body: Vec<u8>,
        charge: crate::wire::ByteCharge,
    ) -> Self {
        Self {
            header,
            body,
            charge,
        }
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

#[derive(Clone)]
pub struct FrameSender {
    tx: mpsc::Sender<OutboundFrame>,
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

    pub async fn send(&self, frame: OutboundFrame) -> Result<(), WriterGone> {
        self.send_before(frame, self.admission_deadline()).await
    }

    pub fn admission_deadline(&self) -> Instant {
        Instant::now() + self.admission_timeout
    }

    /// An expired admission deadline retires the writer and the generation.
    pub async fn send_before(
        &self,
        frame: OutboundFrame,
        deadline: Instant,
    ) -> Result<(), WriterGone> {
        tokio::select! {
            biased;
            () = self.retired.cancelled() => Err(WriterGone),
            sent = timeout_at(deadline, self.tx.send(frame)) => match sent {
                Ok(sent) => sent.map_err(|_| WriterGone),
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
    rx: mpsc::Receiver<OutboundFrame>,
    pub retired: CancellationToken,
    pub discard: CancellationToken,
    pub finish: CancellationToken,
}

impl SenderQueue {
    pub(crate) async fn recv(&mut self) -> Option<OutboundFrame> {
        self.rx.recv().await
    }

    pub(crate) fn try_recv(&mut self) -> Result<OutboundFrame, mpsc::error::TryRecvError> {
        self.rx.try_recv()
    }
}

pub(crate) fn frame_sender(
    queue_frames: usize,
    generation: CancellationToken,
    admission_timeout: Duration,
) -> (FrameSender, SenderQueue) {
    let (tx, rx) = mpsc::channel::<OutboundFrame>(queue_frames);
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

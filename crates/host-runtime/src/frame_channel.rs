//! The module defines a private complete-frame channel boundary between the connection engine and a transport.
//!
//! The contract is directional: a cloneable [`FrameSender`] admits complete
//! outbound frames in FIFO order against one logical writer, and the
//! single-owner receive side yields complete, structurally validated
//! inbound frames whose bodies are owned transport leases. Receive bytes are
//! copied into private memory through [`InboundFrame::into_private`] before any
//! decoder sees them; nothing exposes a slice over the shared mapping.

use std::io;
use std::sync::{Arc, PoisonError, RwLock};

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

/// One admitted inbound frame: the validated header, the owned transport lease that holds the
/// body, and the ingress charge reserved for its private copy. Body bytes leave the transport
/// only through [`InboundFrame::into_private`], which copies them into stable private memory,
/// validates the copied length against the header, and returns the block before any parser
/// runs. The lease is `Send`, so the copy may run on a blocking worker joined by the request's
/// work ledgers.
pub struct InboundFrame {
    pub header: EnvelopeHeader,
    lease: shm_transport::lease::PayloadLease,
    charge: crate::wire::ByteCharge,
}

/// Why a frame's body could not become private bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateCopyError {
    /// The copied body length disagrees with the header's declared length.
    LengthMismatch,
    /// The transport view could not be read.
    Transport,
}

impl InboundFrame {
    pub(crate) fn new(
        header: EnvelopeHeader,
        lease: shm_transport::lease::PayloadLease,
        charge: crate::wire::ByteCharge,
    ) -> Self {
        Self {
            header,
            lease,
            charge,
        }
    }

    /// Body length the transport delivered.
    pub fn len(&self) -> usize {
        self.lease.len()
    }

    /// Whether the body is empty.
    pub fn is_empty(&self) -> bool {
        self.lease.is_empty()
    }

    /// Returns the transport block without copying. A pure-header frame has no body to copy;
    /// its lease still returns through the doorbell, and a failed return is `Transport`.
    pub fn release(self) -> Result<(), PrivateCopyError> {
        let Self { lease, charge, .. } = self;
        drop(charge);
        lease.release().map_err(|_| PrivateCopyError::Transport)
    }

    /// Copies the body into private bytes and returns the transport block. A pure-header frame
    /// copies nothing. The copied length is checked against the header before the bytes are
    /// handed to any decoder, and the lease is released before this returns, so no storage or
    /// response work ever observes transport input still held.
    pub fn into_private(self) -> Result<OwnedInboundFrame, PrivateCopyError> {
        let Self {
            header,
            lease,
            charge,
        } = self;
        let body = if lease.is_empty() {
            Vec::new()
        } else {
            lease.to_vec().map_err(|_| PrivateCopyError::Transport)?
        };
        lease.release().map_err(|_| PrivateCopyError::Transport)?;
        if body.len() as u64 != u64::from(header.len) {
            return Err(PrivateCopyError::LengthMismatch);
        }
        Ok(OwnedInboundFrame {
            header,
            body,
            charge,
        })
    }
}

/// Asynchronous handlers and control decoders receive private bytes only.
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
    /// Terminal credit that follows the frame's block until the block physically returns.
    pub credit: Option<tokio::sync::OwnedSemaphorePermit>,
}

/// Senders hold `admission` shared across the retired re-check and the queue push; the finishing endpoint takes it exclusively so its final empty `try_recv` proves no admitted frame is still landing.
type AdmissionGate = Arc<RwLock<()>>;

#[derive(Clone)]
pub struct FrameSender {
    tx: mpsc::Sender<OutboundFrame>,
    retired: CancellationToken,
    generation: CancellationToken,
    discard: CancellationToken,
    finish: CancellationToken,
    admission: AdmissionGate,
    admission_timeout: Duration,
}

impl FrameSender {
    /// Closes admission before the endpoint drains, so every frame `send` admitted is published and none admitted afterwards is silently dropped.
    pub fn finish(&self) {
        self.retired.cancel();
        self.finish.cancel();
    }

    /// Closes admission, then drops every queued frame.
    pub fn discard(&self) {
        self.retired.cancel();
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
        let permit = tokio::select! {
            biased;
            () = self.retired.cancelled() => return Err(WriterGone),
            reserved = timeout_at(deadline, self.tx.reserve()) => match reserved {
                Ok(Ok(permit)) => permit,
                Ok(Err(_)) => return Err(WriterGone),
                Err(_) => {
                    self.retired.cancel();
                    self.generation.cancel();
                    return Err(WriterGone);
                }
            },
        };
        let _admitted = self
            .admission
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        if self.retired.is_cancelled() {
            return Err(WriterGone);
        }
        permit.send(frame);
        Ok(())
    }

    pub fn is_retired(&self) -> bool {
        self.retired.is_cancelled()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriterGone;

pub(crate) struct SenderQueue {
    rx: mpsc::Receiver<OutboundFrame>,
    admission: AdmissionGate,
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

    /// Takes the next queued frame after `finish`. `None` is final: admission is closed and every push that passed its retired check has landed.
    pub(crate) fn drain_finished(&mut self) -> Option<OutboundFrame> {
        if let Ok(frame) = self.rx.try_recv() {
            return Some(frame);
        }
        let _exclusive = self
            .admission
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        self.rx.try_recv().ok()
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
    let admission: AdmissionGate = Arc::new(RwLock::new(()));
    let sender = FrameSender {
        tx,
        retired: retired.clone(),
        generation: generation.clone(),
        discard: discard.clone(),
        finish: finish.clone(),
        admission: Arc::clone(&admission),
        admission_timeout,
    };
    let queue = SenderQueue {
        rx,
        admission,
        retired,
        discard,
        finish,
    };
    (sender, queue)
}

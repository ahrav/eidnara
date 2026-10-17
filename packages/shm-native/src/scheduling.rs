use std::collections::{BTreeSet, HashMap};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use napi::bindgen_prelude::Function;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::{Error, Result, Status};
use rustix::buffer::spare_capacity;
use rustix::event::{EventfdFlags, epoll, eventfd};
use rustix::io::Errno;
use shm_transport::backend::ring::Ring;

fn retry_interrupted<T>(
    closing: &AtomicBool,
    mut operation: impl FnMut() -> std::result::Result<T, Errno>,
) -> std::result::Result<Option<T>, Errno> {
    loop {
        if closing.load(Ordering::Acquire) {
            return Ok(None);
        }
        match operation() {
            Err(Errno::INTR) => {}
            result => return result.map(Some),
        }
    }
}

/// Identifies the control eventfd; channel events start at 4, so none share it.
const CONTROL_EVENT: u64 = 0;

/// Which of a channel's descriptors an epoll event names; the low two bits of the event data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventKind {
    /// The inbound data doorbell.
    Data = 0,
    /// The setup socket; readiness there is the host closing its end.
    Setup = 1,
    /// The outbound capacity doorbell: the host consumed a descriptor or returned a block.
    Capacity = 2,
}

fn channel_event(channel_id: u32, kind: EventKind) -> u64 {
    ((u64::from(channel_id) + 1) << 2) | kind as u64
}

/// Epoll event data for a channel's data doorbell.
fn data_event(channel_id: u32) -> u64 {
    channel_event(channel_id, EventKind::Data)
}

/// Epoll event data for a channel's setup socket.
fn setup_event(channel_id: u32) -> u64 {
    channel_event(channel_id, EventKind::Setup)
}

/// Epoll event data for a channel's capacity doorbell.
fn capacity_event(channel_id: u32) -> u64 {
    channel_event(channel_id, EventKind::Capacity)
}

/// Inverse of the `*_event` encodings.
fn decode_event(data: u64) -> (u32, EventKind) {
    let kind = match data & 0b11 {
        1 => EventKind::Setup,
        2 => EventKind::Capacity,
        _ => EventKind::Data,
    };
    (((data >> 2) - 1) as u32, kind)
}

/// Channels the watcher saw wake, plus the setup sockets that reported hangup. `data` and
/// `capacity` hold wakes recorded since the last dispatch began; starting a dispatch seals
/// them into `dispatched_data` and `dispatched_capacity`, which that dispatch's end settles.
/// A wake recorded while a dispatch runs therefore reaches the next dispatch instead of being
/// settled by one that started before it. `capacity` holds channels whose outbound capacity
/// doorbell rang while a producer was parked on it. `peer_closed` is latched rather than
/// drained: the hangup fires once (the registration is one-shot) and `peer_closed()` must
/// keep reporting it until the channel unregisters; `hangup_undispatched` records a hangup
/// that no dispatch has started since.
#[derive(Default)]
struct ReadyState {
    data: BTreeSet<u32>,
    capacity: BTreeSet<u32>,
    dispatched_data: BTreeSet<u32>,
    dispatched_capacity: BTreeSet<u32>,
    peer_closed: BTreeSet<u32>,
    hangup_undispatched: bool,
}

impl ReadyState {
    fn has_undispatched(&self) -> bool {
        !self.data.is_empty() || !self.capacity.is_empty() || self.hangup_undispatched
    }

    /// Hands every recorded wake to the dispatch that is starting.
    fn seal(&mut self) {
        self.dispatched_data.append(&mut self.data);
        self.dispatched_capacity.append(&mut self.capacity);
        self.hangup_undispatched = false;
    }
}

/// The setup socket carries no traffic after activation, so any readiness on it is the
/// host closing its end. `ONESHOT` disables the registration after that first report; a
/// level-triggered hangup would otherwise wake the reactor on every `epoll_wait` until
/// the channel closes.
fn register_setup_socket(
    reactor: &OwnedFd,
    setup: &UnixStream,
    event_data: u64,
) -> Result<OwnedFd> {
    let setup = setup
        .try_clone()
        .map_err(|_| Error::new(Status::GenericFailure, "readiness registration failed"))?;
    epoll::add(
        reactor,
        &setup,
        epoll::EventData::new_u64(event_data),
        epoll::EventFlags::IN
            | epoll::EventFlags::HUP
            | epoll::EventFlags::ERR
            | epoll::EventFlags::RDHUP
            | epoll::EventFlags::ONESHOT,
    )
    .map_err(|_| Error::new(Status::GenericFailure, "readiness registration failed"))?;
    Ok(setup.into())
}

/// Native worker limit.
pub(crate) const WORKER_LIMIT: u32 = 0;

type ReadinessCallback = ThreadsafeFunction<(), (), (), Status, false, true, 2>;

struct Registration {
    descriptors: Vec<OwnedFd>,
    /// The outbound capacity doorbell, registered on the first `arm_capacity`.
    capacity: Option<OwnedFd>,
}

pub(crate) struct Reactor {
    epoll: Arc<OwnedFd>,
    control: Arc<OwnedFd>,
    registrations: HashMap<u32, Registration>,
    ready: Arc<Mutex<ReadyState>>,
    pending: Arc<AtomicBool>,
    kick: Arc<AtomicBool>,
    closing: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    _callback: Arc<ReadinessCallback>,
    watcher: Option<JoinHandle<()>>,
}

impl Reactor {
    pub(crate) fn new(callback: Function<(), ()>) -> Result<Self> {
        let callback = Arc::new(
            callback
                .build_threadsafe_function::<()>()
                .weak::<true>()
                .max_queue_size::<2>()
                .build()?,
        );
        let epoll = Arc::new(
            epoll::create(epoll::CreateFlags::CLOEXEC)
                .map_err(|_| Error::new(Status::GenericFailure, "readiness reactor failed"))?,
        );
        let control = Arc::new(
            eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)
                .map_err(|_| Error::new(Status::GenericFailure, "readiness reactor failed"))?,
        );
        epoll::add(
            &epoll,
            &control,
            epoll::EventData::new_u64(CONTROL_EVENT),
            epoll::EventFlags::IN,
        )
        .map_err(|_| Error::new(Status::GenericFailure, "readiness reactor failed"))?;
        let ready = Arc::new(Mutex::new(ReadyState::default()));
        let pending = Arc::new(AtomicBool::new(false));
        let kick = Arc::new(AtomicBool::new(false));
        let closing = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let watcher = {
            let epoll = Arc::clone(&epoll);
            let control = Arc::clone(&control);
            let ready_state = Arc::clone(&ready);
            let pending = Arc::clone(&pending);
            let kick = Arc::clone(&kick);
            let closing = Arc::clone(&closing);
            let failed = Arc::clone(&failed);
            let callback = Arc::clone(&callback);
            std::thread::Builder::new()
                .name("shm-readiness".to_owned())
                .spawn(move || {
                    let mut events = Vec::with_capacity(64);
                    loop {
                        events.clear();
                        match retry_interrupted(&closing, || {
                            epoll::wait(&epoll, spare_capacity(&mut events), None)
                        }) {
                            Ok(Some(_)) => {}
                            Ok(None) => break,
                            Err(_) => {
                                failed.store(true, Ordering::Release);
                                if pending
                                    .compare_exchange(
                                        false,
                                        true,
                                        Ordering::AcqRel,
                                        Ordering::Acquire,
                                    )
                                    .is_ok()
                                    && callback.call((), ThreadsafeFunctionCallMode::NonBlocking)
                                        != Status::Ok
                                {
                                    pending.store(false, Ordering::Release);
                                }
                                break;
                            }
                        }
                        let mut ready = false;
                        for event in events.drain(..) {
                            let data = event.data.u64();
                            if data == CONTROL_EVENT {
                                let mut value = [0u8; 8];
                                let _ = rustix::io::read(&control, &mut value);
                                ready |= kick.swap(false, Ordering::AcqRel);
                                continue;
                            }
                            let (channel_id, kind) = decode_event(data);
                            if let Ok(mut state) = ready_state.lock() {
                                match kind {
                                    EventKind::Setup => {
                                        state.peer_closed.insert(channel_id);
                                        state.hangup_undispatched = true;
                                    }
                                    EventKind::Data => {
                                        state.data.insert(channel_id);
                                    }
                                    EventKind::Capacity => {
                                        state.capacity.insert(channel_id);
                                    }
                                }
                            }
                            ready = true;
                        }
                        if closing.load(Ordering::Acquire) {
                            break;
                        }
                        // Readiness recorded while a dispatch is pending is claimed by that
                        // dispatch's `handled`; the doorbells are edge-triggered, so the watcher
                        // returns to `epoll_wait` at once instead of waiting for it.
                        if ready
                            && pending
                                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                                .is_ok()
                        {
                            if let Ok(mut state) = ready_state.lock() {
                                state.seal();
                            }
                            let status = callback.call((), ThreadsafeFunctionCallMode::NonBlocking);
                            if status != Status::Ok {
                                // A rejected call ends the reactor: nothing drains the doorbells
                                // or reports the setup hangup.
                                failed.store(true, Ordering::Release);
                                pending.store(false, Ordering::Release);
                                break;
                            }
                        }
                    }
                })
                .map_err(|_| Error::new(Status::GenericFailure, "readiness reactor failed"))?
        };
        Ok(Self {
            epoll,
            control,
            registrations: HashMap::new(),
            ready,
            pending,
            kick,
            closing,
            failed,
            _callback: callback,
            watcher: Some(watcher),
        })
    }

    pub(crate) fn register(
        &mut self,
        channel_id: u32,
        ring: &Ring,
        setup: Option<&UnixStream>,
    ) -> Result<()> {
        self.ensure_healthy()?;
        if self.registrations.contains_key(&channel_id) {
            // A re-registration re-arms the existing wait: after a handler failure the frame is
            // still visible, no doorbell will fire for it, and the JavaScript handler was
            // dropped, so the fresh handler needs an immediate wake to observe it.
            match ring.arm_data_wait() {
                Ok(true) => {}
                Ok(false) | Err(_) => self.kick(channel_id),
            }
            return Ok(());
        }
        let descriptor = ring
            .duplicate_data_ready()
            .map_err(|_| Error::new(Status::GenericFailure, "readiness registration failed"))?;
        epoll::add(
            &self.epoll,
            &descriptor,
            epoll::EventData::new_u64(data_event(channel_id)),
            epoll::EventFlags::IN | epoll::EventFlags::ET,
        )
        .map_err(|_| Error::new(Status::GenericFailure, "readiness registration failed"))?;
        let mut descriptors = vec![descriptor];
        if let Some(setup) = setup {
            match register_setup_socket(&self.epoll, setup, setup_event(channel_id)) {
                Ok(setup) => descriptors.push(setup),
                Err(error) => {
                    let _ = epoll::delete(&self.epoll, &descriptors[0]);
                    return Err(error);
                }
            }
        }
        self.registrations.insert(
            channel_id,
            Registration {
                descriptors,
                capacity: None,
            },
        );
        match ring.arm_data_wait() {
            Ok(true) => {}
            Ok(false) => self.kick(channel_id),
            Err(_) => {
                self.unregister(channel_id);
                return Err(Error::new(
                    Status::GenericFailure,
                    "readiness registration failed",
                ));
            }
        }
        Ok(())
    }

    /// Watches `producer`'s capacity doorbell for `channel_id`, once. The channel must already
    /// be registered for data readiness; the caller arms the wait on the ring itself.
    pub(crate) fn watch_capacity(&mut self, channel_id: u32, producer: &Ring) -> Result<()> {
        self.ensure_healthy()?;
        let registration = self
            .registrations
            .get_mut(&channel_id)
            .ok_or_else(|| Error::new(Status::GenericFailure, "native channel is not watched"))?;
        if registration.capacity.is_some() {
            return Ok(());
        }
        let descriptor = producer
            .duplicate_capacity_ready()
            .map_err(|_| Error::new(Status::GenericFailure, "readiness registration failed"))?;
        epoll::add(
            &self.epoll,
            &descriptor,
            epoll::EventData::new_u64(capacity_event(channel_id)),
            epoll::EventFlags::IN | epoll::EventFlags::ET,
        )
        .map_err(|_| Error::new(Status::GenericFailure, "readiness registration failed"))?;
        registration.capacity = Some(descriptor);
        Ok(())
    }

    pub(crate) fn unregister(&mut self, channel_id: u32) {
        if let Some(registration) = self.registrations.remove(&channel_id) {
            for descriptor in registration
                .descriptors
                .iter()
                .chain(registration.capacity.iter())
            {
                let _ = epoll::delete(&self.epoll, descriptor);
            }
        }
        if let Ok(mut state) = self.ready.lock() {
            state.data.remove(&channel_id);
            state.capacity.remove(&channel_id);
            state.dispatched_data.remove(&channel_id);
            state.dispatched_capacity.remove(&channel_id);
            state.peer_closed.remove(&channel_id);
        }
    }

    pub(crate) fn is_registered(&self, channel_id: u32) -> bool {
        self.registrations.contains_key(&channel_id)
    }

    /// Channels whose data wake the running dispatch was started for.
    pub(crate) fn take_ready(&self) -> Vec<u32> {
        match self.ready.lock() {
            Ok(mut state) => std::mem::take(&mut state.dispatched_data)
                .into_iter()
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Forgets a recorded capacity wake for `channel_id`: its owner re-armed, which drained
    /// the doorbell itself, so `readiness_handled` must not complete the new park.
    pub(crate) fn clear_capacity_ready(&self, channel_id: u32) {
        if let Ok(mut state) = self.ready.lock() {
            state.capacity.remove(&channel_id);
            state.dispatched_capacity.remove(&channel_id);
        }
    }

    /// Channels whose capacity wake the running dispatch was started for.
    pub(crate) fn take_capacity_ready(&self) -> Vec<u32> {
        match self.ready.lock() {
            Ok(mut state) => std::mem::take(&mut state.dispatched_capacity)
                .into_iter()
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    pub(crate) fn peer_closed(&self, channel_id: u32) -> bool {
        self.ready
            .lock()
            .is_ok_and(|state| state.peer_closed.contains(&channel_id))
    }

    pub(crate) fn ensure_healthy(&self) -> Result<()> {
        if self.failed.load(Ordering::Acquire) {
            Err(Error::new(
                Status::GenericFailure,
                "readiness reactor failed",
            ))
        } else {
            Ok(())
        }
    }

    /// Ends a dispatch. The watcher records readiness that arrives while a dispatch is pending
    /// and leaves the callback to that dispatch, so when any is recorded this claims and seals
    /// the next dispatch for the caller and returns `true`. `pending` is cleared before the
    /// recorded state is read, and the watcher records before it tries to set `pending`, so
    /// each recorded wake starts exactly one dispatch.
    pub(crate) fn handled(&self) -> bool {
        self.pending.store(false, Ordering::SeqCst);
        let Ok(mut state) = self.ready.lock() else {
            return false;
        };
        if !state.has_undispatched()
            || self
                .pending
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return false;
        }
        state.seal();
        true
    }

    /// Marks `channel_id` ready without a doorbell event, for data found visible while arming.
    pub(crate) fn kick(&self, channel_id: u32) {
        self.mark_ready(channel_id);
        self.kick.store(true, Ordering::Release);
        let _ = rustix::io::write(&self.control, &1u64.to_ne_bytes());
    }

    /// Records `channel_id` as ready without waking the watcher.
    pub(crate) fn mark_ready(&self, channel_id: u32) {
        if let Ok(mut state) = self.ready.lock() {
            state.data.insert(channel_id);
        }
    }

    pub(crate) fn shutdown(&mut self) {
        self.closing.store(true, Ordering::Release);
        let _ = rustix::io::write(&self.control, &1u64.to_ne_bytes());
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
        self.registrations.clear();
        self.pending.store(false, Ordering::Release);
    }
}

impl Drop for Reactor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, Ordering};

    use rustix::buffer::spare_capacity;
    use rustix::event::epoll;
    use rustix::io::Errno;

    use super::{
        EventKind, capacity_event, data_event, decode_event, register_setup_socket,
        retry_interrupted, setup_event,
    };

    #[test]
    fn setup_socket_eof_is_reported_once() {
        let reactor = epoll::create(epoll::CreateFlags::CLOEXEC).unwrap();
        let (watched, peer) = UnixStream::pair().unwrap();
        let _registration = register_setup_socket(&reactor, &watched, 17).unwrap();
        drop(peer);

        let mut events = Vec::with_capacity(1);
        epoll::wait(&reactor, spare_capacity(&mut events), None).unwrap();
        assert_eq!(events.len(), 1);
        let event = events[0];
        let data = event.data;
        let flags = event.flags;
        assert_eq!(data.u64(), 17);
        assert!(
            flags.intersects(
                epoll::EventFlags::IN | epoll::EventFlags::HUP | epoll::EventFlags::RDHUP
            )
        );

        // The hangup is a level condition that persists until the socket closes; the
        // one-shot registration must not report it again.
        let zero = rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let mut again = Vec::with_capacity(1);
        epoll::wait(&reactor, spare_capacity(&mut again), Some(&zero)).unwrap();
        assert!(again.is_empty(), "one-shot setup socket fired twice");
    }

    #[test]
    fn channel_events_round_trip_and_never_collide_with_control() {
        for channel_id in [0, 1, 7, u32::MAX] {
            assert_ne!(data_event(channel_id), super::CONTROL_EVENT);
            assert_ne!(setup_event(channel_id), super::CONTROL_EVENT);
            assert_ne!(capacity_event(channel_id), super::CONTROL_EVENT);
            assert_eq!(
                decode_event(data_event(channel_id)),
                (channel_id, EventKind::Data)
            );
            assert_eq!(
                decode_event(setup_event(channel_id)),
                (channel_id, EventKind::Setup)
            );
            assert_eq!(
                decode_event(capacity_event(channel_id)),
                (channel_id, EventKind::Capacity)
            );
        }
    }

    #[test]
    fn interrupted_wait_retries_until_success_or_close() {
        let closing = AtomicBool::new(false);
        let mut attempts = 0;
        let result = retry_interrupted(&closing, || {
            attempts += 1;
            if attempts == 1 {
                Err(Errno::INTR)
            } else {
                Ok(7)
            }
        })
        .unwrap();
        assert_eq!(result, Some(7));
        assert_eq!(attempts, 2);

        closing.store(true, Ordering::Release);
        assert_eq!(
            retry_interrupted(&closing, || Ok::<_, Errno>(9)).unwrap(),
            None
        );
    }
}

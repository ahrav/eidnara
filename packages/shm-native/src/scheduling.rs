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
use rustix::event::{EventfdFlags, PollFd, PollFlags, epoll, eventfd};
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

/// Identifies the control eventfd; channel events start at 2, so none share it.
const CONTROL_EVENT: u64 = 0;

/// Epoll event data for a channel's data doorbell: even, and at least 2.
fn data_event(channel_id: u32) -> u64 {
    (u64::from(channel_id) + 1) << 1
}

/// Epoll event data for a channel's setup socket: odd, paired with `data_event`.
fn setup_event(channel_id: u32) -> u64 {
    data_event(channel_id) | 1
}

/// Inverse of `data_event`/`setup_event`; `true` marks the setup socket.
fn decode_event(data: u64) -> (u32, bool) {
    (((data >> 1) - 1) as u32, data & 1 == 1)
}

/// Channels the watcher saw wake since the main thread last drained the set, plus the
/// setup sockets that reported hangup. `peer_closed` is latched rather than drained: the
/// hangup fires once (the registration is one-shot) and `peer_closed()` must keep
/// reporting it until the channel unregisters.
#[derive(Default)]
struct ReadyState {
    data: BTreeSet<u32>,
    peer_closed: BTreeSet<u32>,
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

fn wait_until_handled(
    control: &OwnedFd,
    pending: &AtomicBool,
    closing: &AtomicBool,
) -> std::result::Result<bool, Errno> {
    let mut fds = [PollFd::new(control, PollFlags::IN)];
    while pending.load(Ordering::Acquire) && !closing.load(Ordering::Acquire) {
        if retry_interrupted(closing, || rustix::event::poll(&mut fds, None))?.is_none() {
            return Ok(false);
        }
        if fds[0].revents().contains(PollFlags::IN) {
            let mut value = [0u8; 8];
            let _ = rustix::io::read(control, &mut value);
        }
    }
    Ok(!closing.load(Ordering::Acquire))
}

/// Native worker limit.
pub(crate) const WORKER_LIMIT: u32 = 0;

type ReadinessCallback = ThreadsafeFunction<(), (), (), Status, false, true, 2>;

struct Registration {
    descriptors: Vec<OwnedFd>,
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
                            let (channel_id, is_setup) = decode_event(data);
                            if let Ok(mut state) = ready_state.lock() {
                                if is_setup {
                                    state.peer_closed.insert(channel_id);
                                } else {
                                    state.data.insert(channel_id);
                                }
                            }
                            ready = true;
                        }
                        if closing.load(Ordering::Acquire) {
                            break;
                        }
                        if ready
                            && pending
                                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                                .is_ok()
                        {
                            let status = callback.call((), ThreadsafeFunctionCallMode::NonBlocking);
                            if status != Status::Ok {
                                // A rejected call ends the reactor: nothing drains the doorbells
                                // or the setup hangup, so `epoll_wait` would return immediately
                                // on every iteration.
                                failed.store(true, Ordering::Release);
                                pending.store(false, Ordering::Release);
                                break;
                            }
                            match wait_until_handled(&control, &pending, &closing) {
                                Ok(true) if kick.load(Ordering::Acquire) => {
                                    let _ = rustix::io::write(&control, &1u64.to_ne_bytes());
                                }
                                Ok(true) => {}
                                Ok(false) => break,
                                Err(_) => {
                                    failed.store(true, Ordering::Release);
                                    let _ =
                                        callback.call((), ThreadsafeFunctionCallMode::NonBlocking);
                                    break;
                                }
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
            return Ok(());
        }
        let descriptor = ring
            .duplicate_data_ready()
            .map_err(|_| Error::new(Status::GenericFailure, "readiness registration failed"))?;
        epoll::add(
            &self.epoll,
            &descriptor,
            epoll::EventData::new_u64(data_event(channel_id)),
            epoll::EventFlags::IN,
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
        self.registrations
            .insert(channel_id, Registration { descriptors });
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

    pub(crate) fn unregister(&mut self, channel_id: u32) {
        if let Some(registration) = self.registrations.remove(&channel_id) {
            for descriptor in registration.descriptors {
                let _ = epoll::delete(&self.epoll, &descriptor);
            }
        }
        if let Ok(mut state) = self.ready.lock() {
            state.data.remove(&channel_id);
            state.peer_closed.remove(&channel_id);
        }
    }

    pub(crate) fn is_registered(&self, channel_id: u32) -> bool {
        self.registrations.contains_key(&channel_id)
    }

    pub(crate) fn take_ready(&self) -> Vec<u32> {
        match self.ready.lock() {
            Ok(mut state) => std::mem::take(&mut state.data).into_iter().collect(),
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

    pub(crate) fn handled(&self) {
        self.pending.store(false, Ordering::Release);
        let _ = rustix::io::write(&self.control, &1u64.to_ne_bytes());
    }

    /// Marks `channel_id` ready without a doorbell event, for data found visible while arming.
    pub(crate) fn kick(&self, channel_id: u32) {
        if let Ok(mut state) = self.ready.lock() {
            state.data.insert(channel_id);
        }
        self.kick.store(true, Ordering::Release);
        let _ = rustix::io::write(&self.control, &1u64.to_ne_bytes());
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
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use rustix::buffer::spare_capacity;
    use rustix::event::{EventfdFlags, epoll, eventfd};
    use rustix::io::Errno;

    use super::{
        data_event, decode_event, register_setup_socket, retry_interrupted, setup_event,
        wait_until_handled,
    };

    #[test]
    fn pending_callback_waits_for_acknowledgement() {
        let control = Arc::new(
            eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK).expect("control eventfd"),
        );
        let pending = Arc::new(AtomicBool::new(true));
        let closing = Arc::new(AtomicBool::new(false));
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = {
            let control = Arc::clone(&control);
            let pending = Arc::clone(&pending);
            let closing = Arc::clone(&closing);
            std::thread::spawn(move || {
                done_tx
                    .send(wait_until_handled(&control, &pending, &closing))
                    .unwrap();
            })
        };

        rustix::io::write(&control, &1u64.to_ne_bytes()).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(25)).is_err());

        pending.store(false, Ordering::Release);
        rustix::io::write(&control, &1u64.to_ne_bytes()).unwrap();
        assert!(
            done_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap()
        );
        waiter.join().unwrap();
    }

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
            assert_eq!(decode_event(data_event(channel_id)), (channel_id, false));
            assert_eq!(decode_event(setup_event(channel_id)), (channel_id, true));
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

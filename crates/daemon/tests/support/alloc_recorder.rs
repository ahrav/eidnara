//! A ledger that records global-allocator events on its owning thread without allocating.
//!
//! Recording uses only the fixed-capacity atomic ledger, so it never allocates or
//! recurses. Sizes are layout sizes as requested by the caller. Allocator rounding
//! is not included, so live and peak bytes are floors on the process heap
//! footprint. A `realloc` request does not indicate whether the allocator moved
//! the allocation.
//!
//! Each binary that observes allocations declares the global allocator itself:
//!
//! ```ignore
//! #[global_allocator]
//! static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Maximum events one window retains. Overflow is reported, never silently dropped.
pub const LEDGER_CAPACITY: usize = 1 << 16;

const KIND_ALLOC: usize = 1;
const KIND_DEALLOC: usize = 2;
const KIND_REALLOC: usize = 3;

struct Slot {
    kind: AtomicUsize,
    ptr: AtomicUsize,
    old_ptr: AtomicUsize,
    size: AtomicUsize,
    old_size: AtomicUsize,
}

impl Slot {
    const fn empty() -> Self {
        Self {
            kind: AtomicUsize::new(0),
            ptr: AtomicUsize::new(0),
            old_ptr: AtomicUsize::new(0),
            size: AtomicUsize::new(0),
            old_size: AtomicUsize::new(0),
        }
    }
}

static LEDGER: [Slot; LEDGER_CAPACITY] = [const { Slot::empty() }; LEDGER_CAPACITY];
static ENABLED: AtomicBool = AtomicBool::new(false);
static OWNER: AtomicUsize = AtomicUsize::new(0);
static CURSOR: AtomicUsize = AtomicUsize::new(0);
static OVERFLOW: AtomicBool = AtomicBool::new(false);
/// Bytes live above the window's starting point. Freeing memory that predates
/// the window drives this negative, so the value is an `isize` stored as bits.
static LIVE_DELTA: AtomicUsize = AtomicUsize::new(0);
static PEAK_DELTA: AtomicUsize = AtomicUsize::new(0);
static REQUESTED_BYTES: AtomicUsize = AtomicUsize::new(0);

pub struct RecordingAlloc;

/// `pthread_self` reads thread-local storage set up before any Rust code runs and
/// never allocates, unlike `std::thread::current()`, which can materialize a handle.
fn current_thread_token() -> usize {
    // SAFETY: `pthread_self` has no preconditions and cannot fail.
    unsafe { libc::pthread_self() as usize }
}

fn recording_here() -> bool {
    ENABLED.load(Ordering::Relaxed) && OWNER.load(Ordering::Relaxed) == current_thread_token()
}

fn record(kind: usize, ptr: usize, old_ptr: usize, size: usize, old_size: usize) {
    let index = CURSOR.fetch_add(1, Ordering::Relaxed);
    if index >= LEDGER_CAPACITY {
        OVERFLOW.store(true, Ordering::Relaxed);
        return;
    }
    let slot = &LEDGER[index];
    slot.kind.store(kind, Ordering::Relaxed);
    slot.ptr.store(ptr, Ordering::Relaxed);
    slot.old_ptr.store(old_ptr, Ordering::Relaxed);
    slot.size.store(size, Ordering::Relaxed);
    slot.old_size.store(old_size, Ordering::Relaxed);
}

fn grow(bytes: usize) {
    let live = (LIVE_DELTA.load(Ordering::Relaxed) as isize).wrapping_add(bytes as isize);
    LIVE_DELTA.store(live as usize, Ordering::Relaxed);
    if live > PEAK_DELTA.load(Ordering::Relaxed) as isize {
        PEAK_DELTA.store(live as usize, Ordering::Relaxed);
    }
}

fn shrink(bytes: usize) {
    let live = (LIVE_DELTA.load(Ordering::Relaxed) as isize).wrapping_sub(bytes as isize);
    LIVE_DELTA.store(live as usize, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for RecordingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() && recording_here() {
            record(KIND_ALLOC, ptr as usize, 0, layout.size(), 0);
            REQUESTED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            grow(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if recording_here() {
            record(KIND_DEALLOC, ptr as usize, 0, 0, layout.size());
            shrink(layout.size());
        }
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() && recording_here() {
            record(
                KIND_REALLOC,
                new_ptr as usize,
                ptr as usize,
                new_size,
                layout.size(),
            );
            REQUESTED_BYTES.fetch_add(new_size, Ordering::Relaxed);
            if new_size >= layout.size() {
                grow(new_size - layout.size());
            } else {
                shrink(layout.size() - new_size);
            }
        }
        new_ptr
    }
}

/// One recorded allocator call, copied out of the ledger after the window closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Alloc {
        ptr: usize,
        size: usize,
    },
    Dealloc {
        ptr: usize,
        size: usize,
    },
    Realloc {
        old_ptr: usize,
        new_ptr: usize,
        old_size: usize,
        new_size: usize,
    },
}

/// Everything one window observed, plus aggregate counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger {
    pub events: Vec<Event>,
    /// `alloc` plus `realloc` calls; `dealloc` calls are counted separately.
    pub allocation_events: usize,
    pub realloc_events: usize,
    pub dealloc_events: usize,
    /// Sum of every requested `alloc` size and every `realloc` new size.
    pub requested_bytes: usize,
    /// Maximum live layout bytes above the window's starting point.
    pub peak_live_bytes: usize,
    /// Live layout bytes above the starting point when the window closed.
    pub live_bytes_at_close: isize,
    /// The ledger array filled; counters remain exact but `events` is truncated.
    pub overflow: bool,
}

impl Ledger {
    /// Events whose requested size equals `size`, in ledger order.
    pub fn allocations_of_size(&self, size: usize) -> Vec<Event> {
        self.events
            .iter()
            .copied()
            .filter(|event| match event {
                Event::Alloc { size: s, .. } | Event::Realloc { new_size: s, .. } => *s == size,
                Event::Dealloc { .. } => false,
            })
            .collect()
    }

    /// The allocation chain that produced `ptr`, oldest first: the originating
    /// `alloc` followed by each `realloc` that moved or resized it. Returns an
    /// empty chain when `ptr` was not produced inside the window.
    pub fn growth_chain(&self, ptr: usize) -> Vec<Event> {
        let mut chain = Vec::new();
        let mut current = ptr;
        let mut cursor = self.events.len();
        while cursor > 0 {
            cursor -= 1;
            match self.events[cursor] {
                Event::Alloc { ptr: p, .. } if p == current => {
                    chain.push(self.events[cursor]);
                    break;
                }
                Event::Realloc {
                    old_ptr, new_ptr, ..
                } if new_ptr == current => {
                    chain.push(self.events[cursor]);
                    current = old_ptr;
                }
                _ => {}
            }
        }
        chain.reverse();
        chain
    }

    /// Whether `ptr` was freed or reallocated away inside the window after its
    /// last production event.
    pub fn was_released(&self, ptr: usize) -> bool {
        let produced_at = self.events.iter().rposition(|event| match event {
            Event::Alloc { ptr: p, .. } | Event::Realloc { new_ptr: p, .. } => *p == ptr,
            Event::Dealloc { .. } => false,
        });
        let Some(produced_at) = produced_at else {
            return false;
        };
        self.events[produced_at + 1..]
            .iter()
            .any(|event| match event {
                Event::Dealloc { ptr: p, .. } | Event::Realloc { old_ptr: p, .. } => *p == ptr,
                Event::Alloc { .. } => false,
            })
    }
}

/// Records every allocator call the current thread makes while `f` runs.
///
/// A second caller blocks until the active recording window closes. Keep reference
/// work, fixture setup, and assertions outside `f`.
pub fn record_window<T>(f: impl FnOnce() -> T) -> (T, Ledger) {
    // A `std::sync::Mutex` never allocates on Linux, so taking it cannot be recorded.
    static WINDOW: Mutex<()> = Mutex::new(());
    let _window = WINDOW
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert!(
        !ENABLED.load(Ordering::Relaxed),
        "allocation recording windows do not nest"
    );
    CURSOR.store(0, Ordering::Relaxed);
    OVERFLOW.store(false, Ordering::Relaxed);
    LIVE_DELTA.store(0, Ordering::Relaxed);
    PEAK_DELTA.store(0, Ordering::Relaxed);
    REQUESTED_BYTES.store(0, Ordering::Relaxed);
    OWNER.store(current_thread_token(), Ordering::Relaxed);
    ENABLED.store(true, Ordering::SeqCst);
    let value = f();
    ENABLED.store(false, Ordering::SeqCst);
    let recorded = CURSOR.load(Ordering::Relaxed).min(LEDGER_CAPACITY);
    let mut events = Vec::with_capacity(recorded);
    let mut allocation_events = 0;
    let mut realloc_events = 0;
    let mut dealloc_events = 0;
    for slot in &LEDGER[..recorded] {
        let event = match slot.kind.load(Ordering::Relaxed) {
            KIND_ALLOC => {
                allocation_events += 1;
                Event::Alloc {
                    ptr: slot.ptr.load(Ordering::Relaxed),
                    size: slot.size.load(Ordering::Relaxed),
                }
            }
            KIND_DEALLOC => {
                dealloc_events += 1;
                Event::Dealloc {
                    ptr: slot.ptr.load(Ordering::Relaxed),
                    size: slot.old_size.load(Ordering::Relaxed),
                }
            }
            KIND_REALLOC => {
                allocation_events += 1;
                realloc_events += 1;
                Event::Realloc {
                    old_ptr: slot.old_ptr.load(Ordering::Relaxed),
                    new_ptr: slot.ptr.load(Ordering::Relaxed),
                    old_size: slot.old_size.load(Ordering::Relaxed),
                    new_size: slot.size.load(Ordering::Relaxed),
                }
            }
            other => unreachable!("ledger slot kind {other}"),
        };
        events.push(event);
    }
    let ledger = Ledger {
        events,
        allocation_events,
        realloc_events,
        dealloc_events,
        requested_bytes: REQUESTED_BYTES.load(Ordering::Relaxed),
        peak_live_bytes: PEAK_DELTA.load(Ordering::Relaxed),
        live_bytes_at_close: LIVE_DELTA.load(Ordering::Relaxed) as isize,
        overflow: OVERFLOW.load(Ordering::Relaxed),
    };
    (value, ledger)
}

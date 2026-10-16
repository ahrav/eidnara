//! Test hook that holds the production inbound copy on its blocking thread.
//!
//! `dispatch_request` copies each non-empty request body into private bytes as
//! request-scoped blocking work. A held copy lets a test deliver `Cancel`, a
//! route close, or shutdown while the copy is still reading its transport
//! block. Only a body of the held length waits, so unrelated requests in the
//! same process pass through.

use std::sync::{Condvar, Mutex, PoisonError};

struct State {
    held_len: Option<u32>,
    started: usize,
    released: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    held_len: None,
    started: 0,
    released: false,
});
static CHANGED: Condvar = Condvar::new();

/// Holds every inbound copy of a `len`-byte body until [`InboundCopyHold::release`] or drop.
pub struct InboundCopyHold(());

/// Starts holding copies of `len`-byte request bodies. One hold may be active per process.
///
/// # Panics
///
/// Panics when another hold is active.
pub fn hold_inbound_copies_of_len(len: u32) -> InboundCopyHold {
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    assert!(
        state.held_len.is_none(),
        "another inbound copy hold is active"
    );
    *state = State {
        held_len: Some(len),
        started: 0,
        released: false,
    };
    InboundCopyHold(())
}

impl InboundCopyHold {
    /// Copies of the held length that have reached the gate.
    pub fn started(&self) -> usize {
        STATE.lock().unwrap_or_else(PoisonError::into_inner).started
    }

    /// Lets every held and later copy proceed.
    pub fn release(&self) {
        let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
        state.released = true;
        CHANGED.notify_all();
    }
}

impl Drop for InboundCopyHold {
    fn drop(&mut self) {
        self.release();
        STATE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .held_len = None;
    }
}

/// Called on the blocking thread before the copy reads the body.
pub(crate) fn pass(len: u32) {
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    if state.held_len != Some(len) {
        return;
    }
    state.started += 1;
    while !state.released {
        state = CHANGED.wait(state).unwrap_or_else(PoisonError::into_inner);
    }
}

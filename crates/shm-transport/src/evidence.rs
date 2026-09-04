//! Counters the hardware-envelope bench records per arm, and the rule that turns them into
//! disqualification reasons. A run is evidence only if the timed path did no work the ring
//! is supposed to avoid.

/// Operations observed on the timed path of one bench arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OperationCounters {
    /// Times a frame body was copied rather than leased in place.
    pub body_copies: u64,
    /// Heap allocations in the transport during the timed path.
    pub native_allocations: u64,
    /// Syscalls during the timed path, including doorbell reads and writes.
    pub syscalls: u64,
    /// Thread park and wake transitions.
    pub park_wakes: u64,
    /// Hand-offs through a general-purpose queue rather than the ring.
    pub generic_queue_hops: u64,
    /// Hand-offs to another thread through the OS scheduler.
    pub scheduler_handoffs: u64,
}

impl OperationCounters {
    /// One reason per nonzero counter. Body copies, allocations, and queue hops always
    /// disqualify. Syscalls, park/wakes, and scheduler hand-offs are allowed only when
    /// `doorbell_wake_qualified` is set and a park was recorded, since the ring's doorbell is
    /// the one wake path the design permits and nothing else on the timed path may block.
    pub fn disqualifications(self, doorbell_wake_qualified: bool) -> Vec<&'static str> {
        let mut reasons = Vec::new();
        if self.body_copies != 0 {
            reasons.push("transport_body_copy");
        }
        if self.native_allocations != 0 {
            reasons.push("native_transport_allocation");
        }
        if self.generic_queue_hops != 0 {
            reasons.push("generic_queue_hop");
        }
        let wake_allowed = doorbell_wake_qualified && self.park_wakes != 0;
        if self.syscalls != 0 && !wake_allowed {
            reasons.push("timed_path_syscall");
        }
        if self.park_wakes != 0 && !wake_allowed {
            reasons.push("unqualified_park_wake");
        }
        if self.scheduler_handoffs != 0 && !wake_allowed {
            reasons.push("scheduler_handoff");
        }
        reasons
    }
}

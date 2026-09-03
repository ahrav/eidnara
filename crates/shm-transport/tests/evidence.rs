use shm_transport::evidence::OperationCounters;

#[test]
fn purity_gate_rejects_injected_copy_allocation_queue_and_wake() {
    let injected = OperationCounters {
        body_copies: 1,
        native_allocations: 1,
        syscalls: 1,
        park_wakes: 1,
        generic_queue_hops: 1,
        scheduler_handoffs: 1,
    };
    assert_eq!(
        injected.disqualifications(false),
        [
            "transport_body_copy",
            "native_transport_allocation",
            "generic_queue_hop",
            "timed_path_syscall",
            "unqualified_park_wake",
            "scheduler_handoff",
        ]
    );
    assert!(
        OperationCounters::default()
            .disqualifications(false)
            .is_empty()
    );
}

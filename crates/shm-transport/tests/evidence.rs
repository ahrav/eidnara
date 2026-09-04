use shm_transport::evidence::OperationCounters;

#[test]
fn purity_gate_rejects_injected_copy_allocation_queue_and_wake() {
    let injected = OperationCounters {
        body_copies: 1,
        native_allocations: 1,
        doorbell_syscalls: 1,
        other_syscalls: 1,
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

#[test]
fn purity_gate_excuses_wake_operations_only_for_a_qualified_arm_that_parked() {
    let doorbell_wake = OperationCounters {
        doorbell_syscalls: 3,
        park_wakes: 1,
        scheduler_handoffs: 1,
        ..OperationCounters::default()
    };
    assert!(doorbell_wake.disqualifications(true).is_empty());
    assert_eq!(
        doorbell_wake.disqualifications(false),
        [
            "timed_path_syscall",
            "unqualified_park_wake",
            "scheduler_handoff"
        ]
    );

    let no_park = OperationCounters {
        doorbell_syscalls: 1,
        scheduler_handoffs: 1,
        ..OperationCounters::default()
    };
    assert_eq!(
        no_park.disqualifications(true),
        ["timed_path_syscall", "scheduler_handoff"]
    );

    let copied = OperationCounters {
        body_copies: 1,
        native_allocations: 1,
        generic_queue_hops: 1,
        park_wakes: 1,
        ..OperationCounters::default()
    };
    assert_eq!(
        copied.disqualifications(true),
        [
            "transport_body_copy",
            "native_transport_allocation",
            "generic_queue_hop",
        ]
    );
}

#[test]
fn purity_gate_never_excuses_a_syscall_the_doorbell_did_not_issue() {
    let park_plus_other = OperationCounters {
        doorbell_syscalls: 3,
        other_syscalls: 1,
        park_wakes: 1,
        scheduler_handoffs: 1,
        ..OperationCounters::default()
    };
    assert_eq!(
        park_plus_other.disqualifications(true),
        ["timed_path_syscall"]
    );
}

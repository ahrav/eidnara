use shm_transport::evidence::OperationCounters;

#[test]
fn purity_gate_names_every_disqualification_and_excuses_only_a_qualified_parked_wake() {
    let cases: [(OperationCounters, bool, &[&str]); 7] = [
        (OperationCounters::default(), false, &[]),
        (
            OperationCounters {
                body_copies: 1,
                native_allocations: 1,
                doorbell_syscalls: 1,
                other_syscalls: 1,
                park_wakes: 1,
                generic_queue_hops: 1,
                scheduler_handoffs: 1,
            },
            false,
            &[
                "transport_body_copy",
                "native_transport_allocation",
                "generic_queue_hop",
                "timed_path_syscall",
                "unqualified_park_wake",
                "scheduler_handoff",
            ],
        ),
        (
            OperationCounters {
                doorbell_syscalls: 3,
                park_wakes: 1,
                scheduler_handoffs: 1,
                ..OperationCounters::default()
            },
            true,
            &[],
        ),
        (
            OperationCounters {
                doorbell_syscalls: 3,
                park_wakes: 1,
                scheduler_handoffs: 1,
                ..OperationCounters::default()
            },
            false,
            &[
                "timed_path_syscall",
                "unqualified_park_wake",
                "scheduler_handoff",
            ],
        ),
        (
            OperationCounters {
                doorbell_syscalls: 1,
                scheduler_handoffs: 1,
                ..OperationCounters::default()
            },
            true,
            &["timed_path_syscall", "scheduler_handoff"],
        ),
        (
            OperationCounters {
                body_copies: 1,
                native_allocations: 1,
                generic_queue_hops: 1,
                park_wakes: 1,
                ..OperationCounters::default()
            },
            true,
            &[
                "transport_body_copy",
                "native_transport_allocation",
                "generic_queue_hop",
            ],
        ),
        (
            OperationCounters {
                doorbell_syscalls: 3,
                other_syscalls: 1,
                park_wakes: 1,
                scheduler_handoffs: 1,
                ..OperationCounters::default()
            },
            true,
            &["timed_path_syscall"],
        ),
    ];
    for (counters, doorbell_wake_qualified, expected) in cases {
        assert_eq!(
            counters.disqualifications(doorbell_wake_qualified),
            expected,
            "{counters:?} qualified={doorbell_wake_qualified}"
        );
    }
}

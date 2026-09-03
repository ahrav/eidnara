# `cross-process-exclusive-race-is-reached`

- **Discovery:** coverage/vacuity evaluation of the exclusive invariant.
- **Primary evidence:** `acquire_then_second_holder_is_rejected` is same-process and sequential; `shared_lease_across_processes_blocks_exclusive` is cross-process but shared-versus-exclusive.
- **Coverage condition:** two independent processes, same inode/key, both past open and poised at exclusive try-lock before either releases.
- **Why independent:** this precondition can occur in a correct system; it does not require two successful holders.
- **Timing need:** explicit barrier prevents scheduler serialization.
- **Instrumentation:** missing process IDs and pre-lock barrier events.
- **Open questions:** none.

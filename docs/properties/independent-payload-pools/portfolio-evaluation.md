# Portfolio evaluation: independent payload pools

Independent read of the catalog at the tree of this catalog's introducing commit, performed by the
implementing task after the transport landed. Findings are numbered; none
blocks #546, and each names the task that owns its resolution.

| # | Finding | Disposition |
| --- | --- | --- |
| 1 | Eight records are `not yet` or `partial` because their enabling seams (request conversion, reserved publication selection, native failure injection, retained-result quotas) belong to #548, #552, and #550. | Accepted: the catalog keeps target claims separate from implemented facts; each record names its handoff. |
| 2 | The native and direct-host real-process suites skip on Bun 1.3.14 (`markAsUntransferable` unimplemented) and Node (`node_detachment_unavailable`). | Recorded as an explicit unsupported capability in `real-process-current-layout-witness`; a probe copy of `runtime.ts` with the transfer gate removed passed locally, which is diagnostic only, not evidence. |
| 3 | Miri proves same-shape access and ownership within one process; hostile cross-process writers are unprovable there. | Accepted limitation; the two-process job supplies the process boundary without Miri. |
| 4 | The Valgrind job cannot run child-process witnesses. | Resolved by the separate `two-process` job with named witnesses. |
| 5 | The producer scans every published block's completion cell before each reservation. | Accepted: bounded by 187 cells; the specification defers scan optimization until measured. |
| 6 | `reclamation.completed` in host diagnostics still counts generation ends. | #548 (`reclamation-diagnostics-meaning`). |
| 7 | Terminal credit, encoding reserve, and 63+1 reconciliation are unimplemented. | #548. |
| 8 | Capacity model outputs are recorded but the trace inputs are invented. | Accepted: labeled uncalibrated; no performance claim is made. |

Biases for a human reviewer: the author of the implementation authored this
catalog; the five independent parallel reviews the landing policy requires
were not available in this autonomous run and are recorded as outstanding in
the PR description.

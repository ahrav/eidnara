# Portfolio evaluation: independent payload pools

Independent read of the catalog at the tree of this catalog's introducing commit, performed by the
implementing task after the host input, reserved progress, and accounting
work of #548 landed over the #546 transport. Findings are numbered; none
blocks #548, and each names the task that owns its resolution.

| # | Finding | Disposition |
| --- | --- | --- |
| 1 | The remaining `partial` records wait on client publisher selection, native failure injection, and retained-result quotas that belong to #552 and #550, plus one setup failure injection deferred to the last task's combined matrix. | Accepted: the catalog keeps target claims separate from implemented facts; each record names its handoff. |
| 2 | The native and direct-host real-process suites skip on Bun 1.3.14 (`markAsUntransferable` unimplemented) and Node (`node_detachment_unavailable`). | Recorded as an explicit unsupported capability in `real-process-current-layout-witness`; a probe copy of `runtime.ts` with the transfer gate removed passed locally, which is diagnostic only, not evidence. |
| 3 | Miri proves same-shape access and ownership within one process; hostile cross-process writers are unprovable there. | Accepted limitation; the two-process job supplies the process boundary without Miri. |
| 4 | The Valgrind job cannot run child-process witnesses. | Resolved by the separate `two-process` job with named witnesses. |
| 5 | Before each reservation the producer reclaims only the completion cells flagged in the return summary; the full scan of every published block's cell is `Ring::probe`, run on demand rather than per reservation. | Accepted: a held lease costs nothing per reservation; the probe scan is bounded by 187 cells. |
| 6 | `reclamation.completed` keeps its wire name and counts generation ends; `reclamation.meaning`, `returns`, and `exhaustion.by_resource` now carry the distinct quantities. | Resolved by #548 (`reclamation-diagnostics-meaning`). |
| 7 | Terminal credits follow their blocks through `Ring::take_reclaimed` and remain held after publication until the blocks return or the generation ends; the frame deadline bounds pending publication, not retention of an already published terminal block. | Accepted: a peer that retains published terminals holds credits until it returns them or the generation ends; the credit-exhaustion witness in `tests/dispatch.rs` covers refusal, not a return timeout. |
| 8 | Capacity model outputs are recorded but the trace inputs are invented. | Accepted: labeled uncalibrated; no performance claim is made. |

Biases for a human reviewer: the author of the implementation authored this
catalog; the five independent parallel reviews the landing policy requires
were not available in this autonomous run and are recorded as outstanding in
the PR description.

# Portfolio evaluation: independent payload pools

Independent read of the catalog at the tree of this catalog's introducing commit, performed by the
implementing task after the native and TypeScript publication, capacity
readiness, and retained-response accounting of #550 landed over #546, #548,
and #552. Findings are numbered; none blocks this PR, and each names the owner
of its resolution.

| # | Finding | Disposition |
| --- | --- | --- |
| 1 | The remaining `partial` records are `environment-finalizer-confinement` (a late-finalizer witness needs a runtime that reports detachment), `partial-setup-reclaims-only-unexposed-resources` (a failure between descriptor duplication and grant transfer), `send-outcome-no-generic-replay` and `real-process-current-layout-witness` (native daemon-level witnesses on a capable runtime), `request-conversion-completion-ownership` (no test pauses the production inbound copy under Cancel, route close, or shutdown), `acceptance-artifact-provenance` (build metadata is not an artifact digest), and the Miri same-process limitation. | Accepted: the catalog keeps target claims separate from implemented facts; each record names its handoff. |
| 2 | The native and direct-host real-process suites skip on Bun 1.3.14 (`markAsUntransferable` unimplemented) and Node (`node_detachment_unavailable`). | Recorded as an explicit unsupported capability in `real-process-current-layout-witness`; a probe copy of `runtime.ts` with the transfer gate removed passed locally, which is diagnostic only, not evidence. |
| 3 | Miri proves same-shape access and ownership within one process; hostile cross-process writers are unprovable there. | Accepted limitation; the two-process job supplies the process boundary without Miri. |
| 4 | The Valgrind job cannot run child-process witnesses. | Resolved by the separate `two-process` job with named witnesses. |
| 5 | The producer scans every published block's completion cell before each reservation. | Accepted: bounded by 187 cells; the specification defers scan optimization until measured. |
| 6 | `reclamation.completed` keeps its wire name and counts generation ends; `reclamation.meaning`, `returns`, and `exhaustion.by_resource` now carry the distinct quantities. | Resolved by #548 (`reclamation-diagnostics-meaning`). |
| 7 | Terminal credits follow their blocks through `Ring::take_reclaimed` and remain held after publication until the blocks return or the generation ends; the frame deadline bounds pending publication, not retention of an already published terminal block. | Accepted: a peer that retains published terminals holds credits until it returns them or the generation ends; the credit-exhaustion witness in `tests/dispatch.rs` covers refusal, not a return timeout. |
| 9 | Ordinary descriptor headroom counts every outstanding descriptor, so a control the host has not yet consumed holds one of the 32 ordinary slots against the client's data. | Accepted: the reserve exists so controls can still publish; `ring_bridge_blocked_data_waits_for_capacity_while_controls_bypass` records the two consumptions the 33rd ordinary frame then needs. |
| 10 | `NativeChannel`-level tests (`runNativeLifecycle`, the TypeScript channel's real-addon tests) skip on Bun 1.3.14 and Node 22 because neither reports the detachment capability; the native capacity-wake and control-bypass witnesses therefore run against the raw addon in a child process, where the process-wide reactor's callback is the one under test. | Recorded limitation; the capability witness now records the exact artifact and runtime with each outcome. |
| 8 | Capacity model outputs are recorded but the trace inputs are invented. | Accepted: labeled uncalibrated; no performance claim is made. |

## Combined matrix at the last implementation task

#550 is the last implementation task of the stack (#546, #548, #552, #550),
so its closing evidence is the combined run below, executed on the final tree
of this branch on Linux x86-64 with Bun 1.3.14, Node 24.18.0, and
`cargo +1.98`. Each row names what it covers of lifetimes, aliases, quotas,
simultaneous exhaustion, ordering, and stop/restart, and what it could not.

| Surface | Command | Result | Covers | Cannot cover here |
| --- | --- | --- | --- | --- |
| Rust workspace (transport, host, Rust client, daemon) | `cargo +1.98 nextest run --workspace --locked --profile ci` | 4008 passed, 74 skipped (bench kinds, shm child-role entry) | lifetimes, quotas, ordering, exhaustion, stop/restart (`shm_failure_modes.rs`), 64 MiB both directions, one-over refusal | Valgrind (CI only) |
| Transport unsafe | `cargo +nightly-2026-07-27 miri test -p shm-transport --lib -- lease:: backend::ring::miri` | 15 passed | ownership, return-once, same-shape access | hostile cross-process writers |
| Native addon | `EIDNARA_SHM_NATIVE_CLAIMED_TARGET=1 bun run --cwd packages/shm-native test` | 24 pass | aliases, injected detach and deletion failures, control bypass, capacity wake on descriptor consumption and on a lease return alone, exhaustion | wrapper-level lifecycle on this Bun (`markAsUntransferable` absent) |
| Native capability, both runtimes | `test:capability:bun`, `test:capability:node`, `test:node` | recorded limitations with artifact identity | artifact identity and runtime versions | activation on a detachment-capable runtime |
| TypeScript client | `bun test packages/opencode-plugin/src/shared/host-client` | 211 pass (mock-addon witnesses; the real-addon cases return early because `probeCapabilities()` reports `runtime_mechanism_unavailable` on this Bun) | queue ordering and bound, flush waiting on the queue, control bypass, retained quotas and post-close refusal, capacity re-arm against a mock addon | the live-addon capacity wake (`a saturated outbound ring cannot block inbound readiness`) and the daemon-level TypeScript run, both on a runtime that reports the detachment capability |
| Plugin | `bun run --cwd packages/opencode-plugin test`, `smoke` | 3532 pass on Node 24.18.0 (the SSRF parity tests fail below Node 24.15), smoke pass | integration of the client into the plugin | none |
| Direct host E2E | `bun run --cwd packages/e2e-tests test:fixture-contract` | 3 pass, 2 skip | fixture contract | the two capability skips |
| Sole surface | `rg` for `SpanPlan`, `SamplePrefix`, `MADV_REMOVE`, `ReleaseSink`, `RingGrant`, `host-test-ring-v1`, `BRIDGE_RESERVE_SLICE`, `reserve_until` outside the transport's own API and tests | no matches in production sources (`PoolGeometry::arena_bytes` is the current layout's field, not the retired arena) | one transport, one layout reader, one receive representation, no shim or flag | none |

Remaining evidence gaps after this run: a late-finalizer witness and a
wrapper-level or daemon-level native run on a runtime that reports the
detachment capability; a failure injected between descriptor duplication and
grant transfer; Valgrind locally; and the five independent parallel reviews
the landing policy requires.

Biases for a human reviewer: the author of the implementation authored this
catalog; the five independent parallel reviews the landing policy requires
were not available in this autonomous run and are recorded as outstanding in
the PR description.

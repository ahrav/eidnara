# synapse-degrades-to-disabled-and-keeps-the-context-routable

## Discovery trigger

`mod.rs:4` states the contract: "Artifact faults keep Synapse's catalog
identity published and make binds reject with `artifact_invalid`". Synapse is
an optional secondary component inside a composite whose primary is the
context module. If a missing or corrupt model bundle made `activate` return
`Err`, the host's activation task would trip the fatal latch and take the
whole product down for an optional lane. The audit traced which faults become
`Disabled` and which still fail activation.

## Evidence trail

All references are at `e16e39e`, paths relative to `crates/host-runtime/`.

Unconfigured. `initialize` (`src/synapse/mod.rs:993-1013`) sets
`LaneState::Disabled { reason: "no bundle configured" }` when `config` is
`None` (`:1008-1010`), or `Starting` when a config exists (`:1002`). `activate`
returns `Ok(())` immediately for no config (`:1016-1018`).

Artifact faults. `activate` (`:1015-1063`) spawns a blocking task that runs
`bundle::load_bundle` (`:1025-1029`) and `Backend::load` (`:1035-1036`), both
returning `bundle::BundleError`. An `Ok(Err(error))` result sets
`LaneState::Disabled { reason }` and returns `Ok(())` (`:1052-1058`). The
bundle faults each produce a distinct reason: missing artifact
(`src/synapse/bundle.rs:670`), hash mismatch (`:694`), unlisted entry
(`:731`), and the ORT identity faults in `verify_ort_library`
(`src/synapse/inference.rs:105-129`: bad digest string, missing, not regular,
size bound, hash mismatch).

Two paths are not `Disabled`. `validate_limits` failure returns `Err`
(`:1019-1022`), and a panic or cancellation in the blocking task surfaces as
a `JoinError` and returns `Err` (`:1059-1061`). In the runtime, an activation
`Err` trips the fatal latch (`src/runtime.rs:840-853`), so both are
host-fatal by design.

Bind and handle. `bind` returns `Reject { code: "artifact_invalid" }` for
`Disabled` or `Failing` (`:859-862`) and `module_reloading` for `Starting`
(`:855-858`). `handle` returns `artifact_invalid` when no ready lane exists
(`:867-869`). `health` reports `Degraded` for `Disabled` (`:960-970`).

Composite. `StaticComposite::activate` uses `try_join!` over the three
children (`src/composite.rs:214-222`), so the primary's activation does not
depend on Synapse's outcome once Synapse returns `Ok`.

Existing checks:

- `unconfigured_component_is_disabled_not_fatal`
  (`tests/synapse_bundle.rs:226-265`): `SynapseComponent::new(None)`;
  reason contains "no bundle configured" (`:229`); bind is
  `artifact_invalid` (`:240`); health is `Degraded` (`:243-246`).
- `one_bit_changes_to_each_artifact_disable_the_lane` (`:282-304`): flips
  the last byte of each of seven artifacts (`:283-291`); the reason contains
  "hash mismatch". It does not cover `manifest.json`.
- `missing_artifact_disables_the_lane` (`:307-312`): removes
  `embedding.bin`; the reason contains "missing".
- `wrong_ort_identity_disables_the_lane` (`:708-728`): wrong hash, missing
  library, and placeholder digest each disable. It returns early when
  `EIDNARA_SYNAPSE_TEST_ORT_LIBRARY` is unset (`:709`, `:29-37`). That
  variable is not set in `.github/workflows/ci.yml`, so CI passes this test
  vacuously.
- `corrupt_bundle_degrades_synapse_and_keeps_context_routable`
  (`tests/synapse_roundtrip.rs:57-117`): a flipped model byte over a real
  loopback host; the catalog lists three modules (`:88`), the Synapse route
  open is `artifact_invalid` (`:92-93`), and a request on a `context` route
  echoes (`:96-114`). The `context` primary is `EchoPrimary`
  (`tests/support/synapse.rs:126-170`), a test stub.

The pre-ORT tests use `pre_ort_identity` (`:79-84`), so `load_bundle`
rejects before `Backend::load` and no runtime library is needed. All three
binaries run in CI via `cargo test --workspace --all-targets`
(`.github/workflows/ci.yml:118`).

## Failure scenario

1. A deployment ships a bundle whose `embedding.bin` is truncated.
2. `load_bundle` returns `BundleError("artifact hash mismatch: ...")`.
3. Had `activate` propagated that error, `spawn_activation_task` would call
   `fatal.trip` (`src/runtime.rs:846-853`) and the host would shut down with
   the context module unreachable.

As written, `:1052-1058` converts the error to `Disabled` and returns `Ok`.
The remaining fatal paths are configuration (`validate_limits`) and a panic
inside `load_bundle` or `Backend::load`, neither of which is an artifact
fault in the record's sense.

## Timing windows and dependencies

Transport publishes before activation completes (`mod.rs:1000-1002`,
`src/runtime.rs:828-829`). A bind that arrives while the lane is `Starting`
receives `module_reloading`, not `artifact_invalid`. The roundtrip harness
polls through `module_reloading` until the terminal code arrives
(`tests/support/synapse.rs:286-311`), so the test's assertion at `:93` is not
racy, but a client that treats `module_reloading` as final would misread a
lane that is about to become `Disabled`.

The `liveness` half of the record (a context request completes while Synapse
is disabled) depends on the composite routing by `module_id`
(`src/composite.rs:225-236`), not on anything in the Synapse module.

## What a test must construct

1. A fault injected during inference on a real backend, so `Failing` and
   `Disabled` are both reached from a lane that was once `Ready`. Today this
   is covered only through the deterministic engine's `fail_next`.
2. A CI configuration that provides `EIDNARA_SYNAPSE_TEST_ORT_LIBRARY`, or a
   companion test that exercises the ORT identity faults through
   `verify_ort_library` directly without a real library (the missing-library
   and bad-digest cases need no ORT and could run unconditionally).
3. A `manifest.json` bit flip, to close the gap in the seven-artifact loop.

## Investigation log

### Q: Is every artifact fault non-fatal, as the guarantee says?

- Sources examined: `src/synapse/mod.rs:1015-1063`; `src/runtime.rs:832-857`;
  `src/synapse/bundle.rs:175-214`; `src/synapse/inference.rs:71-95`,
  `:105-129`.
- Findings: every `BundleError` and every `InferenceError` from `Backend::load`
  reaches `:1053` and becomes `Disabled`. Two `Err` returns remain: invalid
  limits (`:1020-1022`) and a `JoinError` from the blocking task (`:1059`).
  A panic inside `load_bundle` on a hostile bundle would therefore be fatal.
  I found no `unwrap` or index on untrusted bytes in the `load_bundle` path
  I read, but I did not audit `fastembed` or `ort` for panics on a corrupt
  `model.onnx`.
- Missing evidence: a panic-safety audit of `Backend::load` against a
  malformed but hash-valid model, which requires the runtime library.
- Conclusion: resolved for the code this crate owns; unresolved for panics
  raised inside `fastembed`/`ort` during construction.

### Q: Does the roundtrip test prove the real context module keeps serving?

- Sources examined: `tests/synapse_roundtrip.rs:57-117`;
  `tests/support/synapse.rs:126-170`, `:180-240`.
- Findings: the primary is `EchoPrimary`, a stub that accepts every bind and
  echoes the body. The test proves the composite still routes to the primary
  and that the host stays up; it does not exercise the production context
  component.
- Missing evidence: none needed for this record's claim, which is about the
  host and the composite, not the context module's own behaviour.
- Conclusion: resolved with that scoping. The record's phrase "the context
  module keeps serving requests" is true of the routing, not of the module.

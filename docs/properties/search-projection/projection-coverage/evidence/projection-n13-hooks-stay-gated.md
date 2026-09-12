# projection-n13-hooks-stay-gated

## Discovery trigger

The lifecycle lens finds an existing negative boundary: absent N1.3 product
hooks cannot be enabled through daemon routes or configuration. RP2.1 adds
specific class, freshness, resource and both-harness acceptance prerequisites.
This record retains that gate claim without treating missing implementations
as evidence that future activation logic is correct.

## Evidence trail

Provenance: [source register](../_lenses/model.md#source-register), dated
2026-09-10, Eidnara HEAD `913234433ae36a80a6e22c6aac14c7f9aab74386`.
No enablement, deployment or runtime tests ran in this pass.

- [N1.3][n1] names message cleanup, embedding bootstrap/routing/registry,
  backfill/identity GC, git rows/sweeps and promoted-memory work.
- [RP2.1 success criteria][plan] require disabled hooks until class coverage,
  freshness, resource and both-harness gates pass.
- [Production handler][handler] parses a request and calls the same dispatcher
  exercised by tests. [Dispatcher][dispatch] explicitly routes known methods
  and rejects unknown shapes; it is not compiled only for tests.
- [Config test][config] checks that hostile user/project flags cannot configure
  absent indexing, embedding or git subsystems.
- [Production config keys][keys] enumerate the supported tier keys, and the
  [facade whitelist][facade] rejects names outside the existing facade tools.
- [Route test][routes] submits absent names in flat and facade envelopes and
  asserts distinct rejection codes.
- [Scheduler task][scheduler] names review-user-memories, not the RP2.1 slices.
  [Bridge][bridge] requires a bound project and Rust-owned task inputs.

Reachability: `test-only`. No production RP2.9 evidence gate evaluates class,
freshness, resource and harness acceptance for N1.3 hook activation. Incoming
requests do reach existing absent-route rejection and configuration omits the
absent keys, but those are adjacent behaviors, not this proposed gate. The
record is `Exercised: not yet`; no redundant absent-route property is added.

## Failure scenario

A later projection adds a route, timer slice or configuration alias that starts
message indexing or embedding work before source coverage or accepted limits
exist. Checking only a public method name misses a startup sweep that bypasses
the gate. An unsupported harness can also be treated as capable by configuration.

A competing explanation is an unrelated enabled task: Dreamer scheduling and
the lower-level Synapse runtime already exist. Their existence is not a gate
violation. The oracle is scoped to the frozen N1.3 product-hook inventory,
not every model call or every maintenance action in the process.

## Timing windows and dependencies

Activation is observed on startup, request dispatch, configuration reload and
supervisor ticks. Missing evidence is not approval. A failed prerequisite must
block a new activation attempt even if a different class or harness passed.
The future evidence identity must prevent incompatible stale approval reuse.
Shutdown of already-running work after gate invalidation is a separate lifecycle
question for the supervisor/embedding owners, not a fabricated instantaneous
cancellation promise. The negative admission check remains required throughout.

## What a test must construct

1. A frozen hook-to-gate inventory that includes non-route timer slices.
2. User-only, project-only and combined hostile enable flags, including aliases.
3. Flat method, kind and facade attempts for each applicable hook.
4. Startup and supervisor attempts with missing evidence, present failed
   evidence and unsupported/inapplicable evidence in separate scenario cells.
   Record the gate, entry point and evidence reason with each witness.
5. A supported and unsupported harness capability fixture, with capability
   supplied by the owner rather than a caller-controlled success string.
6. Independent observations of hook execution and hook-created durable work.
7. A valid accepted configuration to show the gate fixture is discriminating
   once an implementation exists; no such positive path is claimed at HEAD.
8. The three separate gate-evidence markers in [fault-map](../fault-map.md).
   The [acceptance witness record][acceptance] requires every declared scenario,
   not one generic failed activation standing in for the whole matrix.

Current tests provide adjacent source evidence and remain unaudited. They do
not partially exercise the proposed evidence gate. Reuse the existing Dreamer
scheduler record rather than duplicating lease/receipt rules.

## Investigation log

### Q: Who owns the frozen hook inventory and evidence invalidation rules?

- Sources examined: [N1.3][n1], RP2.1 success criteria, current dispatcher,
  configuration test, scheduler constants and bridge.
- Findings: N1.3 lists product responsibilities, while current source enforces
  absence. No implemented acceptance artifact binds hook, class, harness and
  resource evidence across startup or reload.
- Missing evidence: Activation manifest, acceptance identity and the rule for
  invalidating approval after source-policy/model/schema changes.
- Conclusion: needs human input from daemon lifecycle and RP2.9 owners.
  Existing absent-route tests cannot certify that future gate transitions work.

[n1]: ../../../../../../commons/docs/plans/2026-09-08-1614-feat-eidnara-rust-product-state-ownership-plan.md#L156-L169
[plan]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L54-L59
[handler]: ../../../../../crates/daemon/src/lib.rs#L11850-L11871
[dispatch]: ../../../../../crates/daemon/src/lib.rs#L12603-L12695
[config]: ../../../../../crates/daemon/src/config.rs#L1881-L1929
[keys]: ../../../../../crates/daemon/src/config.rs#L532-L624
[facade]: ../../../../../crates/daemon/src/lib.rs#L10267-L10279
[routes]: ../../../../../crates/daemon/src/lib.rs#L32613-L32653
[scheduler]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L25-L36
[bridge]: ../../../../../crates/daemon/src/lib.rs#L14024-L14089
[acceptance]: ../catalog.md#projection-acceptance-situations-witnessed

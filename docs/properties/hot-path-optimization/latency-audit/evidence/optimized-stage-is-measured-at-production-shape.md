# optimized-stage-is-measured-at-production-shape

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

Every other record in this area constrains what an optimization must preserve.
None of them says what "faster" means. The wildcard pass asked whether any
artifact in the repository could distinguish a regression from an improvement
for a daemon stage, and found none: the benches run as a smoke check, the
production-sized fixtures are ignored by default, and the transport bench
labels its own record `BLOCKED`. This record names the situation a campaign
must reach before any "faster" claim is checkable.

## Evidence trail

- CI runs every bench target once in test mode with
  `cargo +1.98 test --workspace --all-features --locked --bench '*'`
  ([ci.yml:514-518][ci-bench]); the step compares nothing and has no
  threshold. [`.config/nextest.toml`][nextest] keeps bench targets out of
  every nextest run because the `hardware_envelope` bench does not answer
  `--list`.
- The daemon bench's [header][hp-header] states the claim under measurement
  (single-threaded, warm-process, warm-tokenizer service time of one stage
  call at a fixed corpus point) and says one process's numbers are not a
  baseline. The bench needs `--features bench-internals`
  ([Cargo.toml:71-75][cargo-bench]).
- Payload sizes: the tokenizer arm sweeps [`PAYLOAD_SIZES`][hp-sizes]
  (256, 2_048, 4_096 bytes); the projection and tail-hygiene arms use
  [`MESSAGE_COUNTS`][hp-counts] (100, 1_400, 2_500 messages) at 2 KiB
  mixed content; the end-to-end arms use [`E2E_MESSAGE_COUNTS`][hp-e2e-counts]
  (100, 1_000) because a 1_400-message first HARD pass is rejected by the
  store's 512 KiB durable-text bound, as the comment at
  [`:30-33`][hp-e2e-counts] and the note at [`:341-343`][hp-cliff] say.
- The end-to-end arms ([`bench_e2e_first_hard`][hp-e2e], steady, output
  cache, caveman) call [`transform_cached`][hp-e2e] on a request built by
  [`serde_json::from_value`][hp-req] against a [fresh tempfile store][hp-store]
  with a fixed [`ProducerContext`][hp-ctx]. The production handler wraps that
  call with the projection-cache lookup, side-channel drain, and receive
  trace at [`:8113-8130`][h-pre], the `run_transform` closure's
  `project_memory`, `historian_active`, and guidance reads at
  [`:8136-8191`][h-run], and the response encoding in
  [`respond_transform`][respond]; none of that is in the bench.
- The 1_400 and 1_000 points are pinned by
  [`first_hard_pass_meta_respects_the_store_durable_text_bound`][meta-bound]
  (1_000 commits, 1_400 fails). The test carries
  `#![cfg(feature = "bench-internals")]` and a `required-features` gate in
  [Cargo.toml:67-69][cargo-bench].
- The two production-sized fixtures are `#[ignore]` and print to stderr:
  [`apply_once_stage_timings_large_fixture`][fx-1400] (1_400 messages) and
  [`full_module_pass_timing_fixture`][fx-2500] (2_500 messages, 47_075
  frozen units, 4_096-byte payloads).
- The transport bench uses a fixed payload, [256 bytes in smoke and 4_096 in
  a campaign][he-payload], where a campaign needs `--bench` or `--campaign`
  ([`:217-221`][he-payload]); it [exits with status 2 on
  `--designated-host`][he-designated] and writes
  [`designated_host_verdict: BLOCKED`][he-blocked] into its record. Its
  [manifest][he-manifest] declares 24 `byte_size_boundary_probes`, three
  workload classes, and designated-host fields set to
  `UNSET_REQUIRES_DESIGNATED_HOST`.
- The host-runtime bench support's [`evidence.rs`][evidence] states the
  manifest discipline (schema, workload, build, host, arm identity; sidecars
  before summary; checksums; one terminal state) and its
  [`Manifest`][evidence-manifest] struct carries `build` and `host` fields.
  The host-runtime harness sends a [69-byte fixture body][pm-body].
- Production emits per-stage numbers on every response:
  [`TransformTimings`][tt] rides in the body and
  [`emit_pass_timing`][emit] prints an `eidnara-pass-timing` line. Nothing
  records either as a comparable artifact.

## Failure scenario

Not a violation; a coverage gap. A stage's lines run under a 100-message
bench request or a 69-byte fixture body while the production state (a steady
session of 1_400 or 2_500 messages, a warm store, an ingress body through
`Handler::handle`) never occurs. A change then ships with numbers that are
not comparable across builds, hosts, or workloads, and a regression and an
improvement look the same.

## Timing windows and dependencies

None in time. The dependency is on reaching the state: a first pass cannot
commit a 1_400-message session, so the production size class is reachable
only by incremental growth across committing passes, with a warm store for
steady passes and a cold store for the first pass.

## What a test must construct

A recorded measurement run whose record names the stage, the input size
class, and one build, host, and workload identity, taken before and after the
change. The input must enter through `Handler::handle`, not a typed request.
The [wildcard checks](../existing-checks.md#wildcard-and-cross-cutting) are
the smoke run, the host-runtime manifests, and the timing-line tests; none is
this record. No measurement ran for this catalog.

One constant marker per authorized stage; the marker list is fixed when the
specification enumerates the stages, and no name is built at run time.

## Investigation log

### Q: Which size class is "production-shaped", 1_400 or 2_500 messages?

- Sources examined: [`MESSAGE_COUNTS`][hp-counts], the two fixtures
  ([1_400][fx-1400], [2_500][fx-2500]), [`E2E_MESSAGE_COUNTS`][hp-e2e-counts].
- Findings: The bench header treats 1_400 at 2 KiB mixed as the
  production-shaped point; the module fixture uses 2_500 with 4_096-byte
  payloads and 47_075 frozen units. No document reconciles them.
- Missing evidence: A production session-size distribution.
- Conclusion: needs human input.

### Q: Manifest in the `evidence.rs` style, or the stderr timing line?

- Sources examined: [`evidence.rs`][evidence], [`emit_pass_timing`][emit].
- Findings: The manifest carries build and host identity; the timing line
  carries neither and is not captured by any job.
- Missing evidence: A specification decision.
- Conclusion: needs human input.

### Q: Is the 460-bytes-per-message attribution of the 512 KiB cliff verified?

- Sources examined: [hot_path.rs:30-33][hp-e2e-counts],
  [`transform_meta_bound.rs`][meta-bound].
- Findings: The test pins 1_000 ok and 1_400 refused with `InputLimit`. The
  per-message figure appears only in the comment; no test or measurement
  derives it.
- Missing evidence: A `meta` byte count per message at both points.
- Conclusion: unresolved, needs a measured `meta` size at 1_000 and 1_400.

[ci-bench]: ../../../../../.github/workflows/ci.yml#L514-L518
[nextest]: ../../../../../.config/nextest.toml#L4-L7
[hp-header]: ../../../../../crates/daemon/benches/hot_path.rs#L1-L10
[hp-counts]: ../../../../../crates/daemon/benches/hot_path.rs#L29
[hp-e2e-counts]: ../../../../../crates/daemon/benches/hot_path.rs#L30-L33
[hp-sizes]: ../../../../../crates/daemon/benches/hot_path.rs#L36
[hp-req]: ../../../../../crates/daemon/benches/hot_path.rs#L233-L246
[hp-ctx]: ../../../../../crates/daemon/benches/hot_path.rs#L248-L273
[hp-store]: ../../../../../crates/daemon/benches/hot_path.rs#L275-L280
[hp-e2e]: ../../../../../crates/daemon/benches/hot_path.rs#L282-L315
[hp-cliff]: ../../../../../crates/daemon/benches/hot_path.rs#L352-L354
[cargo-bench]: ../../../../../crates/daemon/Cargo.toml#L62-L75
[meta-bound]: ../../../../../crates/daemon/tests/transform_meta_bound.rs#L1-L22
[fx-1400]: ../../../../../crates/daemon/src/transform.rs#L12434-L12494
[fx-2500]: ../../../../../crates/daemon/src/transform.rs#L28083-L28292
[h-pre]: ../../../../../crates/daemon/src/lib.rs#L8181-L8198
[h-run]: ../../../../../crates/daemon/src/lib.rs#L8204-L8270
[respond]: ../../../../../crates/daemon/src/lib.rs#L14479
[emit]: ../../../../../crates/daemon/src/lib.rs#L14554-L14576
[tt]: ../../../../../crates/daemon/src/transform.rs#L1026-L1207
[he-payload]: ../../../../../crates/shm-transport/benches/hardware_envelope.rs#L217-L223
[he-designated]: ../../../../../crates/shm-transport/benches/hardware_envelope.rs#L211-L214
[he-blocked]: ../../../../../crates/shm-transport/benches/hardware_envelope.rs#L283-L286
[he-manifest]: ../../../../../crates/shm-transport/benches/manifests/v1.json
[evidence]: ../../../../../crates/host-runtime/benches/support/evidence.rs#L1-L8
[evidence-manifest]: ../../../../../crates/host-runtime/benches/support/evidence.rs#L103-L131
[pm-body]: ../../../../../crates/host-runtime/tests/support/perf_measurement.rs#L16-L19

# Credential refresh qualification records

Source: the AWS credential refresh specification
([#860](https://github.com/ahrav/eidnara/issues/860)), approved staging
revision with body SHA-256
`08d217b0ab2d569c5a099b9ab7554f766035aeb49baa8b607be8581e67f700c5`. The M4
ticket ([#872](https://github.com/ahrav/eidnara/issues/872)) lands the records
its qualification relies on: N2, N8, N9, N10, and N13. Code references are
verified against the #872 branch.

Each record names its runnable check and the fault or enabling state that
check constructs. A record is exercised only where that construction runs; a
check that runs fault-free is not coverage.

### credential-lifetime-covers-dispatch

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the owner unit test drives lifetimes one second below, equal
to, and one second above the deadline plus skew, and the subprocess suite
starts no child for a row five seconds short or with no lifetime
Guarantee: a refreshable row starts a model child only while it expires
strictly after the run's deadline plus `EXPIRY_SKEW`, by both its monotonic
bound and the current wall clock.
Check: `always` - at every spawn the lease the owner returns satisfies
`covers`; an equal or shorter lifetime, or a wall-clock jump past the bound,
starts no child, and this must hold at every evaluation.
Fault/timing angle: a wall-clock jump between acquisition and spawn.
Required faults and enabling state: a row whose lifetime is at, below, or just
above the deadline plus skew; a wall jump injected at the read between
acquisition and the spawn recheck.
Confidence: high - [evidence](evidence/credential-lifetime-covers-dispatch.md).
The predicate and its three checks were read at the #872 branch.
Existing check: `crates/host-runtime/src/model_execution/aws_refresh.rs:1105`
`lifetime_must_exceed_the_deadline_plus_skew_strictly`;
`crates/host-runtime/tests/model_execution_subprocess.rs:4073`
`profile_rows_without_lifetime_start_no_child`; `:4140`
`a_wall_jump_after_acquisition_refuses_the_spawn`.
Impact: a child could run on credentials that expire mid-run.
Open questions: None.

### typed-source-failure-preserves-model-certainty

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the producer tests inject an unproven start effect, an
unproven cancel after a source failure, and a cross-incarnation unknown
outcome, and count the model calls that follow
Guarantee: when a model's effect is unknown, the summarizer dispatches no
fallback model, and a typed source failure skips models on the same source
without parsing error text.
Check: `always` - after an outcome with an unproven model effect the count of
further model dispatches is zero at every evaluation, because one extra
dispatch is a second billable call.
Fault/timing angle: a start failure, cancel, or reconnect whose model effect
the daemon cannot prove.
Required faults and enabling state: a start failure without proof the model
did not run; a source failure whose cancel is unproven; an unknown outcome
recorded under another incarnation; a source failure with another source
available.
Confidence: high - [evidence](evidence/typed-source-failure-preserves-model-certainty.md).
The four tests and their fault construction were read at the #872 branch.
Existing check: `crates/daemon/src/history_summarizer.rs:4621`
`cross_incarnation_unknown_records_completion_backoff_without_fallback`;
`:4690` `a_start_failure_with_an_unproven_effect_starts_no_second_model`;
`:4919` `a_source_failure_skips_same_provider_models_and_falls_back_to_another_source`;
`:5062` `a_source_failure_whose_cancel_is_unproven_starts_no_further_model`.
Impact: a second billable model call, or a fold published from two lineages.
Open questions: None.

### eligible-demand-recovers-after-renewal

Type: liveness
Reachability: default-production
Status: active
Exercised: yes - each folding path folds again after a source failure and its
cooldown, and the 24-hour soak rotates 24 rows and survives an external login
through one owner and one adapter
Guarantee: after a source failure, once the cooldown expires, the source
answers, and an eligible operation arrives, the same daemon and routes publish
a valid fold on each of the normal, emergency, wrapup, and reattach paths.
Check: `sometimes` - each path must reach a valid fold after source failure
and recovery at least once per campaign; the four paths are witnessed
separately because one path's recovery says nothing about another's.
Fault/timing angle: the cooldown boundary, and an external login between
rotations.
Required faults and enabling state: a typed source failure on each path; the
cooldown elapsing; a usable source response; an eligible operation; 24
rotations with one external login.
Confidence: high - [evidence](evidence/eligible-demand-recovers-after-renewal.md).
The five tests were read at the #872 branch.
Existing check: `crates/daemon/src/source_recovery_tests.rs:68`, `:93`, `:120`,
`:151` (normal, emergency, wrapup, reattach);
`crates/host-runtime/tests/model_execution_subprocess.rs:4266`
`a_day_of_rotations_and_an_external_login_reuses_one_adapter_and_owner`.
Impact: folding stalls after a credential outage until the daemon restarts.
Open questions: None.

### source-health-is-advisory-and-secret-free

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the shared vectors cover every state, the bounds, malformed
blocks, and selector and token canaries in Rust and TypeScript; the host
reports the cached observation for each source mode
Guarantee: source health performs no credential I/O, carries no secret or
selector canary, and an absent or malformed block never reads as ready.
Check: `always` - every serialized `aws_credentials` block matches the closed
shape or is dropped whole, at every evaluation, because one leaked canary is a
disclosure.
Fault/timing angle: none.
Required faults and enabling state: blocks with extra keys, out-of-set values,
out-of-bound numbers, and canary strings for profile, account, role ARN, path,
session, hash, token, and error text.
Confidence: medium - [evidence](evidence/source-health-is-advisory-and-secret-free.md).
The vectors and the host test were read; the absence of credential I/O in
`health()` is shown by code reading, not by an I/O counter.
Existing check: `crates/host-runtime/tests/source_health_vectors.rs:5`
`rust_sanitizer_matches_the_shared_vectors`;
`packages/opencode-plugin/src/shared/source-health.test.ts:25`;
`crates/host-runtime/tests/model_execution_protocol.rs:408`
`health_reports_the_cached_source_observation_for_each_mode`.
Impact: a secret reaches a status surface, or health authorizes a dispatch.
Open questions:
- Should `health()` carry an I/O counter so the no-credential-I/O half has a
  runtime witness? (needs human input)

### bounded-history-outage-recovery

Type: liveness
Reachability: test-only
Status: active
Exercised: partial - the campaign harness, its gates, the outage judge, and
the lineage oracle run, and their failing controls fire; no run on the
required 4 CPU, 16 GiB, local-SSD runner has passed every gate
Guarantee: under the fixed fixture and load, every case keeps each operation's
publication unique and its raw history, passes the RSS, latency, read, and
allocation gates, and returns the backlog to its pre-outage band within 360
seconds after a 120-second outage.
Check: `sometimes` - a complete five-repetition campaign on the required
runner must pass every gate at least once; per-operation lineage is asserted
with `always` inside each repetition by `audit_lineage`.
Fault/timing angle: the 120-second source outage, its cooldown, and restored
demand at the frozen 50% arrival rate.
Required faults and enabling state: the outage schedule, the retained-history
tiers from 10k to 1m messages and 1000 sessions of 1000 messages, and every
witness above recorded as passed at the report's source commit.
Confidence: medium - [evidence](evidence/bounded-history-outage-recovery.md).
The gates and oracles were read and run; the qualifying run needs external
hardware.
Existing check: `crates/eval-core/src/qualification.rs:910` `audit_lineage`;
`:1296` `the_outage_judge_fires_on_each_failure`;
`crates/daemon/examples/eval_runner/qualification.rs` (the campaign runner);
`crates/host-runtime/tests/model_execution_supervisor.rs:1813`
`warm_acquisition_p99_stays_within_one_millisecond` (ignored, release only).
Impact: a million-message or outage claim without evidence that it holds.
Open questions:
- Supply the dedicated Linux x64 GNU runner with 4 logical CPUs, 16 GiB, and
  local SSD. (needs human input)

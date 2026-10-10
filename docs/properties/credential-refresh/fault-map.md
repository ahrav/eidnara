# Fault map: credential refresh qualification

Fault classes the five records require, where each is available, and the
cheapest valid oracle. The specification's own map routes expiry and clocks to
N2, source and model classifications to N8, restored eligible opportunities to
N9, malformed projections and canaries to N10, and retained-history skew and
outage to N13. Every safety witness runs while its fault is active; N9 and N13
need the fault-free interval after it.

## Fault classes and availability

| Class | Injection seam | Available in |
| --- | --- | --- |
| Row lifetime at, below, or above the bound | `scripted_dispatch` outcomes `Ok(lifetime)`; the owner harness `h.ok(lifetime)` | `crates/host-runtime/tests/model_execution_subprocess.rs`, `crates/host-runtime/src/model_execution/aws_refresh.rs` tests |
| Wall-clock jump or rollback | `ShiftedClock` with `jump: Some((read, seconds))` and `shift`; the owner harness clock | `model_execution_subprocess.rs:3870`, `aws_refresh.rs` tests |
| Typed source failure with cooldown | `HistorySummarizerProducerError::RunFailed` with `ErrorScope::CredentialSource` and `retry_after_secs` | `crates/daemon/src/source_recovery_tests.rs:7`, `crates/daemon/src/history_summarizer.rs` tests |
| Unproven model effect | scripted start failure, `with_cancel_result(Err(TimedOut))`, cross-incarnation unknown | `history_summarizer.rs` tests |
| Backoff expiry | `expire_history_summarizer_backoff`, `fire_and_settle` | `crates/daemon/src/lib.rs:40623`, `:41620` |
| External login during rotation | `TransactionFailure::Withdrawn` in the scripted outcome list | `model_execution_subprocess.rs:4266` |
| Malformed or canary-bearing health block | the shared vector fixture | `crates/host-runtime/tests/fixtures/source-health-vectors.json` |
| 120-second source outage under load | the campaign runner's fixed schedule | `crates/daemon/examples/eval_runner/qualification.rs`; needs the dedicated runner |
| Retained-history tiers to one million messages | the campaign catalog | same runner; needs the dedicated runner |

## Required faults per record

| Record | Required faults and enabling state | Witnessed |
| --- | --- | --- |
| `credential-lifetime-covers-dispatch` | lifetime at, below, and just above the bound; a wall jump between acquisition and the spawn recheck | yes |
| `typed-source-failure-preserves-model-certainty` | unproven start effect; unproven cancel after a source failure; cross-incarnation unknown; a source failure with another source available | yes |
| `eligible-demand-recovers-after-renewal` | a typed source failure on each of four paths; backoff expiry; a usable response; an eligible operation; 24 rotations with one external login | yes |
| `source-health-is-advisory-and-secret-free` | extra keys, out-of-set values, out-of-bound numbers, eight canaries | serialization yes; credential I/O count no |
| `bounded-history-outage-recovery` | the outage schedule; the retained-history tiers; every witness recorded at a clean commit | harness and controls yes; the qualifying run no |

## Coverage checks to add

- A counter on the owner's refresh path, asserted unchanged across a health
  read, for the no-I/O half of N10. Awaits the open question in the record.
- A binding between a `WitnessRun` and the environment and build profile it
  ran under, so a `warm_acquisition` run from the wrong host or a debug build
  cannot clear `pending_witnesses`. Awaits the open question in the N13
  record.
- An append-only publication trail or a per-operation `audit_lineage` call in
  the campaign runner, so a transient duplicate or raw loss repaired before the
  end-of-run observation still fails the run. Awaits the open question in the
  N13 record.
- A recorded campaign run on the dedicated runner, which is the only coverage
  N13 accepts.

## Oracle cost ranking

Cheapest valid oracle first.

1. N10 serialization: a pure function over a JSON fixture; the vectors run in
   milliseconds in two languages.
2. N2: owner unit tests under a paused tokio clock; the subprocess checks add
   two child-free spawns per adapter.
3. N8: scripted producer counts in unit tests; no child, no clock.
4. N9: path witnesses need a handler, a store, and a scripted producer with
   10-second wait budgets; the soak drives 49 scripted runs.
5. N13: the dedicated runner, a release build, five repetitions of at least
   10,000 interactive operations per case, and a 24-hour virtual soak.

# tail-hygiene-integrated-payoff

## Discovery trigger

The integrated decoded-ingress fixture meets the frozen ticket-local payoff
rule: **YES**, with **72.5513% less measured warm-call time**. This supports
[B4](../catalog.md#hygiene-digest-is-kind-prefixed-part-content), not a new
property or a global performance gate. It closes the merged-workload evidence
gap without reinterpreting the [historical 73.1659% result](tail-hygiene-payoff.md).
That earlier result retains its original fixture, artifacts, and limits.

## Evidence trail

The safe experiment root is `/tmp/opencode/tail-hygiene-integrated-payoff/`.
The [compact manifest](tail-hygiene-integrated-payoff.json) records selected
identities, receipt hashes, commands, and results. `plan.md` fixes experiment
version 3 before measurement; `freeze.json` pins the plan, collector, parser,
build receipts, source identities, executables, and runtime libraries.
`results.json`, `aa-summary.json`, and `ab-summary.json` record the result.
Raw samples and per-attempt receipts remain local scratch artifacts, not an
in-repository archive or a promise of CI retention.

### Before and after

Three adjacent A/A pairs run AB, BA, AB, with both labels using A. Five adjacent
A/B pairs follow in order AB, BA, AB, BA, AB. All 16 fresh processes are valid
and exit zero. Each has 20 Criterion samples and a unique empty Criterion home.
There are no retries, discarded samples, extra pairs, or numerical early stops.
A is measured again beside B, not compared only with the A/A pilot.

Times are milliseconds. Each median and p95 summarizes 20 batch-average call
times, `times[j] / iters[j]`. The nearest-rank p95 is sorted element 19 of 20,
not an individual-call tail percentile. Samples are not pooled as independent
replicates, and process p95s are not averaged.

| Pair | Order | A median | A p95 | B median | B p95 | B/A median ratio |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | AB | 2.580180 | 2.598944 | 0.693683 | 0.695588 | 0.268850492 |
| 2 | BA | 2.502206 | 2.506712 | 0.706451 | 0.707134 | 0.282331262 |
| 3 | AB | 2.504463 | 2.510349 | 0.690969 | 0.694540 | 0.275894918 |
| 4 | BA | 2.563657 | 2.639327 | 0.698248 | 0.701688 | 0.272364149 |
| 5 | AB | 2.550050 | 2.590678 | 0.696614 | 0.705474 | 0.273176731 |

The A/A control uses the same units and summaries; B here is only a label.

| Pair | Order | A-label median | A-label p95 | B-label median | B-label p95 |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | AB | 2.557044 | 2.559142 | 2.569327 | 2.604335 |
| 2 | BA | 2.541800 | 2.545796 | 2.529493 | 2.531642 |
| 3 | AB | 2.530232 | 2.553868 | 2.516758 | 2.522700 |

The six A/A medians span 2.516758 to 2.569327 ms, or 2.088761% relative to
the minimum. Their paired `ln(B/A)` values are 0.004792127, -0.004853438,
and -0.005339433; the mean is -0.001800248 and sample SD is 0.005714333.
The new guard is `N = ln(max AA median / min AA median) = 0.020672455349861535`.
It is a local empirical sensitivity guard, not a calibrated noise floor. No
older experiment supplies samples or a guard to this analysis.

For the five A/B contrasts, `g = ln(A median / B median)`. Their minimum,
`1.2646742080085076`, exceeds N. Their mean `G = 1.292852184943174` exceeds
`2s = 0.03652124802183599`, where `s = 0.018260624010917995` is the sample SD.
Both strict conditions hold. The geometric mean B/A ratio is
`exp(-G) = 0.2744867785014561`; the time reduction is
`100 * (1 - exp(-G)) = 72.55132214985439%`.

The prespecified conditional paired-t 95% interval uses
`G +/- 2.776445105 * s / sqrt(5)`. Its B/A interval is
`[0.26833321147542144, 0.28078146256229874]`, equivalent to 71.9219% to
73.1667% less time. It assumes approximately normal, independent, stationary
pair contrasts for these exact artifacts and this host window. Five pairs do
not establish those assumptions. Deterministic counterbalancing is not random
assignment; order counts are unequal and fresh processes do not reset host,
VM, or thermal state. The interval is conditional reporting, not an extra
acceptance threshold, calibrated merge-confidence limit, or CI guarantee.

### Exact source and build identity

A is parent `16542f5eb2f39ecd324f1b6bc804c5968e313d92`, exported with
`git archive` to the safe root's `baseline/` outside the working tree, plus
only the required release-accessor repair. The repair changes six lines to
six replacement lines in `enforce_unique_tool_use_ids` in `transform.rs`.
It replaces private-field access with accessors and uses `content_mut()` for
mutation in the release branch. It adds no memo or benchmark change. The
unrepaired source is not the measured baseline. The preserved repair patch
SHA-256 is `d38af4f5d80f494a1183fede732ea0b0891cad9041cd2db30dd28da44458d155`;
the applied diff SHA-256 is
`cf809da2d65dd1dcec698ba7f47f832f1f78e757eadcdddc03ed6d51a59cfd74`.

B is exactly `05c33bf03736b394dba65af572fbe15be0ad89ae`. Both arms use the
same decoded-ingress helper, corpus, wire/projection code, serialization
dependencies, and tokenizer inputs. Only `hot_path.rs`, `lib.rs`,
`tail_hygiene.rs`, and `transform.rs` differ between their source-input maps.
The benchmark diff consists only of memo construction/priming and the timed
memo argument. Fixture equivalence follows from deterministic construction
and source equality, not a captured runtime payload hash.

Executable paths below are relative to `/local/home/ahrav/scratch/eidnara`.

| Identity | A | B |
| --- | --- | --- |
| Executable | `target/hygiene-integrated-before/release/deps/hot_path-c149ac66da8b63fb` | `target/hygiene-integrated-after/release/deps/hot_path-c149ac66da8b63fb` |
| Executable SHA-256 | `34906f151678a54a195357f0d5f7b2fc1c7c9e70e8d87eb583c559f3df0825d7` | `c63376f94550eb0d02801412d71afc587577eebde9a1e119de254d0427162970` |
| Source-input manifest SHA-256 | `3ca4bccf7828df97d7f7ec5baba9d8afdf5d5bdc4df8ef859d5614b5105059c4` | `e2f84153bd7e9f327b7f12a3482a3a0a92d1dc9d59bcd6f236740395dadbf142` |

Each manifest covers 477 inputs and hashes Python's
`json.dumps(inputs_sha256, sort_keys=True)` encoding. The existing dirty
tracker document is excluded from build identity and remains untouched.
Both target directories are new for this experiment; source roots and build
layouts differ. The claim is conditional on the exact linked artifacts, not
an independently replicated build population.

Both builds request toolchain 1.98 and resolve Rust/Cargo 1.98.1, with LLVM
22.1.8 and native `x86_64-unknown-linux-gnu`. The explicit `bench` profile has
optimization level 3, no debuginfo, and disabled debug assertions and overflow
checks. The effective features are `bench-internals` and `test-support`, with
matching resolved feature closure. Criterion is 0.8.2. The host is AMD EPYC
9R14 under KVM, Linux `6.12.103-127.188.amzn2023.x86_64`. All observed thread
masks during warmup, collection, and analysis are CPU 0, within cpuset 0-127.
Affinity is not CPU isolation. The process window is 2026-09-12 04:34:30 to
04:38:09 UTC. Absence of overlapping controller work is controller-reported,
not evidence that the host is otherwise idle.

### Recorded commands

These identify completed runs, not instructions to overwrite frozen evidence.
A's build cwd is `/tmp/opencode/tail-hygiene-integrated-payoff/baseline`;
B's is `/local/home/ahrav/scratch/eidnara`:

```sh
cargo bench --locked -p daemon --features bench-internals --bench hot_path --profile bench --no-run --message-format=json --target-dir /local/home/ahrav/scratch/eidnara/target/hygiene-integrated-before
cargo bench --locked -p daemon --features bench-internals --bench hot_path --profile bench --no-run --message-format=json --target-dir /local/home/ahrav/scratch/eidnara/target/hygiene-integrated-after
```

The first A/B pair uses these argv values. Each process cwd is its source
root's `crates/daemon` directory. The collector assigns separate Criterion
homes at `raw/ab-p1-A/criterion` and `raw/ab-p1-B/criterion` under the safe
root. Later receipts record the corresponding pair and label; A/A uses A
at both label positions.

```sh
/usr/bin/taskset -c 0 /local/home/ahrav/scratch/eidnara/target/hygiene-integrated-before/release/deps/hot_path-c149ac66da8b63fb tail_hygiene/measure/1400msgs_2KiB_mixed --exact --warm-up-time 3 --measurement-time 5 --noplot --bench
/usr/bin/taskset -c 0 /local/home/ahrav/scratch/eidnara/target/hygiene-integrated-after/release/deps/hot_path-c149ac66da8b63fb tail_hygiene/measure/1400msgs_2KiB_mixed --exact --warm-up-time 3 --measurement-time 5 --noplot --bench
```

The fixed schedule waits 30 seconds before each phase, uses Auto sampling,
and allows 900 seconds per process. Three seconds of warmup and a five-second
measurement target do not truncate Criterion's batch-duration expansion.
The collector constructs allowlisted child settings instead of inheriting
or recording a full environment. This manifest exports selected identities,
not environment snapshots or credential values. The older secret-bearing
raw experiment is neither read nor copied for this update.

## Failure scenario

Reusing the older typed-ingress result for the integrated fixture would
misstate the measured treatment. Naming the unrepaired parent as A, omitting
slot overhead or full-result construction, pooling batch subsamples as
replicates, or calling batch p95 individual-call latency would also exceed
the evidence. The fixed rule treats invalid/incomplete evidence as BLOCKED,
not permission to retry or replace a run.

## Timing windows and dependencies

The [decoded helper](../../../../../crates/daemon/benches/hot_path.rs#L68-L81)
serializes and decodes the seeded corpus, then asserts retained original
message JSON. The cell has 1,400 messages with nominal 2,048-byte text and
seed `0x9E3779B97F4A7C15`. Each four-message cycle contains user text,
assistant text, a tool call, and a tool result; command input is capped at
256 bytes. Serialized messages are not uniformly 2 KiB.

The [cell](../../../../../crates/daemon/benches/hot_path.rs#L137-L175)
constructs the projection and primes B's actual 16-slot pool outside both
the benchmark callback and `b.iter`. The
[wrapper](../../../../../crates/daemon/src/lib.rs#L148-L174) uses namespace 0
and session ID `benchmark`. Session hashing, slot locking, memo validity and
accounting, full measurement construction, and its drop remain inside the
timed call. A uses the original non-memo wrapper with its warm token cache.
Tokenizer initialization is outside timing.

B is the fixed-slot memo revision. The later session-table revision, which
refuses over-budget blocks instead of resetting the memo, fingerprints caveman
units instead of cloning them, and replaces the sixteen hashed slots with a
least-recently-used table behind per-session locks, is not measured here; no
timing is claimed for it.

Core, tag rows, and protected IDs are empty; coverage is `None` and
`protected_tags` is 20. U is zero. Populated attribution and caveman
invalidation are not exercised. This is repeated warm in-process service
time at one synthetic point, not production, concurrent-session, cold-call,
delta-ingress, total-turn, or session latency. No peak-allocation or RSS
measurement is made. B4's retained-accounting tests are separate evidence.

## What a test must construct

Payoff does not replace B4's correctness or retention checks. The
[existing-check inventory](../existing-checks.md#shared-input-equivalence)
keeps their adequacy status unaudited. The controller reports all 14 local
gates passed on `05c33bf0`; their outputs are
`/tmp/opencode/memo-integrated-*.log`, listed in the manifest. Selected log
summaries corroborate execution outputs, but empty logs do not independently
prove exit status or source revision. No test, build, or benchmark is rerun
for this documentation update, and no remote CI result is claimed. The
pre-memo characterization remains agent-witnessed, transcript-only evidence.

## Investigation log

### Q: Does the safe bundle support the integrated local payoff claim?

- Sources examined: The plan, freeze, result and phase summaries, source maps,
  archived baseline, repair diffs, build receipts, raw samples, process and
  affinity receipts, executable/runtime hashes, and selected gate-log summaries.
- Findings: All 230 files named by the safe root's `checksums.json` match.
  Its SHA-256 is
  `07162218af255239dbf1bd21e1e1802346b0cbadd4539c0bfb5148d82c285e81`.
  Recomputing all 16 medians/p95s and the paired analysis reproduces the
  summaries, interval, and both strict inequalities. Both 477-input source
  maps and the executable/runtime hashes still match during this docs-only
  pass. The archived baseline differs from A only by the 12-line accessor
  repair. The prior executables remain unchanged; no older raw receipt is read.
- Missing evidence: Host/build replication, production or concurrent-session
  payoff, cold-call timing, allocator/RSS validation, and independently
  replayable pre-memo characterization remain outside this receipt.
- Conclusion: resolved with answer - the new experiment supports the integrated
  local warm-call payoff decision. The historical 73.1659% record is preserved,
  not deleted or extended to a workload it did not measure.

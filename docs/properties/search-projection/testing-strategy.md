# RP2.1 testing strategy

## Owner, status, and scope

Independent analyst `ses_f7623ccbaffen7QcEpC0dqyIIS` supplied the testing-owner
report using `/testing:test-strategy`, after coordinator routing through
`/research-planning:ask-skills`. This document preserves the adopted strategy,
not a test run. Product revision is
`913234433ae36a80a6e22c6aac14c7f9aab74386`, inspected on 2026-09-10.

All 30 catalog records are proposed-only, individually classified `test-only`,
and unexercised. The 25 safety, four liveness, and one reachability claims need
nine shared groups, not 30 harnesses. Catalog records retain the exact METHOD
schema, check semantics, evidence, required faults, and open questions. Existing
checks stay `unaudited`; neither this report nor the discovery pass supplies an
invariant-test adequacy verdict.

## Shared groups

### T1. Pure identity, raw-buffer, and token fixtures

Primary properties:

- [projection-occurrence-payload-separation](projection-coverage/catalog.md#projection-occurrence-payload-separation)
- [projection-raw-tools-exact-and-lexical](projection-coverage/catalog.md#projection-raw-tools-exact-and-lexical)
- [embedding-count-authority-is-untruncated](embedding/catalog.md#embedding-count-authority-is-untruncated)

Use examples for exact boundaries and proptest for tuple/byte transformations.
Fixture-owned source tuples, independently captured buffers, and pinned complete
token sequences are the oracles. Vary class, namespace, revision, representation,
and span separately; equal payloads must retain separate occurrences. Force a
digest collision between unequal bytes. Cover CRLF, whitespace, Unicode, escaped
JSON-looking text, repeated/overlapping spans, multipart results, and errors.
Token fixtures include special-token/padding semantics, same-prefix texts that
straddle the model limit, and artifact-path replacement after verification.

Negative controls collapse occurrence to payload identity, normalize raw bytes,
or count only a truncated prefix. Each must fail its distinct assertion. Codec
JSON value equality is not an exact raw-buffer oracle. Canonical encoding,
span convention, collision response, and immutable token fixture need approval.

### T2. Real preflight with an engine-attempt counter

Primary property:

- [embedding-input-is-rejected-before-inference](embedding/catalog.md#embedding-input-is-rejected-before-inference)

Combine real tokenizer preflight with the deterministic Synapse engine's
observable attempt boundary. Take the counter baseline after certification.
Use valid control input, exact-token limit, limit plus one while bytes fit,
byte-cap edge/overflow, unavailable count, and unavailable identity. Compare
submitted bytes independently; invalid stored-source attempts retain lexical
state, missing dense coverage, and no completion. Queries return no dense result.

The negative control moves validation after dispatch or silently truncates.
Counter growth falsifies preflight safety even when returned output looks valid.
This group cannot replace the campaign's required certified-artifact inference
witness with a double, ignored test, or disabled lane.

### T3. Held completion and identity-GC matrix

Primary properties:

- [embedding-completion-is-identity-fenced](embedding/catalog.md#embedding-completion-is-identity-fenced)
- [embedding-identity-gc-preserves-live-work](embedding/catalog.md#embedding-identity-gc-preserves-live-work)
- [projection-remediation-invalidates-derived-bytes](projection-coverage/catalog.md#projection-remediation-invalidates-derived-bytes)

Hold a validated old result while independently changing revision, model name,
fingerprint, dimension, input hash, epoch, or tombstone. Include equal bytes in
distinct occurrences and a model-name change with unchanged fingerprint.
Completion and GC use an independent current-source/live-reference oracle, not
the saved job or projection row as authority. Race GC selection with new work,
a held result page, identity change, and late completion. Check eligible
unreferenced removal as well as preservation, so a no-op GC cannot pass.

Where approved mapping includes `domains.name`, remediate that canonical field
without changing source revision, recompute mapped input independently, and
release/replay old work. Do not assume every work-key component is unchanged
or prescribe a new generation. If mapping excludes the field, record supported
applicability rather than manufacture a dependency. Unresolved mapping is not a
passing exemption. Negative controls compare revision alone, trust a stale
projected hash, ignore one identity dimension, or delete a live holder.

### T4. Real-store seeded canonical operation ledger

Primary properties:

- [rp21-export-fixed-s-exactly-once](export-recovery/catalog.md#rp21-export-fixed-s-exactly-once)
- [rp21-catchup-complete-commit-prefix](export-recovery/catalog.md#rp21-catchup-complete-commit-prefix)
- [projection-replay-does-not-resurrect](projection-coverage/catalog.md#projection-replay-does-not-resurrect)
- [projection-source-inventory-complete](projection-coverage/catalog.md#projection-source-inventory-complete)
- [projection-lexical-dense-coverage-distinct](projection-coverage/catalog.md#projection-lexical-dense-coverage-distinct)

Extend the existing kernel proof fixture with a small all-class reference
ledger. Generate bounded insert/revise/delete/control histories with deterministic
seeds. The ledger owns source bytes, classes, occurrence tuples, revisions,
tombstones, complete commit boundaries, and expected policy exclusions.
Materialized projector output and `pending_outbox` cannot define expectations.

Export multiple pages at fixed S across mutations and class/empty-page edges;
catch up split commits, published-but-retained rows, empty commits, and control
events. Replay identical and overlapping prefixes after reopen and deletion.
Compare multiplicity and per-class coverage sets, including empty classes and
unknown inventory. Missing dense coverage is required minus valid; pending
coverage is its subset backed by current non-obsolete durable pending work.
Include missing occurrences with and without pending work, plus a valid vector
with pending completion bookkeeping. The last consumes job capacity but is not
missing coverage. Legitimate canonical remediation belongs in the operation
ledger; recovery must not undo it.

Negative controls omit a class from both report and denominator, advance on a
partial commit, omit published rows, duplicate an occurrence, or revive an older
revision. The existing model tracks domain, decision, and observation, not all
five RP2.1 classes. Its CrossRoot digest normalization is not a fixed-S identity
oracle; preserve exact required identifiers, revisions, and bytes.

### T5. Shared named-boundary process-crash pattern

Primary properties:

- [projection-commit-checkpoint-pending-atomic](projection-coverage/catalog.md#projection-commit-checkpoint-pending-atomic)
- [rp21-ack-follows-local-release](export-recovery/catalog.md#rp21-ack-follows-local-release)
- [embedding-complete-requires-durable-vector](embedding/catalog.md#embedding-complete-requires-durable-vector)
- [rp21-replacement-selects-complete-compatible-state](export-recovery/catalog.md#rp21-replacement-selects-complete-compatible-state)

Reuse CAS child stdout barrier, bounded parent wait, kill/reap, and reopened-state
comparison. Introduce only concrete product cuts around local COMMIT/release,
kernel ack attempt/COMMIT/response, vector persistence/completion, and selector
publication. Retain an external witness for each declared cut; a callback
executed in a surviving process is not a crash. Recover twice and compare the
same independently specified complete prefix and selected compatible state.

Projection owns local rows/checkpoint/pending atomicity. Export/recovery alone
owns ack/lock ordering and `rp21_ack_local_commit_interrupted` plus
`rp21_ack_response_lost`; reuse those definitions. Embedding owns durable
vector-before-completion. Count attempts and observed acknowledgements separately
from durable effects per identity. Lost responses remain unknown until readback;
aggregate totals can conceal per-identity duplication or loss.

Negative controls ack before local durability/release, mark complete before
vector durability, or select partial state. Contend the kernel writer to expose
cross-store lock overlap. SQLite durability mode, complete journal-bearing
publication unit, and vector completion boundary must be settled first. These
tests prove process-crash/injected-error behavior on the tested filesystem, not
power-loss, torn-write, or cold-device persistence.

### T6. Pre-admission bounds and physical observation

Primary properties:

- [rp21-export-predecode-bounds](export-recovery/catalog.md#rp21-export-predecode-bounds)
- [projection-bounded-admission-preserves-progress](projection-coverage/catalog.md#projection-bounded-admission-preserves-progress)

Observe independently measured encoded sizes and decoder/materialization entry
before allocation, then local state after refusal. Exercise exact caps, an
oversized single row, a row fitting alone but not in the remaining page, many
small rows, decoded JSON expansion, full durable pending capacity, and a complete
commit larger than the batch/transaction cap. Refusal must not skip input or
advance local progress. Reuse size queries and test-support before new seams.

Logical accounting is not live-heap proof. A separately approved observer must
measure decoded-memory high water and identify counted allocations. The existing
unsafe perf allocator counts cumulative requested bytes; kernel forbids unsafe
code. No allocator, new test crate, dependency, or lint change is selected here.
Negative controls truncate a fully loaded snapshot, skip the oversized row,
or acknowledge a refused commit. Small test fixtures expose the mechanism;
only RP2.9-approved envelopes can clear acceptance bounds.

### T7. Real daemon source, authority, gate, and disable boundaries

Primary properties:

- [rp21-export-retention-fence-covers-read](export-recovery/catalog.md#rp21-export-retention-fence-covers-read)
- [projection-n13-hooks-stay-gated](projection-coverage/catalog.md#projection-n13-hooks-stay-gated)
- [projection-canonical-eligibility-authority](projection-coverage/catalog.md#projection-canonical-eligibility-authority)
- [rp21-disable-preserves-consumer-obligations](export-recovery/catalog.md#rp21-disable-preserves-consumer-obligations)
- [rp21-recovery-preserves-canonical-authority](export-recovery/catalog.md#rp21-recovery-preserves-canonical-authority)

Use real daemon/kernel fixtures, independent source and hook inventories, and
actual consumer/barrier rows. Attempt pruning under a valid export fence and
remove required source/history coverage mid-build. Compare both policy adapters
on pinned canonical facts, then revise/retire/change scope or sensitivity before
final use. Observe that recovery changes only derived state and separately
accounted lifecycle/audit facts.

The hook matrix covers startup, reload, supervisor and explicit entry points,
hostile project/user flags, each missing/failed/unsupported/inapplicable gate,
and valid supported activation controls once the product gate exists. Exercise
lagging/caught-up consumers, last/non-last removal, barriers, restart, and
explicit audited abandonment separately. Empty consumers mean unavailable
gated reads and no prune horizon; direct tip reads retain their own rule.

Negative controls accept stale projected grants, publish after source loss,
bypass one hook gate, silently abandon a pending consumer, or rewrite canonical
facts during repair. N1.3 source sweeps remain source-owner work, not general
Dreamer scope. Product fences, gates, and recovery authorization remain proposed.

### T8. Contended backfill and shared-deadline physical census

Primary properties:

- [embedding-pending-drives-one-job-table](embedding/catalog.md#embedding-pending-drives-one-job-table)
- [embedding-backfill-preserves-query-admission](embedding/catalog.md#embedding-backfill-preserves-query-admission)
- [embedding-supervisor-shares-budget-and-joins](embedding/catalog.md#embedding-supervisor-shares-budget-and-joins)

Use durable pending readback plus admission/worker observations to rediscover
one job concurrently, lose an admission response, and fill capacity. An
`Existing` response must not create a worker; refusal cannot erase pending.
Offer independently recorded queries while bounded backfill remains saturated
and query occupancy is below its approved cap. Observe offered, admitted,
started, and completed states separately; do not select only successful queries.

Reuse engine gates and the scheduler's in-crate ManualClock. Cross the original
EvalBudget deadline between stages; cancel during queue wait, tokenization,
native execution, or completion. Hold native work while another maintenance
slice is due and shutdown begins. The task/permit/live-charge census remains
nonzero until physical exit, even after the future is cancelled. Negative
controls dispatch without pending, create a second worker/queue, renew a timeout,
allow FIFO backfill starvation, or release charges on future drop.

RP2.1.U3 integrates priority and preflight. RP2.7 owns the async route and shared
budget integration; its implementation is not imported into RP2.1. RP2.9 and
admission owners must approve query service opportunities and physical-drain
bounds before this group can establish bounded service.

### T9. Bounded catch-up, rebuild, and authorized recovery

Primary properties:

- [rp21-rebuild-after-pruning-converges](export-recovery/catalog.md#rp21-rebuild-after-pruning-converges)
- [rp21-catchup-and-authorized-recovery-converge](export-recovery/catalog.md#rp21-catchup-and-authorized-recovery-converge)
- [embedding-restart-retries-durable-pending](embedding/catalog.md#embedding-restart-retries-durable-pending)

Construct four explicit episodes: ordinary finite backlog; projection deletion
after proven outbox pruning with retained sources; Disabled followed by explicit
recovery authorization and accepted gates; and process loss with stable durable
pending followed by a fresh JobTable. T4 supplies canonical expectations, T5
supplies actual crashes, and T7 supplies authorization/lifecycle observations.

At admission fix mode, start, finite target, workload bounds, healthy dependency
and service assumptions, and approved work/attempt ceilings. Stop faults and new
write pressure for the declared window. Require actual Current, complete
coverage/local prefix and reconciled kernel ack, or matching durable vector
completion as appropriate. Retry cannot move the target or reset the clock.
Changed work becomes explicitly obsolete without completing newer work.

Negative controls remain safely Disabled forever, restart endlessly, do no work,
or move the deadline/target. Safety without progress cannot pass. RP2.9 must
approve symbolic `B_catchup_ms`, `B_recovery_ms`,
`B_authorized_recovery_ms`, embedding recovery/attempt bounds, and finite
envelopes. Harness timeouts and existing serving defaults do not supply them.

## Binding situation coverage across T1-T9

The remaining property is
[projection-acceptance-situations-witnessed](projection-coverage/catalog.md#projection-acceptance-situations-witnessed).
It applies across every group rather than adding another harness. The
[export map](export-recovery/fault-map.md),
[projection map](projection-coverage/fault-map.md), and
[embedding map](embedding/fault-map.md) define 23/24/16 markers, 63 total.
Definitions are globally unique and constant. The aggregate marker is counted
as a definition but excluded from its own prerequisite matrix.

Before execution, freeze every required `(marker, scenario_id)` cell and its
bounded dimensions: all five source classes; revision/deletion/identity changes;
all hook/gate entry cases; nonempty independent inventory and actual completeness
declaration; certified tokenizer/inference offered input; every declared local,
ack, vector, and selector crash boundary; and actual Current recovery outcomes.
A marker must witness independent preconditions or the specified positive
situation, not the safety violation. One fired marker cannot cover its other
dimensions. Unknown, skipped, or unfired required cells fail acceptance.

Negative control: remove any required witness, crash boundary, source class,
or valid supported activation scenario from an otherwise passing report. The
campaign verdict must fail. Disabled unsupported optional paths never replace
required supported evidence. Resolve conditional mapping before the campaign;
post-run scope reduction is not permitted.

## Reuse, gates, and retained evidence

The [source register](source-register.md#current-implementation-evidence-and-qualifications)
links inspected current-head fixtures: kernel proof/test-support and canonical
digest, CAS crash pattern, deterministic Synapse engine, ManualClock, real
daemon eligibility/serving, and generation lifecycle. Use existing proptest and
temporary-store patterns. No broad deterministic-simulation framework or new
development dependency is the default. Existing fixture code is reusable
substrate, not an oracle for a different product contract.

Each future PR runs applicable focused behavioral and crash regressions plus
repository and RP2.9 gates. The coordinator declines deferring the whole crash
battery to weekly execution. Broad scheduled seed/fault sampling may add
coverage but cannot replace a changed boundary's focused regression. Required
checks come from [.github/workflows/ci.yml](../../../.github/workflows/ci.yml);
the specification preserves the exact source-plan gate block and the unresolved
`release:check` requirement. Every future PR also has the five independent
parallel pre-opening review obligations. None ran in this documentation task.

Retain seeds and minimized operation histories; product revision, toolchain,
features, shard, configuration, source hashes, fault boundaries, and witness
matrix version; external barrier/kill/reopen receipts; per-identity attempts,
acknowledgements, effects, and unknown outcomes; and resource/time observations.
Keep large crash images and restricted corpora in approved external storage.
Approved production caps and hard model token windows are distinct from bounded
experimental fixtures. Green results apply only to tested models, inputs,
platform/filesystem contracts, and crash cuts. No tests, builds, benchmarks, or
runtime campaigns ran for this report.

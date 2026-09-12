# Hot-path latency audit supplement

## Scope and provenance

This area extends the [parent supplement](../catalog.md) with the remaining
surfaces of a per-turn latency audit. It supplies reusable
`/property-discovery-and-catalog` input for a specification; it does not
authorize implementation or create tickets.

- The system is `/local/home/ahrav/scratch/eidnara`.
- The baseline is `913234433ae36a80a6e22c6aac14c7f9aab74386`.
- This is a working-tree supplement against that source baseline, not an
  artifact contained in the baseline commit.
- The source verification date is 2026-09-10.
- The parent records K1-K4, E1-E3, S1-S2, H1-H4, and R1-R3 cover canonical
  read pushdown, execution placement, callback batching, history-budget
  rendering, and prepared-field ownership. This area covers request ingress
  decode and admission, deep-copy elimination inside the pass, cache-state
  load and commit traffic with its pass trace and side channel, the
  TypeScript plugin's pre-send work, ring arena punching and direct
  serialization, artifact usage accounting, and the cross-cutting findings
  of the wildcard pass. The two gaps the parent's
  [portfolio evaluation](../portfolio-evaluation.md#gaps-queued) queued for
  transform-pass surfaces, the SOFT pressure-refold predicate and the
  post-commit bookkeeping under relocation, are records here.
- Six lens passes wrote their findings under `_lenses/`. Those files are
  working material for traceability, not test evidence and not independent
  corroboration. Every anchor below was rechecked against this HEAD; the
  corrections are listed in the synthesis report, not in the lens files.
- The supplied scope is the audit and in-repository code, documents, tests,
  and history. External plans and incident reports were not supplied. Final
  scope confirmation remains pending; this does not mean none exist.
- No tests, campaigns, or benchmarks ran as part of the discovery audit.
  The B4 implementation and local payoff evidence are a separate, dated update
  in [the B4 evidence](evidence/hygiene-digest-is-kind-prefixed-part-content.md)
  and [the historical payoff receipt](evidence/tail-hygiene-payoff.md).
  The [integrated payoff receipt](evidence/tail-hygiene-integrated-payoff.md)
  supplies the separate measurement for candidate `05c33bf0`.
- `portfolio-evaluation.md` in this directory records the fresh evaluation
  and, under "Disposition", what was applied from it.

The records constrain preservation. They do not claim an optimization exists
or is faster; W1 states what a "faster" claim needs before it is checkable.
Generic lifecycle, fencing, snapshot, redaction, punch-soundness, and
tokenizer obligations remain in their canonical catalogs, named under
"Relationships and retained canonical obligations".

## Reachability and boundaries

| Records | Class | Evidence and limit |
| --- | --- | --- |
| A1-A2 | default-production | Every request runs [`Handler::handle`][handle] and [`dispatch_value_with_inbound_bytes`][dispatch]. Refusal arms and the over-1 MiB probe need constructed input because the plugin [pages at 512 KiB][paging]. |
| A3 | test-only | Pool pressure needs concurrent oversize parses; production occurrence is plausible but unverified. |
| B1-B5 | default-production | Every pass with [`compaction_enabled`][cfg-compaction] (default true) projects, serves, and normalizes; the incremental arms need a cache hit, which the plugin's delta protocol produces on steady turns. |
| C1, C2, C4, C5 | default-production | The [handler path][handler] runs for every transform request; the Emergency95 arm needs usage at the emergency threshold. |
| C3 | explicit-config-only | The empty drain runs every pass, but outbox rows come from [`publish_historian_chunk`][publish], which needs a configured [`model_chain`][cfg-models]; `user_observation` rows also need [`user_memory_collection_enabled`][cfg-user-mem]. |
| C6 | explicit-config-only | The same gate as C3; the due row also needs a failed inline delivery at publish time or a process end between the enqueue commit and the inline drain, which the [`test-support` seam][fail-sc] constructs. |
| P1-P4 | default-production | The plugin runs the verdict, mid-turn, paging, and logging steps on every production pass; `deps.client` is the SDK client ([hook.ts][hookclient]). |
| P5 | test-only | Needs an injected SDK fault after a stored deny. |
| T1-T2 | default-production | [`process_limits`][process-limits] runs at host startup; every inbound frame calls [`to_vec`][to-vec] and every outbound frame calls `copy_in`. |
| T3-T4 | test-only | [`reserve_direct`][reserve-direct] is reached only through [`output_from_writer`][from-writer], whose sole non-crate caller is a [fixture arm][fixture-arm] no test sends. |
| G1-G2 | default-production | Every daemon [`ingest_artifact`][route-ingest] reaches [`check_budget`][check-budget]; GC and purge decrements have no daemon caller. |
| G3 | test-only | Four of five decrement paths run only in kernel tests and benches. |
| W2-W5, W7-W10 | default-production | The handler populates `timings`, injects the token cache, prepares `meta`, fires the historian, and merges config on ordinary passes; SOFT pressure needs workload. |
| W6 | explicit-config-only | The dreamer schedule [defaults to `None`][sched-default]; smart notes need a cron on the note. |
| W13 | explicit-config-only | The same gate as W6 for the scheduler consumer: [`scheduled_projects`][sched-projects] drops a project with no schedule ([`:14027`][sched-filter]) or outside `MODULE` authority, so a default campaign never calls `next_due`. |
| W1, W11 | test-only | Benches need `bench-internals` or manual `--ignored` runs; the only abort seam after commit is the `#[cfg(test)]` [hook][hook]. |
| W12 | default-production | Every `kernel.*` route that reaches the store runs its work through [`kernel_routes::blocking`][blocking] on a `spawn_blocking` worker with the redaction guard at depth `0`; the panic itself is the injected fault. |

Execution topology is unresolved at this HEAD, as the parent records. W8 and
W11 state the failure class a `spawn_blocking` relocation opens; they do not
prove such a worker is ready. W12 records the one worker-thread boundary the
daemon already crosses in production, for kernel routes. T3 and T4 are
test-only at HEAD because the
direct path has no production sender; the specification would move them to
default-production for whichever routes it moves to the direct path,
transform responses first. T3's check applies unchanged to a
`MeasuredSource::Json` response, which the owned path serializes twice at
HEAD (`crates/daemon/src/dispatch.rs:132-148` measures, `:237-249` writes).

## Index

| ID | Record | Type | Check |
| --- | --- | --- | --- |
| A1 | [admission-chain-charges-before-decode-and-refuses-effect-free][a1] | safety | always |
| A2 | [route-and-typed-decode-are-independent-of-entry-path][a2] | safety | always |
| A3 | [scratch-pool-shortfall-reaches-the-parse-reservation][a3] | reachability | sometimes |
| B1 | [derived-artifacts-are-ownership-independent][b1] | safety | always |
| B2 | [synthetic-normalization-is-scoped-to-the-pass][b2] | safety | always |
| B3 | [tag-baseline-cache-entry-is-never-mutated-by-a-pass][b3] | safety | always |
| B4 | [hygiene-digest-is-kind-prefixed-part-content][b4] | safety | always |
| B5 | [replayed-synthetic-pair-arrives-unflagged-on-a-delta-turn][b5] | reachability | sometimes |
| C1 | [consolidated-cache-state-reads-match-per-consumer-loads][c1] | safety | always |
| C2 | [pass-trace-writes-count-every-pass-outside-the-cache-cas][c2] | safety | always |
| C3 | [side-channel-drain-delivers-each-row-once-and-keeps-its-schedule][c3] | safety | always |
| C4 | [meta-json-preparation-scans-every-persisted-byte][c4] | safety | always |
| C5 | [foreign-write-lands-between-pass-loads][c5] | reachability | sometimes |
| C6 | [side-channel-row-is-due-during-a-drain][c6] | reachability | sometimes |
| P1 | [cached-todowrite-verdict-never-lifts-a-deny-or-outlives-its-inputs][p1] | safety | always |
| P2 | [mid-turn-read-is-invariant-under-query-collapse-and-statement-caching][p2] | safety | always |
| P3 | [paged-body-measure-equals-declared-frame-length-and-fits-host-caps][p3] | safety | always |
| P4 | [log-lines-keep-sanitizer-and-file-hardening-guarantees][p4] | safety | always |
| P5 | [todowrite-deny-then-read-failure-is-exercised][p5] | reachability | sometimes |
| T1 | [arena-residency-is-bounded-by-admission-and-one-punch-batch][t1] | safety | always |
| T2 | [arena-payload-copies-keep-the-address-derived-atomic-shape][t2] | safety | always |
| T3 | [direct-frame-publishes-declared-length-or-nothing-and-holds-its-charges][t3] | safety | always |
| T4 | [direct-frame-outlives-its-handler-before-publication][t4] | reachability | sometimes |
| G1 | [artifact-admission-fails-closed-against-on-disk-object-bytes][g1] | safety | always |
| G2 | [reported-artifact-usage-equals-on-disk-object-bytes-after-recovery][g2] | safety | always |
| G3 | [artifact-byte-decrement-paths-are-exercised][g3] | reachability | sometimes |
| W1 | [optimized-stage-is-measured-at-production-shape][w1] (invalidated) | reachability | sometimes |
| W2 | [stage-timing-fields-keep-their-boundaries][w2] (invalidated) | safety | always |
| W3 | [token-cache-is-a-pure-declared-memo-behind-one-estimator-interface][w3] | safety | always |
| W4 | [bounded-secret-scan-finds-every-whole-input-finding][w4] | safety | always |
| W5 | [historian-firing-input-is-preserved-by-cheaper-construction][w5] | safety | always |
| W6 | [cron-next-occurrence-matches-the-minute-stepper][w6] | safety | always |
| W7 | [effective-config-reads-observe-a-tier-change-by-the-next-pass][w7] | safety | always |
| W8 | [committed-transform-bookkeeping-is-applied-or-recomputed][w8] | safety | always |
| W9 | [soft-pressure-refold-predicate-preserves-its-classification][w9] | safety | always |
| W10 | [soft-pressure-refold-thresholds-are-each-crossed][w10] | reachability | sometimes |
| W11 | [abort-lands-between-transform-commit-and-bookkeeping][w11] | reachability | sometimes |
| W12 | [worker-thread-panics-stay-inside-the-redaction-boundary][w12] | safety | always |
| W13 | [cron-schedule-is-evaluated-for-a-configured-project][w13] | reachability | sometimes |

Twenty-seven `always` checks and ten `sometimes` checks are active. Two
records are invalidated and stay in the index for traceability:
[optimized-stage-is-measured-at-production-shape][w1] (W1, `sometimes`) and
[stage-timing-fields-keep-their-boundaries][w2] (W2, `always`). No catalog
record owns a before-and-after measurement artifact; each optimization change
proves its own payoff where the benefit is uncertain.
No record uses `always-or-unreached`, `reachable`, or `unreachable`. There is
no liveness claim with an invented deadline; C3's backoff and W6's search cap
restate bounds the code already fixes.

## Ingress admission and decode

### admission-chain-charges-before-decode-and-refuses-effect-free

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No handler-level refusal or charge-versus-decode
comparison runs; existing tests enter at `dispatch_value`. The real-host
fixture in `crates/daemon/tests/support/direct_host.rs` (`FixtureProcess`,
used by `direct_host.rs:48-70`) reaches `Handler::handle` over the ring and is
the seam for a handler-level test.
Guarantee: The pre-dispatch admission chain charges resident bytes before any
typed decode, never waits, and refuses without a dispatch-side effect.
Check: `always` - For every request entering [`Handler::handle`][handle]: the
[byte cap][bytecap] and [footprint scan][footprint] precede the parse charge,
which is a non-awaiting `try_charge` on the host [scratch pool][pools] and
never on the ingress pool; the terminal is `invalid_params` exactly when the
body exceeds its cap or `footprint > resident_capacity()`
([`request_too_large_error`][toolarge]) and `queue_full` exactly when the
footprint fits but the pool is short ([`resident_capacity_error`][queuefull]);
a refused request creates no [`TransformDispatchTicket`][ticket], changes no
`transform_route_channels`, prompt freeze, page staging, or store state, and
releases any taken charge when the future ends; and for every admitted
request `charge >= nodes * size_of::<Value>() * VALUE_NODE_SLACK *
RETAINED_NODE_COPIES + string_bytes * RETAINED_STRING_COPIES +
VALUE_ENVELOPE_BYTES`, with at most
[`RETAINED_STRING_COPIES`][copies] owned copies of each string block
retained by the typed decode, observed structurally (the `Value` node, the
`WireMessage` `original`, and the `WireBlock` `original` for one known
block) or through a counting allocator, since the decoded type exposes no
copy count. `always` because every request evaluates this
chain and the refusal set is defined by code position, not an observed
defect.
Fault/timing angle: A fused decode-and-charge design starts the typed decode
before the admission decision and reaches the post-parse side effects at
[`transform_route_channels`][routechan] and
[`freeze_prompt_surface_selection`][freeze], which precede
[`ticket.accept()`][accept]; a charge taken during or after allocation lets
the allocation escape the resident envelope, as
[`try_reserve_resident`][reserve] documents; an awaiting reservation would
park while holding pending and task permits.
Required faults and enabling state: A body over the applicable cap; a body
whose footprint exceeds [`resident_capacity()`][capacity] (about 176 MiB at
the fixed [`SCRATCH_RESERVED_BYTES`][scratchconst]); a scratch pool drained
by concurrent parses (A3); a large text block decoded through the full typed
path with the copy count taken from the resulting `TransformRequest`.
Confidence: high - [Evidence](evidence/admission-chain-charges-before-decode-and-refuses-effect-free.md).
The four steps, the pool identity, both refusal helpers, and the footprint
arithmetic are source-verified. The audit's `request_too_large` code exists
only in the [direct-host fixture control channel][fixture]; the handler
emits `invalid_params`, which matches [§6.3][wire63].
Existing check: [Ingress checks](existing-checks.md#ingress-admission-and-decode)
cover the cap, the footprint arithmetic, and pool splitting as pure
functions; no handler-level refusal check is found; all unaudited.
Impact: A refused body can leave route or staging state behind, or an
admitted body can hold more resident bytes than it charged.
Open questions:
- Does the context module owe a `retry_after_ms` on `queue_full` as Synapse
  does under [§7.5.1][wire751]? [`RequestOutcome::error`][outcome] sends
  none today. (needs human input)
- The paged lane reaches the typed decode from [assembled pages][pageapply]
  with no footprint reservation for the assembled whole; is the page staging
  budget the intended cover? (needs human input)
- [`SCRATCH_RESERVED_BYTES`][scratchconst] is documented as sized for Synapse
  budgets; the transform footprint shares the slice without appearing in
  that sizing. Is that intended? (needs human input)

### route-and-typed-decode-are-independent-of-entry-path

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No two-lane decode differential or discriminator corpus
runs.
Guarantee: The lane a body reaches, the cap it is admitted under, and the
typed request it decodes to depend only on raw discriminator and page-field
reads, and agree across the unpaged lane, the page lane, and any replacement
decode.
Check: `always` - Over a fixed corpus of body shapes assert three oracles
against a frozen reference: a test-only copy of HEAD's routing read, probe,
and `Value` decode kept under `crates/daemon/tests/` in the form of
[`historian_truncate_differential.rs`][diff-ref], or a recorded corpus of
expected route, code, and decoded request per body. Routing: `route(body)`
equals the
[`Value` read][dispatch] (`method` as a string, else `kind` as a string, a
non-string treated as absent; `{name, arguments}` without either selects the
facade; any of the six [`TRANSFORM_PAGE_FIELDS`][pageconst] present by key,
including `null`, selects the [page lane][pagefields]; anything else is
[`unrecognized_request_shape`][unrecognized]). Cap: for bodies over
`MAX_FACADE_FRAME_BYTES` the [probe][probe] admits only when the route is one
of the three [transform-class names][class], the ceiling is inclusive at
`MAX_TRANSFORM_FRAME_BYTES`, and the probe holds no body-proportional memory.
Decode: `decode_unpaged(body)` and `decode_via_value(body)` agree on accept
versus reject, on the error code, and on the resulting `TransformRequest`
for duplicate top-level keys (last wins today), unknown fields, `null` on
`Option` and on defaulted fields, missing `session_id` or `render_config`,
and malformed JSON (`unrecognized_request_shape` with `non-object JSON
(null)`). Conservative probe refusals on duplicate discriminator keys are
permitted. `always` because routing and decode run on every request and a
divergence misroutes or rejects silently; the corpus makes the check finite.
Fault/timing angle: None in time. A direct `from_slice::<TransformRequest>`
on one lane derives the lane from `kind`, treats a `null` page field as
absent, returns `duplicate field` where the `Value` round trip keeps the last
value, or changes malformed-JSON reporting, so the two lanes diverge.
Required faults and enabling state: Bodies such as
`{"method":0,"kind":"transform"}`, `{"method":"transform","kind":"x"}`,
`{"kind":"transform","transform_page_id":null}`, and
`{"name":"x","arguments":{}}`; bodies over 1 MiB with string, numeric, array,
object, and 2 MiB `method`; duplicate `method` keys; exactly 32 MiB and
32 MiB plus one; each fed through [`dispatch_value_for_test`][testentry] and
through [assembled pages][pageapply].
Confidence: high - [Evidence](evidence/route-and-typed-decode-are-independent-of-entry-path.md).
The dispatch read, the page-field key test, the probe class set,
[`TransformRequestWire`][wirestruct] (no `deny_unknown_fields`, no `method`
field, no `kind` validation), and the `Value`-retaining
[`WireMessage`][wiremsg] and [`WireBlock`][wireblock] decodes are
source-verified; serde duplicate-key behavior is cited from upstream source,
not run.
Existing check: [Ingress checks](existing-checks.md#ingress-admission-and-decode)
cover `kind` routing, the unrecognized shape, retired aliases, the probe
class set, and one full envelope decode; none found for a `null` page field,
a non-string `method` at dispatch, duplicate keys, `null` handling, malformed
JSON on either lane, or exactly 32 MiB; all unaudited.
Impact: A body can be misrouted, admitted under the wrong cap, or decode
differently depending on which lane carried it.
Open questions:
- Is last-wins on duplicate top-level keys a contract or an accident of the
  `Value` round trip? Synapse rejects duplicates ([§7.5.1][wire751]); the
  context module has no written rule. (needs human input)
- Must malformed JSON keep reporting `unrecognized_request_shape`, or may it
  become `bad_request`? (needs human input)
- Integers above `u64::MAX`, exponents, and `-0` pass through `Value`
  normalization today; equivalence with a direct typed decode for the `u64`,
  `usize`, and `f64` fields is unresolved, needs a differential run.

### scratch-pool-shortfall-reaches-the-parse-reservation

Type: reachability
Reachability: test-only
Status: active
Exercised: not yet - No pool-pressure witness is recorded.
Guarantee: An ingress-preservation campaign constructs the transient
shortfall that separates `queue_full` from `invalid_params` at least once.
Check: `sometimes` - At entry to [`try_reserve_resident`][reserve] for some
request, `footprint <= resident_capacity()` and the scratch pool's available
bytes are below `footprint` because concurrent admitted parses hold them;
the marker records both preconditions and the concurrent holders, not the
handler's outcome.
Fault/timing angle: With one request at a time the pool is never short, so
the `queue_full` arm and its release path are never evaluated.
Required faults and enabling state: Two or more concurrent bodies whose
footprints sum above [`SCRATCH_RESERVED_BYTES`][scratchconst] while each fits
alone; a barrier that holds the first charge until the second request reaches
the reservation.
Confidence: high - [Evidence](evidence/scratch-pool-shortfall-reaches-the-parse-reservation.md).
The single host-wide scratch [`ByteBudget`][pools] shared by three components
and the two refusal helpers are source-verified; production occurrence of the
shortfall is plausible but not verified.
Existing check: [`ByteBudget` tests](existing-checks.md#ingress-admission-and-decode)
cover transient versus permanent `None` on the budget alone; no handler-level
shortfall witness is found; unaudited.
Impact: A green admission suite never evaluates the transient refusal arm.
Open questions: None.

## Shared-input equivalence

### derived-artifacts-are-ownership-independent

Type: safety
Reachability: default-production
Status: active
Exercised: partial - The selection differential passes unchanged against its
frozen reference. The [sharing check][selection-sharing] compares the selected
inputs from all 48 frozen-corpus tool calls with the projected wire value and
proves pointer identity through clones
and historian input construction. The [sidecar check][sidecar-order-check]
compares full and incremental order, metadata, and pins across three
generations, including repeated IDs and sparse cached metadata. The
[complex native replay][native-sharing] compares fresh, reattached, and shared
inputs with the native differential enabled by the compiled test setting.
Negative controls verify that the differential detects drift. The replay
test proves pointer sharing for
reattached values, sidecar envelopes, metadata, and encoded prefix chunks.
The [ingress-core check][native-ingress-sharing] covers snapshot fallback,
value-based output reuse, and cold request allocation accounting. The complex
replay also checks the warm request charge against its own allocation sizes.
The [cache-charge check][native-charge-floor] preserves an allocation-based
charge alongside the sidecar's smaller serialized-size estimate. Broader
projection and served-segment comparisons remain in the shared-input suites.
The [canonical-shell check][shell-sharing] proves repeated reattachment and
incremental projection retain shell pointers without mutating raw ingress.
The [decode check][shell-decode] asserts `Send + 'static` and unchanged malformed
input errors. The complex replay also compares fresh, reattached, and shared
projections, served bytes, shell pointers, and cold and warm shell charges.
The [shell metadata check][shell-metadata] reparses nonempty origin and provider
extras with non-default, non-synthetic harness metadata. It checks that replay
drops only the unknown message field while retaining every block and known
shell field. The exact allocation oracle includes nonzero metadata heap terms.
The [served-byte witnesses][served-byte-witnesses] compare literal canonical
bytes and measured `Served` segment writes for original, latent-edited, typed,
and block-edited shells. The frozen wire corpus also runs with fully typed
blocks against the value-round-trip reference. Fallback witnesses distinguish
latent fields, unknown originals, duplicate candidates, positional precedence,
integer zero, and signed floating zero.
Guarantee: The projection, the native attachment, and the served message
bytes depend only on message values, never on which allocation holds them or
which lane assembled them.
Check: `always` - For every pass, three artifact families agree with their
value-only construction. Projection: `project_messages(&msgs)` is equal under
[`FlatProjection`][flatproj]'s derived `PartialEq` whether the slice is the
fresh request, the normalized clone, a reattached prefix plus suffix, or a
shared view. The projection owns canonical replay shells: unknown top-level
message fields are discarded once, original block JSON survives, and effective
synthetic metadata is retained. Flat blocks hold an immutable shell and block
index, not an independent copy of the wire block. Reattached requests share
those shells rather than raw ingress shells; per block `content_hash ==
sha256(bytes)` and `bytes == to_string(wire)`, and `FlatBlock` holds no tool
input outside `wire` (the shared wire block itself may keep the input in both
`kind()` and its retained `original`, and the shell charge counts both);
and `project_messages_incremental(msgs, cached, k) == project_messages(msgs)`
with equal [`differential_bytes`][diff-bytes]. Native attachment: under
`serve_native`, `to_vec(incremental native_messages) ==
to_vec(encode_full_native_messages(..))` as the [differential][native-diff]
already compares; with complete prefix metadata, the incremental sidecar has
the same `order`, `messages`, and pins as a full decode with the same inherited
pins. With discarded prefix metadata, order still matches the full decode,
while metadata remains sparse unless replaced by the suffix. Order keeps
first-seen positions ([`remember_message`][remember]); and
[`native_ingress_chunks`][ingress-chunks] shares a chunk for index `i`
exactly when `chunk.value == native_messages[i]` by value. Served bytes: for
every `ServedMessage`, `canonical_bytes ==
serde_json::to_vec(&serde_json::to_value(&message))`, which sorts object keys
because the workspace enables only [`raw_value`][serde-features];
`canonical_hash == sha256(canonical_bytes)`; the prepared output writes
exactly `canonical_bytes` per [`Served` segment][segment-served]
([segment construction][segments]). Fingerprints retain the positional-first
rule: the positional helper uses exact `WireBlock` equality without hashing
either identity. An unequal positional candidate forces a fresh wire hash;
only an absent position searches the first identity-equal candidate. The fallback
index uses a [request-local SHA-256 identity][block-identity] over typed kind,
provider extras, and retained original, normalizing floating signed zero but
not integer zero. Its key is distinct from the projected serialized-byte hash.
The selected candidate then passes through the same [equality helper][fp-reuse]
as a positional candidate, which supplies its fingerprint and byte length or
rejects it; otherwise the result is
`(fingerprint(to_string(block)), to_string(block).len())`.
Equal floating zeros can therefore reuse a candidate whose byte spelling
differs, as in the structural-equality baseline. `always` because
every pass projects and serves, every plugin turn attaches, and every
downstream digest keys on these fields.
Fault/timing angle: None in time. A projector that shares block backing but
computes `bytes` from another serialization; a consumer
reading a projected input copy that is not the `wire.kind()` value
([`sel_item_from_flat`][sel-item] and
[`sel_kind_for_flat`][sel-kind] both borrow the typed wire input, and
`FlatBlock` holds no copy outside `wire`); chunk reuse decided by
pointer identity; an incremental sidecar merge that changes first-seen order
on a repeated mid ([sidecar merge][sidecar-merge]); a direct `to_vec(&message)`
on a typed shell (rebuilt prefix, reduced, overlaid, or synthetic) that emits
struct field order instead of sorted keys, which
[`Serialize for ServedMessage`][ser-served] already does and the handler
avoids only by taking `messages` out before `to_value(response)`
([response encoding][segments-take]).
Required faults and enabling state: A second-pass projection cache hit; a
delta body so the prefix is [reattached][reattach] and the native prefix
comes from the attachment cache; tool calls, tool results in a user message,
repeated call ids, and a suffix that repeats a prefix mid; a message equal by
value but not by pointer to a cached chunk; a response holding a harness
message with `original`, a rebuilt prefix message, a reduced message, a
tag-overlaid message, and the synthetic m0 and m1.
Confidence: high - [Evidence](evidence/derived-artifacts-are-ownership-independent.md).
Both differential gates ([prefix][gate-prefix], [native][gate-native]), the
[flatten][flatten] fields, [`from_message_reusing`][served-reusing], the
sorted-key cause, and the segment writer are source-verified.
Existing check: [Shared-input checks](existing-checks.md#shared-input-equivalence)
include both differentials, fingerprint reuse, pinned fingerprint IDs, the
selection-sharing check, sidecar order/pin equality, native prefix sharing,
and fresh/full native byte equality. Projection and request snapshot caches
charge shell backing, content capacity, retained block JSON, and Arc counters
using the existing conservative full-charge-per-holder rule. Cached prefix
charges can exceed the canonical shell's smaller footprint; only suffix sizes
are recomputed. Cache budgets are unchanged. Canonical serialization uses
serde formatter spans rather than a materialized `Value` round trip; keys
without escapes order by their raw bytes, and an object already in order is
not sorted, so a retained-original shell whose keys carry no escapes decodes
no keys. An object with an escaped key decodes every key before ordering,
even when the object is already in order. The
positional map, lazy digest index, and serialization spans are per-message
scratch, not retained cache entries. All checks remain unaudited for adequacy.
Impact: Output identity, served fingerprints, token caches, tag mint, and the
plugin's replay source can drift from the message values.
Open questions:
- Is the release-build `assert_eq!` panic under
  `EIDNARA_PREFIX_PROJECTION_DIFFERENTIAL` and
  `EIDNARA_NATIVE_ATTACHMENT_DIFFERENTIAL` the intended production contract
  or a developer switch? The transform catalog's
  [portfolio evaluation][tc-g2] queued this as gap G2 and it is still open.
  (needs human input)

### synthetic-normalization-is-scoped-to-the-pass

Type: safety
Reachability: default-production
Status: active
Exercised: yes - The [typed-flag reference comparison][synthetic-reference]
checks served bytes, projection state and digests, native bytes, tag rows,
historian boundary messages and chunk input ordinals. The
[handler delta comparison][synthetic-delta-parity] checks full versus delta
projection and native bytes; the [delta witness][synthetic-delta-witness]
captures production historian prompts and native output on the second and
third turns, including replay carried in the third turn's cached prefix.
The [lineage rebase comparison][synthetic-lineage-rebase] covers a normalized
synthetic head on a non-subagent descent replay that rebases ordinals.
Guarantee: Replacing the normalization clone with a shared view changes no
observer's synthetic set and no served byte.
Check: `always` - For every pass, observers see the same synthetic
sets as at the discovery baseline: inside `apply_once` a message is synthetic iff
`meta.synthetic || any block id has the synthetic_todo_ prefix`
([`normalize_synthetic_todo_ingress`][normalize] records borrowed message IDs
in the pass-local projection view, and request-dependent helpers consume
that view); in
[`cached_boundary_messages`][cached-boundary],
[`assemble_historian_firing`][assemble],
[`store_projection_cache`][store-pc], and
[`attach_native_messages_incremental`][native-attach] a message is synthetic
iff `parsed.messages[i].ck.meta.synthetic` in that pass's ingress, including
flags restored by prefix reattachment. The override set is not retained;
derived projection metadata is retained with normalized flags, as in the
clone-based baseline. The served wire
bytes of a message whose flag was set by normalization equal the bytes of
the same decoded message without the flag, because
[`Serialize for WireMessage`][ser-msg] replays `original` and a `meta` edit
does not clear it ([`:210-216`][meta-doc]). `always` because the sets decide
which blocks count for coverage, historian ordinals, native reasoning
clears, and the output; the check is on each observer, not on a defect.
Fault/timing angle: A shared-reference design that marks `parsed` in place
before `apply` widens the normalized view to the historian and native
attach; a design that marks through `mark_modified` or rebuilds the message
drops `original`, so `"synthetic":true` appears on the wire for the first
time. Both are behavior changes relative to HEAD, not preservation.
Required faults and enabling state: The B5 situation: a prior bust pass froze
a todo pair ([`tail_reclaim`][tail-reclaim] is true for every shipping
profile), then an array in which the harness replays the pair as ordinary
messages without the `synthetic` marker, a historian firing on that pass,
and `serve_native` on.
Confidence: high - [Evidence](evidence/synthetic-normalization-is-scoped-to-the-pass.md).
The pass-local view, the handler consumers of the original request, and
retained-JSON replay are source-verified. Focused comparisons pass; this is
not performance evidence or a whole-workspace gate.
Existing check: [Shared-input checks](existing-checks.md#shared-input-equivalence)
include the typed-flag reference, the delta witness and the replayed pair's
cache reuse. The transform catalog's [synthetic-strip record][tc-synthetic]
states the inside invariant. Test adequacy remains unaudited.
Impact: Historian ordinals, native reasoning clears, and wire bytes change
when the clone is shared or the flag is written through.
Open questions: None. The preservation contract retains both observer
semantics: an unflagged suffix remains a zero-block historian message, while
a reattached normalized prefix is filtered out. Passthrough fingerprints keep
`eidnara_todo:` identities while retained ingress JSON stays unflagged. The
third-turn native and prompt reference uses the same reattached observer
input, not a full raw array whose historian ordinals differ. The discovery
questions and their evidence remain in the evidence file.

### tag-baseline-cache-entry-is-never-mutated-by-a-pass

Type: safety
Reachability: default-production
Status: active
Exercised: partial - Cold, drop, reset, remint, poisoned-refill, and interleaved
session tests pass. A failed second mint insert leaves the baseline pointer,
contents, and durable state unchanged. Row-sharing checks fail on the
deep-copy predecessor. Source equality is exercised on clean text; the
prepared-field policy for detected secrets remains an open question.
Guarantee: Pass-local mint rows never become visible through the tag
baseline cache, and every visible row's `source_bytes` is byte-equal to the
projected text it tags.
Check: `always` - After every pass, the [`TagBaselineCacheEntry.tags`][tag-entry]
for the session equals `store.load_tags_for_session(session)` for the
`(store_namespace, generation, count, max_tag_number)` the entry records, in
[`ORDER BY tag_number ASC`][load-order]. [`tag_mint_rows`][append-mint]
creates a pass-owned tail with `tag_number = max + offset + 1` in projection
block order. The [combined slice][combined-tags] contains baseline row handles
followed by mint row handles without copying their source allocations; and
every committed `TagRow.source_bytes` equals the block's
[`taggable_source`][taggable] text bytes exactly ([mint capture][mint-input]).
`always` because the entry is read on the next pass of the same session and
a stale or speculative row changes the [active-tag match][active-match].
Fault/timing angle: The cache owns `Arc<[Arc<TagRow>]>`; the pass never
mutates its entry. The pass retains an [immutable named baseline][tag-baseline]
for [bootstrap protection][tag-protection], separate from the combined view.
[Overlay computation][mint-tail] reads the baseline and
mint tail together before the caller combines their handles. The commit
reads only the [pass-owned mint tail][commit-mints]. A successful commit
does not publish those speculative rows to the cache. Only
[`load_cached_tags`][load-tags] publishes store-read rows, validating the
summary across an append refill. Store numbering remains independent, as
the [historical two-authorities record][tc-tagnum] describes.
Required faults and enabling state: Tagging active (a profile with
`tool_present`), a warm baseline entry, a pass that mints, then a second pass
on the same session; for the rollback arm, the existing attempt hook installs
a temporary SQLite trigger that aborts the second mint insert.
Confidence: high - [Evidence](evidence/tag-baseline-cache-entry-is-never-mutated-by-a-pass.md).
The shared baseline, mint tail, combined view, hygiene consumers, commit
inputs, and store-read-only refill are source-verified. Cache accounting
charges row capacities, row pointers, and row/slice Arc headers in full,
including shared rows, under the unchanged 64 MiB baseline budget. The
64-byte allocator allowance remains separate from the Arc counters.
Existing check: [Shared-input checks](existing-checks.md#shared-input-equivalence)
cover cold-versus-cached parity, direct-SQL refill, session isolation,
failed-commit rollback, source bytes, row sharing, capacity-driven admission
refusal, protected-orphan iterator parity, and bootstrap protection with mints;
all unaudited.
Impact: A speculative or stale row changes which tags are treated as active
on the next pass.
Open questions:
- The transform commit uses [`write.bytes("tag_source_bytes", ..)`][mint-prepared],
  whose [content policy][tag-content-policy] substitutes detected secrets.
  The older evidence cites the separate tag-mint API. How should unconditional
  source equality apply to redacted inputs? [R1][r1] owns the unchanged
  prepared-field policy; the clean-text corpus does not settle this conflict.
  (needs human input)

### hygiene-digest-is-kind-prefixed-part-content

Type: safety
Reachability: default-production
Status: active
Exercised: yes - Cold and warm memo walks match the frozen full-result digest.
Independent kind-prefixed digest assertions, a poisoned projection token key,
and content, caveman, context, namespace, reset, and eviction cases pass.
Least-recently-used eviction and poison recovery preserve cold-reference
results. Sixteen distinct sessions hold their memos concurrently, over-budget
walks keep a warm prefix, and production transforms reuse unchanged
measurements while recounting only an edited block.
Guarantee: The hygiene digest is a function of the part kind and derived
content and is never the projection `content_hash`.
Check: `always` - For every hygiene part, `content_hash ==
hex(sha256(kind_name ++ "\0" ++ content))` where `content` is the derived
part string ([caveman-substituted and reminder-stripped text][hyg-text],
[`to_string(input)`][hyg-input], or [`tool_output_content`][hyg-output]);
excluded parts hash `"excluded\0" ++ block.bytes` on the pre-match branch
(`tail_hygiene.rs:861-885`) and the kind-level branch (`:931-934`), and
`"excluded\0" ++ content` with the derived empty or drop-sentinel content on
the text, tool-result, and media branches (`:892-893`, `:905-906`,
`:917-918`, through `excluded_part` at `:624-626`); the token cache is keyed
by that digest ([`count_with_digest`][count-digest] at
[`tail_hygiene.rs:614`][th-cwd]); and the measurement is identical with a
cold and a warm memo. `always` because the digest is both the reported
hash and the cache key on every measured pass.
Fault/timing angle: Each session's own lock serializes that session's
measurements. The table lock covers only lookup, insertion,
least-recently-used eviction, and removal, so distinct sessions run
independently and removal never waits on a walk. Poison recovery replaces the
memo and its interrupted accounting before clearing poison. A shortcut
substitutes
`FlatBlock.content_hash` for the hygiene digest; the two hash different
inputs (full serialized `WireBlock` versus kind-prefixed derived content), so
every reported `content_hash` changes and the cache key mixes counts of the
serialized block with counts of its text. The projection digest already keys
a different cache, the boundary [`token_count`][token-count], which counts
`block.bytes`.
Required faults and enabling state: A tail with text, tool call, tool result
in text and content variants, media, an excluded reduced block, and a
caveman-substituted text block; the same input measured twice, then edited
under the same block ID. Change caveman identity and payload, protection,
coverage, reduction, role, and synthetic status independently. Interleave
sessions, exceed the session limit, change the store namespace A/B/A, panic
during a fill, reset, evict, and exceed byte limits with multiple blocks. Fill
all sixteen sessions past budget and recompute their retained-byte counters
using the same capacity-to-bucket model as production. This checks
accounting, not actual allocator RSS or hashbrown internals. The 16 MiB plus
fixed-container bound is post-operation retained storage, not peak
allocation.
Confidence: high - [Evidence](evidence/hygiene-digest-is-kind-prefixed-part-content.md).
[`part_measurement`][part-measure] and [`measure_tail_hygiene`][hygiene] are
source-verified; W3 owns the key-domain non-aliasing clause.
Existing check: [Shared-input checks](existing-checks.md#shared-input-equivalence)
include cold/warm identity, the frozen full-result digest, explicit digest and
token-key separation, invalidation, and bounded retention; all unaudited.
Impact: Reported hygiene hashes and cached token counts silently change
meaning.
Open questions: None for the ticket-local payoff decision. The
[integrated three-pair A/A and five-pair A/B run](evidence/tail-hygiene-integrated-payoff.md)
meets both predeclared retention conditions and reports a 72.5513% reduction
in warm-call time. This does not establish production, concurrent-session,
cold-call, or total session latency. The frozen characterization has
agent-witnessed, transcript-only pre-memo provenance, not an independently
reexecuted or artifact-hash-verified characterization run.

The historical [73.1659% measurement](evidence/tail-hygiene-payoff.md) applies
to candidate `e1a0d06a` before integration with
`16542f5e`. The merged [hygiene input setup][hyg-bench-input] decodes the
corpus through JSON and retains original message JSON; the measured candidate
constructed typed ingress directly. The [memo setup and timed loop][hyg-bench-loop]
still construct and prime the actual slot pool outside the callback. The old
result does not establish the same gain for this merged input representation.
The recorded experiment artifacts, paths, and hashes remain historical evidence.
The new measurement resolves that gap by comparing archived `16542f5e` plus
only the required release-accessor repair with `05c33bf0`, using decoded
ingress on both sides and a newly measured A/A guard. Its conditional paired
interval does not establish host/build population coverage or allocator RSS.

### replayed-synthetic-pair-arrives-unflagged-on-a-delta-turn

Type: reachability
Reachability: default-production
Status: active
Exercised: yes - [The delta witness][synthetic-delta-witness] freezes a pair
on a HARD pass, reattaches two prefix messages, sends the pair unflagged in
the protected suffix, and observes a prepared firing with native output.
A third delta reuses all 84 prefix messages, including the normalized pair;
its captured production prompt and native bytes match a typed-flag baseline
prefix reconstruction.
Guarantee: A shared-input campaign reaches the situation in which the
normalized and un-normalized views of one request differ for a downstream
observer.
Check: `sometimes` - For some pass with `compaction_enabled`, the request is
a `tail_delta` body whose prefix [`expand_transform_tail_delta`][expand]
reattaches, at least one non-synthetic message carries a
[`synthetic_todo_`][todo-prefix] call or result id (the input condition under
which [`normalize_synthetic_todo_ingress`][normalize] adds an override; the
marker asserts the input, not the implementation's allocation), and
[`prepare_historian_fire`][historian-fire] runs on that pass with
`serve_native` on. The marker asserts these preconditions, not observer
agreement.
Fault/timing angle: A campaign that never combines prefix reattachment,
unflagged replay and a firing cannot witness the delta-specific observer
split.
Required faults and enabling state: A prior bust pass that froze a todo pair;
a harness replay of the pair without the `synthetic` marker, as
[`warm_cache_selection_bust...`][t-collapsed] constructs; a delta turn; a
configured `model_chain` so the firing is prepared; `serve_native` on. The
witness puts the pair in the protected tail. A pair inside the selected chunk
can retain the baseline `MissingBlockIdentity` refusal.
Confidence: medium - [Evidence](evidence/replayed-synthetic-pair-arrives-unflagged-on-a-delta-turn.md).
The override trigger and the delta expansion are source-verified; whether the
harness replays pairs on delta turns in production is inferred from the
plugin's delta protocol, not observed.
Existing check: [The delta witness][synthetic-delta-witness] asserts the input
flags and frozen call ID, configured compaction and model chain, positive
prefix reuse, and `historian.fired` before emitting the constant marker
`replayed-synthetic-pair-arrives-unflagged-on-a-delta-turn`. It compares captured
producer prompts, third-turn boundary and chunk inputs, and native bytes;
unaudited.
Impact: B2 can pass while the divergent observer is never reached.
Open questions: None.

## Cache-state load, pass trace, side channel, and meta preparation

### consolidated-cache-state-reads-match-per-consumer-loads

Type: safety
Reachability: default-production
Status: active
Exercised: partial - The no-fire CAS test and the emergency interleave test
exercise the post-commit load. The
[memory-store differential test](evidence/consolidated-cache-state-reads-match-per-consumer-loads.md#single-load-evidence)
compares the `revert_epoch`, `historian.state`, and
`publication_floor_ordinal` scalar reads and the `meta`-only load with
`MemoryStore::load` over absent keys, JSON `null`, booleans, unknown variants,
negative and textual epochs, a non-object `historian`, JSON5, malformed `meta`,
and malformed `core_state`; the daemon load-count test shows one pre-transform
`meta` load, no pre-transform full load, and one post-commit full load per
steady pass, split by the interleave hook, on handles the probe shows were
never re-created; the durable-phase test exercises `historian_active` on every
`PassState`.
Guarantee: Consolidating or narrowing `cache_state` loads never changes what
any consumer observes: a post-commit consumer sees its own commit, and a
scalar or `meta`-only projection decodes its field as the full load does and
fails where the full load fails on that field; the per-column and per-field
divergences (a corrupt `core_state`, a corrupt sibling field, a non-object
ancestor, an integer above `i64::MAX`) are recorded in the evidence, and a
pre-transform consumer that proceeds on a row whose `core_state` the full load
refuses does so only in a pass the transform's own snapshot then rejects before
any commit.
Check: `always` - Freshness: within one pass, every `cache_state` read that
executes after `commit_transform`, [`descend_lineage`][descend],
[`truncate_compartments_for_revert`][truncate], or an awaited historian
firing observes a `row_version` at least as new as the one that work
returned, any CAS write derived from that read ([`record_no_fire`][no-fire]
under `loaded.row_version`) uses that value, and the two Emergency95
`publication_floor_ordinal` reads ([`:8303-8308`][floor-a],
[`:8425-8444`][floor-b]) stay distinct because their comparison is the rerun
trigger. Decode: for every stored `meta` text, a scalar projection of
`revert_epoch` and `historian.state` returns the same value as
`serde_json::from_str::<ModuleMeta>(meta)` when that succeeds, and when that
field fails to deserialize the consumer takes the branch it took on a failed
full load (`None` for the projection cache at
[`lookup_full_projection_cache`][epoch-read] and
[`expand_transform_tail_delta`][epoch-read-delta], `false` for
[`historian_active`][active]), except for the recorded divergences: a
corrupt `core_state` or a corrupt sibling field no longer takes that branch
(the pass proceeds and the transform's own snapshot load refuses the row
before any commit), a non-object `historian` reads as `Idle`, and an epoch
above `i64::MAX` fails the scalar read alone. `always` because the daemon
consumes these reads on every pass, not only under a fault.
Fault/timing angle: A consolidation reuses a snapshot taken before
`commit_transform` for a consumer placed after it, or reuses the first
`run_transform` snapshot for an Emergency95 rerun after an inline firing; a
narrow read returns NULL where [`revert_epoch`][meta-epoch] and
[`historian`][meta-historian] carry `#[serde(default)]` (`0`, `Idle`), or
succeeds on an unknown [`HistorianPhase`][phase] string or a `null` epoch
that serde rejects.
Required faults and enabling state: A pass that commits, then reaches
[`prepare_historian_fire`][prepare] with a new no-fire reason; an Emergency95
pass with a publication landing between the transform and the floor check
(C5); a CAS conflict injected between snapshot and commit so the
[retry loop][cas-retry] reloads; rows whose `meta` lacks `revert_epoch` or
`historian`, carries an unknown `historian.state`, or holds `null` under
`revert_epoch`; rows whose `core_state` is not valid JSON.
Confidence: high - [Evidence](evidence/consolidated-cache-state-reads-match-per-consumer-loads.md).
[`MemoryStore::load`][load] (one deferred read of
[`CACHE_STATE_FULL_SELECT`][full-select] decoding both columns), the three
pre-commit loads, the post-commit load, the floor reads, and the serde
defaults are source-verified.
Existing check: [State checks](existing-checks.md#cache-state-load-pass-trace-side-channel-and-meta-preparation)
cover the no-fire CAS, the emergency rerun, the CAS retry, and snapshot
pinning; none found for narrow-read equivalence; all unaudited.
Impact: A no-fire reason is never persisted, a rerun trigger disappears, a
stale projection-cache entry is selected, or a historian veto is cleared.
Open questions:
- Which of the three pre-commit loads (epoch, `historian_active`, snapshot)
  may share one snapshot? Merging them removes a window in which the epoch is
  read before a concurrent recut; the specification should state whether
  closing that window is intended. (needs human input)
- Must a narrow read fail the same way on a corrupt `core_state`, or may a
  meta-only read proceed? (needs human input)
- Does SQLite JSON path extraction return the first or the last duplicate
  key? Relevant only if a writer bypasses
  [`parse_json_with_unique_names`][unique]; none found for caller-influenced
  `meta`, and [`reset_session_for_recomp`][recomp] writes a self-generated
  `ModuleMeta`. Unresolved.

### pass-trace-writes-count-every-pass-outside-the-cache-cas

Type: safety
Reachability: default-production
Status: active
Exercised: partial - Reject, success, repeated-reject, frozen-state, status,
scheduler, and secret-session tests exist; none covers `first_divergence`
after a rejected pass, `receive_count` after an Emergency95 rerun, or a
`pass_trace` failure beside a successful cache commit.
Guarantee: The receive breadcrumb is independent of the pass outcome and of
the cache-state CAS, and diagnostics can neither veto nor enlarge the state
commit.
Check: `always` - For each transform request that reaches the handler after
admission, `pass_trace.receive_count` increases by exactly one and
`last_received_at_ms` is set before the outcome is known, whether the
transform later commits, stays stable, or is rejected; `reject_count`
increases by exactly one on a rejection while `cache_state.row_version` is
unchanged; `first_divergence` is NULL after a rejected pass;
`scheduler_history` gains exactly one observation per accepted pass, from
either [`trace_pass_stable`][stable] or [`commit_transform`][commit-trace],
never both; a `pass_trace` write never changes `cache_state`; and a
`pass_trace` failure never aborts an otherwise valid cache commit unless the
specification states the new coupling. `always` because status, health, and
the plugin display read these counters on every request, and every call site
discards the trace result with `let _ =` ([`:8129`][received-call],
[`:8192-8199`][rejected-call], [`:8434`][completed-call],
[`record_stable_pass_trace`][stable-call]).
Fault/timing angle: A fold moves the bump after `run_transform`, so a rejected
or stable pass under-counts, or attaches it to every commit so a rerun
Emergency95 pass double-counts; inside a fused transaction a `pass_trace`
constraint failure or a `session_id` identity refusal rolls back the
cache-state row. The identity refusal is already reachable: a secret-bearing
`session_id` on a known session is [tolerated][flagged] only because the row
exists.
Required faults and enabling state: A transform that rejects (ordinal
violation); a stable pass; an Emergency95 pass that reruns and commits twice;
a CAS conflict on the first commit attempt so the [retry loop][cas-retry]
reruns `apply_once` and commits once (one breadcrumb, not zero or two); a
fresh session whose first pass commits; an injected failure in the
`pass_trace` upsert during a committing pass (no store seam exists at HEAD;
the four `fail_next_*_for_test` seams at `memory-store/src/lib.rs:5900-5928`
cover the side channel, the authority route read, and dreamer tasks only).
Confidence: high - [Evidence](evidence/pass-trace-writes-count-every-pass-outside-the-cache-cas.md).
[`trace_pass_received`][received] (its [doc][received-doc] says the write
never contends with or extends the pass commit),
[`trace_pass_completed`][completed] (its [doc][completed-doc] says it cannot
alter CAS semantics), [`trace_pass_rejected`][rejected], the
[`PassTrace` doc][passtrace-doc], and the in-commit upsert that initializes
`receive_count` to `0` and leaves it alone on conflict are source-verified.
[`load_pass_scheduler_history`][sched-history] has one non-store caller and
it is a [test][sched-test].
Existing check: [State checks](existing-checks.md#cache-state-load-pass-trace-side-channel-and-meta-preparation)
list seven pass-trace tests and the identity gate; all unaudited.
Impact: Rejected and stable passes disappear from status and health, or a
diagnostic write starts rolling back state commits.
Open questions:
- Is under-counting rejected passes an acceptable semantic change, or must
  the receive breadcrumb keep a home on the reject and stable paths? Existing
  tests encode `receive_count == reject_count` after rejects. (needs human
  input)
- If a fold is accepted, which of the two doc comments is rewritten, and what
  replaces the "never contends with the pass commit" promise? (needs human
  input)
- The `session_id` scan for `pass_trace` is owned by the `pass_trace` owner;
  a fold moves it under `cache_state`. [R3][r3]'s owner-relationship
  normalization decides whether that is a change.

### side-channel-drain-delivers-each-row-once-and-keeps-its-schedule

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: partial - Restart redelivery and per-kind fault isolation exist;
none covers a crash between the mark commit and the delete commit, two
overlapping drainers, ordering across firings, the per-kind limit, or the
backoff values.
Guarantee: The outbox row is the only duplicate guard for events and user
observations, and the drain's scheduling shape is unchanged by any
transaction restructuring.
Check: `always` - For every `historian_side_channel_outbox` row, the target
table receives exactly one row across all drains, restarts, and overlapping
drainers; the outbox state change that retires the row commits in the same
transaction as the target insert; and a delivery whose outbox row is no
longer pending when its transaction runs rolls back its target insert. A
drain visits kinds in the order of [`HISTORIAN_SIDE_CHANNEL_KINDS`][kinds]
(`event`, `primer`, `user_observation`); within a kind it delivers due rows
ordered by `firing_seq, source_start, source_end, item_index` up to
`min(per_kind_limit, 32)`; a row is due only when
`next_attempt_at_ms <= now_ms`; a failed delivery increments
`attempt_count`, sets `next_attempt_at_ms = now + 1000 * 2^min(attempt, 6)`
capped at 60000, and stores the error capped at 2000 characters; a failure in
one kind does not stop other kinds; and the first bookkeeping error is
returned after the loop completes. `always` because
[`compartment_events`][events-insert] and
[`user_memory_candidates`][obs-insert] are plain inserts with no dedupe
(only [`primer_candidates`][primer-insert] upserts), and these rules define
which rows a pass touches.
Fault/timing angle: Process crash between the mark commit and the
[per-row delete][delete-one]; the publish task's own drain
([`:11074-11083`][publish-drain]) overlapping the pass drain on one session;
a target insert failing after the outbox state change in a reordered
transaction; an empty-drain shortcut that skips the [leftover delete][delete-all];
a delete-in-place that changes which rows [count as pending][status-sc].
Required faults and enabling state: A published firing with events, primers,
and user observations; a crash or abort injected between the two fenced
transactions; a second drainer started between load and deliver; multiple
rows per kind across two firings; an injected failure on one kind; `now_ms`
before and after the computed `next_attempt_at_ms`.
Confidence: high - [Evidence](evidence/side-channel-drain-delivers-each-row-once-and-keeps-its-schedule.md).
[`drain_historian_side_channels`][drain] and its [doc][drain-doc],
[`load_due_historian_side_channels`][load-due] with the
[order index][idx-order], [`deliver_historian_side_channel`][deliver],
[`mark_historian_side_channel_delivered_tx`][mark] (requires `changed == 1`
under `delivered_at_ms IS NULL`), and
[`record_historian_side_channel_failure`][failure] are source-verified.
Existing check: [State checks](existing-checks.md#cache-state-load-pass-trace-side-channel-and-meta-preparation)
cover restart redelivery, per-kind isolation, CAS-loser enqueue, and revert
deletion; all unaudited.
Impact: A compartment event or user observation is delivered twice or never,
or a pass drains rows it did not drain before.
Open questions: None.

### meta-json-preparation-scans-every-persisted-byte

Type: safety
Reachability: default-production
Status: active
Exercised: yes - Duplicate names, key-directed substitution, container
refusal, integrity refusal, and the cache-state policy test exist; the
[single-pass tests](evidence/meta-json-preparation-scans-every-persisted-byte.md#single-pass-evidence)
add byte identity of a clean stored `meta` against
`serde_json::to_string(meta)`, a secret in a `block_identity_by_mid` value
substituted and recorded, and a secret in a `block_identity_by_mid` key
refused.
Guarantee: No byte reaches the `meta` column that the scanner did not walk,
and the audit receipt matches the bytes stored.
Check: `always` - For the `meta` text handed to [`json_content`][json-content]
at [`commit_transform`][commit-meta], the stored bytes are either
byte-identical to `serde_json::to_string(meta)` when no substitution occurred
(the [unchanged-input branch][clean-branch]) or the serialization of the
redacted tree; a duplicate object name at any depth refuses the write; every
object key is bound-checked and scanned, in the [object arm][walk-keys] of
the walk for containers it descends and by [`validate_json_keys`][keys] for a
subtree it judges whole; a detected value under an identity or integrity key
refuses; a protected key with a container value refuses; a protected scalar
substitutes and records a detection ([`record_observed_scan`][record-scan]);
and the recorded scan for field `meta` carries the same detections as the
walk observed. `always` because every committing pass runs this path.
Fault/timing angle: A single-pass redaction streams input and returns the
original bytes for an unchanged prefix while a later duplicate name shadows
an earlier value, which is the bypass the [comment][unique-doc] on
[`parse_json_with_unique_names`][unique] states; or it substitutes without
recording the detection.
Required faults and enabling state: `meta` text with duplicate names at top
level and nested; a secret in a `BTreeMap` key such as
`block_identity_by_mid`; a secret under an integrity-named field such as
`tail_hygiene_baseline.content_signature`; a protected key holding an
object; a clean `meta` compared byte-for-byte with the stored column.
Confidence: high - [Evidence](evidence/meta-json-preparation-scans-every-persisted-byte.md).
The [policy][policy], [`prepare_json_content_collecting`][prepare-collecting],
[`prepare_json_content_single_pass`][single-pass],
[`prepare_value`][prepare-value], and the clean branch are source-verified;
every reader deserializes and the transform compares values
([`next_meta != loaded.meta`][value-compare]), so no reader depends on byte
form.
Existing check: [State checks](existing-checks.md#cache-state-load-pass-trace-side-channel-and-meta-preparation)
list eight preparation tests; the canonical policy records are
[preserved-identity-name-does-not-exempt-its-value][ms-preserved] and
[refused-durable-write-leaves-no-row-and-no-receipt][ms-refused]; all
unaudited.
Impact: A secret persists in `meta` under a shadowed duplicate name, or the
audit receipt and stored bytes disagree.
Open questions:
- Must a redacted `meta` keep today's alphabetical key order, or is any
  deserializable form acceptable? No reader depends on order. (needs human
  input)

### foreign-write-lands-between-pass-loads

Type: reachability
Reachability: default-production
Status: active
Exercised: yes - The
[emergency interleave test](evidence/foreign-write-lands-between-pass-loads.md#marker-evidence)
records the transform's committed `row_version` and the publish's committed
`row_version` from the store inside the hook and asserts the publish landed
after the transform and after the Emergency95 pre-hook floor read, before
`prepare_historian_fire`'s load and the final floor check.
Guarantee: A state-load campaign reaches the interleaving that distinguishes
one-load-per-pass from per-consumer loads.
Check: `sometimes` - For some pass, a foreign commit by another actor
(historian publish, wrapup recut, or state sync) through a second store
handle returns a `row_version` greater than the one the pass's transform
committed, and that commit returns between two of the pass's post-commit
`cache_state` reads (the hook sits after the Emergency95 pre-floor read at
[`:8303-8308`][floor-a] and before `prepare_historian_fire`'s load and the
final floor check); both versions and the ordering are recorded from
the store and the actor, never from the pass's own read, which is what C1
tests. `sometimes` rather than `reachable` because the rerun
lines at [`:8425-8444`][floor-b] execute on every Emergency95 pass while the
interleaving that makes C1 meaningful may never occur.
Fault/timing angle: The window between `commit_transform` and
[`prepare_historian_fire`][prepare] or the floor check.
Required faults and enabling state: A concurrent publish or recut committed
through a second handle inside that window; the
[`between_transform_and_prepare`][hook] hook is the existing seam.
Confidence: high - [Evidence](evidence/foreign-write-lands-between-pass-loads.md).
The rerun logic and the hook are source-verified.
Existing check: [Emergency interleave test](existing-checks.md#cache-state-load-pass-trace-side-channel-and-meta-preparation)
constructs the interleaving; unaudited.
Impact: A single-load design and the current design are indistinguishable to
the suite.
Open questions: None.

### side-channel-row-is-due-during-a-drain

Type: reachability
Reachability: explicit-config-only
Status: active
Exercised: partial - [`status_diagnostics_surface_pending_historian_side_channel_failure`][t-status-sc]
constructs one pending `event` row and a pass whose drain delivers it;
[`historian_side_channel_faults_are_isolated_and_retryable_per_kind`][t-faults-sc]
leaves one pending row per kind, one kind at a time, and drains it directly;
no campaign marker records the situation and no run has all three kinds due
in one pass drain.
Guarantee: A cache-state campaign reaches a pass drain with a due outbox row
of each kind, so C3's per-row clauses are evaluated on real rows rather than
on an empty drain.
Check: `sometimes` - Under three constant markers
`side-channel-row-is-due-during-a-drain-event`, `-primer`, and
`-user-observation`, for some call to [`drain_historian_side_channels`][drain]
from the handler's pass drain ([`:8122-8126`][pass-drain]), the outbox holds
at entry, for that kind, at least one row with `delivered_at_ms IS NULL` and
`next_attempt_at_ms <= now_ms` for the `now_ms` the call passes (the due
predicate at [`:11205-11206`][due-predicate]). Each marker records the
pending rows read from the outbox and the drain's `now_ms` before delivery
runs, never the delivery result. `sometimes` because the drain lines execute
on every pass while a due row may never exist, so `reachable` would be
trivially satisfied.
Fault/timing angle: Under default configuration no [`model_chain`][cfg-models]
is set, so nothing publishes, the outbox is empty on every pass, and C3's
per-row clauses are never evaluated. With publishing,
[`publish_historian_chunk`][publish] drains inline right after its commit
([`:11074-11083`][publish-drain]), so the pass drain finds a due row only when
that inline delivery failed (the next attempt is
`now + 1000 * 2^min(attempt, 6)` ms, [`:11293-11297`][backoff]) or when the
process ended between the enqueue commit and the inline drain.
Required faults and enabling state: A published firing with events, primers,
and user observations (a direct `publish_historian_chunk` call in a unit
test, or a configured `model_chain` with
[`user_memory_collection_enabled`][cfg-user-mem]); a failed first delivery
for each kind through [`fail_next_historian_side_channel_for_test`][fail-sc],
which is set-valued and takes one call per kind, and is available to daemon
tests through the `test-support` dev-dependency
([`Cargo.toml:92`][daemon-cargo]), or a store reopen between publish and
drain on the [restart test][t-restart] pattern; a pass whose `pass_now` is at
or past `next_attempt_at_ms`, 1000 ms after the first failure.
Confidence: high - [Evidence](evidence/side-channel-row-is-due-during-a-drain.md).
The pass drain call, the due predicate, the inline publish drain, the
backoff, the seam, and the two fixtures are source-verified.
Existing check: [State checks](existing-checks.md#cache-state-load-pass-trace-side-channel-and-meta-preparation)
list restart redelivery, per-kind isolation, and the daemon status test;
all unaudited; none records a marker.
Impact: C3 passes on an empty drain on every pass of a default campaign.
Open questions: None.

## Plugin pre-send

### cached-todowrite-verdict-never-lifts-a-deny-or-outlives-its-inputs

Type: safety
Reachability: default-production
Status: active
Exercised: yes - Hook fixtures exercise rejection and timeout after a cached
deny, missing clients, shared fresh hits, agent and session isolation, all
declared invalidations, and shared pending capture/transform reads, including
an empty host agent normalized to absence. Resolver fixtures exercise the TTL
boundary at lookup
and settlement, shared timeout, missing named agents, malformed SDK evidence,
empty-cache and expired-allow failure, late completion fencing, and eviction.
Guarantee: An unavailable, rejected, timed-out, or invalidated permission read
reports todowrite absent, and a cache hit uses only the last successful,
invalidation-free read for the same session and active agent within 30 seconds.
Check: `always` - Failed reads produce `todo_tool_present: false` and suppress
capture even with an empty cache or expired allow. A fresh hit performs no SDK
read and equals the live evaluator at the latest successful read for the same
`(sessionId, toolName, activeAgent)`, subject to the unchanged frozen tools-map
and compaction gates. Freshness ends at read-start plus 30,000 monotonic
milliseconds or invalidation, whichever comes first. The core key distinguishes
undefined from every string; both host hooks normalize an empty agent to
undefined and evaluate session rules alone. Overlapping same-key reads share
one fill while it remains
valid, so two successful allows cannot invent a deny. A completion after
invalidation, deletion, eviction, or TTL expiry cannot publish or return an
allow. These are per-read safety checks, not campaign occurrence checks.
Fault/timing angle: The [shared resolver][permission-cache-resolver] owns the
2,000 ms timeout and the cache. Undefined-agent reads require only
`session.get`; named-agent reads also require `app.agents`.
The pending promise belongs to one LRU entry;
followers share its deadline rather than restart it. Entry identity and an
explicit invalidation flag reject superseded completions. Publication and
caller return both check read-start expiry, including after event-loop stalls.
Only a successful, still-valid result can publish. Every `session.updated`,
`session.compacted`, and `/ctx-flush` expire all agent entries for that session
without erasing their successful verdicts; `session.deleted` removes them.
There is no permission-change subscription. A silent edit can remain
unobserved within the approved 30-second window.
Required faults and enabling state: A frozen callable tools map and enabled
compaction; a successful deny followed by expiry or freshness invalidation
and SDK rejection or timeout; empty-cache and expired-allow failures; distinct
session and agent identities; each invalidation; delayed completions across
invalidation, deletion, timeout, TTL expiry, and pending-entry eviction.
Confidence: high - [Evidence](evidence/cached-todowrite-verdict-never-lifts-a-deny-or-outlives-its-inputs.md).
The focused Bun campaign passes 170 tests across six files. This establishes
the constructed cases, not exhaustive interleavings or a latency benefit.
Existing check: [Plugin checks](existing-checks.md#plugin-pre-send) cover
the hook witnesses, resolver lifetime matrix, unchanged availability and
evaluation rules, and empty-cache timeout outcomes; adequacy remains unaudited.
Impact: A denied `todowrite` is reported present, so the host injects a
synthetic pair the harness cannot execute, or an allowed one is reported
absent.
Open questions: None. User approval provenance for the changed failure default
and accepted staleness window is appended to the evidence investigation log.

[permission-cache-resolver]: ../../../../packages/opencode-plugin/src/hooks/context/ctx-reduce-availability.ts#L306-L362

### mid-turn-read-is-invariant-under-query-collapse-and-statement-caching

Type: safety
Reachability: default-production
Status: active
Exercised: yes - The example-state suite compares against a frozen predicate,
then repeats with another session. Both native adapters exercise statement
reuse, close, path changes, deletion, and same-path replacement. The transform
hook observes idle, a committed user part, and a replacement database.
Guarantee: On static snapshots where each part's session matches its owning
message's session, query collapse and statement caching preserve the mid-turn
answer; read errors stay fail-closed and native statements never outlive their
connection or accumulate outside the bounded cache.
Check: `always` - Under the user-approved association scope, the collapsed
[`isMidTurn`][ismidturn] returns the same boolean on the static corpus as the
[frozen reference][midturn-reference] from
`7ed1e9845af1a76ff04c31d95ea811367a926bb0`. Let A be the latest message by
`(time_created DESC, id DESC)` with `role = 'assistant'` that is not both
`summary = 1` and `finish = 'stop'`, using the `json_valid` CASE guard so
malformed `data` reads as NULL; return true if any `role = 'user'` message
with `(time_created, id)` greater than A's (using `-1` and `""` for nullish
time and id) and no same-session part of `type = 'compaction'` is real,
meaning it has no parts, or some part parses as an object, is not machine-authored
(`synthetic`, `syntheticTodoMarker`, `ignored`, or `metadata.marker.kind`),
and is a non-text typed part or a text part with non-empty cleaned text;
otherwise false if A's id is not a string, true if the SQL-extracted
`time.completed` is not a JavaScript number,
true if A's `finish = 'tool-calls'`, else true iff some part of A parses as
an object with `type = 'tool'`, is not provider-executed, and is not
machine-authored; a missing database returns false and any error on an
existing database returns true. Part parsing, exact provider flags, machine
flags, and cleaned text stay in JavaScript. Every cached statement belongs
to the [read-only connection][dbcache] that prepared it. Two consecutive
reads reuse statements, not results. A path change, missing file, explicit
close, or changed `(st_dev, st_ino)` discards the connection and its cache
before another callback. Caller-held handles retain only SQL and the owner,
not native statements. Eviction and uncached execution release native handles
without waiting for GC; repeated `get`/`all` calls reprepare when needed.
`always` because each evaluated corpus state must agree and closed or retired
native statements must never execute.
Fault/timing angle: [`isMidTurnFromOpenCodeDb`][midturndb] reads two tables
without a transaction, so a writer landing between the assistant query and
the [candidate query][newer] can make the reads inconsistent. The user
candidate class is one statement with a same-session part join and exclusion;
the assistant row and, only for a completed non-`tool-calls` assistant, its
parts are two more, in the reference's order. A streaming or `tool-calls`
assistant answers from its row without reading `part`. No jointly atomic
snapshot is promised. Static equivalence does not imply
equal answers under arbitrary concurrent-writer schedules. A stat before and
after every native open checks replacement, including Node connection
recycling. Bun finalizes retired statements; Node lacks a statement finalizer
and closes the native connection on retirement. The next read reopens it with
the same identity checks. Close attempts every cached finalizer and closes the
native database in `finally` even if a finalizer throws.
Required faults and enabling state: A populated `message`/`part` pair with
the shapes the tests build (streaming assistant, `tool-calls` tail,
compaction summary after `tool-calls`, same-millisecond rows, malformed JSON
rows and parts, marker-only user parts); rows from a second session sharing
the database; a read error on an existing database; a connection replacement
or close between two passes while cached statements exist; a database file
replaced at the same path between two passes (renamed over, or deleted and
recreated with different rows) while the connection is cached, with the
file identity read before and after; eviction and oversized SQL/bind removal
paths, throwing executions, and a finalizer failure; repeated logical handles,
bounded array binds, named binds, and a partless user on both native adapters;
normal 800-ID time/part chunks between mid-turn reads, with native prepare and
close counters and connection identities observed on both runtimes; lists
growing from 801 to 870 IDs across 70 final-chunk widths, with exact returned
maps, ordered message/part contents, frozen inputs and stable native identities;
a streaming and a `tool-calls` assistant each holding 40 tool parts, with the
rows every read materializes counted.
Confidence: medium - [Evidence](evidence/mid-turn-read-is-invariant-under-query-collapse-and-statement-caching.md).
The local differential and native lifetime checks pass. No production latency
or native-heap-size claim follows from those checks.
Existing check: [Plugin checks](existing-checks.md#plugin-pre-send) cover
the differential states, both native caches, the actual transform hook, the
wrapper, overrides, and spread positional binds; all unaudited for adequacy.
Impact: A pass is treated as mid-turn when it is not, or a cached statement
executes against a closed connection.
Open questions:
- Whether a collapsed query's plan depends on an index that exists in
  OpenCode's schema is unresolved, needs the pinned OpenCode schema; every
  fixture declares only `id TEXT PRIMARY KEY`.
- Does OpenCode ever replace `opencode.db` in place (rename over, or delete
  and recreate) while a plugin process holds a read-only handle? Nothing in
  this repository states its write behavior. The implementation assumes it
  can happen and checks identity regardless. (needs external input)

### paged-body-measure-equals-declared-frame-length-and-fits-host-caps

Type: safety
Reachability: default-production
Status: active
Exercised: partial - The [carrier campaign][carrier-campaign] joins the pager,
module transport, body encoder, header encoding, and UTF-8 writer at an
injected native-writer fake. The live hook sends the carrier. A separate real
host run completes twelve transforms, stages nine pages, and preserves six
lone-surrogate refusals; one oversized scalar body is refused by the
pager. Actual TypeScript native attachment is unavailable on this host.
Guarantee: The length the plugin measures for paging is the length the frame
writer declares and emits for the same serialized snapshot, and pages with
Rust-valid strings fit the host's page cap while lone surrogates retain their
encoded bytes and existing host refusal.
Check: `always` - For every `{ page, bytes }` emitted by
[`buildPagedModuleTransformPayloads`][paged], `bytes` equals
the carried text's UTF-8 length, the decoded header length, and the captured
byte-array length, with byte-for-byte equality to that text. For Rust-valid
JSON carrying `transform_page_index`, require
`serde_json::to_vec(&request).len() <= 524288` and actual host page admission
without a size refusal. The unpaged fast path retains only its original
`wireBytes <= 524288` paging threshold; the host applies its 32 MiB raw
transform limit, not the reserialized page cap, to that request. For the
lone-surrogate corpus, require unchanged bytes and
`host.unrecognized_request_shape`, not acceptance. This scope is the owner's
approved disposition of the [original counterexamples][page-admission-probe].
`always` applies to each admitted snapshot, not to a sum across pages.
Fault/timing angle: Source getters and `toJSON` run before snapshot creation;
mutation after measurement cannot alter the stored text. Ordinary object
fields cannot impersonate the private symbol identity. Numeric tokens parsed
as f64 can expand on the host, so paged packing charges a separate conservative
growth bound and never adds that allowance to exact wire telemetry.
Invalid strings retain byte-only packing to preserve their parse refusal.
Required faults and enabling state: Unicode and escaped control characters;
high and low lone surrogates; unpaged, intermediate, final, and continuation
pages; exact-cap and over-cap scalar bodies; f64 growth near the cap; source
getters, `toJSON`, post-measurement mutation, and ordinary-field collisions.
Confidence: medium - [Evidence](evidence/paged-body-measure-equals-declared-frame-length-and-fits-host-caps.md).
The scoped corpus passes at the writer-fake and real-host seams. The numeric
bound uses the existing integer-token classifier and the locked serializer's
24-byte f64 bound, not a second approximate number renderer. This is not an
end-to-end TypeScript native-attachment or performance result.
Existing check: [Plugin checks](existing-checks.md#plugin-pre-send) cover
the joint writer, snapshot mutation, stringify spies, plain objects, live
hook, and real-host corpus; all unaudited.
Impact: A frame declares a length it does not emit, or a page the plugin
accepted is refused by the host with `buffer_overflow`.
Open questions:
- Which supported runtime can execute the TypeScript path through an actual
  native channel on this host? The tested Bun and Node capability probes
  refuse startup.

[page-admission-probe]: evidence/paged-body-measure-equals-declared-frame-length-and-fits-host-caps.md#q-what-does-the-real-host-admission-probe-establish
[carrier-campaign]: evidence/paged-body-measure-equals-declared-frame-length-and-fits-host-caps.md#q-what-do-the-unpaged-correction-and-registered-cargo-test-prove

### log-lines-keep-sanitizer-and-file-hardening-guarantees

Type: safety
Reachability: default-production
Status: active
Exercised: yes - Production-mode subprocesses check default and info-level
sanitization, truncation, modes, symlink and foreign-owner refusal, swallowed
batches, and exit flush. Real transforms and events exercise debug, warn,
and off with equal served bytes and unchanged failure fallback.
Guarantee: A level gate removes lines; it never weakens the sanitization,
truncation, permissions, or swallow accounting of the lines that remain.
Check: `always` - Every line the logger appends begins with an ISO-8601
timestamp in brackets, contains no code point in `0x00-0x08`, `0x0b-0x1f`,
or `0x7f`, has `\n`, `\r`, and `\t` flattened to spaces, has each field
truncated at `MAX_FIELD_CHARS = 2048` with a trailing ellipsis
([`sanitizeField`][sanitize]), and is written through a descriptor opened
with `O_WRONLY|O_APPEND|O_CREAT|O_NOFOLLOW|O_NONBLOCK` at mode `0600` under a
managed `0700` chain owned by the current uid ([`ensureLogDir`][ensuredir],
[`appendPrivate`][appendpriv]); a write failure increments
`swallowedWriteCount` once per failed batch and never throws. A gated call
does not inspect caller objects, sanitize, serialize, timestamp, schedule a
flush, or write. Explicit and exit flushes still drain admitted entries after
the level becomes off. `always` because the sanitizer and
hardening are the only defense against log forgery and symlink redirection,
and a gate changes which lines exist, not what a written line may contain.
Fault/timing angle: The shared [`writeLog` gate][sessionlog] precedes session
prefix conversion and all entry processing. Calls remain observable through
the level methods, including the [per-pass debug spy][t244]. Environment
changes apply on the next call, not to an already-buffered batch.
Untagged calls use info regardless of message wording. Warn/error thresholds
cover only explicitly classified paths; info retains other diagnostic errors.
Call-site argument construction still runs before the shared gate.
`sanitizeField` does not strip C1 controls or `U+2028`/`U+2029`; that is the
current contract, not a defect claim.
Required faults and enabling state: Untrusted text with embedded newlines and
control characters in a message or data field; file and managed-directory
symlinks; a foreign uid returned by the directory-stat seam; caller getters
and `toJSON` under a rejecting level; a pending batch when the level turns off.
Confidence: high - [Evidence](evidence/log-lines-keep-sanitizer-and-file-hardening-guarantees.md).
The [gate, hardening, and hook checks][log-gate-checks] pass locally. The
[batched flush][flush] remains bounded at 50 lines or 500 ms. Logging does not
invoke [`shared/redaction.ts`][redaction]; the gate adds no secret-redaction
policy, cache, or claim about end-to-end latency.
Existing check: [Plugin checks](existing-checks.md#plugin-pre-send) cover
control characters, size bound, modes, symlink, swallow counter, exit flush,
level ordering, zero-work rejection, and actual transform/event lines; all
unaudited for independent adequacy review.
Impact: Untrusted text forges log lines, or the log is redirected through a
symlink.
Open questions:
- Provider error bodies and model output reach the log unredacted; the CLI
  redacts on export. Is that the intended boundary, or must the plugin
  redact before write? (needs human input)

### todowrite-deny-then-read-failure-is-exercised

Type: reachability
Reachability: test-only
Status: active
Exercised: yes - The hook fixture constructs both rejection and timeout after
a stored deny, for transform and capture independently.
Guarantee: The campaign independently witnesses a cached deny followed by a
failed live read at least once, regardless of the returned verdict.
Check: `sometimes` - For some pass, both preconditions hold independently:
`peekToolPermissionDeniedForTest(sessionId, "todowrite", activeAgent) === true` at
resolver entry, and the live read for that
same pass rejects or times out. The marker asserts the preconditions, not
the outcome. This is a reachability witness, not evidence that cached deny
changes the outcome relative to the fail-closed empty-cache case.
Fault/timing angle: An earlier successful read stores `true`; expiry or
freshness invalidation forces a later live read without erasing that deny.
Required faults and enabling state: An SDK fake that answers `deny` once and
then rejects or hangs; a session whose map verdict is frozen and callable.
Confidence: high - [Evidence](evidence/todowrite-deny-then-read-failure-is-exercised.md).
The [constant hook marker][permission-failure-witness] asserts the entry
snapshot, SDK invocation, and observed Error or TimeoutError separately from
the wire-body and capture-suppression assertions. Both variants pass.
Existing check: [hook.test.ts:167][permission-failure-test] drives the public
hook with SDK mocks and Bun fake timers; adequacy remains unaudited.
Impact: P1's fallback clause passes without the window ever opening.
Open questions: None.

[permission-failure-witness]: ../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L218-L244
[permission-failure-test]: ../../../../packages/opencode-plugin/src/hooks/context/hook.test.ts#L167

## Ring arena and direct frame

### arena-residency-is-bounded-by-admission-and-one-punch-batch

Type: safety
Reachability: default-production
Status: active
Exercised: partial - The admission quotient and the batch boundary have
tests; none asserts the dead-byte inequality over a long run or measures an
idle ring.
Guarantee: The host's only memory ceiling on the ring is a hard admission
bound on virtual arena bytes, and the bytes a producer ring keeps resident
beyond its live and pending frames are bounded by one punch batch plus one
boundary page, with an aborted reservation contributing nothing.
Check: `always` - At every admission decision the sum of `arena_bytes`
charged to admitted connections is at most
[`MAX_RING_RESIDENT_BYTES`][max-resident], the host refuses
(`ExceedsResidentBytes` at [startup][process-limits], connection refusal at
runtime) rather than exceed it, and the bound is evaluated on the grant's
virtual arena size, never on `mincore` residency. After every return from
[`reclaim_completed_inner`][reclaim],
`arena_reclaimed - punched < punch_batch_bytes()` holds for the producer
handle (the branch at [`:2129-2134`][punch-decision] punches when the run
reaches [`punch_batch_bytes`][batch], `arena_bytes / 4`, so 16 MiB at the
[64 MiB arena][arena-const]), and after every [`abort_reservation`][abort] no
page of the aborted range is resident. `always` because the admission check
has no legal exception, and the dead-byte inequality is the current implicit
residency bound; if punching moves to idle, the record's check becomes the
replacement bound the specification states, evaluated at every idle point of
the [endpoint loop][idle-select].
Fault/timing angle: A deferred-punch design that reads the constant's name
as an RSS promise adds an invariant with no oracle; one that treats it as a
budget for unpunched bytes lets one idle ring hold its full 64 MiB per
direction resident, which the current bound already permits. Punching is
coupled to the next [`try_reserve`][try-reserve], so an idle ring after a
burst keeps up to one batch resident until the next publish. The client
runs the same crate for the peer-to-host direction
([`reserve_until` in shm-native][native-reserve]) and has no `trim` caller
either, so a host-only change alters one direction.
Required faults and enabling state: `max_connections` above
[`affordable_connections`][affordable]; a connection attempt when every
affordable slot is charged; a released run of at least one batch on an
otherwise idle ring; a reservation written then aborted; a wrapped run
crossing the arena end so [`removal_ranges`][removal-ranges] splits.
Confidence: high - [Evidence](evidence/arena-residency-is-bounded-by-admission-and-one-punch-batch.md).
The quotient, [`process_limits`][process-limits], the
[configuration validation][config-validate], the reclaim branch, the batch
divisor, and the abort punch are source-verified. No production code reads
residency; [`resident_arena_pages`][resident-api] is a test-only probe.
Existing check: [Ring checks](existing-checks.md#ring-arena-and-direct-frame)
cover the quotient, the batch boundary, abort residency, reuse as zeroes,
sub-page releases, and punch-failure quarantine; all unaudited.
Impact: A relocation of punching silently changes the residency bound, or a
new residency check is added with nothing to test it against.
Open questions:
- Does the specification intend a physical residency bound at all, or only to
  preserve the admission bound and the per-ring dead-byte bound? (needs human
  input)
- What is the replacement bound when punching is deferred: bytes, pages, a
  time since the last reserve, or "punched by the next idle point"? (needs
  human input)
- Does an idle-time punch count as a "timed ring" activity under
  [§7.7][wire77], which forbids a timed ring poll and any prefault? (needs
  human input)

### arena-payload-copies-keep-the-address-derived-atomic-shape

Type: safety
Reachability: default-production
Status: active
Exercised: partial - Round-trip, concurrent-writer, and per-byte agreement
tests run natively and under Miri; none covers `to_vec` with span lengths
that do not sum to `body_len`.
Guarantee: A concurrent peer store of the same shape yields stale bytes,
never a mixed-size data race, and a receiver never reads uninitialized
process memory as payload.
Check: `always` - Every byte moved between the arena and process memory goes
through [`copy_in`][copy-in], [`copy_out`][copy-out], `read_byte`, or
`checksum`, each of which partitions the range with
[`AccessShape::of(address, len)`][shape]; no `&[u8]` or `&mut [u8]` is
formed over arena bytes; and a [`to_vec`][to-vec] that skips the zero-fill
still returns a `Vec` whose every byte in `0..body_len` was written by
`copy_out` before the `Vec` is observable, or returns `Err` without exposing
the buffer. `always` because the soundness argument in the
[`LeaseSpan` contract][span-safety] and the crate's [AGENTS.md][agents]
treat this as a verification boundary.
Fault/timing angle: The zero-fill at [`to_vec:331`][to-vec-fill] makes the
`Vec` initialized before any early `Err` return; replacing it with capacity
plus `set_len` or `MaybeUninit` moves the initialization proof onto the
per-span length checks. A `memcpy` replacement forms a reference over
peer-writable memory, which the shm-transport catalog's
[no-reference record][shm-noref] forbids.
Required faults and enabling state: A peer writing the same span while the
host copies (the [Miri thread test][t-concurrent] constructs it); a lease
whose span lengths do not sum to `body_len`; a span starting at each of the
eight word offsets.
Confidence: high - [Evidence](evidence/arena-payload-copies-keep-the-address-derived-atomic-shape.md).
The shape derivation, both copies, the zero-fill, and the raw-pointer
[exclusive-write documentation][span-ptr] are source-verified;
[`receive_one`][receive-to-vec] calls `to_vec` on every inbound frame and
[`write_reservation`][write-res] calls `copy_in` on every outbound frame.
Existing check: [Ring checks](existing-checks.md#ring-arena-and-direct-frame)
include the three lease tests under the [Miri job][ci-miri] and the
[Valgrind job][ci-valgrind]; all unaudited.
Impact: A torn multi-byte read or an uninitialized payload byte reaches the
handler.
Open questions: None.

### direct-frame-publishes-declared-length-or-nothing-and-holds-its-charges

Type: safety
Reachability: test-only
Status: active
Exercised: partial - The deadline arm has one in-crate test; underfill,
overflow, serializer error, panic, and the direct charge lifetime have none.
Guarantee: The direct path never publishes a frame whose body differs from
its header's declared length, a mid-write failure leaves no hole, partial
frame, or leaked slot, and deferring serialization releases neither the
egress charge nor the source bytes before they reach the ring.
Check: `always` - For every [`DirectFrame`][direct-frame] handed to
[`publish_direct`][publish-direct], either the serializer writes exactly
`body_len` bytes and `commit(body_len)` publishes one frame whose header
`len` equals `body_len`, or no frame becomes visible: a short write fails
`commit` with `Underfill` ([`:2533-2570`][commit-underfill]), an over-write
fails [`ReservationWriter::write`][res-writer] through
[`ProducerReservation::write`][res-write] with `Overflow`, a serializer `Err`
or panic drops the reservation, and each path runs
[`abort_reservation`][abort]. A Direct [`OutputBuffer`][outbuf] holds an
egress `ByteCharge` of exactly `exact_len + HEADER_LEN`
([`reserve_direct`][reserve-direct]), [`into_parts`][into-parts] passes it
through unshrunk, the charge drops only after `commit` in
[`publish_one`][publish-one], and any request-owned bytes the serializer
closure captures count as retained until that point. `always` because
[`prepare_commit`][prepare-commit] checks the declared length against
`body_len` on every commit, there is no partial-frame state, and [E2][e2]
requires each charge to cover its resource's lifetime.
Fault/timing angle: The serializer runs on the [endpoint thread][idle-select]
after `reserve_until` returns, so its CPU time holds a ring reservation and
blocks inbound receives; a serializer that finishes after `frame_deadline` is
refused at [`commit_before`][commit-before]. Every `publish_direct` error is
reported through `publish_one` as
[`ReadClose::Corrupt("shared-memory publish failed")`][publish-fail], which
closes the whole connection, while the owned path turns the same failure into
a request-scoped `encode_failed` terminal in
[`settle_prepared_with`][settle-with]. On the direct path the closure holds
the source tree after `handle` returns, so a scratch charge released at
return undercounts until the endpoint thread serializes; a cancelled or
retired generation drops the queued frame with its closure and must release
both.
Required faults and enabling state: A serializer that writes `body_len - 1`,
`body_len + 1`, returns `Err` after some bytes, or panics; a body wrapping
the arena end; a deadline expiring during serialization; a direct response
queued behind slow egress with the handler future gone (T4); generation
retirement with a direct frame queued; an egress budget near capacity.
Confidence: medium - [Evidence](evidence/direct-frame-publishes-declared-length-or-nothing-and-holds-its-charges.md).
The reservation, writer, commit, abort, charge, and failure classification
are source-verified; the path has no production sender at HEAD, so the
charge class for captured source bytes is undecided.
Existing check: [Ring checks](existing-checks.md#ring-arena-and-direct-frame)
cover the deadline arm, quarantine at ring level, and the owned-arm
`into_parts` cases; none for the direct arm's charge; all unaudited.
Impact: A response that won settlement closes the connection for every
in-flight request, or source bytes outlive the charge that admitted them.
Open questions:
- Is a connection close the intended outcome for a serializer failure on a
  settled response, or must the direct path preserve the owned path's
  request-scoped `encode_failed` terminal? (needs human input)
- Which charge class covers the captured source bytes between `handle`
  returning and `publish_one` completing: the egress charge already taken,
  the request scratch charge, or a new class? (needs human input)
- [The host-runtime terminal record][hr-terminal] holds trivially on the
  failure arm because nothing is emitted; the settled `Response` is then
  never delivered, which [its publication-failure record][hr-pubfail]
  already covers for owned frames.

### direct-frame-outlives-its-handler-before-publication

Type: reachability
Reachability: test-only
Status: active
Exercised: not yet - No direct frame is queued by any test after its handler
future ends.
Guarantee: A direct-serialize campaign reaches the window in which the
serializer closure is the only owner of the response's source bytes.
Check: `sometimes` - For some request, an `OutboundFrame` carrying a
[`DirectFrame`][direct-frame] is queued, the handler future that produced it
has returned or been dropped, and [`publish_one`][publish-one] has not yet
committed. The marker asserts these three preconditions, not the charge
accounting.
Fault/timing angle: For a unary response the frame is queued from `settle`
after the handler future has completed (`dispatch.rs:956-997` joins the
future and calls `settle` at `:995`; `:409-420` emits the frame), so the
closure outlives the handler whenever the frame is queued. With an idle ring
and a free endpoint thread the window between queueing and commit is short
and a marker can miss it. The endpoint thread can serialize before the
handler returns only for a stream item sent through `StreamSink::send`
(`:556-582`); the transform response is unary.
Required faults and enabling state: A slow egress or a held ring reservation
ahead of the direct frame, to lengthen the window so the marker can observe
it; a handler that returns immediately after
[`output_from_writer`][from-writer]; observation of handler completion (the
joined future) and of the `publish_one` commit as separate events.
Confidence: medium - [Evidence](evidence/direct-frame-outlives-its-handler-before-publication.md).
The queueing and commit points are source-verified; the direct path is
reached only through the [fixture arm][fixture-arm] at HEAD.
Existing check: none found.
Impact: T3's charge clause passes without the window ever opening.
Open questions: None.

## CAS usage accounting

### artifact-admission-fails-closed-against-on-disk-object-bytes

Type: safety
Reachability: default-production
Status: active
Exercised: partial - Cap error, invalidated-retained bytes, reclaim, and the
inclusive payload limit have tests; none covers dedup at exactly the cap or a
refusal after an unrecovered orphan publish.
Guarantee: No ingest publishes bytes that would raise the on-disk regular-file
sum under `objects` above `artifact_cap`, and a refused ingest leaves no
reservation row and no published object.
Check: `always` - For every ingest, [`check_budget`][check-budget] runs under
the exclusive [writer lock][lock-writer] before the reservation row, the
shard creation, and the publish rename ([`:407-416`][ingest-lock]); it refuses
with [`Capacity`][cap-error] carrying `usage` and `cap` when
`usage + byte_length > artifact_cap`, adds zero for a digest already present
([`object_is_present`][present]), and counts invalidated-but-retained objects
because [`regular_file_bytes`][walk] reads the filesystem, not
`evidence_meta`. `always` because the cap is a hard promise with no admission
on the failure arm.
Fault/timing angle: The temp file is written and synced under `tmp`
([`:379-405`][ingest-temp]) before the lock and the check, so `tmp` bytes are
never counted and a refused ingest still cost one full write; two ingests
serialize on the writer lock, so the walk cannot race a concurrent publish,
but it does race the health sampler's lock-free walk. A counter consulted
here instead of the walk must be updated inside the same lock scope that
publishes or unlinks, or it admits over the cap by whatever it lags.
Required faults and enabling state: A store at `cap - 1` receiving a two-byte
payload; the same digest re-ingested at the cap; an invalidated reference
whose bytes remain; a crash between publish and reference commit followed by
a second ingest before recovery.
Confidence: high - [Evidence](evidence/artifact-admission-fails-closed-against-on-disk-object-bytes.md).
[`ingest_artifact_inner`][ingest], the lock order, the budget walk summing
[`st_size`][stat-bytes], and the [4 GiB default][cap-default] are
source-verified.
Existing check: [CAS checks](existing-checks.md#cas-usage-accounting) cover
the cap error, retained bytes, reclaim, the inclusive limit, and the daemon
route at the cap; all unaudited.
Impact: A counter that lags admits bytes over the cap, or a refusal leaves a
reservation behind.
Open questions:
- The daemon maps `Capacity` to [`StoreBusy`][busy], a retryable class, while
  the only production paths that lower usage are failed-ingest cleanup and
  startup recovery ([`run_staging_maintenance`][maintenance] and
  [`delete_artifact`][delete] have no daemon caller). Is "busy" the intended
  classification of a cap that only a restart or an operator relieves?
  (needs human input)

### reported-artifact-usage-equals-on-disk-object-bytes-after-recovery

Type: safety
Reachability: default-production
Status: active
Exercised: partial - The fault-injection oracle asserts the equality after
every fault point and after a second recovery; no test compares two
independent usage sources because only the walk exists.
Guarantee: Reported artifact usage never drifts from the bytes actually on
disk once recovery has run, whatever crash window preceded it.
Check: `always` - At every quiescent point (after `KernelStore::open`
completes [`recover_interrupted_work`][recover], and after each ingest,
cleanup, GC pass, or purge returns with the writer lock released), the usage
the store reports through [`artifact_budget_facts`][facts] and the usage
`check_budget` admits against both equal an independent sum of `st_size`
over regular files under `objects` and its shard directories. `always`
because the [fault-injection oracle][t-oracle] asserts exactly this equality,
and a durable counter turns it from a tautology into the drift check.
Fault/timing angle: Every path that changes on-disk object bytes is a
filesystem operation beside, not inside, a SQLite transaction: publish by
rename after the [reservation row][ingest-reservation] commits and before the
reference commits ([`:476-526`][ingest-publish],
[`:580-595`][ingest-commit]); unlink inside a fenced write in
[`cleanup_failed_reference`][cleanup], [`reclaim_candidate`][reclaim-cand]
(reading the size it removes in [`unlink_artifact`][unlink-artifact]), and
[`complete_pending_purge_locked`][purge-unlink]; a dedup hit or a failed
publish only [releases the row][release-res]. A crash after the rename and
before the reference commit leaves bytes the walk counts and a `Live`
reservation that [`prepare_startup_cas_recovery`][startup] promotes and
[`run_artifact_recovery`][recovery] unlinks; a crash after an unlink and
before its commit leaves a row without bytes that recovery retires
([`:109-136`][startup-unreachable]). A counter written in the transaction
over-reports in the second window and under-reports in the first until
recovery reconciles it; the walk is correct in both because it is the
reconciliation. The [restore path][restore] and tests writing
[directly to disk][t-orphan] bypass any counter. The health sampler walks
without the writer lock, so equality is claimed only at quiescence.
Required faults and enabling state: The six ingest fault points and three GC
fault points in [`cas_fault_injection.rs`][t-faults]; SIGKILL at the
[post-reservation and post-publish crash barriers][t-crash]; an object added
or removed under `objects` by something other than the store.
Confidence: high - [Evidence](evidence/reported-artifact-usage-equals-on-disk-object-bytes-after-recovery.md).
Every byte-changing path and both recovery branches are source-verified; GC
and purge reclamation are reached only by tests and benches.
Existing check: [CAS checks](existing-checks.md#cas-usage-accounting) include
the semantic oracle, `recover_twice`, the orphan reconciliation, the
cancelled walk, and the retired live reservation; all unaudited.
Impact: A durable counter silently diverges from the disk and either refuses
valid ingests or admits over the cap.
Open questions:
- Is the walk retained as a periodic or startup reconciliation, and what is
  the fail-closed action on `counter != walk`: refuse ingest, latch the CAS
  failure ([`latch_cas_failure`][latch]), or adopt the walk value? (needs
  human input)
- [`regular_file_bytes`][walk] counts a root-level regular file and any file
  in any subdirectory, while GC's [`scan_objects`][scan-objects] skips
  non-canonical shards; which definition does a counter follow? (needs human
  input)

### artifact-byte-decrement-paths-are-exercised

Type: reachability
Reachability: test-only
Status: active
Exercised: partial - The fault tables and crash windows assert state
convergence, not a usage delta per path.
Guarantee: Every decrement path a durable counter must observe is reached
with bytes at stake before the counter ships.
Check: `sometimes` - A campaign records the independent `st_size` sum over
`objects` ([`regular_file_bytes`][walk]) before and after each path that
removes object bytes, with the writer lock released at both readings, under
one constant marker per path
(`artifact-byte-decrement-paths-are-exercised-cleanup`, `-recovery`,
`-reclaim`, `-purge`, `-retry`), and each marker fires at least once with a
non-zero byte delta: [`cleanup_failed_reference`][cleanup] after a
[`PublishOutcome::Published`][ingest-publish] followed by a reference-commit
failure; [`run_artifact_recovery`][recovery] unlinking an orphan publish
after a crash; [`reclaim_candidate`][reclaim-cand] reclaiming an invalidated
object past its grace; [`complete_pending_purge_locked`][purge-unlink]
unlinking a purged digest; and a GC unlink that fails and is retried.
`sometimes` because these are operational states, and executing the unlink
lines with a zero-byte candidate would not exercise the accounting.
Fault/timing angle: At HEAD only cleanup and startup recovery run in
production; GC and purge decrements are reached only when a caller is added,
so a counter validated against the walk in production traffic never sees
them.
Required faults and enabling state: `ArtifactIngestFault::AfterEvents` after a
new publish; a SIGKILL child at `INGEST_CRASH_POINT`; an invalidated
reference aged past `RESERVATION_MS`; a purge request;
`ArtifactGcFault::Unlink`.
Confidence: high - [Evidence](evidence/artifact-byte-decrement-paths-are-exercised.md).
The five unlink sites and the absence of daemon callers for GC and purge are
source-verified.
Existing check: [CAS fault tables](existing-checks.md#cas-usage-accounting)
exercise the fault points and crash windows; all unaudited and none records
a per-path delta.
Impact: A counter ships validated against paths that never decremented it.
Open questions: None.

## Wildcard and cross-cutting

### optimized-stage-is-measured-at-production-shape

Type: reachability
Reachability: test-only
Status: invalidated - no catalog record owns a before-and-after measurement
artifact; the record is kept as authored for traceability.
Exercised: not yet - No daemon measurement records build, host, and workload
identity, and no bench reaches `Handler::handle` or a production-sized steady
session.
Guarantee: "The optimization is faster" is a claim with an artifact behind it
rather than an assertion.
Check: `sometimes` - Under one constant marker per stage the specification
authorizes to change (the marker list is fixed when the specification
enumerates the stages; no name is built at run time), a recorded measurement
run reaches that stage with an input in the
production size class, before and after the change, under one build, host,
and workload identity, and the record names the stage and the input. This is
situation coverage: a stage's lines execute under a 69-byte body or a
100-message request without the production state ever occurring, so
`reachable` is the wrong semantics.
Fault/timing angle: None; a coverage record.
Required faults and enabling state: A session at the production size class
(the bench header's 1_400-message 2 KiB mixed point or the fixture's 2_500),
reached by incremental growth because a first pass cannot commit it (the
store's 512 KiB durable-text bound rejects a 1_400-message first HARD pass,
pinned by [`transform_meta_bound.rs`][meta-bound]); an ingress body through
`Handler::handle`, not a typed request; a warm store for steady passes and a
cold store for the first pass; a 64 MiB direction arena for the ring probes.
Confidence: high - [Evidence](evidence/optimized-stage-is-measured-at-production-shape.md).
No CI performance gate exists: [`ci.yml`][ci-bench] runs every bench once in
test mode and compares nothing; [`.config/nextest.toml`][nextest] excludes
bench targets. The daemon bench's [header][hp-header] disclaims its own
numbers as a baseline; its end-to-end arms call [`transform_cached`][hp-e2e]
on an already-typed request against a fresh store, skipping the handler work
at [`:8113-8130`][h-pre] and the response encoding in
[`respond_transform`][respond]. The two production-sized fixtures
([1_400][fx-1400], [2_500][fx-2500]) are `#[ignore]` and print to stderr. The
transport bench measures a fixed [256- or 4096-byte payload][he-payload],
[rejects `--designated-host`][he-designated], and labels its own record
[`BLOCKED`][he-blocked] while its [manifest][he-manifest] declares 24 probes.
The host-runtime [`evidence.rs`][evidence] already implements the manifest
discipline a daemon record needs.
Existing check: [Wildcard checks](existing-checks.md#wildcard-and-cross-cutting)
list the bench smoke, the evidence manifests, and the timing-line tests; none
records a daemon measurement or compares two builds; all unaudited.
Impact: A latency change ships with no comparable artifact, so a regression
and an improvement are indistinguishable.
Open questions:
- Which size class is "production-shaped": the bench header's 1_400 or the
  fixture's 2_500 messages? (needs human input)
- Does the specification adopt an `evidence.rs`-style manifest for the daemon,
  or accept the stderr timing line as the record? (needs human input)
- The bench comment attributes the 512 KiB cliff to `meta` growing about 460
  bytes per message; the cliff is pinned at 1_000 ok and 1_400 refused, the
  per-message figure is not verified here.

### stage-timing-fields-keep-their-boundaries

Type: safety
Reachability: default-production
Status: invalidated - the one recorded stage delta, the single pass-state
load ([C1][c1]), gave the moved read its own `pass_state_load` field and key
instead of leaving it inside `delta_expand` and `projection_cache_lookup`,
so no existing field changed what it brackets; the record is kept as
authored for traceability.
Exercised: partial - The line's key set and the default deserialization are
pinned; nothing ties the TypeScript key list to the Rust struct or asserts
what a field brackets.
Guarantee: A stage that reports a smaller number after the change got faster
rather than moving out from under its timer.
Check: `always` - Every key printed by [`format_pass_timing_line`][fmt] and
every key the plugin's [`rust module stages:` line][ts-stages] reads resolves
to a field of [`TransformTimings`][tt] under a pinned key-to-field map, which
at HEAD is the identity except `post_attach_ms` for `post_attach`
(`transform.rs:1264`, `:1342`, field at `:1172`); each field's start and stop
instants are stated in a per-field table beside the struct and a relocation
changes the table in the same change (a review gate, not a runtime
assertion); a field for a removed or merged stage
is removed from the struct and both consumers rather than left to report
`0.0` through `#[serde(default)]`; and the two `local_stats` reads that form
the token-cache delta ([`record_token_cache_delta`][rtcd]) run on one thread.
`always` because the handler populates `timings` on every ordinary pass and
[`respond_transform`][respond] emits the line for every response.
Fault/timing angle: The transform moves to a blocking thread while the
handler-level `Instant` pairs at [`:8538-8562`][h-timings] stay on the
handler task; a stage split across an `.await` splits its
[thread-local counter][tc-local] delta. Both reads sit inside the synchronous
[`apply_additive_only`][snap-add] and [`apply_once`][snap-once] bodies today.
Every field carries `#[serde(default)]`, so a dropped field deserializes as
zero and the plugin prints `n/a` only when the key is absent
([`:1019-1024`][ts-stage-fn]).
Required faults and enabling state: A pass that populates `timings`; a code
change that relocates or splits a stage.
Confidence: high - [Evidence](evidence/stage-timing-fields-keep-their-boundaries.md).
The struct, the formatter, the handler assignments, and the plugin reader
([`:999-1012`][ts-read]) are source-verified. The wire contract has no
`timings` statement; the field is a daemon-to-plugin convention.
Existing check: [Wildcard checks](existing-checks.md#wildcard-and-cross-cutting)
include the line test, the default test, and the plugin's stage-log test; all
unaudited.
Impact: A relocated stage reports zero or a fragment and reads as an
improvement.
Open questions:
- Are the stage fields a contract with the plugin, or free to change with the
  plugin's log line? (needs human input)
- Does a recorded delta that adds its own bucket reactivate this record, or
  does it stay invalidated until a delta moves work under an existing timer?
  (needs human input)

### token-cache-is-a-pure-declared-memo-behind-one-estimator-interface

Type: safety
Reachability: default-production
Status: active
Exercised: partial - Memo parity, hit accounting, rotation, stats partition,
and key-domain non-aliasing have tests. The [whole-module scan][t-bypass]
covers SOFT, serialization, tag minting, and nudge derivation. The
[SOFT spy][soft-threshold-check], [estimator gates][soft-gates-check],
[tag/nudge accounting][tag-accounting-check], and
[serialization gate][serialization-gate-check] pass. Cache sizing and
contention are unchanged and receive no new measurement.
Guarantee: Sharding or resizing the token cache changes latency only, and an
optimization cannot make a decision-bearing token count invisible to the
pass's own accounting.
Check: `always` - For every input the cache returns
`tokenizer::estimate_tokens(input)`; the tail-hygiene key domain
`kind ‖ NUL ‖ content` (B4) and the raw `NUL ‖ content` domain never alias;
`calls == hits + misses + bypassed` on every thread; a count above
`u32::MAX` is returned uncached ([`:135-137`][tc-u32]); the constant summed
into [`DECLARED_RETAINED_RESIDENT_BYTES`][declared] is recomputed from the
new shard count, generation count, and cap; and inside a pass every token
estimate that feeds a budget, a threshold, or a persisted `token_count` is
obtained through the injected `estimate_tokens` parameter of
[`apply_once`][ao-sig] or through [`cached_estimate_tokens`][tc-cet], so it
is counted in `tokenize_calls`. The SOFT predicate uses the injected function;
tag minting and nudge derivation call the existing cached estimator directly.
Serialization performs no token estimate. `always` because
[`transform_with_projection_cached`][tc-inject] passes
`cached_estimate_tokens` on every pass and the declaration's [doc][declared-doc]
says the runtime bound holds only when the declaration is truthful.
Fault/timing angle: Two sessions miss on the same digest at once
([concurrent misses may tokenize twice][tc-concurrent]); a generation
rotation at [`GENERATION_CAP = 65_536`][tc-cap] while a promote-on-hit insert
runs; a sharded replacement that omits its term from the declaration.
The source scan rejects direct tokenizer paths in production `transform.rs`;
it is not a whole-call-graph proof. Runtime spies cover the SOFT measurement
inputs and cache counters cover tag/nudge paths.
Required faults and enabling state: Two concurrent transform passes; 65_536
distinct digests; a pass minting new tags on the tail; a pass whose SOFT
predicate crosses a threshold.
Confidence: high - [Evidence](evidence/token-cache-is-a-pure-declared-memo-behind-one-estimator-interface.md).
The [module doc][tc-doc], the global [`Mutex`][tc-static] over two
generations, [`RETAINED_BYTES_BOUND`][tc-bound], [`count_with_digest`][tc-cwd],
and estimator routing are source-verified. The sharding note at
[`:112-113`][tc-shard] is conditional and cites no measurement.
Existing check: [Wildcard checks](existing-checks.md#wildcard-and-cross-cutting)
list the token-cache tests and
[`production_transform_module_has_no_global_estimator_bypass`][t-bypass], plus runtime checks
for SOFT, tag/nudge accounting, and serialization; all unaudited.
Impact: A rendered byte or budget decision changes, or the resident-memory
declaration undercounts a cache.
Open questions:
- Is there any measured lock contention at HEAD? The comment is conditional;
  the audit should measure before sharding.
- Accounting scope is resolved: tag and nudge counts use the existing cache;
  no forwarding parameters or additional retained memo are introduced.

### bounded-secret-scan-finds-every-whole-input-finding

Type: safety
Reachability: default-production
Status: active
Exercised: partial - Preselection soundness on sixteen inputs, provider
canaries, pinned evaluator constants, and redaction window tests exist; none
compares a bounded scan to the whole-input scan.
Guarantee: A bounded scan never drops a detection the current scan reports,
and the audit trail identifies which evaluator produced each row.
Check: `always` - For every rule set, profile, limits, and input, a scan that
restricts regex evaluation to regions around anchor hits returns the same
`findings` (same spans, same order), the same `limits_hit`, and the same
`semantic_digest` as the whole-input scan, retained as a reference
evaluation mode or as a frozen copy of [`evaluate`][eval] at HEAD so the
comparison survives the change; if any input can differ,
[`REVISION.semantic_digest_version`][revision] is bumped. `always` because
every committing pass prepares `meta` and `core_state` through the durable
redaction path ([`content`][ms-content], C4).
Fault/timing angle: None; a data-shape difference. [`evaluate`][eval] runs
[`preselect`][preselect] over the whole input with an
[ASCII-case-insensitive Aho-Corasick][anchor-ci] automaton, then for each
selected rule runs [`captures_iter(bytes)` over the whole input][captures];
`radius` bounds only the context window around the full match at
[`:266-297`][radius-window]. Rule validation checks radius bounds only
([`:598-602`][radius-valid]); nothing requires an anchor to occur in every
match, and for rules whose anchor is not a substring of the regex (such as
[`airtable-personnal-access-token`][airtable]) the binding comes from
`keywords_any` or `must_contain`. Artificial slice edges change `\b`, `^`,
and `$`, which [`redaction.rs`][edge-margin] defers with an edge margin;
candidate counting order decides which findings a `Candidates` or `Work`
limit drops ([`ScanLimits::DEFAULT`][limits]).
Required faults and enabling state: An input where a rule's match and its
nearest anchor hit are separated by more than the region bound; a match
ending within `EDGE_MARGIN_BYTES` of a region edge; an input reaching
`max_candidates` so evaluation order matters; an input over
`MAX_REDACTABLE_BYTES` so redaction windows and region bounds compose.
Confidence: high - [Evidence](evidence/bounded-secret-scan-finds-every-whole-input-finding.md).
The evaluator, [`MAX_MATCH_BYTES`][max-match], [`MAX_RULE_RADIUS`][max-radius],
the digest binding ([rules.rs:382][digest-doc]), and the memory store's
[per-batch digest persistence][ms-digest] are source-verified. The rule-set
[`radius` description][rules-radius-doc] describes a design the code does not
implement; a bounded scan is judged against this record, not against that
doc.
Existing check: [Wildcard checks](existing-checks.md#wildcard-and-cross-cutting)
list preselection soundness, canaries, the one-case qualification fixture,
pinned constants, window placement, and the single-path test; all unaudited.
Impact: A secret the current scan redacts persists, and old and new audit
rows become indistinguishable.
Open questions:
- Is bounding wanted at all, given the redaction windows already cap one scan
  at `MAX_REDACTABLE_BYTES`? (needs human input)
- A per-rule proof that every match contains an anchor or is keyword-bound
  within `radius` does not exist; it would be a new rule-set test, not a
  property of the evaluator.

### historian-firing-input-is-preserved-by-cheaper-construction

Type: safety
Reachability: default-production
Status: active
Exercised: partial - The frozen construction corpus compares owned boundary
inputs, length-only snapshots, exact transcript and prompt bytes, and refusal
behavior. It also checks a frozen-size lookup hit through a borrowed block ID.
The scripted producer pins captured prompts on two delta lanes.
The fingerprint literal, three truncation differentials, golden, and marker
test also pass. No test crosses a binary upgrade during an in-flight firing.
Guarantee: The historian receives the same prompt bytes, and the durable
chunk fingerprint still matches across restart.
Check: `always` - [`truncate_historian_input_if_needed`][trunc] returns bytes
identical to the frozen reference in
[`historian_truncate_differential.rs`][diff-ref] (same cut point, same
marker), and a snapshot item carrying only the UTF-8 byte length yields the same
[`compute_chunk_fingerprint`][fp] string as one carrying the bytes. `always`
because the fingerprint is computed on every firing and its string is durable
state.
Fault/timing angle: A fingerprint format change lands while a firing is in
flight across a restart, so the stored string no longer equals the recomputed
one and publication fails with `FingerprintMismatch`. The [snapshot][snap-build]
stores `byte_len: block.bytes.len()` without retaining content. Its
[`as_item`][as-item] view feeds the fingerprint, which writes the same UTF-8
length into the literal `id:kind:len|...`, stored in
[`HistorianDurableState.chunk_fingerprint`][fp-field] and compared by
[`verify_chunk_fingerprint`][fp-verify] and the [publish predicate][fp-predicate].
Truncation binary-searches UTF-16 unit positions with an uncached
`estimate_tokens` per probe; the differential's [header][diff-header] says the
probe sequence must be preserved because token counts are not monotonic in
prefix length.
Required faults and enabling state: A restart with an in-flight historian
firing; a chunk whose text exceeds `token_budget`, which needs a large
session.
Confidence: high - [Evidence](evidence/historian-firing-input-is-preserved-by-cheaper-construction.md).
The snapshot, the fingerprint, its two comparisons, and the [truncation
call][trunc-call] are source-verified. [Boundary construction][boundary-view]
borrows block IDs and shares the projection's original `Arc<str>` allocation.
The snapshot owns IDs and kinds because assembled firings outlive the
projection; rendering reads borrowed flat blocks, not snapshot content.
Existing check: [Wildcard checks](existing-checks.md#wildcard-and-cross-cutting)
list the fingerprint test, the production-window, exact-budget, and
small-window differentials, the golden, the marker test, the [construction
corpus][construction-corpus], and the [producer capture][firing-capture]; all
unaudited.
Impact: Historian prompt bytes change, or an in-flight firing fails
publication after a restart.
Open questions: None. Exact bytes, including the truncation probe sequence,
remain required; this construction change does not relax that contract.

### cron-next-occurrence-matches-the-minute-stepper

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: partial - Vixie semantics, extreme instants, and a fixture-zone
golden exist; none covers DST or an unsatisfiable expression.
Guarantee: A field-jumping search never fires a schedule at a different
instant, never fires one the stepper would never fire, and never runs past
the cap.
Check: `always` - For every expression [`parse_cron`][parse] accepts, every
`after_ms`, every `max_search_ms`, and both `Utc` and `Local` across DST
transitions, a replacement returns the same `Option<i64>` as a frozen
test-only copy of [`next_occurrence`][stepper] as it reads at HEAD (the
function itself is replaced by the change and cannot remain the oracle),
including `None` for
unsatisfiable-but-valid expressions within the cap and `None` at
unrepresentable instants; [`next_cron_occurrence`][occurrence] keeps the
`ms != 0` filter. `always` because the scheduler consumes the value directly
as its due time.
Fault/timing angle: The stepper evaluates local civil fields for each epoch
minute, so a spring-forward gap skips that day and a fall-back overlap
matches the earlier instant; `parse_cron` accepts day-of-month 1..31
independent of month, so `0 0 30 2 *` is valid and unsatisfiable and pays
the full [`MAX_SEARCH_MS`][cap] (4 x 366 days) on the scheduler task
([`next_due`][sched-due]); [`matches_day`][vixie] implements Vixie OR
semantics.
Required faults and enabling state: A `Local` zone with DST; expressions such
as `30 2 * * *` on spring-forward, `0 0 30 2 *`, `0 0 31 4,6,9,11 *`, and
`0 0 29 2 *`; `after_ms` at the `i64` extremes.
Confidence: high - [Evidence](evidence/cron-next-occurrence-matches-the-minute-stepper.md).
The parser, the stepper, the cap, the smart-note ceiling
([`:236-239`][note-cap]), and the config acceptance
([`is_valid_smart_note_cron`][valid] at [config.rs][sched-accept]) are
source-verified.
Existing check: [Wildcard checks](existing-checks.md#wildcard-and-cross-cutting)
list the Vixie test, the extreme-instant test, and the golden; all unaudited.
Impact: A scheduled task fires at the wrong instant or never.
Open questions:
- Should validation reject calendar-impossible dates instead? That is a
  config-acceptance change outside a latency change. (needs human input)

### effective-config-reads-observe-a-tier-change-by-the-next-pass

Type: safety
Reachability: default-production
Status: active
Exercised: partial - The mtime cache and the three raise-only tests exist;
none covers override staleness or two project roots sharing one cache.
Guarantee: A config edit takes effect within one pass, as today, and a
project tier can never gain privilege through a stale or cross-project cache
entry.
Check: `always` - After the user tier, the project tier, or the configured
guidance override file changes on disk (mtime advance for the tiers; any
content change for the override, which HEAD re-reads on every call), the
next [`effective_config`][eff-cfg] call for that `project_root` returns the
merge of the new contents; a cache is keyed by `(user_path, project_root)`
and mtimes; [`merge_tiers_with_warnings`][merge] including
[`ProjectRaiseOnly`][raise-only] is applied to the fresh contents unchanged;
and bind-frozen `SessionBinding.config` stays frozen. `always` because
[`prepare_historian_fire`][call-fire] calls it on every non-subagent pass
whose state load succeeds, has no `pending_rewrite`, and has no live
historian completion pending (`lib.rs:8234`, `:5013-5051`), and
[`bind`][call-bind] calls it on every route bind; the check is on each call,
not on each pass.
Fault/timing angle: A tier edit between two passes; two routes on different
project roots alternating, which re-reads the file every call because one
`ConfigCache` holds one project tier ([`read_tier_cached`][tier-cached]);
the override edited without touching a tier
([`resolve_user_guidance_override`][guidance] does a full read per call with
no mtime gate). The staleness contract at HEAD already ignores a same-mtime
edit.
Required faults and enabling state: A user or project tier rewritten with a
later mtime; `prompt_surface.guidance_override_path` configured and its file
edited; two project roots bound at once.
Confidence: high - [Evidence](evidence/effective-config-reads-observe-a-tier-change-by-the-next-pass.md).
[`effective_for_project`][eff-proj], [`effective_with_warnings`][eff-warn]
with its deep clone at [`:288`][eff-clone], the per-pass callers
([`:4788`][call-reattach], [`:5049`][call-fire], [`:5366`][call-wrapup]), and
the bind freeze ([`:11786`][call-bind], [binding doc][binding-doc]) are
source-verified.
Existing check: [Wildcard checks](existing-checks.md#wildcard-and-cross-cutting)
list the mtime test and the three privilege tests; all unaudited. Unit tests
built with `fixed_config` (`lib.rs:3859`, returned at `:4557-4560`) bypass the
cache and cannot exercise this record.
Impact: An edit is ignored for the life of the daemon, or a project tier
reads another project's cached values.
Open questions:
- The doc at [config.rs:266-267][eff-warn-doc] says tier read failures are
  reported on every load so a long-running daemon keeps surfacing them; a
  merged cache silences the repeat. Keep that behavior? (needs human input)
- Should the historian read the bind-frozen config, removing the per-pass
  merge entirely? (needs human input)

### committed-transform-bookkeeping-is-applied-or-recomputed

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No test aborts between commit and bookkeeping and
inspects the next pass's derived inputs.
Guarantee: A committed transform's derived in-memory state is applied on the
pass that committed it or provably recomputed on the next pass for that
session.
Check: `always` - After any pass whose `commit_transform` succeeded, at the
next pass's `ProducerContext` construction for the same session:
`transform_session_roots` contains the lineage root, or
[`module_knows_transform_session`][knows] repopulates it from the durable
table; the projection cache holds the entry [`store_projection_cache`][store-pc]
would have stored, or the next pass takes the full projection path;
`guidance_dates` holds no entry for the session, or the next pass's
`ProducerContext.guidance_date` ([`:8173`][guidance-use]) equals a fresh
computation; and the serialized-output cache holds no entry from a pass the
store rejected, which the transform catalog's
[output-cache record][tc-output] already constrains. `always` because the
four updates run on every committing pass whatever the execution topology.
Fault/timing angle: On the ordinary path there is no `.await` between the
commit at [`:8200`][commit-call] and the bookkeeping at
[`:8207-8212`][roots-insert], [`:8385-8392`][pc-store], and
[`:8396-8401`][guidance-remove]; the awaits at `:8263`, `:8289`, and `:8315`
sit inside the Emergency95 branch. Moving `run_transform` to
`spawn_blocking` introduces an await after the commit, and a blocking task
cannot be cancelled once started, so an abort landing there leaves the
transform committed and the bookkeeping skipped. Lineage roots self-heal
([roots doc][roots-doc]); `guidance_dates` does not, because
[`guidance_date_for_transform`][guidance-fn] returns the pinned entry until
it is removed. The serialized-output cache is replaced inside
`run_transform` after the store commit, so a relocation of the whole closure
does not separate them; a relocation that splits the closure does.
Required faults and enabling state: A committing pass with a pinned guidance
date; an abort injected between commit and bookkeeping (W11); a second pass
on the same session that reads `ProducerContext.guidance_date` and the
projection cache.
Confidence: high - [Evidence](evidence/committed-transform-bookkeeping-is-applied-or-recomputed.md).
The commit call, the three post-commit updates, the self-healing lookup, the
non-healing guidance read, and the Emergency95 awaits are source-verified.
Existing check: none found for derived-state consistency after an abort;
[E1][e1] and [E2][e2] cover route state and resource charges, not these
structures.
Impact: The next pass carries a stale guidance line or a stale projection
cache entry after a relocated transform is aborted mid-bookkeeping.
Open questions:
- What owns and joins any proposed off-worker transform work? This is the
  parent's E1 question and remains unresolved. (needs human input)

### soft-pressure-refold-predicate-preserves-its-classification

Type: safety
Reachability: default-production
Status: active
Exercised: yes - [Forty-eight real SOFT evaluations][soft-threshold-check]
compare classification with a frozen direct-tokenizer reference at the budget
share, m0 floor, and m0 ratio boundaries. An injected spy records the exact
texts and checks cached/direct equality. [Absent m0 and placeholder
checks][soft-gates-check] cover measurement gates at the predicate seam.
Guarantee: The SOFT pass's pressure-refold classification is preserved for
fixed frozen m0, composed m1, and budget on production-reachable inputs.
Check: `always` - For every SOFT pass, `pressure_refold` equals a frozen copy
of the [frozen predicate][soft-reference], evaluated with the uncached
`tokenizer::estimate_tokens` on the frozen m0 payload and on the composed
`m1.body`: `m1` has content and
`m1_tokens > history_budget_tokens * 0.20` with a positive budget, or `m1`
has content and `m0_tokens >= 500` and `m1_tokens > m0_tokens * 0.15`;
`m1_tokens` is `0` when `m1.body == M1_PLACEHOLDER`, and `m0_tokens` is `0`
when no frozen unit has key `m0`. A cache-backed count may replace the direct
call only if it equals the direct count for the exact text at each observed
comparison (H1's cache clause). `M1Composition` carries no memory update count;
composition has no writer for one, and the dead update-count disjunct is
removed by the explicit owner decision recorded in the evidence. The frozen
reference retains the original expression and takes that count as a parameter
the callers set to `0`, so its reachable classifications stay fixed.
`always` because the classification selects
between an ordinary SOFT and a rematerialized m0, which H2 states as distinct
boundaries.
Fault/timing angle: A cached or estimated count crosses a threshold the exact
count does not, or the reverse, at a boundary value; a batched estimate
reuses a count for a different m1 body.
Required faults and enabling state: SOFT passes with `m1_tokens` at the 0.20
budget boundary; `m0_tokens` at 499 and 500 with
`m1_tokens` at the 0.15 boundary; an `m1.body` equal to the placeholder; a
store with no `m0` frozen unit.
Confidence: high - [Evidence](evidence/soft-pressure-refold-predicate-preserves-its-classification.md).
The [predicate][soft-predicate], injected measurements, and placeholder guard
are source-verified. The frozen reference and full-pass characterization run
precede production edits.
Existing check: [Threshold comparison][soft-threshold-check]
and [measurement gates][soft-gates-check]; adequacy remains unaudited.
Impact: A pass refolds m0 when the reference would keep it, or keeps it when
the reference would refold, changing prompt content and frozen bytes.
Open questions: None for this preservation change. The owner authorizes only
removal of the dead update-count disjunct. Constants 0.20, 0.15, and 500 and
their strict/non-strict comparisons remain fixed.

### soft-pressure-refold-thresholds-are-each-crossed

Type: reachability
Reachability: default-production
Status: active
Exercised: yes - The [threshold campaign][soft-threshold-check] records all
three remaining witness conditions from independently measured inputs before
comparing the actual classification, then asserts that all were seen. The
update-count marker is retired by
the explicit owner decision in the evidence, not reported as exercised.
Guarantee: A SOFT-preservation campaign crosses each of the two pressure
conditions independently, so W9 cannot pass on passes that never approach a
boundary.
Check: `sometimes` - Under three constant markers, each pressure condition
holds on at least one SOFT evaluation with the other condition false:
`soft-pressure-refold-thresholds-are-each-crossed-budget-share` records
`m1` has content, `history_budget_tokens > 0`, and
`m1_tokens > history_budget_tokens * 0.20`;
`soft-pressure-refold-thresholds-are-each-crossed-m0-ratio` records `m1` has
content, `m0_tokens >= 500`, and `m1_tokens > m0_tokens * 0.15`; and
`soft-pressure-refold-thresholds-are-each-crossed-below` records a SOFT pass
with content where both are false. Each marker asserts the independently
measured inputs, not `pressure_refold` itself.
Fault/timing angle: Small fixtures keep m1 short, so
every SOFT pass takes the ordinary branch.
Required faults and enabling state: An m1 body whose exact token count
exceeds a fifth of a small positive budget; a frozen m0 of at least 500
tokens with an m1 above 15 percent of it; direct token counts recorded before
the candidate runs.
Confidence: high - [Evidence](evidence/soft-pressure-refold-thresholds-are-each-crossed.md).
The two active disjuncts and their inputs are source-verified. The campaign
uses a real store and composed m1, with a constructed frozen m0 payload of
exactly 499 or 500 tokens; it is not production-workload evidence.
Existing check: [Threshold comparison and three witnesses][soft-threshold-check];
adequacy remains unaudited.
Impact: W9 passes without any boundary being approached.
Open questions: None. The update-count arm and its reachability obligation
are retired. The evidence retains the discovery snapshot and decision
provenance.

### abort-lands-between-transform-commit-and-bookkeeping

Type: reachability
Reachability: test-only
Status: active
Exercised: not yet - The only seam after commit is the `#[cfg(test)]` hook,
and no test aborts inside it.
Guarantee: A relocation campaign reaches the window in which a transform is
committed and its in-memory bookkeeping has not run.
Check: `sometimes` - For some pass, `commit_transform` has returned success
and the handler's abort (cancellation, route close, or generation
retirement) is observed before [`:8207`][roots-insert] executes. The marker
asserts the commit and the abort ordering, not the next pass's state.
Fault/timing angle: At HEAD the window exists only in the Emergency95 branch
at the awaits `:8263`, `:8289`, and `:8315`; it exists on every pass once a
blocking worker separates commit from bookkeeping.
Required faults and enabling state: The [`between_transform_and_prepare`][hook]
hook, or its relocation-era equivalent, triggering an abort; independent
observation of the commit through the store's `row_version`.
Confidence: medium - [Evidence](evidence/abort-lands-between-transform-commit-and-bookkeeping.md).
The window and the hook are source-verified; a production seam does not
exist at HEAD.
Existing check: none found.
Impact: W8 passes without the window ever opening.
Open questions:
- Which abort events can a future worker expose without equating waiter
  cancellation with completion? This is the parent's E3 question. (needs
  human input)

### worker-thread-panics-stay-inside-the-redaction-boundary

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - No test panics inside a `spawn_blocking` closure entered
from a daemon handler and reads the worker thread's stderr; the host-runtime
redaction tests panic on the runtime worker only.
Guarantee: Moving handler work to a worker thread never moves a panic out of
the redacting hook and never changes the terminal the request settles with.
Check: `always` - For every panic raised on a worker thread that carries
request-derived bytes (a `spawn_blocking` closure entered from a handler,
including a relocated transform), the hook that runs on the panicking thread
is the redacting hook: stderr receives exactly
[`REDACTED_DIAGNOSTIC`][pb-redacted] and no byte of the panic payload, the
panic message, or a backtrace; and the request settles with the same terminal
code as an in-handler panic on the runtime worker, which at HEAD is
`internal_error` with `handler request task failed`
([`dispatch.rs:985-989`][panic-terminal]). `always` because the hook decides
per panic from the panicking thread's [`CALLBACK_POLL_DEPTH`][pb-tls]
([`callback_is_polling`][pb-polling], the branch at [`:40-47`][pb-hook]), so
every worker-thread panic is either redacted or forwarded; the check is on
the thread the panic runs on, not on a defect.
Fault/timing angle: The guard is a thread-local depth counter
([`panic_boundary.rs:11-13`][pb-tls]) that [`redact_sync`][pb-sync] and
[`redact`][pb-async] raise on the thread that constructs and polls the
handler future ([`dispatch.rs:928-934`][wrap-callback]). A
`tokio::task::spawn_blocking` worker never runs either, so its depth is the
initial `0` and the hook forwards the full panic info to the previously
installed hook ([`:46`][pb-hook]), the Rust default because the only other
`std::panic::set_hook` in the tree is in a test
([`tests/dispatch.rs:635`][t-panic-child]). Tokio catches the unwinding panic
as a `JoinError` after the hook has printed. At HEAD the daemon runs kernel
work this way through [`kernel_routes::blocking`][blocking] at eight sites
([`commit.rs:1013`][blk-commit-preview], [`:1038`][blk-commit-run],
[`egress.rs:201`][blk-egress], [`eligibility.rs:346`][blk-eligibility],
[`ingest.rs:796`][blk-ingest-decode], [`:797`][blk-ingest-finish],
[`read.rs:291`][blk-read-gate], [`:311`][blk-read-rows]) and directly at
[`health.rs:224`][spawn-health], [`mod.rs:358`][spawn-kernel-open], and
[`lib.rs:3808`][spawn-store-open]. The ingest finish closure calls
[`store.ingest_artifact`][route-ingest] with the upload payload and the page
decode closure holds the base64 page; a relocated transform carries the whole
request. The same boundary crosses the token-cache counters
([`token_cache.rs:57`][tc-local], W2); the third `thread_local!` in the
inspected crates is test-only ([`transform.rs:495-498`][tl-test]).
Required faults and enabling state: A panic injected inside the relocated
work (a `kernel_routes::blocking` closure at HEAD, or the relocated
transform) whose payload carries a unique long sentinel that appears nowhere
else in the process; the process's stderr captured from a child process, on
the pattern of
[`handler_panic_payload_is_redacted_from_process_stderr`][t-panic-stderr];
the terminal frame the request settles with, recorded beside it; a second
injection on the runtime worker for the same route as the control.
Confidence: high - [Evidence](evidence/worker-thread-panics-stay-inside-the-redaction-boundary.md).
The thread-local, the hook branch, the two guard entry points, the handler
wrap, the eight `blocking` sites, the three direct `spawn_blocking` sites,
and the `JoinError` mappings are source-verified. The terminal clause
disagrees with HEAD for kernel routes: `blocking` maps a worker panic to
`KernelOutcome::unavailable(StoreUnavailable)` ([`mod.rs:467`][blocking]),
which the route returns as a `Response` body
`{"kind":"unavailable","reason":"store_unavailable"}` by the documented
intent at [`:460-461`][blocking-doc], not as an `internal_error` terminal;
`open_once` maps it to `KernelError::Fault` after an `eprintln!`
([`mod.rs:360-362`][spawn-kernel-open]); the store opener re-panics
([`lib.rs:3810`][spawn-store-open]). Both sides are cited; the record does not
resolve which terminal is the contract.
Existing check: [Wildcard checks](existing-checks.md#wildcard-and-cross-cutting)
list the three host-runtime redaction tests
([`a_handler_panic_maps_to_one_redacted_internal_error`][t-panic-internal],
[`handler_panic_payload_is_redacted_from_process_stderr`][t-panic-stderr],
and its [child][t-panic-child]); all panic on the runtime worker inside the
guard; none found for a worker-thread panic in `crates/daemon/tests/` or in
`kernel_routes`; all unaudited. The host-runtime records
[every-callback-invocation-is-inside-the-redaction-guard][hr-redact] and
[the-panic-hook-cannot-itself-fail][hr-hook] enumerate host call sites and
the hook's own failure; neither reaches a daemon-spawned thread.
Impact: A panic message or payload built from request bytes, or a backtrace
naming them, reaches stderr unredacted; a relocated transform also settles
with a different terminal than today.
Open questions:
- Which terminal is the contract for a worker-thread panic: the host's
  `internal_error`, or the kernel routes' documented
  `{"kind":"unavailable","reason":"store_unavailable"}` response? A relocated
  transform must pick one. (needs human input)
- Does the relocation carry the guard across (enter `redact_sync` inside the
  closure) or move the redaction decision to a process-wide rule? Either
  changes the inventory in
  [every-callback-invocation-is-inside-the-redaction-guard][hr-redact].
  (needs human input)

### cron-schedule-is-evaluated-for-a-configured-project

Type: reachability
Reachability: explicit-config-only
Status: active
Exercised: partial - The scheduler tests drive [`next_due`][sched-due]
through a [`ScriptedHost`][sched-scripted] with a `*/15 * * * *` project and
a [`ManualClock`][sched-clock]
([`a_task_runs_only_once_its_cron_instant_has_passed`][t-sched-cron] and its
siblings); no campaign marker records that a tick evaluated a configured
cron, and the smart-note consumer has the fixture-zone golden only.
Guarantee: A schedule-preservation campaign reaches a scheduler tick that
evaluates a configured cron, so W6's differential runs on instants the
scheduler consumes rather than on a scheduler that never sees a schedule.
Check: `sometimes` - Under one constant marker
`cron-schedule-is-evaluated-for-a-configured-project`, for some
[`DreamerScheduler::tick`][sched-tick], `scheduled_projects()` returns at
least one project with a non-empty `schedule`, and
[`due_projects`][sched-due-projects] calls [`next_due`][sched-due] for it
with a result other than `i64::MAX`. The marker records the project, the
schedule string, `now_ms`, and the computed instant, not whether the slot
then ran or what W6's differential returned. `sometimes` because the tick
lines execute every [`IDLE_POLL`][sched-idle] on a default campaign while the
list is empty, so `reachable` would be trivially satisfied.
Fault/timing angle: The schedule [defaults to `None`][sched-default], and
[`scheduled_projects`][sched-projects] drops any project whose schedule is
`None` (`schedule: schedule?` at [`:14027`][sched-filter]) or whose memories
authority is not `MODULE` ([`:14002-14007`][sched-authority]), so a default
campaign hands the scheduler an empty list and `next_due` is never called;
W6's clauses then hold on no instant.
Required faults and enabling state: A user tier with
`dreamer_review_user_memories_schedule` set to an expression
[`is_valid_smart_note_cron`][valid] accepts, a bound route whose project is
in `MODULE` memories authority, and a tick at a `now_ms` for which the
expression has an occurrence within [`MAX_SEARCH_MS`][cap]; or, in a unit
test, a `ScriptedHost` project built by the [`project`][sched-fixture]
helper, which activates `MODULE` authority on a real store, and a
`ManualClock` advanced past the instant.
Confidence: high - [Evidence](evidence/cron-schedule-is-evaluated-for-a-configured-project.md).
The filter, the `next_due` call sites, the default, and the fixtures are
source-verified.
Existing check: [Wildcard checks](existing-checks.md#wildcard-and-cross-cutting)
list the scheduler cron test beside the stepper tests; all unaudited; none
records a marker.
Impact: W6 passes on a scheduler that never evaluates a schedule.
Open questions: None.

## Relationships and retained canonical obligations

- A1 owns the magnitude and ordering of the parse charge; the parent's
  [E2][e2] owns its lifetime. A1 does not restate E2's resource ledger. A2
  and A3 retain the host catalog's terminal-frame contract
  ([req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame][hr-terminal]);
  the wire contract leaves routed bodies opaque and states only the
  `invalid_params` cap codes ([§6.3][wire63]).
- B1 through B5 state the equivalences that the transform catalog's
  [synthetic-strip-precedes-every-coverage-read][tc-synthetic] and
  [speculative-tag-numbering-has-two-authorities][tc-tagnum] assume; they add
  the observers outside `apply_once` and the cache-entry immutability, not a
  second statement of those records. B3 assumes the prepared-field bytes that
  land are unchanged, which [R1][r1] owns. B4 and W3 share the hygiene key
  domain; B4 owns the digest input, W3 owns non-aliasing and the memo.
- C1 through C5 sit above the memory-store and shared-primitives catalogs.
  Fencing, `synchronous=FULL`, snapshot isolation, and CAS remain with
  [fenced-write-is-atomic][atomic-old] and
  [durable-identity-decision-is-made-inside-the-write-transaction][identity-old];
  callback batching boundaries remain with [S2][s2]; C4 refines
  [preserved-identity-name-does-not-exempt-its-value][ms-preserved] and
  [refused-durable-write-leaves-no-row-and-no-receipt][ms-refused] for the
  `meta` blob; C2 hands its owner-relationship question to [R3][r3]. C6 is
  the occurrence witness for C3 and asserts only that due rows exist at a
  pass drain; C3 keeps every delivery clause.
- P1 through P5 have no prior catalog; `docs/properties/` has no plugin part
  at this HEAD (the README assigns `cli` and `historian-ts` to a later wave).
  P3 shares the 512 KiB and 32 MiB constants with A1 and A2.
- T1 refines, and does not replace, the shm-transport catalog's
  [reclamation-excludes-pages-with-live-wrapped-bytes][shm-punch] and
  [trim-removes-only-dead-pages-below-the-write-cursor][shm-trim]; T2 refines
  [no-rust-reference-over-peer-writable-payload][shm-noref]; T3 refines
  [E2][e2] for the direct arm and retains the host catalog's
  [publication-failure record][hr-pubfail].
- G1 through G3 have no CAS accounting predecessor; the fault-injection
  oracle is the existing check, not a record.
- W2 and W8 state relocation hazards beside the parent's [E1][e1] and
  [E3][e3]; W8 answers the parent's queued gap on commit-versus-bookkeeping
  and cites the transform catalog's
  [output-cache-replace-trails-the-accepted-commit][tc-output] for the fourth
  structure. W9 and W10 answer the queued gap on the SOFT predicate and sit
  beside [H1][h1] and [H2][h2] without importing their history-render
  reference. W3 retains
  [tokenizer-encoding-matches-the-independent-oracle][tokenizer-old]. W1
  constrains no code; it names what every other record's "faster" claim
  needs. W13 is the occurrence witness for W6's scheduler consumer and
  asserts only that a configured cron reached `next_due`; W6 keeps the
  differential.
- W12 states the diagnostics-preservation obligation across the worker-thread
  boundary that the parent's [E1][e1] and [E3][e3] treat for charges and
  lifecycle, and that G1 and G2 cross at the ingest finish. It retains the
  host-runtime catalog's
  [every-callback-invocation-is-inside-the-redaction-guard][hr-redact] for
  the host's own call-site inventory and
  [the-panic-hook-cannot-itself-fail][hr-hook] for the hook's failure modes;
  W12 adds the daemon-spawned thread those records do not reach.

These relationships identify shared mechanisms, not proven dominance. No
record here supersedes a parent or sibling record.

## Per-record handoff

Every active record goes to `/testing:test-strategy` for its form and
oracle. The following notes define the evidence to request, not tickets.

| Record | Handoff focus |
| --- | --- |
| [A1][a1] | Drive `Handler::handle` directly with oversize and pool-short bodies; measure retained copies from the typed request. |
| [A2][a2] | Build the discriminator and decode corpus once; run it through both lanes and the probe. |
| [A3][a3] | Construct concurrent parses with a barrier so the shortfall is observable. |
| [B1][b1] | Selection inputs, native prefix chunks, snapshot fallback, sidecar order/pins, and canonical `Served` segment writes are checked. Broader cross-process coverage remains separate. |
| [B2][b2] | Typed-flag reference and delta comparisons run. Extend the finite observer corpus when new replay shapes appear. |
| [B3][b3] | Failed-commit rollback and clean-source equality are exercised. Resolve equality semantics for detected secrets. |
| [B4][b4] | Digest separation and memo correctness checks pass; the controller's local payoff run is pending. |
| [B5][b5] | The constructed delta-turn witness fires. A production plugin body remains unobserved. |
| [C1][c1] | Pair a narrow read with `MemoryStore::load` over malformed and defaulted rows; observe the post-commit `row_version`. |
| [C2][c2] | Inject a `pass_trace` failure beside a commit; count breadcrumbs across reject, stable, and rerun passes. |
| [C3][c3] | Crash between mark and delete; run two drainers on one session; pin order, limit, and backoff. |
| [C4][c4] | Compare clean stored `meta` byte-for-byte; seed duplicate names and `BTreeMap`-key secrets. |
| [C5][c5] | Reuse the interleave hook as the campaign marker. |
| [C6][c6] | Fail the inline delivery for all three kinds, then drive a pass past the backoff and record the due rows before the drain delivers. |
| [P1][p1] | Store a deny, then fail the read; switch agents between passes. |
| [P2][p2] | Frozen-reference differential over fixture and second-session states; native reuse/invalidation checks on both adapters and a transform-hook replacement witness. |
| [P3][p3] | Run a lone-surrogate body through paging and the writer; attempt a boundary body under both measures. |
| [P4][p4] | Place any gate where spies and the sanitizer still see every written line. |
| [P5][p5] | Sequence deny-then-fail through an SDK fake. |
| [T1][t1] | Assert the dead-byte inequality over a long publish and release run; decide the deferred bound first. |
| [T2][t2] | Add a mismatched-span `to_vec` case under Miri. |
| [T3][t3] | Cover underfill, overflow, error, and panic on the direct path; decide the failure classification and charge class. |
| [T4][t4] | Hold egress so a direct frame outlives its handler; observe completion and commit separately. |
| [G1][g1] | Dedup at exactly the cap; second ingest before orphan recovery. |
| [G2][g2] | Keep the walk as the oracle; compare a counter against it at every quiescent point. |
| [G3][g3] | Record a usage delta per decrement path with bytes at stake. |
| [W1][w1] | Choose the size class and the manifest form; run through `Handler::handle`. |
| [W2][w2] | Tie the TypeScript key list to the Rust struct; assert what each field brackets. |
| [W3][w3] | Whole-module scan and SOFT/tag/nudge accounting checks exist. Recheck the declared-bytes sum if cache sizing changes. |
| [W4][w4] | Differential bounded-versus-whole over rules, limits, and edge-margin inputs. |
| [W5][w5] | Reuse the frozen reference; add a format-change-across-restart case. |
| [W6][w6] | Differential against the stepper over DST and unsatisfiable expressions. |
| [W7][w7] | Edit tiers and the override between passes; alternate two project roots. |
| [W8][w8] | Abort between commit and bookkeeping; read the next pass's derived inputs. |
| [W9][w9] | Frozen direct-estimator reference and threshold comparison exist; preserve them when changing pressure logic. |
| [W10][w10] | Three active witnesses exist; the dead update-count witness is retired with its disjunct. |
| [W11][w11] | Trigger an abort through the interleave hook or its successor. |
| [W12][w12] | Panic with a sentinel inside a `kernel_routes::blocking` closure from a child process; capture stderr and the terminal; repeat on the runtime worker as the control. |
| [W13][w13] | Give a `ScriptedHost` project a schedule and tick past its instant; record the project, schedule, `now_ms`, and computed instant at the `next_due` call. |

Timing schedules for C3, T3, T4, W8, and W11 may need
`/testing:deterministic-simulation-testing` after seam selection. Existing
test adequacy goes to `/testing:invariant-test-review`; production guards go
to `/low-level-systems:defensive-assertions-and-invariant-guards`. The fresh
evaluation of this area and its disposition are recorded in
`portfolio-evaluation.md` in this directory.

[a1]: #admission-chain-charges-before-decode-and-refuses-effect-free
[a2]: #route-and-typed-decode-are-independent-of-entry-path
[a3]: #scratch-pool-shortfall-reaches-the-parse-reservation
[b1]: #derived-artifacts-are-ownership-independent
[b2]: #synthetic-normalization-is-scoped-to-the-pass
[b3]: #tag-baseline-cache-entry-is-never-mutated-by-a-pass
[b4]: #hygiene-digest-is-kind-prefixed-part-content
[b5]: #replayed-synthetic-pair-arrives-unflagged-on-a-delta-turn
[c1]: #consolidated-cache-state-reads-match-per-consumer-loads
[c2]: #pass-trace-writes-count-every-pass-outside-the-cache-cas
[c3]: #side-channel-drain-delivers-each-row-once-and-keeps-its-schedule
[c4]: #meta-json-preparation-scans-every-persisted-byte
[c5]: #foreign-write-lands-between-pass-loads
[c6]: #side-channel-row-is-due-during-a-drain
[p1]: #cached-todowrite-verdict-never-lifts-a-deny-or-outlives-its-inputs
[p2]: #mid-turn-read-is-invariant-under-query-collapse-and-statement-caching
[p3]: #paged-body-measure-equals-declared-frame-length-and-fits-host-caps
[p4]: #log-lines-keep-sanitizer-and-file-hardening-guarantees
[p5]: #todowrite-deny-then-read-failure-is-exercised
[t1]: #arena-residency-is-bounded-by-admission-and-one-punch-batch
[t2]: #arena-payload-copies-keep-the-address-derived-atomic-shape
[t3]: #direct-frame-publishes-declared-length-or-nothing-and-holds-its-charges
[t4]: #direct-frame-outlives-its-handler-before-publication
[g1]: #artifact-admission-fails-closed-against-on-disk-object-bytes
[g2]: #reported-artifact-usage-equals-on-disk-object-bytes-after-recovery
[g3]: #artifact-byte-decrement-paths-are-exercised
[w1]: #optimized-stage-is-measured-at-production-shape
[w2]: #stage-timing-fields-keep-their-boundaries
[w3]: #token-cache-is-a-pure-declared-memo-behind-one-estimator-interface
[w4]: #bounded-secret-scan-finds-every-whole-input-finding
[w5]: #historian-firing-input-is-preserved-by-cheaper-construction
[w6]: #cron-next-occurrence-matches-the-minute-stepper
[w7]: #effective-config-reads-observe-a-tier-change-by-the-next-pass
[w8]: #committed-transform-bookkeeping-is-applied-or-recomputed
[w9]: #soft-pressure-refold-predicate-preserves-its-classification
[w10]: #soft-pressure-refold-thresholds-are-each-crossed
[w11]: #abort-lands-between-transform-commit-and-bookkeeping
[w12]: #worker-thread-panics-stay-inside-the-redaction-boundary
[w13]: #cron-schedule-is-evaluated-for-a-configured-project

[e1]: ../catalog.md#route-cleanup-waits-for-request-owned-physical-work
[e2]: ../catalog.md#request-work-accounting-covers-retained-resources
[e3]: ../catalog.md#request-close-overlaps-live-work
[s2]: ../catalog.md#callback-batching-preserves-observation-boundaries
[h1]: ../catalog.md#history-budget-selection-preserves-reference-bytes
[h2]: ../catalog.md#history-budget-boundaries-remain-distinct
[r1]: ../catalog.md#prepared-field-output-and-audit-policy-agree
[r3]: ../catalog.md#redaction-audit-does-not-depend-on-retained-payload
[tc-synthetic]: ../../daemon/transform/catalog.md#synthetic-strip-precedes-every-coverage-read
[tc-tagnum]: ../../daemon/transform/catalog.md#speculative-tag-numbering-has-two-authorities
[tc-output]: ../../daemon/transform/catalog.md#output-cache-replace-trails-the-accepted-commit
[tc-g2]: ../../daemon/transform/portfolio-evaluation.md
[shm-punch]: ../../shm-transport/catalog.md#reclamation-excludes-pages-with-live-wrapped-bytes
[shm-trim]: ../../shm-transport/catalog.md#trim-removes-only-dead-pages-below-the-write-cursor
[shm-noref]: ../../shm-transport/catalog.md#no-rust-reference-over-peer-writable-payload
[hr-terminal]: ../../host-runtime/catalog.md#req-a-an-admitted-routed-request-emits-at-most-one-terminal-frame
[hr-pubfail]: ../../host-runtime/catalog.md#req-a-a-response-publication-failure-never-reaches-the-settling-path
[hr-redact]: ../../host-runtime/catalog.md#every-callback-invocation-is-inside-the-redaction-guard
[hr-hook]: ../../host-runtime/catalog.md#the-panic-hook-cannot-itself-fail
[atomic-old]: ../../shared-primitives/catalog.md#fenced-write-is-atomic
[identity-old]: ../../memory-store/catalog.md#durable-identity-decision-is-made-inside-the-write-transaction
[ms-preserved]: ../../memory-store/catalog.md#preserved-identity-name-does-not-exempt-its-value
[ms-refused]: ../../memory-store/catalog.md#refused-durable-write-leaves-no-row-and-no-receipt
[tokenizer-old]: ../../tokenizer/catalog.md#tokenizer-encoding-matches-the-independent-oracle

[wire63]: ../../../host-wire-protocol.md#L308
[wire751]: ../../../host-wire-protocol.md#L440
[wire77]: ../../../host-wire-protocol.md#L666

[handle]: ../../../../crates/daemon/src/lib.rs#L11873-L11895
[bytecap]: ../../../../crates/daemon/src/lib.rs#L15541-L15557
[footprint]: ../../../../crates/daemon/src/lib.rs#L15496-L15524
[copies]: ../../../../crates/daemon/src/lib.rs#L15477-L15486
[toolarge]: ../../../../crates/daemon/src/lib.rs#L15527-L15532
[queuefull]: ../../../../crates/daemon/src/lib.rs#L15534-L15539
[probe]: ../../../../crates/daemon/src/lib.rs#L15390-L15398
[class]: ../../../../crates/daemon/src/lib.rs#L15470-L15481
[dispatch]: ../../../../crates/daemon/src/lib.rs#L12627-L12721
[pagefields]: ../../../../crates/daemon/src/lib.rs#L12723-L12727
[unrecognized]: ../../../../crates/daemon/src/lib.rs#L12743-L12767
[pageconst]: ../../../../crates/daemon/src/lib.rs#L758-L765
[freeze]: ../../../../crates/daemon/src/lib.rs#L8095-L8096
[routechan]: ../../../../crates/daemon/src/lib.rs#L8104-L8107
[accept]: ../../../../crates/daemon/src/lib.rs#L8132
[ticket]: ../../../../crates/daemon/src/lib.rs#L587-L644
[pageapply]: ../../../../crates/daemon/src/lib.rs#L9493-L9501
[testentry]: ../../../../crates/daemon/src/lib.rs#L12553-L12568
[wirestruct]: ../../../../crates/daemon/src/transform.rs#L809-L980
[wiremsg]: ../../../../crates/memory-store/src/lib.rs#L126-L143
[wireblock]: ../../../../crates/memory-store/src/lib.rs#L250-L264
[reserve]: ../../../../crates/host-runtime/src/handler.rs#L474-L484
[capacity]: ../../../../crates/host-runtime/src/handler.rs#L486-L491
[outcome]: ../../../../crates/host-runtime/src/handler.rs#L230-L235
[pools]: ../../../../crates/host-runtime/src/runtime.rs#L814-L822
[scratchconst]: ../../../../crates/host-runtime/src/config.rs#L21-L31
[paging]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.ts#L666-L676
[fixture]: ../../../../crates/daemon/tests/direct_host.rs#L285-L290

[cfg-compaction]: ../../../../crates/daemon/src/config.rs#L121
[expand]: ../../../../crates/daemon/src/lib.rs#L4190-L4282
[store-pc]: ../../../../crates/daemon/src/lib.rs#L4336-L4378
[historian-fire]: ../../../../crates/daemon/src/lib.rs#L5047
[assemble]: ../../../../crates/daemon/src/lib.rs#L5287-L5291
[ingress-chunks]: ../../../../crates/daemon/src/lib.rs#L13099
[gate-native]: ../../../../crates/daemon/src/lib.rs#L13155-L13160
[native-attach]: ../../../../crates/daemon/src/lib.rs#L13163-L13183
[native-diff]: ../../../../crates/daemon/src/lib.rs#L13396-L13413
[segments-take]: ../../../../crates/daemon/src/lib.rs#L14502-L14517
[segments]: ../../../../crates/daemon/src/lib.rs#L14522-L14529
[cached-boundary]: ../../../../crates/daemon/src/lib.rs#L16663
[sel-kind]: ../../../../crates/daemon/src/lib.rs#L16725
[token-count]: ../../../../crates/daemon/src/lib.rs#L2036-L2058
[served-reusing]: ../../../../crates/daemon/src/transform.rs#L164-L224
[ser-served]: ../../../../crates/daemon/src/transform.rs#L301-L308
[served-byte-witnesses]: evidence/derived-artifacts-are-ownership-independent.md#canonical-served-bytes-and-fingerprint-identity
[block-identity]: ../../../../crates/daemon/src/wire.rs#L885
[gate-prefix]: ../../../../crates/daemon/src/transform.rs#L2019
[normalize]: ../../../../crates/daemon/src/transform.rs#L2129
[sel-item]: ../../../../crates/daemon/src/transform.rs#L6380
[tag-entry]: ../../../../crates/daemon/src/transform.rs#L6846-L6871
[load-tags]: ../../../../crates/daemon/src/transform.rs#L6969-L7031
[mint-input]: ../../../../crates/daemon/src/transform.rs#L7231-L7236
[append-mint]: ../../../../crates/daemon/src/transform.rs#L7338-L7361
[taggable]: ../../../../crates/daemon/src/transform.rs#L7365-L7392
[active-match]: ../../../../crates/daemon/src/transform.rs#L7429
[t-collapsed]: ../../../../crates/daemon/src/transform.rs#L27958
[synthetic-reference]: ../../../../crates/daemon/src/transform.rs#L27709
[synthetic-delta-witness]: ../../../../crates/daemon/src/lib.rs#L23662
[synthetic-delta-parity]: ../../../../crates/daemon/src/lib.rs#L23945
[synthetic-lineage-rebase]: ../../../../crates/daemon/src/transform.rs#L28976
[tag-baseline]: ../../../../crates/daemon/src/transform.rs#L3045-L3046
[tag-protection]: ../../../../crates/daemon/src/transform.rs#L3707-L3720
[combined-tags]: ../../../../crates/daemon/src/transform.rs#L3441-L3449
[mint-tail]: ../../../../crates/daemon/src/transform.rs#L7964-L7974
[commit-mints]: ../../../../crates/daemon/src/transform.rs#L4972-L4981
[flatproj]: ../../../../crates/daemon/src/wire.rs#L187-L198
[reattach]: ../../../../crates/daemon/src/wire.rs#L214-L241
[diff-bytes]: ../../../../crates/daemon/src/wire.rs#L367-L375
[flatten]: ../../../../crates/daemon/src/wire.rs#L725-L786
[fp-reuse]: ../../../../crates/daemon/src/wire.rs#L871-L880
[shell-sharing]: ../../../../crates/daemon/src/wire.rs#L1746
[shell-decode]: ../../../../crates/daemon/src/wire.rs#L1793
[shell-metadata]: ../../../../crates/daemon/src/wire.rs#L1705
[hyg-output]: ../../../../crates/daemon/src/tail_hygiene.rs#L572-L591
[part-measure]: ../../../../crates/daemon/src/tail_hygiene.rs#L599-L622
[th-cwd]: ../../../../crates/daemon/src/tail_hygiene.rs#L614
[hygiene]: ../../../../crates/daemon/src/tail_hygiene.rs#L816-L973
[hyg-text]: ../../../../crates/daemon/src/tail_hygiene.rs#L888-L897
[hyg-input]: ../../../../crates/daemon/src/tail_hygiene.rs#L898-L901
[count-digest]: ../../../../crates/daemon/src/token_cache.rs#L103-L143
[hyg-bench-input]: ../../../../crates/daemon/benches/hot_path.rs#L69-L81
[hyg-bench-loop]: ../../../../crates/daemon/benches/hot_path.rs#L161-L199
[sidecar-merge]: ../../../../crates/daemon/src/codec/opencode.rs#L288-L310
[remember]: ../../../../crates/daemon/src/codec/sidecar.rs#L67-L73
[todo-prefix]: ../../../../crates/daemon/src/injection.rs#L187-L189
[segment-served]: ../../../../crates/daemon/src/dispatch.rs#L50-L72
[tail-reclaim]: ../../../../crates/daemon/src/healing.rs#L130-L139
[ser-msg]: ../../../../crates/memory-store/src/lib.rs#L145-L161
[meta-doc]: ../../../../crates/memory-store/src/lib.rs#L210-L216
[mint-prepared]: ../../../../crates/memory-store/src/lib.rs#L8546-L8561
[tag-content-policy]: ../../../../crates/memory-store/src/lib.rs#L2289-L2318
[load-order]: ../../../../crates/memory-store/src/lib.rs#L7557-L7585
[serde-features]: ../../../../Cargo.toml#L45
[load]: ../../../../crates/memory-store/src/lib.rs#L6388-L6415
[full-select]: ../../../../crates/memory-store/src/lib.rs#L4703-L4704
[epoch-read]: ../../../../crates/daemon/src/lib.rs#L4308-L4323
[epoch-read-delta]: ../../../../crates/daemon/src/lib.rs#L4190-L4220
[active]: ../../../../crates/daemon/src/lib.rs#L4600-L4618
[prepare]: ../../../../crates/daemon/src/lib.rs#L5046-L5119
[no-fire]: ../../../../crates/daemon/src/lib.rs#L5502-L5515
[handler]: ../../../../crates/daemon/src/lib.rs#L8181-L8444
[received-call]: ../../../../crates/daemon/src/lib.rs#L8197
[rejected-call]: ../../../../crates/daemon/src/lib.rs#L8271-L8278
[commit-call]: ../../../../crates/daemon/src/lib.rs#L8288
[roots-insert]: ../../../../crates/daemon/src/lib.rs#L8286-L8291
[floor-a]: ../../../../crates/daemon/src/lib.rs#L8294-L8302
[hook]: ../../../../crates/daemon/src/lib.rs#L8303-L8308
[floor-b]: ../../../../crates/daemon/src/lib.rs#L8425-L8444
[pc-store]: ../../../../crates/daemon/src/lib.rs#L8454-L8461
[guidance-remove]: ../../../../crates/daemon/src/lib.rs#L8465-L8470
[completed-call]: ../../../../crates/daemon/src/lib.rs#L8503
[cfg-models]: ../../../../crates/daemon/src/config.rs#L119
[cfg-user-mem]: ../../../../crates/daemon/src/config.rs#L126
[cas-retry]: ../../../../crates/daemon/src/transform.rs#L1942-L1981
[stable-call]: ../../../../crates/daemon/src/transform.rs#L1824-L1848
[descend]: ../../../../crates/daemon/src/transform.rs#L2963-L2974
[value-compare]: ../../../../crates/daemon/src/transform.rs#L3227
[truncate]: ../../../../crates/daemon/src/transform.rs#L4139-L4145
[sched-test]: ../../../../crates/daemon/src/transform.rs#L13689
[received]: ../../../../crates/memory-store/src/lib.rs#L6797-L6847
[received-doc]: ../../../../crates/memory-store/src/lib.rs#L6794-L6796
[flagged]: ../../../../crates/memory-store/src/lib.rs#L6808-L6826
[stable]: ../../../../crates/memory-store/src/lib.rs#L6852-L6944
[completed]: ../../../../crates/memory-store/src/lib.rs#L6949-L6997
[completed-doc]: ../../../../crates/memory-store/src/lib.rs#L6946-L6948
[rejected]: ../../../../crates/memory-store/src/lib.rs#L7003-L7056
[sched-history]: ../../../../crates/memory-store/src/lib.rs#L7104-L7137
[passtrace-doc]: ../../../../crates/memory-store/src/lib.rs#L852-L869
[commit-meta]: ../../../../crates/memory-store/src/lib.rs#L8618-L8627
[commit-trace]: ../../../../crates/memory-store/src/lib.rs#L8739-L8806
[json-content]: ../../../../crates/memory-store/src/lib.rs#L2201-L2211
[record-scan]: ../../../../crates/memory-store/src/lib.rs#L2218-L2228
[policy]: ../../../../crates/memory-store/src/lib.rs#L3162-L3183
[prepare-collecting]: ../../../../crates/memory-store/src/lib.rs#L3201-L3211
[single-pass]: ../../../../crates/memory-store/src/lib.rs#L3217-L3409
[keys]: ../../../../crates/memory-store/src/lib.rs#L3267-L3280
[prepare-value]: ../../../../crates/memory-store/src/lib.rs#L3291-L3392
[walk-keys]: ../../../../crates/memory-store/src/lib.rs#L3382-L3388
[clean-branch]: ../../../../crates/memory-store/src/lib.rs#L3403-L3408
[unique-doc]: ../../../../crates/memory-store/src/lib.rs#L3411-L3412
[unique]: ../../../../crates/memory-store/src/lib.rs#L3413-L3494
[recomp]: ../../../../crates/memory-store/src/lib.rs#L10369-L10462
[meta-epoch]: ../../../../crates/memory-store/src/lib.rs#L1472-L1473
[meta-historian]: ../../../../crates/memory-store/src/lib.rs#L1588-L1589
[phase]: ../../../../crates/memory-store/src/lib.rs#L622-L631
[drain]: ../../../../crates/memory-store/src/lib.rs#L11118-L11163
[drain-doc]: ../../../../crates/memory-store/src/lib.rs#L11115-L11117
[status-sc]: ../../../../crates/memory-store/src/lib.rs#L11165-L11191
[load-due]: ../../../../crates/memory-store/src/lib.rs#L11193-L11227
[deliver]: ../../../../crates/memory-store/src/lib.rs#L11229-L11285
[failure]: ../../../../crates/memory-store/src/lib.rs#L11287-L11326
[delete-all]: ../../../../crates/memory-store/src/lib.rs#L11328-L11341
[delete-one]: ../../../../crates/memory-store/src/lib.rs#L11343-L11365
[publish]: ../../../../crates/memory-store/src/lib.rs#L10871
[publish-drain]: ../../../../crates/memory-store/src/lib.rs#L11074-L11083
[kinds]: ../../../../crates/memory-store/src/lib.rs#L4635-L4638
[events-insert]: ../../../../crates/memory-store/src/lib.rs#L13892-L13913
[mark]: ../../../../crates/memory-store/src/lib.rs#L14051-L14076
[primer-insert]: ../../../../crates/memory-store/src/lib.rs#L14078-L14120
[obs-insert]: ../../../../crates/memory-store/src/lib.rs#L14122-L14143
[idx-order]: ../../../../crates/memory-store/baseline.sql#L531-L535

[ts-read]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L999-L1012
[ts-stages]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1013-L1042
[ts-stage-fn]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1019-L1024
[t244]: ../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L249
[hookclient]: ../../../../packages/opencode-plugin/src/hooks/context/hook.ts#L138-L139
[ismidturn]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L223-L231
[dbcache]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L32-L215
[midturndb]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L233-L287
[newer]: ../../../../packages/opencode-plugin/src/hooks/context/read-session-db.ts#L300-L357
[midturn-reference]: ../../../../packages/opencode-plugin/src/hooks/context/__tests__/mid-turn-reference.ts#L5-L143
[paged]: ../../../../packages/opencode-plugin/src/hooks/context/module-wire.ts#L666-L676
[sessionlog]: ../../../../packages/opencode-plugin/src/shared/logger.ts#L183-L236
[log-gate-checks]: ../../../../packages/opencode-plugin/src/shared/logger.test.ts#L377-L730
[sanitize]: ../../../../packages/opencode-plugin/src/shared/logger.ts#L14-L34
[ensuredir]: ../../../../packages/opencode-plugin/src/shared/logger.ts#L98-L109
[appendpriv]: ../../../../packages/opencode-plugin/src/shared/logger.ts#L117-L134
[flush]: ../../../../packages/opencode-plugin/src/shared/logger.ts#L136-L162
[redaction]: ../../../../packages/opencode-plugin/src/shared/redaction.ts#L1-L20

[agents]: ../../../../crates/shm-transport/AGENTS.md
[arena-const]: ../../../../crates/shm-transport/src/arena.rs#L4-L7
[removal-ranges]: ../../../../crates/shm-transport/src/backend/ring.rs#L375-L427
[try-reserve]: ../../../../crates/shm-transport/src/backend/ring.rs#L1263-L1340
[resident-api]: ../../../../crates/shm-transport/src/backend/ring.rs#L1894-L1899
[reclaim]: ../../../../crates/shm-transport/src/backend/ring.rs#L2070-L2151
[punch-decision]: ../../../../crates/shm-transport/src/backend/ring.rs#L2129-L2134
[batch]: ../../../../crates/shm-transport/src/backend/ring.rs#L2158-L2161
[abort]: ../../../../crates/shm-transport/src/backend/ring.rs#L2268-L2304
[prepare-commit]: ../../../../crates/shm-transport/src/backend/ring.rs#L2306-L2343
[write-res]: ../../../../crates/shm-transport/src/backend/ring.rs#L2388-L2419
[res-write]: ../../../../crates/shm-transport/src/backend/ring.rs#L2519-L2531
[commit-underfill]: ../../../../crates/shm-transport/src/backend/ring.rs#L2533-L2570
[span-safety]: ../../../../crates/shm-transport/src/lease.rs#L25-L33
[span-ptr]: ../../../../crates/shm-transport/src/lease.rs#L49-L52
[shape]: ../../../../crates/shm-transport/src/lease.rs#L134-L156
[copy-out]: ../../../../crates/shm-transport/src/lease.rs#L176-L206
[copy-in]: ../../../../crates/shm-transport/src/lease.rs#L208-L235
[to-vec]: ../../../../crates/shm-transport/src/lease.rs#L328-L348
[to-vec-fill]: ../../../../crates/shm-transport/src/lease.rs#L331
[t-concurrent]: ../../../../crates/shm-transport/src/lease.rs#L581-L619
[max-resident]: ../../../../crates/host-runtime/src/ring_transport.rs#L57-L58
[affordable]: ../../../../crates/host-runtime/src/ring_transport.rs#L60-L65
[process-limits]: ../../../../crates/host-runtime/src/ring_transport.rs#L96-L125
[idle-select]: ../../../../crates/host-runtime/src/ring_transport.rs#L582-L617
[publish-fail]: ../../../../crates/host-runtime/src/ring_transport.rs#L622-L646
[receive-to-vec]: ../../../../crates/host-runtime/src/ring_transport.rs#L731-L736
[publish-one]: ../../../../crates/host-runtime/src/ring_transport.rs#L749-L786
[publish-direct]: ../../../../crates/host-runtime/src/ring_transport.rs#L788-L800
[commit-before]: ../../../../crates/host-runtime/src/ring_transport.rs#L814-L825
[res-writer]: ../../../../crates/host-runtime/src/ring_transport.rs#L827-L843
[config-validate]: ../../../../crates/host-runtime/src/config.rs#L127-L131
[outbuf]: ../../../../crates/host-runtime/src/handler.rs#L320-L335
[into-parts]: ../../../../crates/host-runtime/src/handler.rs#L387-L400
[from-writer]: ../../../../crates/host-runtime/src/handler.rs#L465-L472
[reserve-direct]: ../../../../crates/host-runtime/src/dispatch.rs#L517-L554
[direct-frame]: ../../../../crates/host-runtime/src/frame_channel.rs#L166-L200
[native-reserve]: ../../../../packages/shm-native/src/lib.rs#L1024
[settle-with]: ../../../../crates/daemon/src/lib.rs#L12083-L12139
[fixture-arm]: ../../../../crates/host-runtime/tests/support/mod.rs#L441-L455
[ci-miri]: ../../../../.github/workflows/ci.yml#L597-L635
[ci-valgrind]: ../../../../.github/workflows/ci.yml#L637-L669
[ingest]: ../../../../crates/kernel/src/cas/ingest.rs#L361-L663
[ingest-temp]: ../../../../crates/kernel/src/cas/ingest.rs#L379-L405
[ingest-lock]: ../../../../crates/kernel/src/cas/ingest.rs#L407-L416
[ingest-reservation]: ../../../../crates/kernel/src/cas/ingest.rs#L424-L471
[ingest-publish]: ../../../../crates/kernel/src/cas/ingest.rs#L476-L526
[ingest-commit]: ../../../../crates/kernel/src/cas/ingest.rs#L580-L595
[check-budget]: ../../../../crates/kernel/src/cas/ingest.rs#L665-L682
[release-res]: ../../../../crates/kernel/src/cas/ingest.rs#L756-L772
[cleanup]: ../../../../crates/kernel/src/cas/ingest.rs#L779-L849
[stat-bytes]: ../../../../crates/kernel/src/cas/ingest.rs#L1206-L1208
[walk]: ../../../../crates/kernel/src/cas/ingest.rs#L1228-L1288
[present]: ../../../../crates/kernel/src/cas/ingest.rs#L1290-L1296
[startup]: ../../../../crates/kernel/src/cas/gc.rs#L78-L180
[startup-unreachable]: ../../../../crates/kernel/src/cas/gc.rs#L109-L136
[reclaim-cand]: ../../../../crates/kernel/src/cas/gc.rs#L212-L289
[recovery]: ../../../../crates/kernel/src/cas/gc.rs#L300-L341
[unlink-artifact]: ../../../../crates/kernel/src/cas/gc.rs#L403-L419
[scan-objects]: ../../../../crates/kernel/src/cas/gc.rs#L579
[purge-unlink]: ../../../../crates/kernel/src/cas/deletion.rs#L532-L556
[delete]: ../../../../crates/kernel/src/cas/deletion.rs#L237
[cap-default]: ../../../../crates/kernel/src/cas/mod.rs#L24
[cap-error]: ../../../../crates/kernel/src/cas/mod.rs#L318-L325
[latch]: ../../../../crates/kernel/src/cas/mod.rs#L569
[recover]: ../../../../crates/kernel/src/open.rs#L419-L422
[lock-writer]: ../../../../crates/kernel/src/open.rs#L433-L442
[facts]: ../../../../crates/kernel/src/facts.rs#L144-L164
[maintenance]: ../../../../crates/kernel/src/retention.rs#L190-L211
[restore]: ../../../../crates/kernel/src/backup.rs#L424
[busy]: ../../../../crates/daemon/src/kernel_routes/state.rs#L290-L295
[route-ingest]: ../../../../crates/daemon/src/kernel_routes/ingest.rs#L577
[t-oracle]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L350-L382
[t-faults]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L426-L494
[t-crash]: ../../../../crates/kernel/tests/cas_fault_injection.rs#L924-L990
[t-orphan]: ../../../../crates/kernel/tests/kernel_gc.rs#L602-L642

[ci-bench]: ../../../../.github/workflows/ci.yml#L514-L518
[nextest]: ../../../../.config/nextest.toml#L4-L7
[hp-header]: ../../../../crates/daemon/benches/hot_path.rs#L1-L10
[hp-e2e]: ../../../../crates/daemon/benches/hot_path.rs#L282-L315
[meta-bound]: ../../../../crates/daemon/tests/transform_meta_bound.rs#L1-L22
[he-payload]: ../../../../crates/shm-transport/benches/hardware_envelope.rs#L220-L223
[he-designated]: ../../../../crates/shm-transport/benches/hardware_envelope.rs#L211-L214
[he-blocked]: ../../../../crates/shm-transport/benches/hardware_envelope.rs#L283-L286
[he-manifest]: ../../../../crates/shm-transport/benches/manifests/v1.json
[evidence]: ../../../../crates/host-runtime/benches/support/evidence.rs#L1-L8
[fx-1400]: ../../../../crates/daemon/src/transform.rs#L12436-L12441
[fx-2500]: ../../../../crates/daemon/src/transform.rs#L28085-L28090
[h-pre]: ../../../../crates/daemon/src/lib.rs#L8181-L8198
[h-timings]: ../../../../crates/daemon/src/lib.rs#L8530-L8556
[respond]: ../../../../crates/daemon/src/lib.rs#L14479
[tt]: ../../../../crates/daemon/src/transform.rs#L1026-L1207
[rtcd]: ../../../../crates/daemon/src/transform.rs#L1209-L1220
[fmt]: ../../../../crates/daemon/src/transform.rs#L1226-L1360
[snap-add]: ../../../../crates/daemon/src/transform.rs#L2412
[snap-once]: ../../../../crates/daemon/src/transform.rs#L2892
[tc-doc]: ../../../../crates/daemon/src/token_cache.rs#L1-L7
[tc-cap]: ../../../../crates/daemon/src/token_cache.rs#L16
[tc-bound]: ../../../../crates/daemon/src/token_cache.rs#L24-L28
[tc-static]: ../../../../crates/daemon/src/token_cache.rs#L34-L40
[tc-local]: ../../../../crates/daemon/src/token_cache.rs#L57-L76
[tc-concurrent]: ../../../../crates/daemon/src/token_cache.rs#L108-L109
[tc-cwd]: ../../../../crates/daemon/src/token_cache.rs#L110-L142
[tc-shard]: ../../../../crates/daemon/src/token_cache.rs#L112-L113
[tc-u32]: ../../../../crates/daemon/src/token_cache.rs#L135-L137
[tc-cet]: ../../../../crates/daemon/src/token_cache.rs#L165-L181
[tc-inject]: ../../../../crates/daemon/src/transform.rs#L1804-L1820
[declared-doc]: ../../../../crates/daemon/src/lib.rs#L2250-L2256
[declared]: ../../../../crates/daemon/src/lib.rs#L2256-L2286
[ao-sig]: ../../../../crates/daemon/src/transform.rs#L2876
[soft-predicate]: ../../../../crates/daemon/src/transform.rs#L6340
[t-bypass]: ../../../../crates/daemon/src/transform.rs#L24332
[selection-sharing]: ../../../../crates/daemon/src/transform.rs#L24354
[sidecar-order-check]: ../../../../crates/daemon/src/codec/opencode.rs#L2083
[native-sharing]: ../../../../crates/daemon/src/lib.rs#L21068
[native-ingress-sharing]: ../../../../crates/daemon/src/lib.rs#L21386
[native-charge-floor]: ../../../../crates/daemon/src/lib.rs#L21508
[soft-reference]: ../../../../crates/daemon/src/transform.rs#L24361
[soft-threshold-check]: ../../../../crates/daemon/src/transform.rs#L24387
[soft-gates-check]: ../../../../crates/daemon/src/transform.rs#L24521
[tag-accounting-check]: ../../../../crates/daemon/src/transform.rs#L21486
[serialization-gate-check]: ../../../../crates/daemon/src/transform.rs#L28295
[eval]: ../../../../crates/secret-scanner/src/evaluator.rs#L35-L157
[captures]: ../../../../crates/secret-scanner/src/evaluator.rs#L112-L128
[radius-window]: ../../../../crates/secret-scanner/src/evaluator.rs#L266-L297
[preselect]: ../../../../crates/secret-scanner/src/rules.rs#L353-L370
[digest-doc]: ../../../../crates/secret-scanner/src/rules.rs#L382
[anchor-ci]: ../../../../crates/secret-scanner/src/rules.rs#L449-L473
[radius-valid]: ../../../../crates/secret-scanner/src/rules.rs#L598-L602
[max-match]: ../../../../crates/secret-scanner/src/api.rs#L16
[max-radius]: ../../../../crates/secret-scanner/src/api.rs#L20
[limits]: ../../../../crates/secret-scanner/src/api.rs#L219-L226
[revision]: ../../../../crates/secret-scanner/src/api.rs#L276-L280
[rules-radius-doc]: ../../../../crates/secret-scanner/default_rules.yaml#L12
[airtable]: ../../../../crates/secret-scanner/default_rules.yaml#L297-L309
[edge-margin]: ../../../../crates/context-core/src/redaction.rs#L380-L385
[ms-content]: ../../../../crates/memory-store/src/lib.rs#L2155-L2163
[ms-digest]: ../../../../crates/memory-store/src/lib.rs#L2442-L2477
[snap-build]: ../../../../crates/daemon/src/historian_chunk.rs#L418-L430
[as-item]: ../../../../crates/daemon/src/historian_chunk.rs#L40-L46
[trunc-call]: ../../../../crates/daemon/src/historian_chunk.rs#L693
[trunc]: ../../../../crates/daemon/src/historian_chunk.rs#L744-L777
[boundary-view]: ../../../../crates/daemon/src/lib.rs#L16663-L16723
[construction-corpus]: ../../../../crates/daemon/src/lib.rs#L17601
[firing-capture]: ../../../../crates/daemon/src/lib.rs#L23662
[fp]: ../../../../crates/daemon/src/historian.rs#L140-L158
[fp-field]: ../../../../crates/memory-store/src/lib.rs#L673
[fp-verify]: ../../../../crates/daemon/src/historian.rs#L326-L334
[fp-predicate]: ../../../../crates/daemon/src/historian.rs#L407-L417
[diff-header]: ../../../../crates/daemon/tests/historian_truncate_differential.rs#L1-L11
[diff-ref]: ../../../../crates/daemon/tests/historian_truncate_differential.rs#L13-L58
[cap]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L31-L34
[parse]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L125-L145
[vixie]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L150-L160
[stepper]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L163-L192
[valid]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L205-L210
[occurrence]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L212-L218
[note-cap]: ../../../../crates/daemon/src/smart_note_evaluation.rs#L236-L239
[sched-due]: ../../../../crates/daemon/src/dreamer_scheduler.rs#L412-L416
[sched-default]: ../../../../crates/daemon/src/config.rs#L127
[sched-accept]: ../../../../crates/daemon/src/config.rs#L881-L895
[eff-cfg]: ../../../../crates/daemon/src/lib.rs#L4589-L4598
[binding-doc]: ../../../../crates/daemon/src/lib.rs#L229-L230
[call-reattach]: ../../../../crates/daemon/src/lib.rs#L4842
[call-fire]: ../../../../crates/daemon/src/lib.rs#L5100
[call-wrapup]: ../../../../crates/daemon/src/lib.rs#L5419
[call-bind]: ../../../../crates/daemon/src/lib.rs#L11856
[eff-proj]: ../../../../crates/daemon/src/config.rs#L242-L245
[eff-warn-doc]: ../../../../crates/daemon/src/config.rs#L266-L267
[eff-warn]: ../../../../crates/daemon/src/config.rs#L268-L288
[eff-clone]: ../../../../crates/daemon/src/config.rs#L288
[tier-cached]: ../../../../crates/daemon/src/config.rs#L368-L398
[guidance]: ../../../../crates/daemon/src/config.rs#L414-L496
[merge]: ../../../../crates/daemon/src/config.rs#L716
[raise-only]: ../../../../crates/daemon/src/config.rs#L740
[knows]: ../../../../crates/daemon/src/lib.rs#L4522-L4569
[roots-doc]: ../../../../crates/daemon/src/lib.rs#L2959-L2962
[guidance-fn]: ../../../../crates/daemon/src/lib.rs#L4700-L4707
[guidance-use]: ../../../../crates/daemon/src/lib.rs#L8249

[pb-redacted]: ../../../../crates/host-runtime/src/panic_boundary.rs#L7
[pb-tls]: ../../../../crates/host-runtime/src/panic_boundary.rs#L11-L13
[pb-polling]: ../../../../crates/host-runtime/src/panic_boundary.rs#L30-L34
[pb-hook]: ../../../../crates/host-runtime/src/panic_boundary.rs#L36-L50
[pb-sync]: ../../../../crates/host-runtime/src/panic_boundary.rs#L52-L55
[pb-async]: ../../../../crates/host-runtime/src/panic_boundary.rs#L60-L72
[wrap-callback]: ../../../../crates/host-runtime/src/dispatch.rs#L928-L934
[panic-terminal]: ../../../../crates/host-runtime/src/dispatch.rs#L985-L989
[blocking-doc]: ../../../../crates/daemon/src/kernel_routes/mod.rs#L460-L461
[blocking]: ../../../../crates/daemon/src/kernel_routes/mod.rs#L462-L468
[blk-commit-preview]: ../../../../crates/daemon/src/kernel_routes/commit.rs#L1013
[blk-commit-run]: ../../../../crates/daemon/src/kernel_routes/commit.rs#L1038
[blk-egress]: ../../../../crates/daemon/src/kernel_routes/egress.rs#L201
[blk-eligibility]: ../../../../crates/daemon/src/kernel_routes/eligibility.rs#L346
[blk-ingest-decode]: ../../../../crates/daemon/src/kernel_routes/ingest.rs#L722
[blk-ingest-finish]: ../../../../crates/daemon/src/kernel_routes/ingest.rs#L797
[blk-read-gate]: ../../../../crates/daemon/src/kernel_routes/read.rs#L291
[blk-read-rows]: ../../../../crates/daemon/src/kernel_routes/read.rs#L311
[spawn-health]: ../../../../crates/daemon/src/kernel_routes/health.rs#L224
[spawn-kernel-open]: ../../../../crates/daemon/src/kernel_routes/mod.rs#L358-L362
[spawn-store-open]: ../../../../crates/daemon/src/lib.rs#L3854-L3856
[tl-test]: ../../../../crates/daemon/src/transform.rs#L495-L498
[t-panic-internal]: ../../../../crates/host-runtime/tests/dispatch.rs#L551
[t-panic-stderr]: ../../../../crates/host-runtime/tests/dispatch.rs#L603
[t-panic-child]: ../../../../crates/host-runtime/tests/dispatch.rs#L631-L660

[pass-drain]: ../../../../crates/daemon/src/lib.rs#L8190-L8194
[due-predicate]: ../../../../crates/memory-store/src/lib.rs#L11205-L11206
[backoff]: ../../../../crates/memory-store/src/lib.rs#L11293-L11297
[fail-sc]: ../../../../crates/memory-store/src/lib.rs#L6092-L6101
[daemon-cargo]: ../../../../crates/daemon/Cargo.toml#L92
[t-status-sc]: ../../../../crates/daemon/src/lib.rs#L36750
[t-faults-sc]: ../../../../crates/memory-store/src/lib.rs#L19314
[t-restart]: ../../../../crates/memory-store/src/lib.rs#L19519
[sched-tick]: ../../../../crates/daemon/src/dreamer_scheduler.rs#L244-L261
[sched-due-projects]: ../../../../crates/daemon/src/dreamer_scheduler.rs#L265-L296
[sched-idle]: ../../../../crates/daemon/src/dreamer_scheduler.rs#L36
[sched-scripted]: ../../../../crates/daemon/src/dreamer_scheduler.rs#L595-L600
[sched-fixture]: ../../../../crates/daemon/src/dreamer_scheduler.rs#L586-L593
[sched-clock]: ../../../../crates/daemon/src/dreamer_scheduler.rs#L418-L427
[t-sched-cron]: ../../../../crates/daemon/src/dreamer_scheduler.rs#L680
[sched-projects]: ../../../../crates/daemon/src/lib.rs#L14051-L14103
[sched-authority]: ../../../../crates/daemon/src/lib.rs#L14073-L14078
[sched-filter]: ../../../../crates/daemon/src/lib.rs#L14098

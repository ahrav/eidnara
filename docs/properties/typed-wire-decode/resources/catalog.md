# Typed wire decode: resource properties

System: daemon admission, typed/tree decode, projection, and retained holders.
Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Provenance, supplied scope, baseline comparison, and P line notation are in
[source-register.md](source-register.md). No new tests or benchmarks run.

## Scope and authority

This catalog enriches the settled plan for `/to-spec`. It creates resource
claims under test, not implementation success or a published specification.
The user supplies the plan, host contract, A1-A3/B1/W1, and HP1 issue leads;
no further incident or external repository is supplied. Full issue evidence
and raw measurement artifacts are missing. No interview fills those gaps.

Six records are active discovery obligations. R6 remains as an invalidated
record because payoff evidence is an acceptance gate, not a system property.
The typed-wire U1 implementation exercised R3 and R5 fully and R1, R2, R4, and
R7 in part; each record states what ran and what remains. Existing source
checks remain unaudited beyond the named tests. See
[checks](existing-checks.md), [fault mapping](fault-map.md), and
[handoffs](handoff.md).

W1 remains **invalidated** in its owning catalog, explicitly at
`docs/properties/hot-path-optimization/latency-audit/catalog.md:1772-1773`.
R6's exact obligation lives in [evidence-gates.md](evidence-gates.md), as EG1.
It neither reactivates W1 nor fulfills W1's historical handler-level scope.
The [portfolio disposition](portfolio-evaluation.md) records the independent
analyst's findings and the corrections applied here.

The plan's accepted decisions and stops remain controlling. P:L88 permits a
wider live string-charge ceiling; it does not authorize replacing original
A1-A3 witnesses. P:L18 still stops on changed plugin-shaped golden bytes,
changed original A1-A3 admission outcomes, U1 payoff within noise, or a needed
wire-visible field, literal, or error-code change.

## Observation vocabulary

- `B` is immutable input bytes; `C` is a fixed numeric scratch capacity.
  A frozen admission fixture also fixes other holders and their schedule.
- `F_k` is the meter's needed-byte estimate with string coefficient `k` and
  node copies fixed at two. Upfront unescape reservations and visited-prefix
  accounting are recorded, including restart boundaries.
- `L(t)` is live, request-attributed allocation bytes above the setup baseline.
  `P = max_t L(t)` is one continuous high-water mark. Requested layout sizes
  are only a lower bound on allocator residency, not an exact RSS measure.
- `Q(t)` is live backed reservation bytes, not `ByteCharge::none()` counts.
  Each allocation has a pool/holder attribution and lifetime interval.
- `J` is the exact byte length of the frozen `messages` JSON array, including
  array and message envelopes. `E_msg` and `P_msg` cover only decoding that
  subtree and its attributable workspace. They do not subtract two unrelated
  whole-process peaks.

The host describes named logical payload accounting, not exact process RSS
(`crates/host-runtime/src/config.rs:65-72`). R4 preserves that declared budget
and existing ownership policies. Its observation ledger covers the complete
interval under those rules; it neither defines a new RSS model nor excuses
an omitted transient allocation.

## Accepted coefficient decision

P:L88 keeps `RETAINED_NODE_COPIES = 2`. String copies start at one; measurement
raises the coefficient to the smallest passing positive integer across both
lanes and the declared corpus/build envelope, retaining failed smaller-value
observations. This is coefficient selection, outside R1's `P <= F` predicate.
The new text-heavy ceiling is recorded with additive witnesses under R3.

## Reachability and index

R1-R4 constrain default production paths, not default occurrence of every
fault. `Handler::handle` drives them at
`crates/daemon/src/lib.rs:12153-12170`. R5 and R7 are test-only budget and
situation checks. R6's test-only classification is retained as history.
Each active record repeats its own reachability evidence.

| ID | Record | Type | Reachability | Status | Check |
| --- | --- | --- | --- | --- | --- |
| R1 | [decode-footprint-covers-both-lanes-combined-peak](#decode-footprint-covers-both-lanes-combined-peak) | safety | default-production | active | always |
| R2 | [retained-accounting-follows-typed-ownership](#retained-accounting-follows-typed-ownership) | safety | default-production | active | always |
| R3 | [frozen-admission-outcomes-and-boundaries-stay-stable](#frozen-admission-outcomes-and-boundaries-stay-stable) | safety | default-production | active | always |
| R4 | [decode-and-projection-stay-within-resident-pool](#decode-and-projection-stay-within-resident-pool) | safety | default-production | active | always |
| R5 | [message-decode-allocation-gate-has-isolated-scope](#message-decode-allocation-gate-has-isolated-scope) | safety | test-only | active | always |
| R6 | [decode-projection-payoff-has-comparable-evidence](#decode-projection-payoff-has-comparable-evidence) | safety | test-only | invalidated | always |
| R7 | [resource-witnesses-reach-independent-preconditions](#resource-witnesses-reach-independent-preconditions) | reachability | test-only | active | sometimes |

## Records

### decode-footprint-covers-both-lanes-combined-peak

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `crates/daemon/tests/parse_charge_covers_typed_decode.rs`
holds `P_decode <= F_1` on both lanes for 4 MiB plain and escaped text, 64 KiB
escaped text, dense native values, a typed prefix that fails at a late duplicate
key followed by its tree fallback, and payload-heavy blocks on the direct lane;
the tree lane's payload-heavy peak exceeds `F_1` by container storage that no
string coefficient covers, pinned at no more than one third of the charge (see
the [evidence](evidence/decode-footprint-covers-both-lanes-combined-peak.md#typed-wire-u1-execution-2026-09-13)).
Guarantee: The selected decode estimate covers the complete simultaneous
decode heap on both lanes.
Check: `always` - For every corpus case and lane, assert `P_decode <= F_k_max`,
using one interval from before parsing through successful return or failure
cleanup; `F_k_max` includes admission scratch and the largest needed prefix
across restarts. Tree cases execute actual `from_slice::<Value>(B)` followed
by consuming `from_value::<TransformRequest>`, and production-path cases use
the metered tree parse. Never reset at conversion or fallback. The predicate
is `P_decode <= F_k_max` for the selected coefficient; it applies to every
evaluation, including errors, rather than only completed requests.
Fault/timing angle: Transient tree/typed coexistence, unescape scratch,
internally tagged buffering, partial decode errors, and direct-to-tree restart.
Required faults and enabling state: Large plain and escaped text, dense nodes,
retained payload Values, duplicate envelope keys causing fallback, late typed
errors, trailing malformed bytes, and permanent/transient refusals.
Confidence: medium - [evidence](evidence/decode-footprint-covers-both-lanes-combined-peak.md).
The current meter and production lanes are source-verified at
`crates/daemon/src/lib.rs:12921-12946`; the proposed coefficient is unmeasured.
Existing check: `crates/daemon/tests/parse_charge_covers_typed_decode.rs:103-199`
covers dense-native combined tree and direct text peaks;
`crates/daemon/src/lib.rs:20281-20315` includes a hard-coded three-copy bound
at `:20294-20298`. Both are unaudited; the latter needs explicit disposition
against KTD4 rather than omission from the replacement inventory.
Impact: An underestimated footprint admits more heap than the host can cover.
Open questions:
- What is the smallest passing coefficient on the final build, including
  failure prefixes and escape scratch?
- How will the candidate measurement report distinguish requested-layout
  lower bounds from the declared logical resident budget without claiming RSS?

### retained-accounting-follows-typed-ownership

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `crates/daemon/tests/typed_wire_decode_allocations.rs`
(`source_has_no_envelope_tree_or_replay_entry`) checks that neither wire struct
holds a `Value` field and that no replay entry remains; `retained_size.rs`
(`decoded_envelope_charges_only_typed_fields`) and the independent ledgers in
`wire.rs` and `transform.rs` charge decoded and constructed shells equally.
Guarantee: Retained-size accounting removes only original-envelope ownership
and continues charging every surviving allocation under its holder policy.
Check: `always` - Assert that wire message/block objects retain no original
envelope Value, and that each retained estimate equals an independently built
ownership ledger under the declared estimator rules: inline storage once,
string/vector capacities, map entry storage, Arc headers, payload Values,
canonical text, and metadata. A block handle pointing into a charged ingress
shell adds no second block backing; separately allocated equal content does.
Within a holder, preserve its existing alias accounting; across independent
holders preserve full conservative charges while a charged owner remains.
This checks existing policies, not new global deduplication or ownership
transfer. Every observed ownership snapshot must satisfy this.
Fault/timing angle: Snapshot publication, projection reuse, Arc copy-on-write,
cache eviction, and lease release after the original holder drops.
Required faults and enabling state: Populated tool/media/opaque/JSON/extras
Values, spare capacity, native Values, tail_delta, canonical text, shared and
equal-but-distinct allocations, and an owner surviving cache eviction.
Confidence: high - [evidence](evidence/retained-accounting-follows-typed-ownership.md).
Production holder estimates at `crates/daemon/src/lib.rs:1745-1799` and
`crates/daemon/src/wire.rs:243-365` establish the accounting seams, not success.
Existing check: `crates/daemon/src/retained_size.rs:290-335` and
`crates/daemon/src/wire.rs:958-1238,1749-1792` exercise ownership accounting and
sharing; unaudited, including assertions tied to retained originals.
Impact: Cache/lease admission either undercounts live payloads or loses the
intended saving by charging removed storage.
Open questions:
- Does the revised ledger cover every remaining payload Value and canonical
  buffer without copying the estimator's implementation?
- Does the candidate preserve existing cross-holder conservative charges and
  last-owner release while removing only original-envelope ownership?

### frozen-admission-outcomes-and-boundaries-stay-stable

Type: safety
Reachability: default-production
Status: active
Exercised: yes - `frozen_corpus_footprints_replay_with_only_string_charge_changes`
in `crates/daemon/src/lib.rs` replays all 46 A2 bodies on both lanes against
their frozen three-copy footprints and terminals; the only changes are frozen
too-large terminals on bodies with string bytes, each admitted to its
unbounded-pool terminal, with the footprint difference equal to the removed
copies. Ceiling witnesses at an 8 MiB pool are in
`text_heavy_admission_ceiling_witnesses`; length caps and the node floor are
untouched.
Guarantee: Original A1-A3 cases and fixed admission rules remain stable while
KTD4 permits the live string-charge ceiling to expand outside those cases.
Check: `always` - Replay each original frozen A1-A3 fixture
`(B, C, holders, schedule)` with identical bytes, numeric capacity, and outcome;
stop if its admit/refuse decision or terminal code changes. Keep inclusive
1 MiB facade and 32 MiB transform length caps and the node-only
`footprint_floor` behavior unchanged. Preserve deciding-gate error mapping,
one terminal per settled refusal, and no dispatch ticket acceptance, route
binding, page staging, prompt freeze, or store mutation. Never recompute old
fixture capacities or replace old bodies with candidate-relative witnesses.
Separately check additive witnesses around the measured live string ceiling;
that ceiling may widen under KTD4. Every original case and fixed rule is
checked, without asserting an unchanged admitted set for all possible inputs.
Fault/timing angle: Lowering string charge changes the admitted set; held
capacity can cause transient refusal before permanent size is discovered.
Required faults and enabling state: Original A1-A3 cases, fixed length-cap and
node-floor neighbours, independent held-pool pressure, malformed/fallback
cases, and separately labelled additive string-ceiling witnesses. Observe
terminal frames before any client duplicate filtering.
Confidence: high - [evidence](evidence/frozen-admission-outcomes-and-boundaries-stay-stable.md).
The production gate is `crates/daemon/src/lib.rs:16084-16161`; plan language
contains an unresolved conflict, not permission to retune the fixtures.
Existing check: `crates/daemon/src/lib.rs:19569-19645,20188-20229,20414-20580`;
unaudited. Several capacities depend on the candidate footprint.
Impact: An original refusal changes unnoticed, a fixed admission rule drifts,
or retuned witnesses conceal a required stop.
Open questions:
- P:L176 permits retuning an old refusal witness, unlike P:L18 and P:L70.
  If an original case changes, stop and obtain an owner resolution; do not
  replace it. The wider ceiling itself is an accepted decision. (needs human input)
- Where will the immutable baseline fixture bytes, numeric capacities, and
  expected outcomes be retained by the implementation owner?

### decode-and-projection-stay-within-resident-pool

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `decode_and_projection_fit_the_declared_pool` holds
decode plus projection, with shared shells checked, under the declared scratch
pool and under four times the decode charge for the frozen 40/200 corpora and a
31 MiB text body (peak 130.0 MB against 184.9 MB); with served output the full
owner set reaches seven times the charge (227.5 MB) at the ceiling, above the
pool, which the test pins and the
[evidence](evidence/decode-and-projection-stay-within-resident-pool.md#typed-wire-u1-execution-2026-09-13)
records as an open owner decision. The above-cap probe remains outside the meter.
Guarantee: Full decode plus projection fits the existing declared logical
resident budget without omitting transient demand or changing ownership policy.
Check: `always` - Throughout entry, decode, projection, cleanup, and existing
holder handoff, assert `L_declared,pool(t) <= Q_covering,pool(t) <= C_pool`
for each declared pool, including other owners' demand. `L_declared,pool`
uses the existing accounting rules; `Q_covering,pool` includes backed charges
and already reserved headroom without counting either twice. Attribute all
simultaneous request, tree, canonical-text, serializer-workspace, and projection
demand, preserving existing alias and conservative per-holder rules. Keep
input and egress in their own declared pools. Requested-layout peaks are
supporting lower-bound observations, not exact RSS. No new ownership,
allocator, or global deduplication policy is required by this predicate.
Fault/timing angle: Probe-before-meter, fallback, projection shell rebuilding,
serializer double buffers, eviction with active leases, and concurrent requests.
Required faults and enabling state: Near-ceiling text and dense inputs, escaped
keys above and below the facade cap, both decode lanes, forced late errors,
nonempty projection, synthetic normalization, shared prefixes, and held capacity.
Confidence: medium - [evidence](evidence/decode-and-projection-stay-within-resident-pool.md).
The default handler retains its meter through settlement at
`crates/daemon/src/lib.rs:12153-12170`. The above-cap probe runs before that
meter, contradicting A1's unqualified charge-before-probe statement at
`docs/properties/hot-path-optimization/latency-audit/catalog.md:159-165`.
This ordering discrepancy is verified; its allocation magnitude is unmeasured.
Existing check: `crates/host-runtime/src/config.rs:481-502` checks pool splits;
`crates/daemon/src/lib.rs:23311-23353` checked active projection leases until
#828 deleted the projection cache and that test; the pool-split check is
unaudited. No complete transient allocation-to-pool check is found.
Impact: A smaller decode charge leaves projection or probe allocation unbounded
by actual reservations, especially under concurrent load.
Open questions:
- How does the owner disposition A1's verified charge-before-probe discrepancy
  for above-cap inputs, and which existing reservation covers any associated
  demand? Its allocation magnitude still needs a witness. (needs human input)
- Which existing reservation covers canonical serializer workspace and
  projection construction before retained-cache admission? (needs human input)
- Is the proposed wider text ceiling compatible with the full live peak,
  rather than only `F_k`? Measure before claiming it.

### message-decode-allocation-gate-has-isolated-scope

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `message_decode_stays_within_the_allocation_budget` decodes
the frozen 40-message body's `messages` array bytes (J = 74,934) into
`IngressMessages`: 434 events and a 109,432-byte peak against 640 and
224,802; `a_restored_envelope_tree_fails_the_gate` is the negative control
(2,185 events, 313,025-byte peak).
Guarantee: Decoding the fixed 40-message corpus uses at most 640 attributable
allocation events and strictly less than three times its messages JSON bytes.
Check: `always` - For each valid observation of the fixed plugin-shaped
40-message decode, assert `E_msg <= 640` and `P_msg < 3 * J`. This is a real
test-only resource budget invariant. The instrumentation rules below define
valid observations; they are not a separate runtime property. Invalid
attribution yields no result rather than a vacuous passing budget check.
Fault/timing angle: Fixture/setup contamination, process-global counter noise,
scope drift, integer truncation, or dropped temporaries hiding the peak.
Required faults and enabling state: Frozen mixed text/tool-call/tool-result
messages with 2 KiB results, exact input bytes, and validated instrumentation:
count alloc plus realloc; include wire shells, payload Values, unescape and
tagged-enum workspace; exclude fixture work, other threads, non-message native
data, and projection from this observation while keeping their demand in R4.
Use a continuous peak, production types, no integer division, no derive-only
mirror, and no subtraction of separate peak maxima.
Confidence: medium - [evidence](evidence/message-decode-allocation-gate-has-isolated-scope.md).
P:L67 and P:L213 establish the target. The analogous allocation test at
`crates/daemon/tests/served_json_passthrough_allocations.rs:10-32,58-85` is
test-only and measures serialization, not this gate.
Existing check: No messages decode threshold check is found at HEAD. Existing
allocation and peak counter patterns are unaudited.
Impact: A passing gate can be meaningless or reject good code by comparing a
whole-body numerator with a messages-only denominator.
Open questions:
- Will attribution use a messages-subtree interval inside production decoding,
  or isolated production `IngressMessages` decoding plus an integration
  equivalence certificate? The latter alone cannot claim whole-request costs.
  (needs human input)
- How will counter completeness and unrelated allocation isolation be verified?

### decode-projection-payoff-has-comparable-evidence

Type: safety
Reachability: test-only
Status: invalidated
Exercised: not yet - EG1 is retired and its passing verdict is withdrawn;
the benchmark cleanup removed its manifests, raw samples, and receipt.
W1 remains invalidated.
Guarantee: A typed-wire decode payoff claim requires comparable evidence for
the complete decode-plus-projection operation and stops when the gain is noise.
Check: `always` - The historical record checked whether a payoff verdict had
complete comparable 40/200 before/after evidence and stopped within noise.
That is an evidence-gate predicate, not a runtime property; this classification
is invalidated. [EG1](evidence-gates.md#eg1-decode-projection-payoff) records
retirement, not an active execution handoff.
Fault/timing angle: Wrong baseline branch, mirror decoder, untimed decode,
different drop timing, missing feature, pooled iterations, or cherry-picked cell.
Required faults and enabling state: Immutable before and candidate artifacts,
both corpus sizes, production message types, declared timing interval, and
process-level repeated measurements with a noise rule set before results.
Confidence: high - [evidence](evidence/decode-projection-payoff-has-comparable-evidence.md).
P:L18 and P:L213 historically required payoff evidence. The benchmark and
its evidence are removed; no current performance result is inferred.
Existing check: None. The benchmark and its measurement receipts are retired.
Impact: Treating a manifest requirement as a system invariant mixes acceptance
evidence with behavior coverage. Retiring the campaign withdraws its payoff
verdict and does not reactivate W1.
Open questions:
- What predeclared uncertainty/noise rule governs stop versus proceed for both
  sizes, and who owns its evidence artifact? (needs human input)
- P:L213 says to update W1, whose owning record is invalidated. Keep evidence
  plan-local unless the owner explicitly resolves that status. (needs human input)

### resource-witnesses-reach-independent-preconditions

Type: reachability
Reachability: test-only
Status: active
Exercised: partial - the fault map records which of the twelve markers the
executed tests construct independently and which remain unfired.
Guarantee: A resource campaign constructs every required risky situation and
records its preconditions independently of the candidate's success verdict.
Check: `sometimes` - Evaluate one independent `sometimes(preconditions_m)`
check for each of the twelve fixed resource markers in
[fault-map.md](fault-map.md#independent-coverage-markers), with its own identity
and occurrence result. Their conjunction is only a completion rollup, never
one aggregate `sometimes`. Inputs, holder charges, allocation lifetimes, and
stage entries establish each predicate; neither `queue_full` nor
`peak <= charge` is a witness. Each must fire on a correct implementation.
The retired measurement-pair name belongs to EG1, outside this property.
Fault/timing angle: A safety suite stays green while never taking fallback,
experiencing pressure, holding both decode stages, or retaining a shared owner.
Required faults and enabling state: The fixed marker matrix includes direct
success, tree conversion, fallback after a prefix, escaped scratch, late error,
pool shortfall with another owner, boundary neighbours, live projection,
owner survival, and isolated allocation scope.
Confidence: high - [evidence](evidence/resource-witnesses-reach-independent-preconditions.md).
`crates/daemon/src/lib.rs:20414-20443` supplies a partial test-only witness;
`ShortfallMarker` is supporting data, not the entire independent oracle.
Existing check: The held-pool and sharing tests in
[existing-checks.md](existing-checks.md) are unaudited. No complete campaign
marker matrix is found.
Impact: Passing safety assertions say nothing about unconstructed failure windows.
Open questions:
- Which test owner will retain independent stage/allocation and holder evidence?
- An unfired marker needs investigation: is its setup unreachable or is the
  generator missing it? It is not a finite refutation of formal liveness.

## Relationships and provenance

R1 checks the selected decode estimate; R2 inventories retained ownership;
neither dominates R4's complete logical pool bound. R5's narrow resource
budget cannot replace them. R3 protects original cases and fixed rules while
allowing KTD4's wider string ceiling. R6 is invalidated and replaced in
category by EG1, which is now retired. R7 supplies independent situation
checks, not timing evidence.

Architecture, failure, resource, and wildcard lenses feed R1/R4. State,
replay, lifecycle, and data-integrity lenses feed R2. Protocol, safety, and
version lenses feed R3. Test strategy and wildcard feed R5/R7. Product,
dependencies, and unproven-assumption lenses feed R6. Repeated discovery from
the same source is not independent corroboration.

Five `always` safety records and one `sometimes` reachability record are active.
One historical `always` record is invalidated. EG1 is retired, with no current
passing payoff verdict or pending campaign. No liveness deadline is justified
for this synchronous slice; measurement is not runtime liveness. Distributed
coordination is narrowly N/A; lifecycle and concurrency remain relevant to ownership.

[Portfolio evaluation](portfolio-evaluation.md) records four completed lenses
from independent analyst `ses_f6756093fffeVjNp36S3E8pKrM`, local validation,
applied refinements, and open owner questions. It is not runtime exercise.

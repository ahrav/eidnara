# Fusion identity contract

`crates/retrieval/src/fusion/` implements this document. It freezes the
identities the RP2.7 fusion boundary ranks by, groups by, selects, and binds
edits to, and the admission rules a lane ranking must pass before fusion sums
it. RP2.7.U2 (fusion arithmetic), RP2.7.U3 (the daemon query route),
RP2.7.U4 (prepare and confirm), and RP2.8 (packing and grouping) consume these
types and must not invent a second identity for any of them.

Changing any rule below, or any `-v1` domain tag, requires the matching edit
to this document in the same change and a new domain tag for every digest
whose bytes change.

## Ranking unit

The ranking unit is the kernel occurrence identifier: SHA-256 over the
length-delimited occurrence tuple defined in
`docs/properties/search-projection/construction-contracts.md` CC4, spelled as
64 lowercase hex characters. `OccurrenceId::parse` admits exactly that
spelling. Uppercase, prefixed, truncated, or extended spellings are refused,
never normalized, so one occurrence has one spelling everywhere.

Payload identity (CC5) deduplicates bytes only. Two occurrences with equal
payload bytes at different source, revision, representation, or span
identities are two ranking units.

`OccurrenceId` orders by digest bytes. That byte order is the tie order every
ranking in this contract uses.

## Lanes

`Lane` is the closed set `Exact`, `Lexical`, `Dense`. `Lane::ORDER` is that
sequence and is the one order fusion sums lane contributions in. The lane
codes `exact`, `lexical`, and `dense` are the spelling a future wire encoding
uses; no wire literal carries them yet.

`RawScore` is a lane's own value for one hit and names its lane:
`Exact` (membership, no score), `Lexical(f64)` (FTS5 rank, lower is better),
`Dense(f64)` (similarity, higher is better). Scores are compared only within
one lane and are retained beside fused scores unchanged.

## Lane rankings

`LaneRanking::consolidate(lane, encoding_version, hits)` turns any multiset of
hits from any number of probes or generations into the lane's declared
ranking:

1. Every hit's score kind must match `lane`; a foreign kind is refused.
2. Every score must be finite; NaN and infinities are refused.
3. `encoding_version` must equal `kernel::source_identity::OCCURRENCE_ENCODING_VERSION`;
   any other stamp is refused, because identifiers minted under another
   encoding do not name the same occurrences. This also refuses mixed
   versions across lanes, since every ranking passes the same check.
4. Each occurrence keeps its best score in the lane's own direction.
5. Entries order by score, best first, then by occurrence identifier bytes.
6. Positions are assigned once as `1..=n`.

Duplicated or permuted hits therefore produce a bit-identical ranking.

The exact lane returns a set in key and page order with no rank. Its
declared ranking follows from rule 4 and rule 5: every member ties, so the
ranking is the members in occurrence-identifier byte order with positions
`1..=n`. This is the parent specification's Q1 identity decision.

`DeclaredLanes::admit(rankings)` holds at most one ranking per lane in
`Lane::ORDER`. A second ranking for one lane is refused.

## Fusion arithmetic

`FusionParameters::new(LaneWeights { exact, lexical, dense }, k)` refuses
before any scoring when a weight is negative or not finite, when `k` is not
positive and finite, or when the finite inputs would sum to infinity at rank
one. A negative-zero weight is admitted as `+0.0`, so an
all-zero parameter set has one spelling. `k = 60` and equal weights are
calibration points for tests, not defaults; the caller supplies the
parameters and the fused union bound with no defaults.

`fuse(lanes, parameters, bound)` returns one `Fused` ranking:

1. The union of occurrences across lanes is built one entry at a time and
   refused with `UnionExceeded { bound }` before the first occurrence past
   `bound` is materialized.
2. `score(o) = sum_lane(weight_lane / (k + position_lane(o)))`, summed in
   `f64` left to right in `Lane::ORDER`, one term per lane. A lane that did
   not rank the occurrence, and a lane that was not declared, adds nothing.
   The reference oracle sums terms; it never evaluates a closed fraction.
3. Fused order is descending score, then ascending occurrence-identifier
   bytes. Fused positions are `1..=n` in that order.
4. Each entry keeps every lane's own position and raw score unchanged.

An undeclared lane, including a dense lane reported unavailable, contributes
zero and is listed by `Fused::undeclared_lanes`; why it was undeclared is the
route's knowledge, and fusion never treats it as an error. An incomplete lane participates with the entries it reached; a
`LaneRanking` carries no completion state, so the route reports each lane's
completion beside the ranking.

`Fused` exposes no path back to lane rankings and `Fused::filter` leaves every
survivor's position and score unchanged, so a revalidation pass never rescores
and fusion runs once per query.

## Query route

`retrieval.query` in `crates/daemon/src/query_route.rs` is handler business
semantics behind the existing `method` envelope; the host wire protocol is
unchanged. A request carries `query`, `remaining_ms`, `destination`, and an
optional `harness` claim, and nothing else. The route binding decides scope:
the project root and session are compared before the body is parsed, and a
`harness` claim that disagrees with the bound harness is refused. The route is
enabled by installing a `QueryRouteLimits` set through
`Handler::set_query_route_limits`; with none installed every authorized request
whose kernel store is ready receives the `disabled` terminal and no canonical,
projection, or lifecycle state is read, so rollback is disable, not mutation. A
request that arrives while the store is still starting or unavailable receives
the kernel routes' `state` answer for that condition, as every `kernel.*` route
does, because the binding is decided before the limit set is consulted.
`QueryRouteLimits::validate` refuses a `validation_batch` or `lexical_accepted`
over the kernel's candidate batch, a `response_bytes` below the smallest
empty fused envelope (one lane complete, the others undeclared, no entries),
and a `response_bytes` above the host wire body maximum, so no installed set
can produce an answer the route cannot bound or the host cannot send.

The exact lane reads the query's `id:` mentions and the lexical lane scans the
prose outside selector mentions inside one interruptible projection read under
the request budget; for those two lanes the projection connection is released
before any kernel reader is taken. The handler awaits the blocking work through
`SharedBudget::bridge`, which raises the shared `EvalBudget` flag the moment
the host cancels, so a kernel or retrieval stage holding only that budget
stops on a host cancel instead of running to the deadline; the work is still
joined before the request settles. When a dense limit set is installed and the
query carries prose outside its selector mentions, the query is first embedded
in process by the daemon's embedding lane as one tracked blocking step awaited
in the handler; the dense lane then runs inside the same read through
`DenseProducer`, whose first implementation is the exhaustive f32 oracle. The
oracle judges each page it scores and re-judges its top-K before returning, so
it is the one lane that holds a kernel reader under the projection connection;
it hands the terms it judged each row under to revalidation. A selector-only
query is never embedded and leaves the dense lane `undeclared`, as it does the
lexical lane. An embedding lane that is busy, starting, disabled, failing, or
that refuses the input or now serves another identity, leaves the dense lane
`unavailable` and the answer `degraded`; the lane's own typed refusal decides
the reason, so a lane that changes state during the call reports that state.
Inference that runs and fails, including an artifact the backend declares
unusable, or a stored vector outside the generation's layout ends the request
as `lane_unavailable` with reason `embedding_failed` or `dense_corruption`.
The exact and lexical lanes are then admitted by the kernel's eligibility
adapter under the bound scope: the lexical lane through
`retrieval::lexical::admit`, the exact lane by judging its rows in
`validation_batch` slices before any position is assigned, so a row the caller
may not see earns no lane position and consumes no union slot. The exact
lane's page bound counts every row `exact::page` reads, tombstoned or
ineligible rows included, because the kernel judges only after the projection
connection is released; `page_bound` names a read bound, not a visible-row
count. Fusion
runs once, the fused set is revalidated by the same adapter in
`validation_batch` slices, and only survivors are materialized within
`result_rows` and `response_bytes`. Every judgment refuses to join verdicts
from two kernel snapshots or incarnations: a lane whose kernel moved between
its slices is `unavailable` with reason `snapshot_changed` or
`kernel_incarnation_changed`, and a revalidation whose kernel moved is the
`lane_unavailable` terminal with the same reason. A stored occurrence
identifier outside the contract spelling makes its lane `unavailable` with
reason `identity` rather than shrinking the result. No payload byte is read by
the route.

A fused answer is
`{"kind":"fused","degraded":bool,"lanes":{...},"truncated":bool,"entries":[...]}`.
Each lane reports `complete`, `incomplete` with a reason, `unavailable` with a
reason, or `undeclared`; reasons are closed codes chosen by the route, never
engine text; `degraded` is true when a lane is incomplete or unavailable. Each
entry carries `occurrence_id`, fused `position`, fused `score`, and per-lane
`position` and `raw` score; positions are the fused positions, so an entry
revalidation withheld leaves a gap. Because both lanes are admitted before
fusion, such a gap can arise only from a canonical change between admission
and revalidation within one request, never from rows the caller was never
allowed to see. The envelope is measured before any entry is added; an
envelope alone over `response_bytes` is the `lane_unavailable` terminal with
reason `response_bytes`, never a body over the bound. A terminal answer is
`{"kind":"terminal","terminal":<code>}` with `code` one of `unauthorized`,
`deadline`, `cancelled`, `lane_unavailable`, `required_context_failure`, or
`disabled`; a `lane_unavailable` terminal adds a `reason` code naming the
witness. A malformed request is the transport's `invalid_params` error.

## Preparation and receipts

`retrieval.prepare`, `retrieval.apply`, and `retrieval.confirm` in
`crates/daemon/src/edit_receipts.rs` carry a selection from ranking to a
confirmed edit; their literals are context-application protocol 1 in
`docs/host-wire-protocol.md` Section 7.8. A preparation binds the RP2.7.U1
preparation digest over the caller's context and mints a per-preparation
identity whose fingerprint covers daemon incarnation, context revision,
action, selection digest, and accounting profile. An apply that restates a
different context is `stale_preparation` before any forward; a retry under the
same identity returns the recorded state and forwards nothing; a retry with
another digest after a forward is `conflict`. The receipt completes only on a
confirm whose applied identity equals the identity the daemon forwarded; a
key from another daemon incarnation or a lost acknowledgment is `unknown` and
stays so until such a confirm. Receipts are in memory, keyed by the route's
bound project, bounded per project by count and store-wide by retention, and
an evicted key or a key of another project is refused rather than replayed.

## Probe and generation identities

`ProbeOrdinal(u32)` is one compiled query atom's zero-based position in its
request. `GenerationId` is one immutable vector generation spelled as its
registered `generation_id`, constructed through `GenerationId::parse` or
`TryFrom<&VectorGeneration>`. Both apply the kernel identity-value rule
(nonempty, at most `MAX_IDENTITY_VALUE_BYTES`, no control characters). The
daemon applies the same rule to a consumer binding's `generation_id` before it
records a lifecycle intent, so every generation this daemon registers has a
`GenerationId` spelling and `TryFrom` cannot refuse a live generation. Both are
provenance. Neither is a ranking unit and neither enters a lane ranking entry,
so a probe or generation cannot vote more than once.

## Parent groups

`ParentId` is the whole-buffer lineage digest of an occurrence's source at
one representation: the kernel lineage encoding (role byte `0x01`, revision
omitted) with no span, hashed with SHA-256. Every span occurrence of one
source at one representation shares it. A whole-buffer occurrence's parent is
its own lineage identifier. Because it is a lineage-role digest it never
equals an occurrence identifier.

`ParentGroupKey` is the parent plus the occurrence's own revision, derived
only through `ParentGroupKey::derive(tuple, class, revision, representation, span)`.
The revision, representation, and span witness the stored tuple; a derived
column that disagrees with the tuple bytes is refused. Spans of one source at
different revisions form different groups. This is RP2.8's Q1 decision:
grouping never mixes bytes from two revisions, and a parent key never stands
in for an occurrence.

Which classes group is an RP2.8 decision: only `raw_tool_spans` derives a
grouping key, and `retrieval::packing::Grouping` returns the typed
non-grouping result for every other class. The RP2.8 key adds class and
representation as explicit components over `ParentGroupKey`.

## Selection digest

`SelectionDigest::derive(selection)` is SHA-256 over:

```text
"eidnara-retrieval-selection-v1" 0x00
count64(selection)
  len64(occurrence_bytes) occurrence_bytes   once per entry, in fused order
```

It changes whenever fused order or membership changes.

## Preparation digest

`SelectedSpan::new(occurrence, span, buffer_len)` normalizes a whole-buffer
range to `None` exactly as the kernel does before minting an occurrence, and
refuses a reversed range or one that exceeds the buffer.

`PreparationDigest::derive(inputs)` is SHA-256 over:

```text
"eidnara-retrieval-preparation-v1" 0x00
len64(context_revision) context_revision
len64(context_representation) context_representation
count64(spans)
  len64(occurrence_bytes) occurrence_bytes
  len64(span) span            0x00, or 0x01 start64 end64
len64(selection_digest) selection_digest
```

`ContextRevision` is the harness context's revision token.
`ContextRepresentation` is the surface the invocation is assembled on. The
digest changes when the context revision, the representation, any selected
span, span order, span count, or the selection changes, and two inputs that
split their text into components differently never derive one digest.

## Invocation identity

`InvocationId`, `ContextRevision`, and `ContextRepresentation` are opaque
tokens validated by the kernel identity-value rule: nonempty, at most
`MAX_IDENTITY_VALUE_BYTES`, no control characters. The host mints them; the
daemon binds them to a route.

## No scope

No type in this contract carries a project, session, or harness. Parsing or
constructing an identity yields bytes and never an authorization. The route
binding supplies scope and compares it before any identity is admitted, and a
user-supplied identifier cannot widen that scope.

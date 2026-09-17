# Original-f32 rows, the exhaustive retrieval oracle, and layer resolution

Status: proposed contract, implemented in `crates/retrieval/src/dense/`.

This document states the rules the dense retrieval path applies to the
original f32 vectors of one generation: how a row is stored, which rows a
generation admits, how a score is computed, how rows are ordered, what the
exhaustive oracle reports when it cannot stand for the whole population, and
how the scalar int8 recipe derives from those rows. Every later dense path
(rescoring, compressed scoring, compaction) is checked against this oracle, so
these rules change only with a deliberate contract edit.

Numeric limits are not part of this contract. Callers supply `OracleBounds`
with no default; a missing bound is a compile error, not an experimental
value.

## Row encoding

- A row is `dimension` f32 words. Each word is four little-endian bytes
  (`f32::to_le_bytes`); decoding uses `f32::from_le_bytes`, so every bit
  survives a round trip, including the sign of zero. Nothing canonicalizes
  `-0.0` to `0.0`.
- `occurrence_vectors.vector` in `search.sqlite` holds exactly these bytes.
  `dense::codec::encode` produces them and `dense::codec::decode_shape`
  reads them; there is no second decoder.
- A byte string whose length is not a multiple of four is a truncated word
  and is refused before any coordinate is read.

## Metric

The metric is the inner product (`Metric::InnerProduct`) of the stored f32
coordinates. The generation predicate permits norms within a caller-supplied
tolerance of one; scoring does not divide by those norms. Inner product and
cosine agree mathematically when both norms are exactly one, but accepted
norm differences can change scores and near-tie ordering. The oracle's target
is stored-value inner product, not exact cosine.

No other metric is defined; `dense::score::score` matches the metric
exhaustively, so a new variant cannot fall through to the wrong arithmetic.
The persisted encoding of the metric belongs to the artifact contract, not
to this document.

## The generation predicate `N_gen`

A row belongs to a generation when it satisfies the generation's `RowLayout`:

1. It has exactly `dimension` coordinates.
2. Every coordinate is finite. Finiteness is checked in increasing coordinate
   order, and the rejection names the first failing coordinate.
3. Its squared norm, accumulated in f64 from the f32 coordinates in
   increasing coordinate order starting at `+0.0`, is not zero.
4. `|sqrt(squared norm) − 1| <= unit_norm_tolerance`. The bound is inclusive.

`dimension` is the generation's `vector_dimension`. The tolerance is a value
of the layout, not a constant of this crate: the daemon supplies the inference
owner's tolerance for the generation, and tests supply their own. A tolerance
that is negative or not finite is refused before anything is read. A stored
row or a query row that fails the predicate is refused before scoring. The
oracle refuses the whole request when a stored row of the generation fails,
because a generation with an invalid member is not the generation the caller
asked about; rows of earlier pages have already been scored and discarded at
that point, as have the rows of the refusing page visited before the failing
one; no row of the refusing page is judged or returned.

## Scoring arithmetic

`dense::score::inner_product(query, row)`:

- Each product is formed in f64 from the two f32 coordinates.
- Products are summed into one f64 accumulator in increasing coordinate
  order, starting from `+0.0`.
- Unequal lengths are a programming error and panic; they never truncate.
- There is no fused multiply-add. Rust does not contract `a * b + c`, and the
  code writes the product to a local before adding it.
- No reassociation, pairwise summation, or SIMD reduction.

Every dense scoring path uses this function or reproduces it exactly, so the
same query and row yield the same f64 everywhere.

`dense::score::score_block` reproduces it for eight rows at once, one lane per
row (`BLOCK_ROWS`). Each lane forms the same f64 products from its own row and
adds them in increasing coordinate order from `+0.0`; no lane is ever combined
with another, and the same holds for the sum of squares each lane accumulates
for the generation predicate. A row of finite coordinates therefore scores to
the same f64 bits through `score_block` as through `inner_product`, and its
sum of squares equals the one `N_gen` accumulates. Only the NaN payload of a
row with a non-finite coordinate may differ between the two, because IEEE 754
leaves payload propagation to operand order; such a row is refused before its
score is consulted, and the scalar scan names its first non-finite coordinate.
`tests/dense_properties.rs` pins the bit equality over every f32 exponent and
both signed zeros. The exhaustive oracle scores through `score_block`;
`rescore` and every single-row path use `inner_product`.

## Ordering

`dense::score::rank_order` orders by score descending, then by occurrence
identifier bytes ascending. Scores compare with `f64::total_cmp`; no epsilon
or tolerance is applied. Two rows tie only when their scores are bit-identical,
and the tie is broken by the identifier alone.

## The exhaustive oracle

`dense::oracle::exhaustive` ranks the live dense-required rows of one
generation inside the caller's read transaction:

- The population `R` is every live occurrence whose class requires a vector
  (`batch::dense_eligible`), visited in occurrence identifier byte order
  regardless of class, through bounded keyset pages of `page_rows` rows over
  the `occurrences` primary key. Visit order therefore equals tie order.
- Each page is decoded, validated, and scored in blocks of up to eight rows
  in visit order; a block is scored when it is full, when the page ends, and
  before an ended budget is acted on, so the rows visited before the budget
  ended are validated exactly as they would have been one row at a time. The
  first row of the page that fails the layout refuses the request, whatever
  block it sits in. Every row with a vector has
  its identity fields validated as a kernel candidate before its score is
  consulted, so a corrupt row is refused (`Kernel`) whatever `page_rows` is.
  Only the rows that would enter the top-`k` set as it stood before the page
  are judged for canonical eligibility, in one kernel batch; the eligible
  ones are then offered to the set. A page with no such row runs no batch.
  Eligibility precedes admission;
  enumeration order and score never decide eligibility, only whether a row is
  judged at all. The returned set equals the one a walk judging every row
  would return: a member of the final top-`k` outranks the worst held member
  at every earlier point of the walk. `Consumed.judged`, `batches`, and
  `excluded` describe the judged rows, not the population; `Complete` means
  every live required row was visited with a valid vector and every judged
  row was judged under one snapshot, not that every row was judged.
  A row without a vector is counted and neither judged nor scored.
- The top-`k` set is re-judged in one batch before it is returned. A row the
  kernel no longer admits is dropped and counted as an exclusion.
- A kernel snapshot or incarnation that differs from the first batch ends the
  walk, as does a batch whose classification generation is unknown. The
  admissions of the batch that moved are discarded; its exclusions are still
  counted as judged work. The result is `Incomplete(SnapshotChanged)` or
  `Incomplete(KernelIncarnationChanged)`, never `Complete`.
- A live required row without a vector of the generation is a dense-coverage
  shortfall. It is counted as `missing_pending` when durable work for it is
  still open and `missing_without_pending` otherwise. It is never a policy
  exclusion and it never makes the result complete.
- Dense coverage, pending work, and policy exclusions are
  reported in separate fields and never merged.
- `max_rows` bounds the visit; reaching it with rows remaining yields
  `Incomplete(RowBound)`.
- The request budget is checked per row, per page, inside every kernel
  judgment, and once more after the final re-judgment. A budget that ends
  before the first page is a refusal; one that ends later yields
  `Incomplete(BudgetExhausted)` with no ranked rows, whatever was known
  before it ended.
- A population of zero rows is a complete, empty ranking.

One `IncompleteReason` is reported. A walk that stopped early names its stop
(`RowBound`, `SnapshotChanged`, `KernelIncarnationChanged`);
`BudgetExhausted` replaces any of them because an ended budget means the
result was not finished; `DenseCoverageShortfall` is the reason only when the
walk otherwise completed. The shortfall itself is always visible in
`coverage`, whatever the reason.

Refusals, all raised before any row is scored unless stated: `k` or
`page_rows` above the kernel's eligibility batch (`BatchOverBound`); a
tolerance or query row outside the layout (`Query`); a generation that is
unregistered, retired, or registered under another identity (`Projection`);
a budget that ends before the first page (`BudgetExhausted`); a kernel error
(`Kernel`); and a stored row outside the layout (`StoredRow`, raised during
the walk).

The oracle reads `occurrences`, `occurrence_tombstones`, `occurrence_vectors`,
`embedding_jobs`, and `vector_generations`. It never reads payload bytes.

`dense::score::rescore` ranks an already-selected set of original rows with
the same arithmetic and order after validating the query and every row
against the layout, for rescoring candidates from another path.

## The scalar int8 recipe

`dense::scalar` implements recipe `scalar-int8-symmetric.v1`
(`ScalarRecipe::SymmetricInt8V1`). A change to any rule below is a new recipe
identifier; persisted codes are never reinterpreted under another recipe.

### Calibration

`dense::scalar::calibrate(layout, rows)` takes the generation's original rows,
each of which must satisfy `N_gen`, and produces one scale per coordinate:

- `max_abs_j` is the largest `|x_j|` over the calibration rows, taken in f32.
- `s_j = max_abs_j / 127`, computed as one f32 division, so the stored scale
  is the correctly rounded quotient of the two f32 values.
- A coordinate that is zero in every row takes `s_j = 1`.
- A coordinate whose `max_abs_j` is nonzero but whose quotient is `0` in f32
  is refused (`ScaleUnderflow`); a scale of zero would make every value of
  that coordinate a division by zero.
- Zero calibration rows are refused (`NoRows`). An empty generation has no
  calibration provenance, so it has no scales.

The calibration identity is the recipe, the number of calibrated rows, and
the SHA-256 of the encoded scales. Scales encode as four little-endian bytes
per coordinate, the same word encoding as a row; decoding refuses a truncated
word, a wrong dimension, and any scale that is not a positive finite number.
Subnormal positive scales are admitted: their squares stay normal in f64 and
every quotient stays finite. A layout whose tolerance is negative or not
finite is refused before any row is read, by calibration and by encoding.

### Encoding

`dense::scalar::encode(layout, scales, row)` validates the row against
`N_gen` and the scales against the layout's dimension, then for each
coordinate:

- widens `x_j` and `s_j` to f64 and divides; the quotient is never formed in
  f32, whose rounding could land a near-tie exactly on a half-integer,
- rounds it to the nearest integer with ties to even (`f64::round_ties_even`),
- clamps it to `[-127, 127]`, counting the coordinate as clipped when the
  clamp changed the value,
- and stores the result as an `i8`.

The code `-128` is never produced. Clipping never changes a scale. Query and
document rows are encoded with the same stored scales of the owning
generation; a query encoded under another generation's scales carries other
codes. Codes encode as one two's-complement byte per coordinate; decoding
refuses a wrong dimension and the reserved byte `0x80`.

### Scoring

`dense::scalar::weighted_dot(scales, query_codes, doc_codes)` computes

```text
sum_j (s_j * s_j) * (i32(c_query_j) * i32(c_doc_j))
```

- The integer product is formed in i32 and lies in `[-16129, 16129]`. That
  range holds because scoring accepts only codes in `[-127, 127]`: the
  reserved code `-128` widens to a product outside it, so `weighted_dot`
  rejects `-128` with a debug assertion instead of scoring it.
- The weight `s_j * s_j` is formed in f64 from the f32 scale.
- Each term is `weight * f64(product)`.
- Terms are summed into one f64 accumulator in increasing coordinate order,
  starting from `+0.0`, with no fused multiply-add and no reassociation.
- Unequal lengths panic; they never truncate.

A plain integer dot is not this score: with per-coordinate scales the two
rank differently, and the fixtures show a pair they order oppositely.
Reconstructed vectors are never renormalized and no other metric is
substituted. Ranking over these scores uses `dense::score::rank_order`, the
same order as the f32 oracle.

### Fidelity

For a query `q` and a document `d` whose codes were not clipped,

```text
|Σ q_j d_j − Σ s_j² c_q,j c_d,j| ≤ Σ_j |q_j| · |d_j − s_j c_d,j| + |s_j c_d,j| · |q_j − s_j c_q,j|
```

in real arithmetic, and each unclipped reconstruction error `|x − s c|` is at
most `s / 2`. The two f64 scorers each round their terms and partial sums, so
the difference between the returned scores can exceed this bound by their
accumulation roundoff, at most about `n · 2⁻⁵³ · Σ|term|` per side for `n`
coordinates; a two-coordinate fixture in `tests/dense_scalar.rs` meets the
real bound with equality and lands one ulp past it. The fixtures assert the
bound plus that allowance with the document residual taken as `s_j / 2` and
the query residual taken exactly, so a clipped query is covered too, and they
check that pairs whose exact scores differ by more than both bounds keep
their order under the int8 score. Pairs closer than the bound may reorder;
that is the loss the RP2.9 fidelity campaign measures, and it is not decided
here.

Pinned bytes for a fixed corpus live in `tests/dense_scalar.rs`. A change to
them is a change to the recipe.

## Layer resolution and the layered ranking

`dense::resolve::resolve` turns one base layer and its ordered deltas into the
current occurrence set before anything is judged or scored. Each layer carries
a precedence, the lexicographic pair `(base_epoch, delta_ordinal)`: the base
is ordinal zero and every member of one composition shares the base epoch,
which is the composition's generation epoch; the daemon's reader maps the
composition's base and delta positions onto these ordinals. Resolution is
newest first. A layer's row for an occurrence wins over every older layer's
row for it (those are `superseded`); a layer's tombstone hides every older
row for it (those are `masked`); a newer row after an older tombstone wins,
since the tombstone is older. The order layers are handed in and the order
rows sit in a layer decide nothing, and no score is consulted: the winners
are the same function of the layer contents under every enumeration. Winners
are returned in occurrence identifier byte order, each naming its layer and
row, so a scorer that needs a layer's own scales knows which layer the row
came from.

The resolver refuses, before choosing any winner: no base or more than one;
two layers of equal precedence; a layer whose base epoch is not the base's; a
layer whose snapshot or checkpoint moves before the checkpoint of the layer
before it, or whose snapshot follows its own checkpoint (the same
conservative reading `compose` uses); a layer naming more or fewer
occurrences than it holds rows; identifiers or tombstones out of strictly
increasing order, which also catches repeats; an occurrence a layer both
lists and tombstones; and more rows and tombstones together than the caller's
bound. The resolver checks the total from slice lengths before inspecting
any layer's identifiers or tombstones, so an oversized set cannot consume
content-validation work. Equal-precedence conflicts are refused, not decided:
the owners have not chosen a rule, and a refusal is not a default.

`dense::layered::rank_layers` checks the request budget and the layers' epochs
against the request generation before resolving any layer contents. An ended
budget refuses as `OracleRefusal::BudgetExhausted`. Resolution is synchronous
and does not poll the budget; the oracle checks it again before reading the
projection. If the budget ends while a valid layer set resolves, the oracle
still refuses because no page has completed. The resolved winners then enter
the oracle's own walk. The population, visit order, paging, eligibility batches, top-`k`
admission, final re-judgment, coverage, budget, and completion rules are the
oracle's without change; only the source of each visited row's vector
differs. The walk reads the projection's live dense-required rows in
identifier order and merges them with the winners in the same order: a live
row whose occurrence has a winner takes the winner's row, validated against
the layout as a stored row is; a live row with no winner is a dense-coverage
shortfall, pending or not as the oracle counts it, whether no layer ever held
it or a newer tombstone masked it; a winner whose occurrence the projection
no longer lists as live, or whose class requires no vector, is `revoked` and
never scored, and no older row of its occurrence stands in for it. A walk
whose last page still had rows after it reports the winners past its last
visited row as `unvisited` rather than claiming they are live or not; a walk
that read the last page knows the winners past it are revoked, however the
walk then ended. Canonical eligibility is judged on winners only, and the
final re-judgment rejects a winner whose authority moved after admission
without falling back to any other row of its occurrence. `LayerAccount`
carries the winner, superseded, masked, revoked, and unvisited counts beside
the ranking.

The layered ranking reads `occurrences`, `occurrence_tombstones`,
`embedding_jobs`, and `vector_generations`; it never reads
`occurrence_vectors` or payload bytes. Row bytes come from the caller's
layers, which the daemon verifies before handing them over. Scoring mixed
recipes on int8 codes with each layer's own scales is not part of this
ranking; the winners name their layer so that a later scorer can do so.

## Vector admission

`daemon::vector_admission::Ledger` is the one accounting owner for every byte
vector work holds. It has two pools judged against the runtime manifest's
optional vector limits through the evidence gate: `vector_resident_bytes` for
model memory, tokenizer cache, SQLite cache, embedding text, row scratch,
decoded row buffers, and a view's resident tables; `vector_disk_bytes` for
files being staged and a compactor's working files, on top of what the store
holds. A reservation is atomic against everything already held: the ledger
locks its tally, adds the increment, asks the gate whether the pool's total is
within the limit under the caller's grant, and records the reservation only
on a yes. A disk reservation measures the store itself first, every regular
file at any depth under the generations directory, complete generations,
staging residue, corrupt entries, and other owners' nested payloads alike,
without following symlinks, and counts it with the ledger's own disk
reservations; every disk reserver runs under the lifecycle's exclusive
transaction lock, which is what keeps one reserver's copy in flight from
being counted twice by another's walk. Two reservations racing for the last
bytes cannot both win; a total that leaves the byte domain is refused before
the gate sees it; a class whose limit the manifest does not carry is refused
by the gate as an absent limit; a grant the gate has withdrawn admits nothing
new; a class asked of the wrong pool is refused. Model memory, tokenizer
cache, and SQLite cache are static residents charged once each, and a second
charge of one is refused as a double charge. Dropping a reservation releases
it and nothing else does: cancelling the work that holds one releases nothing
until that work lets go, and output that keeps its view keeps its
reservations.

Staging is judged by two limits, each charged its own bytes: the payload
inventory alone against the admission manifest's `capture_disk_bytes`, as the
construction contract requires, and then the payload inventory plus the
`manifest.json` the store writes beside it against `vector_disk_bytes` as a
disk-pool reservation, taken before anything is copied and released when the
copy is done, since the bytes are then the store's, measured by the next disk
reservation. A manifest the
store already holds is reserved the same way: the store copies the inventory
into a staging temp before it finds the occupant and publishes nothing twice,
so a retry after an unknown outcome needs room for the copy and leaves the
store's total unchanged. Publication admits a composition's delta
count against `vector_delta_count` before anything is staged; the count is
the composition's own, so nothing is held for it, and a delta layer's own
staging is bounded by bytes alone. A reader's view reserves its resident
tables and records the bytes it pins: pins are not reservations, since the
store's total already carries those bytes, and they are what a prune's
readback is reconciled against. `Ledger::reconcile` sets the prune's retained
bytes beside the ledger's pinned bytes, and a readback above them means
readers pin what the ledger was never told about. Ranking reserves one page
of row scratch for the walk's duration in the ledger that holds the view's
tables, taken from the view rather than named by the caller, so the two are
judged against one resident total. Compaction reserves its build's footprint,
the peak resident bytes in the resident pool and every file it writes in the
disk pool, the same way.

Compressed activation, production use or full-corpus publication of
compressed vector layers, is judged by the same gate through
`HookGate::admit_compressed_activation`, which probes the durable lifecycle
record first as every hook admission does, so a stop written by another
owner or before this process started denies activation too. The manifest's
`hooks` object may
carry `search_projection.vector.compressed_activation`; absent is disabled,
and the flag is not a projection hook, so no class coverage or slice runs
under it and the frozen hook map is unchanged. Enabled, the evaluator applies
every gate a dense hook passes and then the compression campaign in the
evidence record: it must be gathered under the current projection identity
and under the binding the daemon itself runs with (build, corpus digest,
quantizer recipe, hardware, and the version of each harness; a binding that
names a version for anything but exactly the known harnesses refuses), not be
revoked,
record the same vector limits the manifest carries (a changed cap
invalidates it), pass every criterion (fidelity, request latency, startup and
cold cache, concurrency, freshness, disk, compaction, cancellation, task
cost), and carry a real full-path trace from each harness showing a real
embedding, the exact, lexical, and dense lanes, canonical validation, fusion,
span grouping, bounded packing, and validated application. A missing, stale,
wrongly bound, failed, revoked, simulated, report-only, or incomplete record
refuses. The evidence record's `compression` section is read as raw JSON and
judged only by the compression gate: a section this build cannot read, a
trace keyed by an unknown harness included, denies activation as a malformed
section and leaves every hook's admission as it was, so the section's shape
never closes the gate. A refresh whose evidence turns an admitted activation
into a refusal withdraws the grant like a changed manifest would. The daemon
supplies no binding of its own here, so activation refuses as missing
whatever the record says; a report fixture proves the evaluator and
authorizes nothing. The vector limits and the activation flag are daemon
vocabulary outside the construction contract's frozen limit and hook sets.

Rollout order. A build without this section refuses a manifest that carries
`vector_resident_bytes`, `vector_disk_bytes`, `vector_delta_count`, or the
`search_projection.vector.compressed_activation` flag as an unknown limit or
hook, and refuses an evidence record that carries a `compression` section as
malformed; either refusal closes the gate for every hook. Deploy the daemon
before the records gain these keys, and a rollback to an older build must
also revert `runtime-manifest.json` and `campaign-evidence.json`. In the
other direction nothing changes: this build reads records without the keys
as before and refuses only the vector work that needs them.

## Compaction

`daemon::vector_compaction` keeps a composition's delta count bounded by
folding a frozen prefix into one new base and republishing it under whatever
deltas followed. The prefix is a view a reader pinned (`vector_reader::acquire`):
its base and deltas are read only through the pinned files, so what
compaction consumed is exactly what it read, and `Cut` names the record, the
base, and the deltas it stood on. Both steps take the shared `Staging` handle,
which carries the lifecycle's exclusive transaction lock, the gate, the grant,
and the ledger.

`compact` first checks every layer's sidecar against the caller's expectation
with the identity check verification uses, so a view of another model space
refuses before anything is reserved, read, or written. A view holding only
its base refuses as `BaseOnly` next: it would compact to itself, and
publishing that would only bump the sequence, once per replay. It then
resolves the view's layers with the resolver and sizes the build with
`vector_generation::footprint`, which lives beside `build` and derives every
file size from the layout `build` writes: the identifier list is serialized
the way the build serializes it and the sidecar is measured from the same
fields with fixed-width hash placeholders, so the disk figure is the inventory
the build will write, sidecar included, whatever the identifiers' lengths or
the identity strings' sizes. The resident figure is the build's peak: the
decoded rows, the row artifact, the codes, the calibration tables, and the
serialized payloads; compaction adds its own per-row state on top. Both are
reserved in the ledger before any row is read. Compaction then reads each
winner's row by offset and builds one base of exactly those rows in
identifier order at the checkpoint of the prefix's last layer. Every
superseded row and every masked row of the prefix is absent from the new
base, and every tombstone of the prefix is applied by that absence, so the new
base carries no tombstones; the tail keeps its own tombstones and its own
order above the new base. Rows are copied bit for bit through the original-row
codec, so the same prefix compacts to the same bytes and the same digest. The
resident reservation ends with the build, since the rows are then on disk; the
files stay reserved as disk scratch until the compacted output is discarded or
dropped, either of which unlinks the build's own files and removes the work
directory. Discard removes only the build's own file names, whoever wrote
them, so a retry succeeds over a crashed attempt's leftovers; a directory
holding other names is left standing and reported. A refused reservation
writes nothing, and a build that fails after writing some of its files, or
whose inventory exceeds its footprint, removes them before returning, so no
refusal leaves an uncounted file behind. A dedicated, empty work directory per
compaction is the caller's contract.

`publish` reads the selected composition's record back under the exclusive lock
and refuses as `PrefixMoved` unless the selection still stands on the cut's
base with the cut's deltas as a prefix, before staging anything. The cut's own
members are not opened again: the view verified and pinned them at acquisition,
and none of them belongs to the new composition. Whatever deltas follow them
are the tail, and their count is the new composition's delta count: it is
checked against the reader bound and admitted through the ledger before a
member is opened or the base is staged. Published independently of the
compactor, each tail delta is verified as a
member and carried over unchanged and in order, so a later insert, update, or
delete keeps its precedence and is represented exactly once. Only then is the
new base staged and the new composition, at the selection's sequence plus one,
sent through `vector_composition::publish`, so the delta count is admitted and
the selector moves in one rename as for any publication. The record files are
removed from the publication's work directory once the store has copied them,
so one directory serves every attempt. A failure before the rename is a known
refusal and a retry from the same cut publishes once, with the base and the
record staged again for nothing. A failure at or after the rename is an
unknown outcome the caller settles with `vector_composition::reconcile` before
any retry; a retry then finds the selection standing on the compacted base and
refuses, so no second history is published. A cut whose base has already moved
was compacted or replaced by someone else and refuses the same way.

The ledger refuses the next delta past `vector_delta_count`, and a namespace
at the cap admits no delta until a compaction clears it. Every layer a view
pins holds at least one row, and the newest layer's rows are never hidden, so
every acquirable prefix resolves to at least one live row. Compaction
re-encodes the winners under a fresh calibration, so the new base's scales
are its own and a tail delta's codes keep scoring with that delta's scales.
That calibration is the recipe's, and the winners can fail it where each
layer alone did not: a coordinate whose nonzero largest winner magnitude
divided by 127 rounds to zero in f32 has no int8 scale, and the build refuses as
`Build(Calibration(ScaleUnderflow))` before writing a file, its reservations
ending with the refusal. Such a live set cannot be published as one layer by
any path, so the refusal is a property of the rows, not of the compactor;
retrying does not clear it, and neither would a fresh base. Scheduling
compaction when the cap is reached belongs to the maintenance owner and is
not part of this module.

## The pinned reader

`daemon::vector_reader::acquire` turns the selected composition into a view a
ranking can run against, all or nothing. It takes the lifecycle's shared
transaction lock only around manifest reads. Under one hold it observes the
selector, lists the candidates `vector_composition::candidates` orders (the
selected composition first, then other records newest first), pins the first
candidate's record and every member its members file names with a shared lock
on a directory descriptor opened by a manifest read alone
(`GenerationStore::pin`), and reserves the bytes the view will keep decoded in
memory (identifiers, tombstones, scales, sidecar, at their manifest-declared
sizes, the set `vector_generation::RESIDENT_FILES` names) in the ledger's
resident pool and records with the ledger the bytes it pins. It then releases
the lock and verifies the candidate as
`vector_composition::verify_composition` does, hashing every member under the
pins alone; a candidate that does not verify gives up its pins and
reservations and the next is pinned the same way. On success the validated descriptors take
their own shared locks before the manifest-read pins go, the row and code
artifacts stay open on the descriptors verification hashed them through,
together with the tables it decoded, and the selector is re-read under one
more brief hold of the shared lock before the view is handed out. Verification
already proved each artifact holds exactly one row per identifier, so every
offset the reader will compute lies inside it. A selector that no longer names
what acquisition observed, a ledger that cannot hold the resident bytes, or
any store refusal returns nothing, and the pins, descriptors, and reservations
are dropped with the failure.

The lock discipline is the lifecycle's: a shared holder is meant to be a brief
probe, and a mutator taking the exclusive lock gives up after a bounded wait
of a few tens of milliseconds. The reader therefore never holds the shared
lock across hashing, whose duration grows with the corpus; a publisher or
pruner that runs while a reader verifies is not held off, and the reader
observes the moved selector at handoff and refuses, leaving its caller to
acquire again. The same bounded wait applies to the reader's shared
acquisitions: a mutator that holds the exclusive lock past it refuses the
reader as `Unprotected`, and the caller retries. Acquisition still hashes every
member, so it runs on a blocking thread, and a view is acquired once and shared
rather than taken per query. Once the view exists its pins alone keep the
record and members in place. `prune` takes no lock of its own; it relies on
every mutator holding the exclusive transaction lock, and skips a pinned
generation, reporting the ones it would otherwise have removed and their
manifest-declared bytes as retained. A pinned generation that is also protected
is not counted, since protection alone keeps it.

The view's layers map the composition onto the resolver's precedence, the
base as ordinal zero and each delta by its position, all under the
composition's generation epoch. Rows and codes stay on disk: a layer answers
the resolver's row requests by reading one row's bytes at
`header + index * dimension * 4` in the row artifact into scratch and decoding
them through the original-row codec, and answers code requests by reading
`index * dimension` in the code artifact and decoding through the scalar
recipe, so a winner's codes score with its own layer's scales and no other's.
An index at or past the declared rows, a short read, or bytes the codec
refuses fail that row; nothing is reinterpreted. These positioned reads are
not re-hashed: pins protect lifetime, not contents, and a same-user write
after verification is outside the cooperative-file threat model.

`vector_reader::rank` checks every layer's sidecar against the request's
expectation with the same identity check verification uses, reserves one page
of decoded rows plus one raw row of scratch in the ledger for the walk's
duration, and runs `dense::layered::rank_layers` over the
view's layers inside the caller's projection read transaction; canonical
eligibility, revalidation, coverage, and completion are the layered
ranking's. A caller runs it through the request's existing blocking seam
(`RequestCtx::run_blocking`, which the host's drain joins) with the
`Arc<PinnedVectors>` moved into the work, so the view, its pins, and its
reservations live until the physical read returns whatever happens to the caller's
future, its deadline, or its cancellation, and whoever keeps the ranking's
view keeps its pins until that view is dropped. The reader does not authorize
anything: physical ownership of the files says which rows exist, and the
kernel's verdicts say which may be returned.

## The immutable vector generation

`daemon::vector_generation` builds one generation from a
`retrieval::dense::export::live_rows` export: every live dense-required
occurrence with a vector of the generation, validated against the layout, in
occurrence identifier order, together with the generation and kernel
incarnation the export checked against the projection and the projection
checkpoint the same read transaction observed. The export walks the
generation's `(generation_id, occurrence_id)` index in order and never sorts.
`build` refuses an export whose generation or kernel incarnation is not the
caller's expectation and stamps the sidecar's generation identifier, epoch,
model, tokenizer fingerprint, and kernel incarnation from the export, so the
provenance the sidecar records is the state the rows came from. The
generation is six files staged through the shared `GenerationStore` under
target `vector-generation`:

| File | Bytes |
| --- | --- |
| `rows.f32` | The original-row artifact: header (`EIDF32R\0`, version `1`, metric code, reserved zero byte, dimension, row count), then the rows in identifier order. |
| `codes.int8` | One two's-complement byte per coordinate per row, in the same row order, encoded under the generation's scales. |
| `scales.f32` | The calibration scales, four little-endian bytes each. |
| `row-ids.json` | A JSON array of the occurrence identifiers, in row order. |
| `tombstones.json` | A JSON array of the occurrence identifiers the layer masks in every older layer, in identifier order; empty for a base. |
| `vector-sidecar.json` | The sidecar, in its canonical byte form. |

The sidecar names every other file by size and SHA-256 and binds the model,
tokenizer fingerprint, dimension, metric, unit-norm tolerance, quantizer
recipe, calibrated row count and scales digest, generation identifier and
epoch, kernel incarnation, the projection checkpoint the rows were taken at,
and the row and tombstone counts. A build refuses rows or tombstones out of
identifier order and an occurrence the layer both lists and tombstones, so a
layer never contradicts itself. A full export of the live population carries
no tombstones; a delta export lists the occurrences tombstoned since the layer
before it. A build still needs at least one row, since the scales are the
calibration of the rows and calibration over no rows is an open owner
question, so a delta that only masks cannot be built until that is settled. Its hash fills the manifest's inputs slot, the compatibility identity's
digest fills the contract slot, and the row artifact's hash fills the payload
slot, so the generation digest is a function of every declared input and two
builds over byte-identical inputs yield the same directory name, the same
bytes, and the same digest. The `GenerationManifest` schema is unchanged.

Verification (`vector_generation::verify`) does not trust the manifest to
describe itself. The store checks inventory, sizes, modes, and hashes; the
verifier then checks that the manifest is a vector manifest, that the sidecar
bytes are canonical, inventory exactly the five payload files, and hash into
the manifest, that the sidecar's checkpoint is one the projection schema
could hold (a non-negative snapshot, a checkpoint at or after it, and a hold),
that every identity field of the sidecar equals the caller's
expectation (model, tokenizer fingerprint, dimension, metric, tolerance,
recipe, generation identifier and epoch, kernel incarnation, and the
checkpoint when the caller names one), and that the payload agrees with
itself under the recipe: the row artifact holds the declared number of rows,
recalibrating those rows reproduces the scale bytes and the calibrated row
count and the scales hash the sidecar records, the identifiers number the
rows in strictly increasing order, the tombstones are as many as the sidecar
says, strictly increasing, and disjoint from the identifiers, and the codes
are exactly the rows encoded under the scales. A generation whose hashes were
rewritten to match changed bytes is refused when the payload itself can reveal
the change. The identifier list is the one payload no other file derives: a
rewrite that keeps it the same length and strictly increasing is
indistinguishable from the original here, so which occurrence each row names
is bound by the export at the recorded checkpoint, not by verification; the
store's owner-only modes exclude other users, a same-user writer is trusted,
and a caller that needs more compares the identifiers with `live_rows` under
the sidecar's checkpoint. A different model space is refused at an equal
dimension. Verification holds a generation's payload in memory and takes a
caller byte bound; a manifest whose files total more than it is refused before
any payload is held. The store's validation has already streamed each file
through a fixed buffer to check its hash, so the bound limits memory, not I/O.

The vector selector is `vector-profile.json`, beside the host and search
selectors. It names a composition, never a layer. A layer's manifest target
is `vector-generation`; a composition's is `vector-composition`, and
`select_vector` refuses any other; the search selector keeps its existing
behavior and checks no target. Any generation may list `members.json`, digests the store must retain with
it: pruning retains every member named by any complete generation in the
store, so a member outlives every record that names it by one prune pass;
discard and exchange repair refuse a member of any record; exchange repair
also refuses to replace a selected record. The store reads only a
generation's manifest and members file for this, never its payload, so a
corrupt payload of a selected generation does not stop pruning; a selected or
pinned generation whose manifest or members file cannot be read, or any
generation of unknown schema, including one behind a directory mode this
build rejects, makes the members unknown, and the store then behaves as with
a quarantined selector: temps only are reclaimed and discard refuses. An owner
selector of unknown schema is quarantined: it may name any digest, so `prune`
removes only temps and counts each such selector, `discard_unselected` and
exchange repair refuse, and selection through that selector refuses, as the
search selector already did. A corrupt owner selector (malformed bytes, a
noncanonical digest, or failed security checks) is an error from every path
that reads it: `prune`, `discard_unselected`, and exchange repair fail before
removing anything, and selection through it fails. Neither stops staging new
bytes or selecting through the other selectors: those touch nothing the
uncertain selector could name, and a host or daemon rolled back to a build
that predates the selector's schema must still be able to launch and publish.
The members rule is owner agnostic: a search seed that lists members retains
them the same way, though no search seed does. The store's selection primitive
checks inventory, sizes, modes, hashes, the target, and that every listed
member validates; the daemon's semantic verification of the composition and
its members precedes selection.

Staging charges the whole payload inventory against the admission manifest's
`capture_disk_bytes` limit under the caller's admission; a denial stages
nothing. The build's work directory is scratch: files are created exclusively
and not synced there, because the store copies and syncs them when it stages,
and a retry uses a fresh directory.

## The vector composition

`daemon::vector_composition` names one base layer and up to a caller-bounded
number of ordered delta layers in one immutable composition generation, staged
through the shared store under target `vector-composition`:

| File | Bytes |
| --- | --- |
| `composition.json` | Canonical record: schema, publication sequence, model, tokenizer fingerprint, dimension, metric, tolerance, recipe, epoch, kernel incarnation, base digest, delta digests in application order. |
| `members.json` | `{"schema":1,"members":[base, deltas...]}`, distinct digests; the lifecycle store reads it to retain every member while the composition is selected. |

`compose` refuses a base whose sidecar counts tombstones, a duplicate member, more deltas than
the bound, a member whose sidecar does not carry the expectation's identity,
and a delta whose checkpoint moves backwards from its predecessor's. This is a
conservative reading of the base/delta checkpoint relation the owners have not
yet frozen: every delta's snapshot and checkpoint are at or after the
checkpoint of the layer before it. Equal-precedence conflicts among members
are not decided here. Base/delta tombstone semantics are those of "Layer
resolution and the layered ranking" above.

`publish` refuses a sequence at or below the selected composition's, stages
the composition under the caller's admission, and moves the selector in one
rename. The sequence gate reads the selected record the same way
`verify_composition` and `recover` do: a selection whose record is not
canonical, does not bind to its manifest, or whose generation fails inventory
validation is not a composition this build accepts, so it sets no sequence
floor and a repair publication proceeds. A selected record of a schema this
build does not know refuses publication as `Quarantined`, because it may be a
later build's selection. It reports how far the attempt got, recorded when each step returns:
`NotStaged`, `Staged` when staging returned the record's digest, `Acknowledged` when
the selector rename returned, `Durable` when the containing-directory sync
returned. A failure carries the last stage reached; a failure at or after
`Acknowledged` is an unknown outcome, and `reconcile` settles it by reading
the selector back, syncing its directory, and comparing digests: `Published`
when it names the attempt, `Other` with whatever it names otherwise,
`Quarantined` when its schema is unknown. An interrupted publication
therefore leaves the old complete selection or the new one, never a partial
or mixed view, and a retry of an acknowledged publication is refused by
sequence without a second record.

`verify_composition` checks the record's target, schema, canonical bytes,
manifest binding, agreement with `members.json`, and identity, verifies every
member with `vector_generation::verify`, and re-checks the topology. A record
of unknown schema is refused as `Quarantined`, not as a malformed record. It refuses
an excessive delta count before opening any member, so the caller's delta bound
limits member-verification work as well as the accepted topology. Each member
is verified under the caller's per-member byte bound, the one
`vector_generation::verify` takes; every member's payload is held at once, so
the memory a verification may hold is that bound times one more than the delta
bound. `recover` takes the same two bounds.
`recover` takes the selected composition when it verifies; otherwise it reads
the manifests of the other generations and, for composition targets, only the
manifest-listed `composition.json` (hash-checked and capped at 1 MiB). Discovery
retains the newest `bound` `(sequence, digest)` pairs, not generation descriptors;
equal sequences use descending digest order. It then fully verifies at most
those `bound` generations, including inventory and member checks, and takes the
first that passes. A candidate whose record is readable but whose other files
are invalid consumes one attempt. Unreadable records are skipped during
discovery. The directory listing and manifest scan still scale with the store's
generation count; `bound` is not a bound on that scan or on total bytes in the
members. Recovery reports the selector as `Stale` or `Absent` rather than
repointing it. A verifying selection is never displaced by a newer composition
that was staged but not selected, and no verifying composition within the bound
means explicit unavailability.

Readers hold a composition the way the store expects: pin the composition
generation, then open every member, then re-read the selector. While the
composition record exists, whether pinned or merely not yet reclaimed, its
members stay retained, so a reader that pinned a superseded composition keeps
its members after a later publication moves the selector. `recover` does not
pin the returned composition: its caller must validate and pin the returned
digest while still holding the lifecycle transaction lock before handing it
to a reader. Releasing the lock first leaves an unselected fallback reclaimable.

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

The metric is the inner product (`Metric::InnerProduct`). Rows are
unit-normalized, so the inner product equals the cosine. No other metric is
defined; `dense::score::score` matches the metric exhaustively, so a new
variant cannot fall through to the wrong arithmetic. The persisted encoding of
the metric belongs to the artifact contract, not to this document.

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
that point, and no row of the refusing page is scored.

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
- Each page is decoded and validated, then judged for canonical eligibility
  in one kernel batch, then scored, then offered to a top-`k` set. Eligibility
  precedes admission; enumeration order and score never decide eligibility.
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
- Lexical presence, dense coverage, pending work, and policy exclusions are
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

- The integer product is formed in i32 and lies in `[-16129, 16129]`.
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

and each unclipped reconstruction error `|x − s c|` is at most `s / 2`. The
fixtures assert this bound with the document residual taken as `s_j / 2` and
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
bound. Equal-precedence conflicts are refused, not decided: the owners have
not chosen a rule, and a refusal is not a default.

`dense::layered::rank_layers` refuses layers whose base epoch is not the
request generation's epoch, then ranks the resolved winners through the
oracle's own walk. The population, visit order, paging, eligibility batches, top-`k`
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
on a yes. A disk reservation measures the store itself first, every byte on
disk under the generations directory, complete generations, staging residue,
and corrupt entries alike, and counts it with the ledger's own disk
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

Staging reserves the payload inventory in the disk pool before it copies
anything and releases the reservation when the copy is done, since the bytes
are then the store's, measured by the next disk reservation. A manifest the
store already holds is charged nothing, so a retry after an unknown outcome
allocates and reserves nothing. Publication admits a composition's delta
count against `vector_delta_count` before anything is staged; the count is
the composition's own, so nothing is held for it, and a delta layer's own
staging is bounded by bytes alone. A reader's view reserves its resident
tables and records the bytes it pins: pins are not reservations, since the
store's total already carries those bytes, and they are what a prune's
readback is reconciled against. `Ledger::reconcile` sets the prune's retained
bytes beside the ledger's pinned bytes, and a readback above them means
readers pin what the ledger was never told about. Ranking reserves one page
of row scratch for the walk's duration. The compactor's entry point reserves
its working files in the disk pool the same way.

Compressed activation, production use or full-corpus publication of
compressed vector layers, is judged by the same gate through
`HookGate::admit_compressed_activation`. The manifest's `hooks` object may
carry `search_projection.vector.compressed_activation`; absent is disabled,
and the flag is not a projection hook, so no class coverage or slice runs
under it and the frozen hook map is unchanged. Enabled, the evaluator applies
every gate a dense hook passes and then the compression campaign in the
evidence record: it must be gathered under the current projection identity
and under the binding the daemon itself runs with (build, corpus digest,
quantizer recipe, hardware, and the version of each harness), not be revoked,
record the same vector limits the manifest carries (a changed cap
invalidates it), pass every criterion (fidelity, request latency, startup and
cold cache, concurrency, freshness, disk, compaction, cancellation, task
cost), and carry a real full-path trace from each harness showing a real
embedding, the exact, lexical, and dense lanes, canonical validation, fusion,
span grouping, bounded packing, and validated application. A missing, stale,
wrongly bound, failed, revoked, simulated, report-only, or incomplete record
refuses. A refresh whose evidence turns an admitted activation into a refusal
withdraws the grant like a changed manifest would. The daemon supplies no
binding of its own here, so activation refuses as missing whatever the record
says; a report fixture proves the evaluator and authorizes nothing. The
vector limits and the activation flag are daemon vocabulary outside the
construction contract's frozen limit and hook sets.

## The pinned reader

`daemon::vector_reader::acquire` turns the selected composition into a view a
ranking can run against, all or nothing. Under the lifecycle's shared
transaction lock it recovers and verifies the composition as
`vector_composition::recover` does, pins the composition record and every
member with a shared lock on its directory descriptor, reserves the bytes the
view keeps decoded in memory (identifiers, tombstones, scales, sidecar, at
their manifest-declared sizes) in the ledger's resident pool and records with
the ledger the bytes it pins, takes each
member's row and code artifacts on the descriptors verification opened and
hashed them through, together with the tables it decoded, re-reads the
selector, and only then releases the shared lock. Verification already proved
each artifact holds exactly one row per identifier, so every offset the
reader will compute lies inside it. A selector that no longer names what
recovery observed, a ledger that cannot hold the resident bytes, or any store
refusal returns nothing, and the pins, descriptors, and reservations are
dropped with the failure. Acquisition hashes every member and blocks on the lifecycle lock, so
it runs on a blocking thread, and a view is acquired once and shared rather
than taken per query: while the shared lock is held no publisher or pruner
can take the exclusive transaction lock, and mutators give up after a bounded
wait. Once the view exists its pins alone keep the record and members in
place. `prune` takes no lock of its own; it relies on every mutator holding
the exclusive transaction lock, and skips a pinned generation, reporting the
ones it would otherwise have removed and their manifest-declared bytes as
retained. A pinned generation that is also protected is not counted, since
protection alone keeps it.

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
occurrence identifier order, together with the projection checkpoint the same
read transaction observed, so the provenance the sidecar records is the state
the rows came from. The generation is six files staged through the shared
`GenerationStore` under target `vector-generation`:

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
the manifest, that every identity field of the sidecar equals the caller's
expectation (model, tokenizer fingerprint, dimension, metric, tolerance,
recipe, generation identifier and epoch, kernel incarnation, and the
checkpoint when the caller names one), and that the payload agrees with
itself under the recipe: the row artifact holds the declared number of rows,
recalibrating those rows reproduces the scale bytes and the calibrated row
count and the scales hash the sidecar records, the identifiers number the
rows in strictly increasing order, the tombstones are as many as the sidecar
says, strictly increasing, and disjoint from the identifiers, and the codes
are exactly the rows encoded under the scales. A generation whose hashes were rewritten to match changed
bytes is refused when its meaning changed. A different model space is refused
at an equal dimension.

The vector selector is `vector-profile.json`, beside the host and search
selectors. It names a composition, never a layer. A layer's manifest target
is `vector-generation`; a composition's is `vector-composition`, and
`select_vector` refuses any other. Any generation may list `members.json`, digests the store must retain with
it: pruning retains every member named by any complete generation in the
store, so a member outlives every record that names it by one prune pass;
discard refuses a member of any record; exchange repair refuses to replace a
selected record. The store reads only a generation's manifest and members
file for this, never its payload, so a corrupt payload of a selected
generation does not stop pruning; a selected generation whose manifest or
members file cannot be read, or any generation of unknown schema, makes the
members unknown, and the store then behaves as with a quarantined selector:
temps only are reclaimed and discard refuses. The members rule is owner
agnostic: a search seed that lists members retains them the same way, though
no search seed does. The store's selection primitive checks inventory, sizes, modes,
hashes, the target, and that every listed member validates; the daemon's
semantic verification of the composition and its members precedes selection.

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
| `members.json` | `{"schema":1,"members":[base, deltas...]}`; the lifecycle store reads it to retain every member while the composition is selected. |

`compose` refuses a base whose sidecar counts tombstones, a duplicate member, more deltas than
the bound, a member whose sidecar does not carry the expectation's identity,
and a delta whose checkpoint moves backwards from its predecessor's. This is a
conservative reading of the base/delta checkpoint relation the owners have not
yet frozen: every delta's snapshot and checkpoint are at or after the
checkpoint of the layer before it. Equal-precedence conflicts among members
are not decided here.

`publish` refuses a sequence at or below the selected composition's, stages
the composition under the caller's admission, and moves the selector in one
rename. It reports how far the attempt got, recorded when each step returns:
`NotStaged`, `Staged` when the store holds the record, `Acknowledged` when
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
member with `vector_generation::verify`, and re-checks the topology under the
caller's delta bound, so a composition current admission would refuse does
not verify. `recover` takes the selected composition when it verifies;
otherwise it reads only the manifests of the other generations to find
compositions, orders them by descending sequence, fully verifies at most
`bound` of them, and takes the first that passes, reporting the selector as
`Stale` or `Absent` rather than repointing it. An acknowledged selection is
never displaced by a newer composition that was staged but not selected, and
no verifying composition means explicit unavailability.

Readers hold a composition the way the store expects: pin the composition
generation, then open every member, then re-read the selector. While the
composition record exists, whether pinned or merely not yet reclaimed, its
members stay retained, so a reader that pinned a superseded composition keeps
its members after a later publication moves the selector.

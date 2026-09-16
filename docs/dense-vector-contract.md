# Original-f32 rows and the exhaustive retrieval oracle

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
generation is five files staged through the shared `GenerationStore` under
target `vector-generation`:

| File | Bytes |
| --- | --- |
| `rows.f32` | The original-row artifact: header (`EIDF32R\0`, version `1`, metric code, reserved zero byte, dimension, row count), then the rows in identifier order. |
| `codes.int8` | One two's-complement byte per coordinate per row, in the same row order, encoded under the generation's scales. |
| `scales.f32` | The calibration scales, four little-endian bytes each. |
| `row-ids.json` | A JSON array of the occurrence identifiers, in row order. |
| `vector-sidecar.json` | The sidecar, in its canonical byte form. |

The sidecar names every other file by size and SHA-256 and binds the model,
tokenizer fingerprint, dimension, metric, unit-norm tolerance, quantizer
recipe, calibrated row count and scales digest, generation identifier and
epoch, kernel incarnation, and the projection checkpoint the rows were taken
at. Its hash fills the manifest's inputs slot, the compatibility identity's
digest fills the contract slot, and the row artifact's hash fills the payload
slot, so the generation digest is a function of every declared input and two
builds over byte-identical inputs yield the same directory name, the same
bytes, and the same digest. The `GenerationManifest` schema is unchanged.

Verification (`vector_generation::verify`) does not trust the manifest to
describe itself. The store checks inventory, sizes, modes, and hashes; the
verifier then checks that the manifest is a vector manifest, that the sidecar
bytes are canonical, inventory exactly the four payload files, and hash into
the manifest, that every identity field of the sidecar equals the caller's
expectation (model, tokenizer fingerprint, dimension, metric, tolerance,
recipe, generation identifier and epoch, kernel incarnation, and the
checkpoint when the caller names one), and that the payload agrees with
itself under the recipe: the row artifact holds the declared number of rows,
recalibrating those rows reproduces the scale bytes and the calibrated row
count and the scales hash the sidecar records, the identifiers number the
rows in strictly increasing order, and the codes are exactly the rows encoded
under the scales. A generation whose hashes were rewritten to match changed
bytes is refused when its meaning changed. A different model space is refused
at an equal dimension.

The vector selector is `vector-profile.json`, beside the host and search
selectors. `select_vector` refuses a generation whose manifest target is not
`vector-generation`; the search selector keeps its existing behavior and
checks no target. Pruning retains every owner-selected generation, discard
refuses one, exchange repair refuses to replace one, and a corrupt or
quarantined owner selector stops the store's mutators for every caller, as
the search selector already did. The store's selection primitive checks
inventory, sizes, modes, and hashes only; the daemon's semantic verification
of a generation precedes selection, and selecting or recovering a complete
composition is a separate contract.

Staging charges the whole payload inventory against the admission manifest's
`capture_disk_bytes` limit under the caller's admission; a denial stages
nothing. The build's work directory is scratch: files are created exclusively
and not synced there, because the store copies and syncs them when it stages,
and a retry uses a fresh directory.

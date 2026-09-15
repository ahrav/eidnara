# Original-f32 rows and the exhaustive retrieval oracle

Status: proposed contract, implemented in `crates/retrieval/src/dense/`.

This document states the rules the dense retrieval path applies to the
original f32 vectors of one generation: how a row is stored, which rows a
generation admits, how a score is computed, how rows are ordered, and what
the exhaustive oracle reports when it cannot stand for the whole population.
Every later dense path (rescoring, compressed scoring, compaction) is checked
against this oracle, so these rules change only with a deliberate contract
edit.

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

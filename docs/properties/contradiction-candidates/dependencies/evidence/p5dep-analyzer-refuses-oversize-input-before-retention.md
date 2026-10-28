# p5dep-analyzer-refuses-oversize-input-before-retention

HEAD `3a3b916b7`. Surface S1.

## Discovery trigger

Plan P5 KTD1 and R3 choose `retrieval::lexical::analyze` as the tokenizer for
both the new descriptor text (up to 64 KiB, `MAX_SOURCE_UNIT_BYTES`) and every
candidate decision summary. The plan's U1 step 1 says "refuse oversize input
before retention (the analyzer's own rule)". The claim comes from the doc
comments at `crates/retrieval/src/lexical/analysis.rs:13-19` and `:76-77`.
Lens: resource boundaries; claimed safety guarantee C1.

## Evidence trail

- `analysis.rs:13-20` `LexicalBounds`: "Bounds checked before any atom is
  retained." `max_input_bytes` "applies across all segments, not per
  segment"; `max_atoms`: "The atom after the last permitted one is refused
  before it is retained."
- `analysis.rs:22-28` `LexicalRefusal::{InputTooLong { bytes, bound },
  TooManyAtoms { bound }}`.
- `analysis.rs:68-70` `analyze(text, bounds)` delegates to
  `analyze_segments(&[text], bounds)`.
- `analysis.rs:78-105` `analyze_segments`:
  - `:82` sums `segment.len()` over segments.
  - `:83-88` returns `InputTooLong` before `Analysis::default()` is built and
    before `atoms(segment)` is called.
  - `:89-103` loop; `:92-96` checks `analysis.atoms.len() ==
    bounds.max_atoms.get()` before `parts(atom)` (`:97`) and `push` (`:101`).
    The check fires when the (max_atoms + 1)th atom is encountered, so exactly
    `max_atoms` atoms are held at refusal and none is returned because the
    function returns `Err`.
- `analysis.rs:122-126` `atoms()` is a lazy iterator over `split`; no
  up-front allocation of all atoms.
- Production bounds: `crates/daemon/src/query_route.rs:1061-1066`
  `lexical_bounds(limits)` maps `query_bytes` and `probes`; every bound is
  caller-supplied (`crates/retrieval/AGENTS.md`: "Every retrieval bound is
  caller-supplied with no default").
- Tests: `crates/retrieval/tests/lexical_analysis.rs:244-277` exact bound
  passes, one byte over refuses, segments sum, `é` is 2 bytes, NUL counts;
  `:279-297` exact atom count passes, one more refuses, segments share the
  count.

## Failure scenario

A regression that truncates instead of refusing (returning `Ok` with the first
`max_atoms` atoms) would let a P5 pass score a descriptor on a prefix and
record a candidate whose matched-atom count is computed over partial input,
while reporting no refusal. A regression that retains before checking (push
then check) would allocate `max_atoms + 1` strings; one extra allocation per
call is small, but the contract "before retention" is what the plan relies
on for its "bounded work per pass" stop condition.

A regression in the byte check (per-segment instead of summed) would let
`analyze_segments` accept inputs whose total exceeds the bound. P5 uses
`analyze` (one segment), so that regression would not affect P5 directly.

## Timing windows and dependencies

None. Pure function; no clock, store, or lock. Depends only on `str::len`
and `char` classification from the toolchain.

## What a test must construct

- Input of exactly `max_input_bytes` bytes (passes) and `max_input_bytes +
  1` (refuses) where the atom count is under `max_atoms`.
- Input with exactly `max_atoms` atoms (passes) and `max_atoms + 1`
  (refuses) where bytes are under the byte bound.
- For the "before retention" clause: an allocation counter or a
  `max_atoms` so small that the retained count can be asserted through a
  custom allocator, or a structural review. The existing tests observe the
  `Err` value only. A property test over random strings with random
  `NonZeroUsize` bounds can assert `Err ⇔ bytes > b || atoms(text).count() >
  a`, with the oracle computing atom count independently via the same split
  rule.

## Investigation log

### Q: Does any production caller pass a bound derived from untrusted input?

- Sources examined: `query_route.rs:1055`, `:1495`, `:1061-1066`;
  `QueryRouteLimits` producers.
- Findings: bounds come from `QueryRouteLimits`, a daemon-side limit set, not
  from the request body.
- Missing evidence: none for this record.
- Conclusion: resolved; the bound is daemon-owned. P5 would supply its own
  constants (plan OQ5-3).

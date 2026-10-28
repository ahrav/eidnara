# cc-oversize-text-refused-whole-never-truncated

Verified at the #1164 branch tip (base `979fb814e`, scorer commits
`05acd0107` and `446c7363f`), and re-verified after the term pipeline moved
to hashed spellings. Surface S-A.

## Discovery trigger

The domain-modeling pass at `3a3b916b7` proposed this record from the
analyzer's refuse-before-retention rule
(`p5dep-analyzer-refuses-oversize-input-before-retention`) and the 64 KiB
publish unit bound. #1163 C5 requires a whole-input refusal that is never a
score of 0; #1164 builds the scorer arm. The pass arm (zero rows and a
counted `descriptors_refused_oversize`) lands with #1167.

## Evidence trail

- `crates/daemon/src/contradiction_candidates/scorer.rs:64-70`
  `DESCRIPTOR_BOUNDS = bounds(65_536)`, `SUMMARY_BOUNDS = bounds(8_192)`;
  `:66-68` a compile-time assertion holds the descriptor bound at or above
  `MAX_SOURCE_UNIT_BYTES` (`crates/daemon/src/harness_sources.rs:642`).
- `scorer.rs:83-96` `bounds`: `max_atoms = max_input_bytes / 2 + 1`, so the
  bounds are `{65536, 32769}` and `{8192, 4097}`. n atoms need at least
  2n - 1 bytes, so `max_atoms` atoms need more than `max_input_bytes`
  bytes: the byte check always refuses first.
- `scorer.rs:103-109` `term_set` calls `analyze_segments(&[text], bounds)?`;
  a refusal returns before any term is built. `DescriptorTerms::new`
  (`:443`) and `score` (`:536`) make the same call first.
- `crates/retrieval/src/lexical/analysis.rs:78-105`: the byte check runs
  before the atom loop and the atom check before retention; neither returns
  a partial `Analysis`.
- `scorer.rs:440-473` `DescriptorTerms::new` returns the refusal;
  `:534-566` `score` maps a summary refusal or an empty term set to
  `SummaryScore::Unscorable` and an analyzed overlap below a gate, zero
  included, to `Below`.
- `scorer.rs:631-648` `score_descriptor` returns `Err` for the descriptor
  (`?` before any summary is read) and lists unscorable summaries by
  decision object id (`:640`). With a repeated object id, one id can appear
  twice in `unscorable`, or in `unscorable` and in `candidates` when
  another entry with that id scores.
- Tests: `crates/daemon/tests/contradiction_candidates_scorer.rs:224-241`
  (descriptor exact fit and one byte over), `:243-274` (summary over bound,
  two floor-emptied summaries, at-bound summary, 4096-atom summary),
  `:100-129` (`Below { matched_terms: 0, decision_terms: 6 }` for disjoint
  texts). `:243-274` asserts the kept ids of the other summaries and does
  not compare their results against a run without the unscorable summary,
  so the record's "result unchanged" clause is unasserted and the record
  is `Exercised: partial`.

## Failure scenario

If the scorer truncated an oversize descriptor to its bound, the scored
terms would come from a prefix and a candidate could rest on text the
descriptor does not end with. If an oversize summary scored as 0, the
caller could not count it as unscorable and a refusal would look like "no
overlap". If the descriptor bound were below the publish unit bound, a
published descriptor would be refused by construction.

## Timing windows and dependencies

None in the scorer. The pass (#1167) owns the counters and the "writes
nothing" half.

## What a test must construct

- A descriptor of exactly 65536 bytes with 32768 atoms, and one byte more
  (present).
- A summary of exactly 8192 bytes and one byte more beside scorable
  summaries (present), and the most atoms 8192 bytes hold (present).
- A summary the floor empties (present: two).
- An analyzed zero overlap compared with a refusal (present).
- The scored result with and without an unscorable summary, compared in
  full (absent).
- An input over `max_atoms` within the byte bound: not constructible, see
  the record.
- Pass arm, for #1167: a refused descriptor writes zero rows and counts
  `descriptors_refused_oversize`; an unscorable summary counts
  `summaries_unscorable`.

## Investigation log

### Q: Which atom bound formula applies?

- Sources examined: #1164 Scope ("`max_atoms` derived as
  `(max_input_bytes + 1) / 2`"), #1164 OQ5-3 (literal values 32769 and
  4097), `scorer.rs:85-96`, the digest pin test at
  `contradiction_candidates_scorer.rs:348-400` (`65536/32769`,
  `8192/4097`).
- Findings: the two ticket statements differ by one; the code and the pin
  follow the literal values. Both formulas leave the byte bound binding.
- Missing evidence: which statement the specification owner intends.
- Conclusion: needs human input (ticket wording only; behavior is the same).

### Q: What are the descriptor and summary bounds? (originally needs human input)

- Sources examined: #1164 OQ5-3, `scorer.rs:64-70`.
- Findings: the ticket fixes 64 KiB and 8 KiB as scaffolding under #1007.
- Missing evidence: the measurement #1007 will supply.
- Conclusion: resolved with answer for this epoch.

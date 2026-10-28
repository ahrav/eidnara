# cc-case-fold-applied-identically-to-both-sides

Verified at the #1164 branch tip (base `979fb814e`, scorer commits
`05acd0107` and `446c7363f`), and re-verified after the term pipeline moved
to hashed spellings. Surface S-A.

## Discovery trigger

The analyzer keeps original bytes and folds nothing
(`p5dep-analyzer-keeps-original-bytes-and-is-pure`), so the scorer must
fold. The domain-modeling pass at `3a3b916b7` asked for one named fold on
both sides. #1164 OQ5-5 fixes it: `str::to_lowercase` per term after
analysis, no diacritic or Unicode normalization.

## Evidence trail

- `crates/daemon/src/contradiction_candidates/scorer.rs:111-132` `terms`
  is the one place that folds and floors atoms and parts. An ASCII term
  keeps its analyzer spelling and stands for its `to_ascii_lowercase`,
  which equals `str::to_lowercase` on ASCII; `scan_ascii` (`:221-270`)
  applies the floor to that lowercase. Any other term is folded with
  `str::to_lowercase` (`:124`) and floored by `kept` (`:147-153`).
  Comparisons fold a spelling with `eq_ignore_ascii_case` and a lowercase
  byte order (`:134-145`).
- `scorer.rs:432-465` `DescriptorTerms::new` takes the descriptor's terms
  from `terms` and stores each folded; `:526-562` `score` takes the
  summary's terms from `scan_ascii` through `count_ascii` (`:483-524`) when
  every term is ASCII and at most 64 are distinct, and from `terms`
  otherwise. Both sides share the fold and the floor; only the bound
  differs.
- `scorer.rs:18-25` the module header states the limits: `İ` lowers to `i`
  plus a combining dot, `ß` stays `ß`, and NFC and NFD spellings are
  different terms.
- `scorer.rs:77` `FOLD_RULE_ID = "str-to-lowercase-per-term.v1"` enters the
  preimage (`:648-683`).
- Tests: `crates/daemon/tests/contradiction_candidates_scorer.rs:131-159`
  `case_folds_identically_on_both_sides_and_diacritics_do_not_fold`;
  `:31-80` `term_sets_follow_the_documented_fold_and_floor` (`ÉCOLE École`
  to one term; `İSTANBUL` distinct from `istanbul`; `THE` lowers to a
  stopword and drops; `İs` lowers to three scalars and is kept);
  `:163-171` `a_case_change_inside_an_identifier_changes_its_parts`
  (`configLoader` gives {configloader, config, loader}; `CONFIGLOADER` and
  `configloader` give {configloader}).
- `crates/retrieval/src/lexical/analysis.rs:193-204` `splits` places part
  boundaries from the original case: at a lower-to-upper change (`:198`)
  and before the last capital of an uppercase run that two lowercase
  letters follow (`:199-201`). The analyzer runs before the fold
  (`scorer.rs:111-132`), so a case change that adds or removes a boundary
  changes the parts, and the term sets differ although the texts differ
  only in case. The fold itself is still one function on both sides.

## Failure scenario

If only the descriptor were folded, `Kernel` in a summary would never match
`kernel` in a descriptor and real overlaps would be hidden. If the floor
ran before the fold, `THE` would survive as a term on one side and `the`
would drop on the other. If the fold also stripped diacritics, `école` and
`ecole` would match, which #1164 rules out. A record that claimed equal
term sets for every case-only difference would be false for
`configLoader` and `configloader`; the record limits that clause to case
changes that keep part boundaries.

## Timing windows and dependencies

None; pure computation.

## What a test must construct

- A descriptor and a summary differing only in case, above the count gates
  (present: 5 terms).
- The summary scored against itself, for equality (present).
- A non-ASCII uppercase pair (present: `É`).
- A pair whose lowercase mapping changes length (present: `İ`, in the term
  set test).
- A case-only pair whose case change crosses a part boundary (present:
  `configLoader`, `CONFIGLOADER`, and `configloader`).
- A diacritic-only pair and an NFC and NFD pair (present; every term of
  each pair differs, so both score
  `Below { matched_terms: 0, decision_terms: 4 }`). The distinctness is per
  term: a text pair that also shares other terms still matches on those.

## Investigation log

### Q: What is F? (originally needs human input)

- Sources examined: #1164 OQ5-5, `scorer.rs:111-132`, `:18-25`.
- Findings: F is `str::to_lowercase` per term, matching the daemon's
  user-hint `lexical_tokens` precedent; no diacritic folding.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Is the score symmetric?

- Sources examined: `scorer.rs:526-562`.
- Findings: the score is containment on the decision side,
  `|A ∩ B| / |B|`, so swapping the texts changes the denominator. The fold
  is applied identically, so term-set equality under the fold is
  symmetric.
- Missing evidence: none.
- Conclusion: resolved with answer: the record checks fold symmetry, not
  score symmetry.

### Q: Do two texts that differ only in case always have equal term sets?

- Sources examined: `crates/retrieval/src/lexical/analysis.rs:4`,
  `:193-204`, `scorer.rs:111-132`, the test at `:163-171`.
- Findings: no. The analyzer splits parts on the original case and the
  scorer folds afterwards, so `configLoader` yields {configloader, config,
  loader} and `configloader` yields {configloader}. Case changes that keep
  every part boundary give equal term sets.
- Missing evidence: whether the case-driven split is intended matcher
  behavior for this feature.
- Conclusion: resolved with answer for the record's wording; needs human
  input on the design (portfolio B2).

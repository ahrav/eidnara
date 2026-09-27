# Portfolio evaluation: window protocol (M1)

## Status

This file records METHOD's independent evaluation of this part (step 5) and
its disposition (step 6). The evaluation ran in a fresh context against the
draft part at `52eda0fa`, after #881, #883, and #884 merged into `main`
(`d68aedf3`); its line references are to that draft and that tree. The
disposition below applies every refinement to the records and evidence,
queues every gap, and surfaces every bias for a human. Code references in the
part are now verified at `f2442b2f`, the last code commit of #833. Catalog
revision 1 of #824 received its own fresh-context evaluation; its eight
required edits are applied in revision 2 (#824 comment 4, section E), which
this part carries. No test-adequacy verdict is issued here.

## Executed state

- Evidence source: the M1 PR descriptions, read with `gh pr view`: #873 to
  #880 on 2026-09-26; #881 (head `1c66f16c`), #883 (head `d7712d75`), and
  #884 (head `f8734c12`) on 2026-09-27, all merged into `main` (last merge
  `d68aedf3`). #833's run is the gate block its own PR description records
  at the final head of `window-protocol/m1-exit`.
- Code tree for references: `f2442b2f` (#833, `window-protocol/m1-exit`).
  `3ebfc3b9` changes only the test-support statement ledger and the #874
  inventory test; references into them are marked at `3ebfc3b9`. The other
  later commits change only documentation.
- 37 records: 35 `Exercised: yes`, 2 `partial` (WP-E10, WP-P14), 0 `not
  yet`.
- Moved to `yes` by #833: WP-E03, WP-P06, WP-P16, WP-P25 (from `not yet`),
  WP-P07 (from `partial`). WP-P15 moved from `not yet` to `partial`, then
  to `yes` once `3ebfc3b9` recorded bytes decoded (Q10). WP-E10 stays
  `partial`: no refused-commit witness. WP-P14 moved from `yes` to `partial` by refinement I2: its tests
  compare tool flags, not the frozen status.
- Measurements: the revision 2 baseline at `43e88bdc` and H sweep at
  `704568ec` (#873, #874; N = 1M INCONCLUSIVE). The revision 3 daemon, RSS,
  meta, and D15 readings are at `cf89a9c2` (#833; Rust 1.98.1, release
  builds); the later commits change only the plugin, the admission scans
  (speed only), and tests. The D15 bytes-decoded readings are at `3ebfc3b9`
  (release, load 4.37). The plugin readings are at `58dbe556`, whose
  production code is identical to `f2442b2f`: three runs at 1-minute load 5.8
  to 7.4, Bun 1.3.14, release `direct_host_fixture`, 30 samples after 10
  warmups per point. Steady whole-hook p99 70.7 to 73.3, 67.2 to 80.9, and
  66.6 to 76.2 ms at N = 10k, 100k, 1M; 1M/100k median 0.968 to 1.082; cold
  start at N = 1M p99 84.7 to 90.2 ms; revert past the boundary two-pass p99
  1,739.1 to 1,848.2 ms; removal inside covered history p99 76.5 to 89.0 ms
  with one id-only scan; the pass after a fold 15/15 `SOFT+` with no retry.
- Criterion readings pending owner confirmation: HARD compared at H = 50k
  against H = 2,500 (≥ K), with H = 100 reported and m0 checked against its
  D14 bound; RSS per idle session read as the slope over k = 8 idle
  sessions; `cache_state.meta` equal across N up to decimal-digit width;
  bytes decoded per D15 row equal across N up to decimal-digit width, except
  m0 and the coverage snapshot's frozen fold render, which follow the fold
  under the D14 cap (`3ebfc3b9`, every row within its declared bound).

## Independent evaluation

The fresh-context evaluation follows as written, with its headings demoted
and its lines wrapped. Its finding ids (H1 to W4) are used in the
disposition below.

### Independent evaluation (METHOD step 5)

**Verdict:** the part's structure passes mechanical verification with one
mismatch, a `TODO-833-PLUGIN` placeholder in three deliverables. The findings
below are mostly consistency corrections. Four need a human decision, and one
gap may block landing.

I read the draft's own evaluation table only after forming the findings below,
and did not rely on its conclusions. Code references were spot-checked at
`52eda0fa`:

- All 10 test-name-plus-`file:line` citations in `catalog.md` and
  `fault-map.md` land on the named test.
- All 12 cited `rust-mode-window.test.ts` lines match their quoted test titles.
- 101 of 113 cited snake_case names resolve at `52eda0fa`. The other 12 are
  tests or symbols that later PRs deleted: `7a8fb84b`, `6477c9f2`,
  `31946f7aa`, and `e592d01a4`. I traced 5 of them to their deleting commit
  with `git log -S`; the other 7 I only checked as grouped entries in
  `existing-checks.md`. The catalog cites these names at their earlier trees,
  as its evidence boundary says it does.

#### Harness fit

**H1 - refinement.** The `sometimes` records are enabling-state assertions
inside deterministic unit tests, not campaign counters. There are eight such
records: WP-P16 to WP-P23. The repository has no campaign runtime, so
`sometimes` in practice means "a named test constructs the state once per
suite run". Also, no `WP-FMnn` token appears in code at `52eda0fa`. The link
between a marker and its test exists only in `fault-map.md`. The daemon
already has one uniqueness test for coverage markers:
`crates/daemon/tests/eval_ingestion.rs:857`.
- *Disposition:* add one sentence under the `fault-map.md` preamble saying
  this. No code change is needed for M1.

**H2 - bias (already recorded).** WP-P15's N = 1M and H = 50k counter equality
can be checked only by the uncommitted driver. CI covers W = 5 at N = 10k and
100k.
- *Disposition:* keep it in the biases list. A human decides whether a
  committed CI-scale counter test is owed.

**H3 - bias (already recorded).** WP-P21's budget witness uses wall-clock
sleeps.
- *Disposition:* keep it surfaced.

**H4 - gap, gating.** Five records moved to `yes` by #833: WP-E03, WP-P06,
WP-P16, WP-P25, and WP-P07. They cite "#833's gate block", which exists only
once the #833 PR description is written. `TODO.md` says the gates ran at
`cf89a9c2`, "five at `52eda0fa`". The diff `cf89a9c2..52eda0fa` touches only
`tail_hygiene.rs`, `transform.rs`, and `memory-store/src/lib.rs`, in one
commit ("Tighten the hash-width upgrade tests").
- *Disposition:* before landing, confirm that the PR description records
  `cargo test -p daemon` (2,747 passed) and names the tree it ran on.
  Otherwise revert these five statuses to `partial`.

#### Coverage balance

Distribution:
- Types: 29 safety, 8 reachability, 0 liveness.
- Surfaces: 15 plugin, 12 daemon, 6 store, 4 mixed.
- Confidence: 23 high, 14 medium, 0 low.
- Markers: 8 of 29 safety records have a paired `sometimes` record.
  `fault-map.md` supplies 22 markers for the rest.

**C1 - gap.** There are no liveness records, although the specification states
two bounded-progress obligations:
- "converge on the next pass with one HARD, at most one extra `revert_epoch`"
  (`824-body.md` Acceptance, interrupted revert).
- "the pass after a fold completes with no error and no retry".

WP-E02 only defers this to an acceptance criterion ("Open questions: None.
Interrupted recut convergence is an acceptance criterion"). WP-E12 checks
epoch safety but not the one-pass bound. METHOD's liveness rules fit this
obligation directly, because the bound is in passes.
- *Disposition:* queue a revision 3 liveness record, bounded to one HARD pass,
  witnessed by `revision_3.rs:1147` and `:1163`. This needs a human decision,
  since the ticket fixes the part at 37 records.

**C2 - gap.** The D10 no-survivor reset is destructive: it deletes segments
and bumps the epoch. It has no record of its own. Its five tests
(`revision_3.rs:447`, `:498`, `:610`, `:635`, `:657`) hang off WP-E03, whose
guarantee covers the opposite path, the `lineage_switched` pass-through. The
relationship map also does not link WP-P10 (null only after a complete walk)
to the reset, and WP-P10 is the reset's safety precondition.
- *Disposition:* add a relationship-map line now ("WP-P10 and WP-P01 gate the
  D10 reset"). Queue a dedicated reset record: reset once under CAS conflict,
  bounded retries, logged range.

**C3 - refinement.** The preamble says "`test-only` marks the seven campaign
markers". There are eight marker records. WP-P16 is labelled
`explicit-config-only`.
- *Disposition:* reword to "eight campaign markers; seven `test-only`, WP-P16
  `explicit-config-only`".

#### Implementability

**I1 - refinement.** Five records say `Open questions: None.` while their
evidence files end in `Conclusion: unresolved`. This breaks the rule that open
questions surface in the record.

| Record | Unresolved conclusion in its evidence file |
| --- | --- |
| WP-E10 | needs a refused-commit test |
| WP-P05 | needs a CK keep witness at a nonzero cut |
| WP-P13 | needs a pass-level witness or an explicit owner decision |
| WP-P14 | needs a frozen-status assertion |
| WP-P23 | needs a pass-level witness or owner acceptance |

WP-P25 says "awaits owner confirmation" under `None.`, and `TODO.md` lists it
as needing a human.
- *Disposition:* copy each unresolved question into the record. Add "(needs
  human input)" to WP-P13, WP-P23, and WP-P25.

**I2 - refinement.** WP-P14 is marked `Exercised: yes`, but its `Check`
requires the freezing behavior to be preserved, and its evidence says the
frozen flag is not asserted.
- *Disposition:* change it to `partial - verdict direction constructed; frozen
  status unasserted`. The exercise distribution becomes 34 `yes` and 3
  `partial`.

**I3 - refinement.** WP-P06's `Check` says "retained identity keys are
confined to the resolved window". The code keeps `cut_prefix` plus
`req.messages`, which is the submitted window (`prune_block_identities`,
`transform.rs:5256-5276`). The code's doc comment says so explicitly.
- *Disposition:* change "resolved window" to "submitted window (cut prefix
  plus resolved messages)" in WP-P06 and WP-P25.

**I4 - refinement.** WP-P25's `Check` says "After the first committing pass
... the map contains exactly the window's mids". The prune returns early on a
Revert unless the plan is HARD or MigrateHard (`transform.rs:5266-5268`). The
record's own `Guarantee` says "first ordinary commit".
- *Disposition:* align the `Check` with the `Guarantee`. A revert's SOFT
  commit is excluded.

**I5 - gap (already queued, confirmed).** WP-E10 has no refusal witness. I
re-derived gaps 2 to 7 of the draft independently, and the evidence supports
each.
- *Disposition:* keep them queued and name owners where possible.

#### Wildcard

**W1 - bias (already recorded).** WP-P12 publishes when host `metadata` is
edited outside the window (#883 deviation). The issue is correctly surfaced as
needing human input.

**W2 - refinement.** The acceptance differential (`revision_goldens.rs:428`)
substitutes a fresh session's expected output after a revert before the first
anchor, and after the deleted covered-drift refusal. `existing-checks.md`
labels this "acceptance differential" without that caveat.
- *Disposition:* add the caveat to that row. A reader could otherwise take it
  for strict equivalence with revision 2.

**W3 - gap (acceptance-level, no record).** The ticket requires that "non-test
search finds no consumer" of the D24 and D25 symbols. No record or existing
check cites that search.
- *Disposition:* record the search result in WP-E12's evidence (memo) and a
  relevant record's evidence (covered-drift), or note it in scope as a
  structural check outside the catalog.

**W4 - note, no class.** `DISPOSITIONS.md` lists edits still owed to other
catalogs: TE18, TE21, TE23, and the history_summarizer fence. These are out of
this part's files, but the draft says they block landing.
- *Disposition:* track them in the #833 PR checklist.

#### Verification results (step 7)

| Check | Result |
| --- | --- |
| `### wp-*` record blocks | 37, no duplicates |
| Index rows | 37. Each anchor equals its slug, and each row's Type, Check, Status, and Exercised match the record. |
| `evidence/*.md` files | 37, same slug set as the records |
| `Confidence` evidence link | 37 of 37 point to `evidence/<own-slug>.md` |
| Record field order | all 12 METHOD fields, in order, in every record |
| Evidence sections | six METHOD headings in every file. Every `### Q:` has Sources examined, Findings, Missing evidence, and Conclusion. |
| Evidence length | 66 to 120 lines, within target |
| Relative links and anchors | none broken |
| Semantics distribution | recorded at `catalog.md:162-163` and in `portfolio-evaluation.md`: 29 `always`, 8 `sometimes`, 0 others. It matches the records. |
| Reachability distribution | 27 / 3 / 7, matching `catalog.md:117` |
| Exercise distribution | 35 `yes`, 2 `partial`, matching `catalog.md:165`. It becomes 34 / 3 if I2 is applied. |
| Fault-map markers | 22 rows, each with an occurrence marker, `WP-FM01` to `WP-FM22` |
| `existing-checks.md` statuses | the audited-check rows are all `unaudited`. The other rows list removed or unresolvable names. The "none found" and "quiet areas" sections are present. |
| **Mismatch** | `TODO-833-PLUGIN` remains at `catalog.md:1386` (WP-P15 Confidence), `fault-map.md:26` (WP-FM07), and `evidence/wp-p15-steady-state-work-is-window-bounded.md:55`. It also remains in the draft `portfolio-evaluation.md:31`, which this section replaces. |
| **Mismatch** | the draft's structural-check claims that "every evidence file ... investigation question" is consistent with its record are false for WP-E10, WP-P05, WP-P13, WP-P14, and WP-P23 (see I1). |

#### Classification summary

- **Gaps:** H4, C1, C2, I5, W3. H4 blocks landing.
- **Refinements:** H1, C3, I1, I2, I3, I4, W2. Apply them before landing.
- **Biases, for a human:** H2, H3, W1, plus the three WP-P15 acceptance
  readings and the WP-P25 table adaptation.

## Disposition (METHOD step 6)

| Finding | Class | Disposition |
| --- | --- | --- |
| H1 | refinement | Applied: the `fault-map.md` preamble states that `sometimes` means a named deterministic test constructs the state once per suite run, that no `WP-FMnn` token appears in code, and cites `crates/daemon/tests/eval_ingestion.rs:857` |
| H2 | bias | Surfaced below |
| H3 | bias | Surfaced below |
| H4 | gap | Queued as Q1. Satisfied by the #833 PR description, which records the gates at the final head, including `cargo test -p daemon` (2,749 passed); the evidence files' #833 gate lines now name that run |
| C1 | gap | Queued as Q2 (needs human input) |
| C2 | gap | Relationship-map line added to `catalog.md` ("Reset preconditions"); the dedicated reset record is queued as Q3 |
| C3 | refinement | Applied: the reachability preamble names eight campaign markers, seven `test-only` and WP-P16 `explicit-config-only` |
| I1 | refinement | Applied: WP-E10, WP-P05, WP-P13, WP-P14, and WP-P23 carry their evidence files' unresolved questions; WP-P13, WP-P23, and WP-P25 are marked "(needs human input)" |
| I2 | refinement | Applied: WP-P14 `Exercised: partial - verdict direction constructed; frozen status unasserted`; index row and distribution (34 `yes`, 3 `partial`) updated |
| I3 | refinement | Applied: WP-P06 and WP-P25 say "submitted window (cut prefix plus resolved messages)", matching `prune_block_identities` (`crates/daemon/src/transform.rs:5256-5280`) |
| I4 | refinement | Applied: WP-P25's `Check` names the first ordinary commit and excludes a revert's SOFT commit (`transform.rs:5266-5268`) |
| I5 | gap | Kept queued (Q5 to Q11) |
| W1 | bias | Surfaced below |
| W2 | refinement | Applied: the `revision_goldens.rs:428` row in `existing-checks.md` states that the differential compares against a fresh session's step after a revert before the first anchor, and has no revision 2 output after a covered-drift refusal |
| W3 | gap | Queued as Q4 |
| W4 | note | Applied by #833 in the same PR: TE18 `Status: invalidated`, the TE21 note, the TE23 memo-copy clause and #832 inventory bullet, the history_summarizer fence pruning bullet, and four stale `rust-mode-transform.ts` citations (see "Existing-catalog dispositions") |
| Mismatch: the plugin placeholder | refinement | Applied: filled from the `58dbe556` plugin run in WP-P15, WP-FM07, its evidence, and "Executed state" |
| Mismatch: evidence consistency claim | refinement | Applied with I1; the structural check below now tests it |

### Author findings

Findings the catalog author recorded while assembling this part, before the
independent evaluation, each classified as METHOD's evaluation would
classify it. Queued gaps in this table are carried into the queue below.

| Finding | Class | Disposition |
| --- | --- | --- |
| WP-E10's 1,000-success and 1,400-refusal characterization no longer exists; `e15a09a6` on `main` replaced it with a flat-meta pin before M1 | refinement | Applied: WP-E10 `Exercised: partial`, correction stated in `Confidence` |
| No witness constructs a refused transform commit with no row and no meta change | gap | Queued below |
| Plugin records WP-E05, WP-E06, WP-E07, WP-E11, WP-P01, and others are `default-production`, while the transform-edit-responses catalog labels its records `explicit-config-only` from the pre-`4061315c` `resolveTransformMode` gate | refinement | Applied here with evidence (`hook.ts:518`, `4061315c`); the TE catalog labels are outside this part and left as recorded |
| #876 and #878 descriptions disagree on the ready-snapshot budget | refinement | Resolved from source: 256 MiB at `f8940a26`, 64 MiB at `f6b3d7bd` and `52eda0fa` (WP-E08) |
| #875's "null-path barrier test" and "unlisted-run test" have no names in the description | refinement | Resolved by content to `a_removal_between_the_core_read_and_the_intersection_is_invisible` and `a_long_run_of_unlisted_rows_does_not_end_the_walk` |
| Tests deleted by later M1 PRs (#876's delta tests, the TE20 baseline, the native attachment cap test, `no_revert_prefix_survives_matches_the_full_prefix_scan`) | refinement | Listed in `existing-checks.md` with the tree where they last ran; no record depends on them except WP-E06's baseline |
| Late commits on #873, #874, #875 (and owner commits in the merged heads of #874, #876, #878 to #880) post-date their descriptions; they are on `main`, so #881's to #884's and #833's gates ran them, but no description records their own numbers | bias | Recorded in the catalog scope; every dependent record says so |
| WP-P13 and WP-P23 witnesses are primitive-level; no discovery pass crosses hostile covered slots | gap | Queued below; #883 adds none (needs human input: accept the primitive witness or add one) |
| WP-P14's tests compare tool flags, not the frozen status the `Check` names | gap | Queued below; #884's tests compare flags only |
| WP-P21's budget witness depends on wall-clock sleeps (300 ms pages, 1 s budget) | bias | Flake risk surfaced; a deterministic clock would remove it |
| #883 stops checking host root properties outside the window (O(N) otherwise); a host `metadata` edit now publishes | bias | Surfaced for a human: #883 records it as an implementer deviation; no owner decision is recorded (needs human input; WP-P12 open question) |
| #833 drafts WP-P25 against a `meta` row near 512 KiB, but `e15a09a6` moved `block_identity_by_mid` into the `block_identities` table (`#[serde(skip)]` at `crates/memory-store/src/lib.rs:2110-2111`) | refinement | Applied by #833: the obligation covers the identity rows (pruned to the window) and a leftover embedded `meta` key (the row ends under 128 KiB); recorded in WP-P25 for owner confirmation |
| Interrupted revert converged one pass late before review fix F1 on #831 (`boundary_unknown` after a committed truncate) | gap | Closed by #881: `a_cas_conflict_after_the_revert_truncate_folds_in_the_same_request` and `a_panic_plus_reopen_after_the_revert_truncate_folds_on_the_next_pass` ran in its daemon gate (WP-E02, WP-E12) |
| Discovery budget anchored at discovery start, not pass start | gap | Closed by #883 (review P6): "declines a rediscovery once a slow first send spent the pass's discovery budget" ran in the #883 and #884 gates (WP-P10, WP-P21) |
| #881 bounds the null-boundary stale-slice match to rows at or below the rendered boundary, a refinement of D10's letter | refinement | Recorded in WP-P02; #881 states wire, rust-code, and rust-design reviewers confirmed it, and `a_null_anchor_hit_above_the_rendered_row_is_not_cut_at` fails without the bound |
| #881 already resets lineage continuation when an unanchored reconcile outside the pending-rewrite arm removes every segment | refinement | Recorded in WP-E03's trail as context; #833 adds the null-anchor reset without `lineage_switched` |
| #833's D10 reset leaves handler caches in place and relies on the `revert_epoch` bump | refinement | Recorded in WP-E12; `a_same_pass_reset_reuses_no_previous_output` pins it for CK and native |
| The D10 reset's log line is not asserted | gap | Queued below |
| A revert's SOFT pressure refold over a reverted segment end failed closed ("minted boundary not present") on #833 before its rebase | gap | Closed on `main` by the owner's defer-once revert (`c6a1d384`, merged with #881); #833's `a_revert_under_pressure_defers_then_folds_and_prunes_to_the_window` covers both segment ends (WP-P06) |
| Acceptance readings: HARD compared at H = 50k against H = 2,500 (≥ K) with H = 100 reported; RSS per idle session as the slope over k = 8; meta equal across N up to decimal-digit width | bias | Surfaced for a human (needs human input; WP-P15 open questions) |

## Gaps queued

Each item is queued: no owner outside #833 has taken it, unless named.

- Q1, queued (H4; WP-E03, WP-P06, WP-P07, WP-P16, WP-P25): the `yes`
  statuses cite #833's gates. The #833 PR description records them at the
  final head, which satisfies the item; if a later revision of the
  description drops the gate block, these five return to their earlier
  statuses.
- Q2, queued (C1; WP-E02, WP-E12): a revision 3 liveness record for
  interrupted revert convergence, bounded to one HARD pass and at most one
  extra `revert_epoch`, witnessed by `revision_3.rs:1147` and `:1163`; the
  pass after a fold completing without error or retry is its second
  obligation. Needs human input: the ticket fixes the part at 37 records.
- Q3, queued (C2; WP-E03, WP-P01, WP-P10): a dedicated D10 reset record:
  the reset runs once under a CAS conflict, its retries are bounded, and its
  removed range is logged. The log line is not asserted (WP-FM02) and a real
  reset under a retained plugin boundary is not constructed (WP-FM06).
- Q4, queued (W3; WP-E12, WP-E10): the acceptance search that finds no
  non-test consumer of the D24 and D25 symbols has no recorded result in a
  record, an evidence file, or `existing-checks.md`. Record it, or note it in
  scope as a structural check outside the catalog.
- Q5, queued (WP-E10): a refused transform commit leaves no row and no meta.
- Q6, queued (WP-P13, WP-P23): a discovery pass crossing hostile covered
  slots. Not in #883, #884, or #833 (needs human input: accept the
  primitive-level witness or add one).
- Q7, queued (WP-P14): a frozen-status assertion for the first-user verdict.
  Not in #884.
- Q8, queued (WP-P05): a handler-level CK input keep at a nonzero cut. Not
  in #881, whose handler test builds a native body.
- Q9, queued (WP-P02, WP-P03): corrupt-storage branches in `resolve` for
  negative stored ordinals and row versions (recorded as a known gap by
  #875).
- Q10, resolved (WP-P07, WP-P15): bytes decoded per D15 row. `3ebfc3b9`
  adds `bytes` to the statement ledger, a declared bytes bound per inventory
  row to the #874 test, and a D15 run at N = 1M / H = 50k in which all 58
  (row, phase) cells hold their bound (WP-P07 evidence).
- Q11, queued (WP-E03, WP-FM02, WP-FM06): the D10 reset's log line and a
  real reset under a retained plugin boundary; tracked with Q3.

## Existing-catalog dispositions

Comment 3's dispositions of the transform-edit-responses, shared-primitives,
daemon transform, history_summarizer, and hot-path catalogs were checked at
`main` `d68aedf3` and at the #833 branch. Applied before #833: TE17, TE19,
TE20, TE26, TE27, TE30; TE24 (`ba7dc0ac`, on `main`); latency B1, B3, B5;
the `need_full_sync` check (deleted). Applied by #833: TE18's `Status` field
now reads `invalidated`; TE21 has a #832 note in `recipe-wire-switch.md`;
TE23's `Check` no longer lists the deleted memo-copy charge and carries an
"Inventory after #832" bullet; the history_summarizer publication fence
(`publish-fence-rejects-selected-content-drift`) carries the pruning
extension; four stale `rust-mode-transform.ts` citations in the
transform-edit-responses header and TE24 are corrected. Preserved records
needing no edit: shared-primitives reconcile records, daemon recut records,
hot-path E2, H1, H2.

## Biases for a human

- H2: WP-P15's N = 1M and H = 50k counter equality is checked only by the
  uncommitted drivers; CI covers W = 5 at N = 10k and 100k. Decide whether a
  committed CI-scale counter test is owed.
- H3: WP-P21's budget witness depends on wall-clock sleeps (300 ms pages,
  1 s budget); a deterministic clock would remove the flake risk.
- W1: #883 stops checking host root properties outside the window (O(N)
  otherwise); a host `metadata` edit outside the window now publishes. #883
  records it as an implementer deviation; no owner decision is recorded
  (needs human input; WP-P12 open question).
- The three WP-P15 criterion readings: the HARD comparison point (H = 50k
  against H = 2,500 ≥ K, H = 100 reported, m0 against D14), RSS as the slope
  over k = 8 idle sessions, and meta equality up to decimal-digit width
  (needs human input).
- WP-P25's adaptation to the `block_identities` table (needs human input).
- Evidence is description-level. A green gate block shows the suite passed
  with the named tests present; it does not show each assertion ran against
  the enabling state the record needs. The markers in `fault-map.md` are the
  check on that.
- In-process fakes for the plugin (`fakeDaemon`) cover selected schedules,
  not real transport loss or daemon effect accounting.
- No liveness record exists; catalog revision 2 omitted them and C1 queues
  one (Q2). Bounded progress after a stalled summarizer is outside the
  specification.
- Measurements are single-host (128 cores) and taken with uncommitted
  drivers under the gitignored `docs/performance/`; the numbers carry
  artifact identity but cannot be re-run from the repository. The plugin
  readings at `58dbe556` ran at 1-minute load 5.8 to 7.4; the `cf89a9c2`
  plugin run at load 16 to 22 failed the 100 ms limit at 100k steady and cold
  start. Cold start keeps about 10 ms of margin at N = 1M; a run at load 20
  was not repeated on the final code.

## Structural check (METHOD step 7)

Run by script against this directory after the disposition; code references
checked at `f2442b2f`.

| Check | Result |
| --- | --- |
| `### wp-*` record blocks | 37, no duplicates |
| Index rows | 37; each anchor equals its slug, and each row's Type, Check, Status, and Exercised equal the record's |
| `evidence/*.md` files | 37, the same slug set as the records |
| `Confidence` evidence link | 37 of 37 point to `evidence/<own-slug>.md` |
| Record fields | all 12 METHOD fields, in METHOD's order, in every record; every enumerated field uses a METHOD value |
| Open questions | every record says `Open questions: None.` or lists bullets; no record says `None.` while its evidence file ends a question unresolved or needing human input |
| Evidence sections | the six METHOD headings in every file; every `### Q:` has Sources examined, Findings, Missing evidence, and Conclusion |
| Evidence length | 66 to 153 lines; WP-P07's and WP-P15's files exceed 120, to keep the `3ebfc3b9` bytes-decoded table and the `58dbe556` plugin readings with their artifact identity |
| Relative links and anchors | none broken |
| Placeholders | no `TODO-8nn` token outside the quoted evaluation above |
| `file:line` citations | 557 checked against `f2442b2f`: every path resolves and every line is in range, except named-tree citations, which are cited at their own tree; citations into the files the later #833 commits changed were moved by a line diff and their test names re-found with `git grep` |
| Semantics distribution | 29 `always`, 8 `sometimes`, 0 `always-or-unreached`, 0 `reachable`, 0 `unreachable`; matches `catalog.md` |
| Types | 29 safety, 8 reachability, 0 liveness |
| Reachability | 27 `default-production`, 3 `explicit-config-only`, 7 `test-only` |
| Exercised | 34 `yes`, 3 `partial`, 0 `not yet` |
| Status | 35 `active`, 2 `invalidated` |
| Confidence | 23 high, 14 medium, 0 low |
| Fault map | 22 rows, 22 unique occurrence markers (`WP-FM01` to `WP-FM22`) |

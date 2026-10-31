# p6-new-pass-bounds-and-accounting

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N7. Converted to active by [#1173](https://github.com/ahrav/eidnara/issues/1173), which lands the bounded pass in `crates/daemon/src/attestation_verifier/bounded_pass.rs`.

## Evidence trail

`ProjectRun::decisions` (`crates/daemon/src/attestation_verifier/bounded_pass.rs:530`) pages a project's decisions and stops the project once it has computed `MAX_VERDICTS` (`:97`, 8) verdicts or read `MAX_VERDICTS` × `MAX_BLOB_BYTES` blob bytes (`:601`). `ProjectRun::decision` (`:666`) counts a verdict only after `verify` (`:722`) returns; a skip (`skipped_stale` at `:575`, `skipped_attested` at `:658`), an unverifiable class, and a decision outside the input count no verdict. The page read runs under the pass budget for its reader wait and its query, so a deadline during its walk of the commit-sequence index stops the pass. Each verdict and each unverifiable class increments `considered`, its bucket, and `bytes_read` under one lock through `Counters::consider` (`:689`, `:705`, `:739`). The effect counters are named in `ProjectRun::outcome` (`:760`), which runs after `attest` (`:755`) returns its `Outcome`, so a refused or rolled-back commit counts `commit_refused_<reason>` and never `attestation_committed`. The deadline is polled before each project and decision, before each object read through `verify`'s `Deadline` adapter, and before each commit (`:752`). The attested skip at the anchor the page's standing read names runs before classification, so for a decision whose standing names a live `reachable_from` anchor that the anchor read returns, a registered attestation counts `skipped_attested` whatever its classification would find, for example a retired evidence row or descriptor, a digest-less descriptor, a path outside the document rule, an admission other than `Active` or `Stale`, a recorded terminal reason, or a classification refusal; `an_attested_decision_skips_before_its_classification_after_its_descriptor_retires` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:2590`) counts `skipped_attested` with `considered = 0` where classification alone reads `descriptor_retired`.

`each_pass_verifies_at_most_n_per_project_and_no_project_starves` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:475`) holds N + 2 verifiable 200 KiB decisions: the first pass computes 8 verdicts, reads 8 × 200 KiB, commits 8 attestations, and leaves the cursor at the eighth; the second pass opens the repository twice. Its second half holds 3N `path_absent` decisions in one project and one verifiable decision in another; each of three passes reads N of the first project's decisions, and the other project's decision is attested in the first pass. `a_rolled_back_commit_counts_its_refusal_and_a_replayed_commit_counts_its_replay` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:590`) injects a fault after the admission: `match = 1`, `attestation_committed = 0`, one `commit_refused_<reason>` = 1; a writer that commits the same write set between the skip check and the pass's commit leaves `attestation_replayed = 1` and one attestation. The tests that count verdicts call `assert_accounting`, which checks `sum(verdict buckets) + sum(unverifiable buckets) == considered` and `attestation_committed <= match`, after the passes they inspect; the derivation-cut tests count nothing and assert `considered = 0` instead. `an_object_another_source_wrote_under_the_attestations_id_skips_nothing` (`:1893`) commits an `assistant` decision under each attestation's object id before the pass: `skipped_attested` stays 0, the mismatch is marked stale, and the match verifies and its commit is refused by the occupied id (`commit_refused_other`), so `ProjectRun::attested` (`crates/daemon/src/attestation_verifier/bounded_pass.rs:615`) skips only for a registry row under the `verifier` source kind, which `kernel.commit` refuses.

The applicability step (#1174) has its own bound under the same N: `ProjectRun::applicability` (`crates/daemon/src/attestation_verifier/bounded_pass.rs:683`) returns `StopProject` before it evaluates an attestation once it has evaluated `MAX_VERDICTS` (`:756`); a skip of an attestation that is not `Active` counts no evaluation. It also reads at most `MAX_VERDICTS` pages of `PAGE_ROWS` rows per pass (`:690`), polling the deadline before each page, and returns `StopProject` at that bound (`:773`). `a_commit_deadline_leaves_its_attestation_first_for_the_next_pass` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:1914`) republishes N + 1 attested documents: after a commit deadline one pass marks N attestations `Stale` and the next marks the last one. `the_applicability_step_pages_at_most_n_pages_of_rows_per_pass` (`:2200`) holds N × 32 + 16 `Stale` verifier decisions ahead of an `Active` attestation: every other pass reaches it, and a pass that stops at the page bound polls the deadline 1 + N times. An applicability evaluation that writes nothing counts `applicability_current` or `applicability_unavailable`, and one whose mark commits counts `applicability_stale`, only after `mark_republished` returns a committed receipt; a refused stale mark counts under a `commit_refused_` reason or `commit_conflict`; a replayed or already `Stale` mark, a read refused at the deadline, and a mark the commit poll cuts short count in no field; a row whose standing read fails for a reason one row can cause counts `applicability_unavailable` with no evaluation.

## Failure scenario

A pass looping to exhaustion, double counting, or an effect counted before commit.

## Timing windows and dependencies

A commit refused after the verdict; a concurrent writer committing the same key between the skip check and the commit.

## What a test must construct

F7 between decisions; F10: a replayed key; F11: a fault-injected rollback; F13: more than N candidates.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1174 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the #1173 pass budget cover blob content or all decoded-object work?

- Sources examined: `Evaluation` and `Objects::read` in `crates/daemon/src/attestation_verifier.rs`; the #1173 ticket's Budgets.
- Findings: The ticket sets 256 KiB per blob and N × 256 KiB bytes per project per pass, the blob content `Evaluation::bytes_read` counts. Commit and tree decoding stay outside `bytes_read`; each object is bounded by the allocation limit, each claim by the 16-component path cap, and each pass by the N verdicts and the deadline, so one project's verdict evaluations request at most N × 18 objects (the commit, one tree per path component, and the blob), each of at most 256 KiB. A root without a cached repository identity also runs the identity derivation within its two-second share, the 262144-commit cap, and its 16 MiB object allocation limit; that work is outside `bytes_read` and runs before the skip checks.
- Missing evidence: none.
- Conclusion: resolved with answer: blob content.

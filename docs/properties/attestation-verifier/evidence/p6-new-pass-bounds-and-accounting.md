# p6-new-pass-bounds-and-accounting

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N7. Converted to active by [#1173](https://github.com/ahrav/eidnara/issues/1173), which lands the bounded pass in `crates/daemon/src/attestation_verifier/bounded_pass.rs`.

## Evidence trail

`ProjectRun::run` (`crates/daemon/src/attestation_verifier/bounded_pass.rs:357`) pages a project's decisions and stops the project once it has computed `MAX_VERDICTS` (`:54`, 8) verdicts or read `MAX_VERDICTS` × `MAX_BLOB_BYTES` blob bytes (`:425`). `ProjectRun::decision` (`:447`) counts a verdict only after `verify` (`:491`) returns; a skip (`skipped_stale` at `:405`, `skipped_attested` at `:481`), an unverifiable class, and a decision outside the input count no verdict. Each verdict and each unverifiable class increments `considered` beside its bucket (`:452`, `:467`, `:500`). The effect counters are named in `ProjectRun::outcome` (`:531`), which runs after `attest` (`:526`) returns its `Outcome`, so a refused or rolled-back commit counts `commit_refused_<reason>` and never `attestation_committed`. The deadline is polled before each project and decision, before each object read through `verify`'s `Deadline` adapter, and before each commit (`:522`).

`each_pass_verifies_at_most_n_per_project_and_no_project_starves` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:449`) holds N + 2 verifiable 200 KiB decisions: the first pass computes 8 verdicts, reads 8 × 200 KiB, commits 8 attestations, and leaves the cursor at the eighth; the second pass opens the repository twice. Its second half holds 3N `path_absent` decisions in one project and one verifiable decision in another; each of three passes reads N of the first project's decisions, and the other project's decision is attested in the first pass. `a_rolled_back_commit_counts_its_refusal_and_a_replayed_commit_counts_its_replay` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:564`) injects a fault after the admission: `match = 1`, `attestation_committed = 0`, one `commit_refused_<reason>` = 1; a writer that commits the same write set between the skip check and the pass's commit leaves `attestation_replayed = 1` and one attestation. Every test checks `sum(verdict buckets) + sum(unverifiable buckets) == considered` and `attestation_committed <= match` at each pass end.

## Failure scenario

A pass looping to exhaustion, double counting, or an effect counted before commit.

## Timing windows and dependencies

A commit refused after the verdict; a concurrent writer committing the same key between the skip check and the commit.

## What a test must construct

F7 between decisions; F10: a replayed key; F11: a fault-injected rollback; F13: more than N candidates.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1173 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the #1173 pass budget cover blob content or all decoded-object work?

- Sources examined: `Evaluation` and `Objects::read` in `crates/daemon/src/attestation_verifier.rs`; the #1173 ticket's Budgets.
- Findings: The ticket sets 256 KiB per blob and N × 256 KiB bytes per project per pass, the blob content `Evaluation::bytes_read` counts. Commit and tree decoding stay outside `bytes_read`; each object is bounded by the allocation limit, each claim by the 16-component path cap, and each pass by the N verdicts and the deadline, so one project's pass decodes at most N × 18 objects (the commit, one tree per path component, and the blob), each of at most 256 KiB.
- Missing evidence: none.
- Conclusion: resolved with answer: blob content.

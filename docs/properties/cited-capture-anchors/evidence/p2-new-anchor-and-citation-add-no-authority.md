# p2-new-anchor-and-citation-add-no-authority

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N6 and constraint C2, activated by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

Admission is `(ModelInference, AssistantInference)` for every capture decision (`crates/daemon/src/kernel_routes/commit.rs:601`). A decision the writer finds eligible for approval (`crates/daemon/src/memory_capture.rs:4540`; `user_attributed` at `:2134` names the same predicate), a whole-sentence quotation of a user message whose lifetime class is `lasting`, `unsettled`, or absent, is admitted under `Approve` on its statement record (`:4719`) when the Kernel accepts that record and the `Approve` admission that rests on it, and every other decision written from a proposal under `Other`. When the Kernel's admission policy refuses the approving commit at a statement record or at the admission that rests on it, `publish_captured_memories` commits the same plan with every decision written from a proposal under `Other`, so admission depends on attribution and on the approval outcome. A contest or settlement rewrite of an existing approved statement admits its successor on that statement's existing approval in either attempt. The lifetime class can only withhold approval: it moves a whole-sentence quotation from `Approve` to `Other` and raises no decision. The anchor is a parent row and the citation is a decision event; neither is an input to `user_attributed` or `record_admission`, and the decision row's class stays its own because `decisions.evidence_id` is empty.

## Failure scenario

A citation that raised maturity would let a model raise its own authority by quoting a captured message.

## Timing windows and dependencies

None.

## What a test must construct

Cited and uncited whole-sentence quotations, and cited fragments, in one session, compared on admission columns and served visibility per attribution (`crates/daemon/src/source_capture_tests.rs:3528`).

That test submits every proposal with no lifetime class (`submission_for` at `crates/daemon/src/source_capture_tests.rs:3331`). A cited and an uncited whole-sentence quotation classed `current_work` or `status`, compared on the same columns, remain to be constructed.

## Investigation log

### Q: Do the admission rows differ between cited and uncited decisions?

- Sources examined: `admission_decisions` rows in the end-to-end test.
- Findings: Within each attribution the distinct set of (source, taint, maturity, effective maturity, elevated support) has one member: the whole-sentence quotations, cited or not, share the approved row, and the fragments share the candidate row.
- Missing evidence: none.
- Conclusion: resolved with answer.

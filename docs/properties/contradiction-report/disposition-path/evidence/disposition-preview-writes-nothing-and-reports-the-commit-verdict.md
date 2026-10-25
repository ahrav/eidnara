# disposition-preview-writes-nothing-and-reports-the-commit-verdict

Catalog record: [catalog.md](../catalog.md#disposition-preview-writes-nothing-and-reports-the-commit-verdict).
HEAD `3a3b916b7`, 2026-10-25.

## Discovery trigger

`CommitRequest.preview` doc: "Judges each disposition operation without
writing: no commit-log row, no receipt, and no token check" (`commit.rs:
68-71`). The TypeScript command's confirmation decision rests on the
preview's verdict (`memory-mark-command.ts:126-139`). Lens: protocol
contracts and idempotency.

## Evidence trail

- `handle_kernel_commit` refuses a preview with tokens or with a non-
  disposition operation as `invalid_params` (`crates/daemon/src/kernel_
  routes/commit.rs:1001-1016`).
- `preview` runs inside `KernelStore::preview` (`:729-755`); if
  `stored_receipt` is `Some`, returns `Replay(receipt)` (`:732-734`) and
  the response carries `receipt.replayed: true` and `previews: []`
  (`:1021-1028`).
- `PreviewRun::disposition` calls `disposition_request` then
  `preview.preview_admission(request)` (`:674-717`); `DispositionResult::
  new` is the same constructor the commit uses (`:636-642`, `:703-709`).
- `Preview::preview_admission` runs `refuse_succession_events`,
  `prepare_admission`, `refuse_changed_authority`, then inserts into
  `admission_latest` and returns the evaluation without writing
  (`crates/kernel/src/admission.rs:1854-1865`). `record_admission_inner`
  runs `refuse_succession_events` then `with_authority_cascade(apply_
  admission)` (`:1036-1050`); `apply_admission` is `prepare_admission`
  then `write_admission` (`:1139-1145`). The evaluation both paths return
  is `prepare_admission`'s.
- The preview's `current` and `projected` surfaces come from
  `served_rows_for` plus the evaluation (`:689-699`); `visibility_changes`
  ignores surfaces that are hidden now (`changes_to`, `:409-414`).
- `refuse_changed_authority` makes the preview refuse what the commit
  would judge differently after a cascade (`admission.rs:1867-1880`,
  mapped at `commit.rs:1030-1036`).
- Tests: `:5114` (counts unchanged, surfaces), `:5271` (in-request prior),
  `:5349` (denials and refusals match), `:5487` (changed authority).

## Failure scenario

A preview that wrote a receipt would consume the identity before the user
confirmed, so the later commit replays "applied" for an operation the user
declined. A preview whose evaluation diverged from the commit's (a cascade
the preview cannot see, a different prior) would ask a confirmation
question about a change that does not happen, or skip one that does.

## Timing windows and dependencies

Preview and commit are separate requests; the object or its approval can
move between them. The command treats the commit's verdict as recorded
(`memory-mark-command.ts:147-148`). Inside one preview, an earlier
operation's judged state feeds the next (`PreviewRun.judged`, `:650-654`,
`:689-693`, `:700-703`).

## What a test must construct

- For every `(prior disposition, event)` pair and approval state: run a
  preview, snapshot the three table counts, run the commit with the same
  operations; assert counts unchanged by the preview and `previews[i]`
  (result fields) equals `dispositions[i]`.
- The same with an interleaved commit from another session between the
  two; assert the commit's verdict differs where the prior changed.
- A preview with a recorded identity: `receipt.replayed == true`,
  `previews == []`.
- Coverage markers `preview_then_commit_same_tip`,
  `preview_then_commit_tip_moved`.

## Investigation log

No open questions.

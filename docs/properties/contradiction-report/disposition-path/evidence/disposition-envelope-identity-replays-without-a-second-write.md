# disposition-envelope-identity-replays-without-a-second-write

Catalog record: [catalog.md](../catalog.md#disposition-envelope-identity-replays-without-a-second-write).
Authored at `3a3b916b7`, 2026-10-25; references re-verified at the PR A
tip of #1160 (code commit `6c4d445e3`), with PR A changes noted inline.

## Discovery trigger

Lens: idempotency and replay. The command's identity is
`(sessionId, event, objectId)` at `3a3b916b7`; PR A appends the evidence
flags (`memoryMarkOperationId`, `memory-mark-command.ts:189-194`), so a
user repeating a command with the same evidence in one session hits the
replay path by design;
P3 plans to join dispute events by `commit_seq`, which assumes one row set
per report.

## Evidence trail

- Key derivation: `operation_key = sha256(producer, actor, operationId)`
  (`shared/kernel-client/client.ts:284-290`); `request_digest = sha256(
  stable(operations, source_kind, asserted classes))` (`:272-281`). The
  daemon prefixes the key with the project and `RECEIPT_FAMILY = "commit"`
  (`project.rs:78-83`, `:152-163`; `commit.rs:87`).
- `commit_prepared_with_writer` opens the writer transaction, checks the
  fence, then `stored_receipt` (`crates/kernel/src/envelope.rs:1383-1396`):
  a `Some` returns the stored receipt without running the operation;
  `Conflict` when the digest differs (`:1342-1372`).
- `operation_receipts` is `UNIQUE(producer, operation_key)` with no-delete
  and identity-immutable triggers (`schema.rs:42`); `commit_log` has
  `idx_commit_operation UNIQUE(producer, operation_key)` (`:26`).
- The route maps a `Conflict` before entering the operation to
  `OperationKeyReused` (`commit.rs:1086-1091`) and reports
  `invalid/operation_key_reused` (`:483`).
- The replayed receipt's `result` is the original `CommitResult` JSON;
  the route re-derives `tokens` from `touched` with `known_as_of =
  receipt.commit_seq` and copies `dispositions` (`:1190-1206`), so a
  replay reports the original verdict even if the subject has since moved.
- Preview honors the same identity (`:874-876`).
- Reissue: the client reissues a write once under the same identity after
  an ambiguous send (`client.ts:300-301`, `:462-468`), relying on this
  property.
- Tests: `kernel_routes.rs:4982-5088` (replay equal `dispositions` and
  `tokens`, counts unchanged; reused key refused; token abort drops the
  whole envelope), `:931`; TypeScript `:321`, `:332`.

## Failure scenario

A receipt lookup moved after the operation would double-write on a
reissue; P3's `served_sessions_for(decision) join events after
served_commit_seq` would count two disputes for one report. A digest that
omitted `source_kind` would let the same key replay a `"user"` commit for
an `"assistant"` request.

## Timing windows and dependencies

A lost response after a durable commit is the enabling window for the
reissue. Replay-after-move: a subject disposed by another identity between
the original commit and the replay; the replay still returns the original
`previous_disposition` and `disposition`.

## What a test must construct

- A disposition commit; a repeat with equal digest: `replayed: true`,
  equal `commit_seq`, byte-equal `dispositions`, unchanged counts.
- A repeat with a different digest: `invalid/operation_key_reused`,
  unchanged counts.
- A second identity moving the subject, then a replay of the first: the
  replay's `dispositions[0].disposition` differs from `stored_disposition`.
- Per-identity accounting: `COUNT(commit_log WHERE producer, operation_key)
  <= 1` across a workload with repeats.
- Coverage markers `disposition_identity_replayed`,
  `disposition_identity_reused`,
  `disposition_replayed_after_subject_moved`.

## Investigation log

No open questions.

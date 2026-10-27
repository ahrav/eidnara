# disposition-commit-log-row-carries-actor-and-cause-only

Catalog record: [catalog.md](../catalog.md#disposition-commit-log-row-carries-actor-and-cause-only).
Authored at `3a3b916b7`, 2026-10-25; references re-verified at the PR A
tip of #1160 (code commit `6c4d445e3`), with PR A changes noted inline.

## Discovery trigger

Plan P4 R3: "The event row can be joined to the commit log entry carrying
caller-asserted `actor`, `cause`, `session_id`, and `source_kind`
(`project.rs:135-141`, `envelope.rs:1402-1411`)". Lens: claimed safety
guarantees, resolved against the schema. The record states what the row
carries so R3's join is specified against the real columns.

## Evidence trail

- `commit_log` columns: `commit_seq, transaction_id, writer_epoch,
  producer, operation_key, request_digest, recorded_at, actor, cause`
  (`crates/kernel/src/schema.rs:26`); update and delete triggers abort.
- INSERT binds exactly `transaction_id, writer_epoch, producer,
  operation_key, request_digest, recorded_at, actor.text, cause.text`
  (`crates/kernel/src/envelope.rs:1400-1415`).
- `RedactedIntent::new` applies `identity()` to `producer` and
  `operation_key` and `redact()` to `actor` and `cause`
  (`envelope.rs:1699-1714`); `RedactedIntent::record` writes a
  `durable_text_redactions` row per detection for `actor` and `cause`
  (`:1716-1726`, called at `:1417`).
- `IntentRequest` has `producer, operation_key, request_digest, actor,
  cause` and is `deny_unknown_fields` (`crates/daemon/src/kernel_routes/
  project.rs:138-146`); `into_intent` namespaces the key (`:148-163`).
- `session_id` is an envelope key stripped before parsing
  (`kernel_routes/mod.rs:461`); the binding knows the session but nothing
  writes it to the Kernel on this path. `source_kind` is consumed by
  `resolve_classes` (`commit.rs:1111-1118`) and copied into inserted specs
  (`:288-294`), never into `commit_log`.
- The admission row joins to the commit log by `commit_seq`
  (`admission_decisions.commit_seq REFERENCES commit_log`, `schema.rs:163`).
- Command values: `actor = "user:opencode"` (`hooks/context/command-
  handler.ts:391`) or `"user:pi"` (`pi-plugin/src/commands/eidnara-memory-
  mark.ts:18`); `cause` is `commandLine(args)`: `/eidnara-memory-mark
  <event> <object_id>`, followed at the PR A tip by `--evidence <id>` and
  `--path <path>@<oid>` when present (`memory-mark-command.ts:158-160`,
  set at `:252`); `producer` defaults to the client's (`client.ts:603`).
- PR A's `a_dispute_citing_live_project_evidence_lands_with_the_id_and_the_reporter`
  (`kernel_routes.rs:5723`) reads `actor` and `cause` from the
  `commit_log` row of two dispute commits (`commit_actor_and_cause`,
  `:5651-5659`) and asserts them equal to the request's. No test reads the
  column set; `commit_log_count` (`kernel_routes.rs:4340`) counts rows
  only. This record stays a pin until PR B (#1162) adds the typed reporter
  columns.

## Failure scenario

A P4 reader that expects `session_id` or `source_kind` on the commit log
finds no column; a reporter-identity join written against R3's wording
fails at implementation. Separately, a `cause` or `actor` carrying a
secret-like token is rewritten by the redactor, so the stored text can
differ from the client's and a byte-equality join against the client's own
record would miss.

## Timing windows and dependencies

None in time. Depends on `RedactedIntent::new`'s validation
(`request_digest` lowercase hex 64, non-blank producer and key,
`envelope.rs:1701-1706`).

## What a test must construct

- A disposition commit from the command path; read the `commit_log` row
  at `receipt.commit_seq`; assert `actor`, `cause`, `producer` equal the
  request's; assert `PRAGMA table_info(commit_log)` names exactly the nine
  columns.
- A `cause` with a detected secret; assert the stored text differs and a
  `durable_text_redactions` row exists with `owner_kind = 'commit_log'`.
- Coverage marker `commit_log_cause_redacted`.

## Investigation log

### Q: Is the PR A reporter identity meant to be recoverable from `actor` and `cause` alone, or does R3 require a schema change before PR A?

- Sources examined: P4 R3, U1 Verification ("the receipt and commit log
  carry actor, cause, and session for the event"), U3 (typed reporter
  columns in Batch K1), OQ4-1; `schema.rs:26`; `envelope.rs:1400-1415`;
  `mod.rs:461`.
- Findings: the commit log carries `actor` and `cause`. The session id is
  dropped at the envelope boundary. OQ4-1 puts the reporter class into a
  structured `reason` for PR A; nothing in the plan adds a session column
  before Batch K1.
- Missing evidence: the owner's intended join key for PR A.
- Conclusion: needs human input.

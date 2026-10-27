# dispute-identity-includes-evidence-key

## Discovery trigger

Plan P4 rule A8 and decision D7. At `3a3b916b7` the command identity was
`[sessionId, event, objectId]`, so a second dispute of one finding with
other evidence in one session hit `operation_key_reused`. The
specification recommended adding the evidence key
([`../open-questions.md`](../open-questions.md) Q2, option (a)).

## Evidence trail

- `memoryMarkOperationId(sessionId, args)` joins the session, event,
  object id, and `evidenceFlags(args)` with the key separator
  (`packages/opencode-plugin/src/shared/memory-mark-command.ts:210-215`).
- `evidenceFlags` emits `--evidence <id>` then `--path <path>@<oid>`, each
  only when present (`memory-mark-command.ts:152-162`), so flag order on
  the command line does not change the identity.
- The runner takes `operationId` from its caller (`MemoryMarkInput`,
  `memory-mark-command.ts:190`; used at `:259`). Both callers pass
  `memoryMarkOperationId(sessionId, parsed.args)`
  (`packages/opencode-plugin/src/hooks/context/command-handler.ts:415`,
  `packages/pi-plugin/src/commands/eidnara-memory-mark.ts:61`).
- The client derives `operation_key = sha256(join(producer, actor,
  operationId))` (`packages/opencode-plugin/src/shared/kernel-client/client.ts:283-290`)
  with `joinKeyFields` (`client.ts:247`); `request_digest` covers the
  operations, so other bytes under one key are `operation_key_reused`.
- The parser refuses U+001F in the object id and the evidence id
  (`memory-mark-command.ts:124-129`) and in the path (`:80`).
- Route side: `commit_log` has a unique index on `(producer,
  operation_key)` (`crates/kernel/src/schema.rs:26`); the receipt lookup
  precedes the operation, so a reused key with another digest is
  `operation_key_reused` (`crates/daemon/src/kernel_routes/commit.rs:1066-1068`).

## Failure scenario

A user disputes finding D with `--evidence A`, then with `--path p@o`.
Under the old identity the second command reuses the first key with other
bytes and is refused; the record keeps only A.

## Timing windows and dependencies

- A lost reply followed by the client's reissue under the same identity
  replays the receipt. The route-level reply drop is not constructed; the
  receipt-before-operation order is read from code.
- Two invocations in one session with the same evidence replay; that is
  the intended unit of "one dispute".

## What a test must construct

All constructed except the reply drop:

- One session, dispute by id then by file: two identities, two
  `applied` outcomes with `replayed: false`, two receipts; the first again
  replays with no new receipt
  (`packages/opencode-plugin/src/shared/memory-mark-command.test.ts:212`).
- Route: two keys, two commit-log and two admission rows; an identical
  repeat returns `replayed: true`, the first `commit_seq` and
  `dispositions`, counts unchanged; the first key with another digest is
  `operation_key_reused`, counts unchanged
  (`dispute_identities_land_replay_and_refuse_reuse_like_every_envelope`,
  `crates/daemon/tests/kernel_routes.rs:6080`).
- Parser refusal of U+001F in the path and the evidence id
  (`memory-mark-command.test.ts:148`).

## Investigation log

### Q: Is the identity independent of flag order?

- Sources examined: `evidenceFlags`; the parser test at
  `memory-mark-command.test.ts:106`; `both flag orders parse to one args
  value and one identity` (`memory-mark-command.test.ts:706`).
- Findings: by construction yes, and the second test parses
  `--path p@o --evidence ev` and `--evidence ev --path p@o`, asserts the
  two `args` values are equal, and asserts `memoryMarkOperationId` yields
  one identity for both.
- Missing evidence: none.
- Conclusion: resolved with answer; exercised.

### Q: Does the route itself include evidence in the identity?

- Sources examined: `IntentRequest::into_intent`
  (`crates/daemon/src/kernel_routes/project.rs:148-165`).
- Findings: no; the route namespaces the caller's key by project and
  family. The evidence key lives in the caller's `operationId`.
- Missing evidence: the #1161 tool arm's identity rule.
- Conclusion: resolved with answer for the command arm.

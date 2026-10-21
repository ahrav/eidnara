# directive-operation-key-replay-is-idempotent

## Discovery trigger

Plan U3 step 5: "idempotency through `operation_key` like `review.accept`";
test scenario "a second `directive.record` with the same `operation_key`
replays without a second row". Serves R1 and R7 (a retried revise or revoke
must not double-apply).

## Evidence trail

Verified at HEAD:

- `crates/kernel/src/envelope.rs:85-92` `CommitIntent { producer,
  operation_key, request_digest, actor, cause }`; the dedup key is
  `(producer, operation_key)`.
- `envelope.rs:1279-1309` `stored_receipt`: a stored row under the same
  producer and key with an equal `request_digest` returns the receipt with
  `replayed: true`; a different digest is `KernelError::Conflict`.
- `envelope.rs:1310-1330` `commit_prepared_with_writer`: the receipt lookup
  runs first inside the `IMMEDIATE` transaction; on a hit the operation
  closure never runs.
- `crates/daemon/src/kernel_routes/commit.rs:906-938` `run`: a `Conflict`
  raised before the closure is entered is reported as
  `CommitFailure::OperationKeyReused`.
- `crates/daemon/src/kernel_routes/project.rs:73-78`
  `ProjectBinding::operation_key(family, key)` namespaces the stored key as
  `<digest>:<family>:<key>` and refuses a blank key; `commit.rs:33` uses
  family `commit`, `ingest.rs:41` uses `artifact_ingest`.
- `crates/daemon/src/memory_reviewer/settlement.rs:1156-1176` `caller_intent`
  builds the key from project digest, causal identity, and the SHA-256 of the
  caller key, and the digest from the request body. This is the reference the
  plan names.
- Tests: `crates/daemon/tests/kernel_routes.rs:931`
  `replayed_intents_return_one_receipt_and_projects_never_collide`; `:4096`
  `a_receipt_is_keyed_by_route_family_and_a_blank_key_is_refused`; `:3681`
  `an_ingestion_key_reused_with_another_payload_is_a_permanent_refusal`.

Plan-only: the directive route family name and what its receipt result
payload carries.

## Failure scenario

Legal witness: `record(key K, text T)` commits at `commit_seq` 41 and returns
object id D. The client times out and resends identical bytes; the daemon
finds the receipt and returns `commit_seq 41, replayed: true` with the same
result payload; one live directive exists.

Smallest violating traces:

1. The route derives `request_digest` from something that changes per
   attempt (a timestamp, a fresh uuid for `directive:<uuid>` computed before
   digesting): the retry is a `Conflict` reported as key reuse, and the user
   sees a failure for a write that succeeded.
2. The route does not namespace the key by family: a directive key equal to
   an earlier `kernel.commit` key replays that commit's receipt and reports a
   directive that was never written.
3. The route computes the uuid inside the closure: two attempts with the same
   key but a `Conflict` on digest is correct, but a replay returns the first
   uuid while the client expected the second; the result payload must be the
   source of the id.

## Timing windows and dependencies

The window is a lost response between `COMMIT` and the client. The receipt
lookup is inside the writer transaction, so two concurrent identical requests
serialize: the first writes, the second replays.

## What a test must construct

- Two identical `directive.record` requests; assert one live row, second
  response `replayed: true`, same object id in both results.
- Same key, different text; assert the reuse refusal and unchanged row count.
- Same key under `kernel.commit` and under `directive.record`; assert both
  commit independently (different family prefix).
- Same for `revise` and `revoke`: a replayed revoke returns the original
  receipt and does not run the cascade twice (the cascade is inside the
  closure, which does not run on replay).

## Investigation log

### Q: Does a replay re-run the authority cascade?
- Sources examined: `envelope.rs:1310-1330`.
- Findings: the closure does not run on a receipt hit; only an alignment
  rebuild may be triggered.
- Missing evidence: none.
- Conclusion: resolved with answer: no.

### Q: What should the digest cover for `record`?
- Sources examined: `settlement.rs:1156-1176` pattern.
- Findings: digest over the semantic request (text, scope, confirmed_from),
  not over the generated uuid.
- Missing evidence: the U3 handler.
- Conclusion: unresolved, resolvable at implementation.

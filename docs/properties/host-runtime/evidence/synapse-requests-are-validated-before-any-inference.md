# synapse-requests-are-validated-before-any-inference

## Discovery trigger

`protocol.rs:14-15` fixes a depth cap and `:13` a 32 MiB body cap, and the
section comment at `:70-72` says typed deserialization avoids materialising a
`Value` tree. The hazard is model time spent on hostile input: a request that
reaches `spawn_blocking` with the CPU permit held blocks every other query
for the duration of one inference. The audit traced the order of checks in
`handle` and confirmed that every rejection returns before the job table or
the engine is touched.

## Evidence trail

All references are at `e16e39e`, paths relative to `crates/host-runtime/`.

Handler order. `handle` (`src/synapse/mod.rs:866-944`) requires a ready lane
(`:867-869`), runs `preflight` (`:870-872`), computes and bounds the parse
reservation (`:873-903`), then `decode_request` (`:904-910`). A decode error
drops the charge and returns (`:906-909`). Only after that does the match at
`:915-943` reach `handle_query`, `handle_batch`, or `handle_result`. The
engine is called from `handle_query` via `spawn_blocking` (`:579`) and from
`spawn_batch_worker` (`:726-729`); both are downstream of the decode.

Preflight. `preflight` (`src/synapse/protocol.rs:553-564`) rejects binary
bodies, bodies over `MAX_BODY_BYTES` (`:13`), and depth over `MAX_BODY_DEPTH`
(`:15`, 8) via `depth_exceeds` (`:492`), which skips string contents.

Decode. `decode_request` (`:566-590`) decodes `MapOnly<MethodEnvelope>`
first (`:571`); `MethodEnvelope` is `deny_unknown_fields` (`:76-82`) and
`MapOnly` rejects sequences (`:88-115`). Per method it decodes
`RequiredParams<QueryParams>` (`:578`), the hand-written batch path (`:582`),
or `RequiredParams<ResultParams>` (`:586`). Unknown methods are `schema` at
`:589`. All `serde_json` errors become `schema_violation` (`:200-202`).

Constraints. `check_constraints` (`:639-665`) returns `substitution_rejected`
(`:44-49`) when model, fingerprint, or epoch differ from the lane
(`:647-655`) or when either `allow_equivalent` or `accept_declared` is true
(`:656-663`). `parse_query` (`:667-690`), `parse_batch` (`:692-742`), and
`parse_result` (`:744-768`) each call it first. `parse_batch` then checks
the key shape, item bounds, per-item hash, duplicate ids, and the aggregate
text cap, all as `schema`.

Foreign jobs. `JobTable::parse_job_id` (`src/synapse/jobs.rs:268-282`)
requires the process incarnation prefix, a random nonce from `getrandom`
(`:247-252`). `poll` returns `Restarted` for any parse failure (`:500-502`),
mapped to `module_restarted` (`mod.rs:780-783`).

Replays. `admit_charged` (`jobs.rs:305-398`) looks up the key (`:326`),
returns `Conflict` on a digest mismatch (`:328-330`), and returns
`Existing` for any non-retryable state (`:336-341`) without inserting a
job. `handle_batch` maps `Existing` to the descriptor body (`mod.rs:669-680`)
and never spawns a worker for it.

Existing checks. All but one are in `tests/synapse_protocol.rs`, a
default-harness binary run by CI (`.github/workflows/ci.yml:118`), and use
`DeterministicEngine` (`tests/support/synapse.rs:40-48`), which counts
`calls` at `:101`:

- `embed_query_rejects_every_constraint_violation` (`:641-734`): eleven
  parameter edits including wrong model, fingerprint, epoch, both flags,
  a non-boolean flag, non-string and empty text, `deadline_ms: 0`, an
  unknown field, and a missing `model` (`:655-693`); oversize text
  (`:697-700`); an unsupported method (`:703-711`); a duplicate-key
  envelope (`:713-726`). `engine.calls == 0` at `:728-731`.
- `embed_batch_validation_creates_no_job_and_no_inference` (`:820-899`):
  wrong `content_sha256`, wrong key, duplicate ids, five items over a cap of
  four, 33 bytes over a per-text cap of 32, and aggregate text over 64.
  `engine.calls == 0` at `:895-898`. It does not inspect the job table.
- `an_unknown_top_level_field_is_rejected_without_reading_its_value`
  (`:1270-1294`): a synchronous unit test calling
  `parse_request_unreserved` (`protocol.rs:594-601`) with a 4 M-element
  array under an unknown key; it asserts the code, not read counts or
  `engine.calls`.
- `a_routed_depth_nine_request_is_a_schema_violation` (`:1302-1338`): a
  depth-nine body with `params` before `method` and braces in a string; a
  valid query with brace-only text succeeds; `engine.calls == 1` (`:1335-1339`).
- `equal_replays_reuse_one_job_and_one_inference` (`:941-993`): replays
  while queued and after completion return the same `job_id`;
  `engine.calls == 1` (`:966-970`); three conflicting payloads under the
  retained key are `idempotency_conflict` and `calls` stays 1 (`:974-992`).
- `unknown_and_foreign_jobs_are_module_restarted` (`:1218-1234`) covers the
  foreign-job clause of the guarantee but is not named in the record.

## Failure scenario

1. A client sends `embed.query` with `required_fingerprint` for a different
   model, hoping the lane will serve it anyway.
2. An implementation that admitted the query and checked constraints after
   inference would spend one CPU-permit slot and return a wrong-space vector
   or a late rejection.
3. As written, `check_constraints` runs inside `decode_request`, which
   `handle` calls before the match that reaches `handle_query`.

The depth case is the concrete resource hazard: without `preflight`, a
recursive `serde_json` decode of a deeply nested body would consume stack
before the typed schema rejected it. `depth_exceeds` is a linear byte scan.

## Timing windows and dependencies

None on the rejection path; every check is synchronous and precedes the first
`await` that could reach the engine. The parse reservation at `:891-903` is
the only state touched before decode, and it is an RAII charge dropped at
`:907` on failure.

The replay guarantee depends on `by_key` being consulted before the admission
cap (`jobs.rs:326` before `:357`), so a replay during `queue_full` still
returns the existing descriptor rather than a rejection.

## What a test must construct

1. A job-table assertion in the batch validation test: after every rejected
   batch, `key_is_retained` (`jobs.rs:284-290`) is false for that key, or a
   metrics snapshot of the table is unchanged. Today only `engine.calls` is
   observed.
2. The unknown-field test routed over the host with the engine counter, so
   the record's `always` check covers that class at the same oracle.
3. A `substitution_rejected` case for `embed.batch` and `embed.result`; the
   host-level constraint cases are all `embed.query`.

## Investigation log

### Q: Does every rejection class return before the engine can be reached?

- Sources examined: `src/synapse/mod.rs:866-944`, `:515-640`, `:642-700`,
  `:702-746`; `src/synapse/protocol.rs:553-590`, `:639-768`.
- Findings: `preflight` and `decode_request` complete before the `match` at
  `mod.rs:915`. The only engine call sites are `:579` and `:726-729`, both
  inside branches of that match. `substitution_rejected`,
  `schema_violation` from decode, and `module_restarted` from
  `parse_job_id` are all produced before those sites. The two later
  `schema_violation` sources, `KeyMismatch` and `BadCursor`
  (`mod.rs:784-789`), come from `poll`, which never calls the engine.
- Missing evidence: none.
- Conclusion: resolved. There is no path from a rejected decode to an
  `embed` call.

### Q: Does a rejected batch leave the job table untouched?

- Sources examined: `src/synapse/mod.rs:904-910`, `:651-662`;
  `src/synapse/jobs.rs:305-398`.
- Findings: decode failures return before `handle_batch`. Inside it, the
  key check at `:651-662` calls `key_is_retained`, which only sweeps, and
  `admit_charged` inserts at `:377-393` only after every rejection return.
  The test observes `engine.calls`, not the table.
- Missing evidence: a table-level oracle in the host test.
- Conclusion: resolved by code reading; unresolved as a tested claim, needs
  a table snapshot or `key_is_retained` assertion.

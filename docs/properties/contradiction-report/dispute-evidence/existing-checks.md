# Existing checks: dispute evidence on kernel.commit

System: the PR A branch tip of #1160 (base `6159fd224`, code commit
`6c4d445e3`, review-round code commits `511cd6d73` and `f68e895ca`,
evidence-walk commit `f94f3163b`; line numbers verified at `f94f3163b`). Every check below is `unaudited`:
adequacy verdicts belong to `/testing:invariant-test-review` for tests and
`/low-level-systems:defensive-assertions-and-invariant-guards` for
production guards. An existing check never removes a property from the
catalog.

## Route integration tests (`crates/daemon/tests/kernel_routes.rs`, section "kernel.commit disputes carrying evidence", from `:5575`)

| Line | Test | What it asserts | Records | Status |
|---|---|---|---|---|
| 5723 | `a_dispute_citing_live_project_evidence_lands_with_the_id_and_the_reporter` | command-shaped (`user`) and tool-shaped (`assistant`) disputes with a live project evidence id land: `deny`, disputed or contradicted, effective `candidate`, historical kept, row classes equal the prior, `evidence_id` on the row, reason parses to event, reporter taint, and id; `commit_log` holds the sent `actor` and `cause` | liveness, visibility, reporter, standing, reason | unaudited |
| 5801 | `a_dispute_citing_unusable_evidence_is_not_found_and_writes_nothing` | unknown, retired, uncited, unscoped-only, and foreign-only ids answer `not_found` from preview and commit under both events; four counts unchanged | liveness, visibility, preview | unaudited |
| 5880 | `a_dispute_citing_a_repository_file_records_it_in_the_reason` | five reserved-byte paths at 40/64-hex oids preview without writing, land with no `evidence_id`, and parse back byte for byte; eight malformed references answer `invalid_input` without writing | form, reason, preview | unaudited |
| 5971 | `a_dispute_needs_evidence_and_other_events_take_none` | dispute with no field or an empty id, and `mark_stale` with a reference, answer `invalid_input` without writing; the other three events land with `reason = kernel.commit` and no evidence | requirement, preview | unaudited |
| 6025 | `a_dispute_reporter_class_is_derived_and_never_asserted_up` | raised assertions `class_over_declared`, unknown `source_kind` `invalid_input`, nothing written; a lowered assertion is the recorded reporter taint | reporter | unaudited |
| 6080 | `dispute_identities_land_replay_and_refuse_reuse_like_every_envelope` | two keys land two commits; identical repeat replays with the first verdict and no write; same key, other digest `operation_key_reused` | identity | unaudited |
| 6146 | `a_dispute_is_denied_against_a_more_restrictive_prior_and_lateral_from_stale` | against rejected, contradicted, quarantined: `denied: true`, prior kept, evidence on the row; from stale: lands disputed | standing | unaudited |
| 6199 | `a_dispute_on_an_authority_bearer_copies_its_reason_onto_demoted_dependents` | a dispute on an approval keeps its historical maturity and classes, sets effective `candidate`, and writes `approval_revoked` on the dependent in the same commit with the same parseable reason | standing, reason, cascade | unaudited |
| 6252 | `a_disposition_carrying_its_own_reason_is_refused_as_an_unknown_field` | a `mark_disputed` body carrying a forged `reason` answers an error (the variant, not the refusal name, is asserted); four counts unchanged | requirement, reason | unaudited |
| 6285 | `a_dispute_judges_every_citer_and_both_evidence_fields_together` | evidence cited by the project and by a foreign scope that sorts first lands; evidence cited by two foreign decisions written before the project's citer lands; both fields on one dispute land and parse back; foreign-only evidence answers `not_found`; a malformed reference beside a `not_found` id answers `invalid_input`; nothing written on refusal | visibility, form, reason | unaudited |
| 6416 | `a_dispute_previewed_while_its_evidence_was_visible_is_judged_again_at_commit` | a preview answers `available`; the only project citer retires; the commit answers `not_found` with counts unchanged | liveness, visibility, preview | unaudited |
| 6459 | `a_dispute_relaxes_a_contradicted_prior_only_with_a_valid_approval` | against a `contradicted` prior: without an approval `denied: true` and the disposition kept; with a valid approval `denied: false`, `disputed`, `outcome = deny`, evidence on the row | standing | unaudited |
| 6516 | `an_insert_citing_evidence_makes_it_citable_later_in_the_same_commit` | uncited live evidence answers `not_found`; an envelope with an `insert_decision` citing it and then the dispute lands with the id on the row (portfolio gap G1) | visibility | unaudited |

Helpers the tests rely on: `assert_refused_without_writing` (`:5703`),
`write_counts` (`:5597`), `latest_admission` (`:5625`), `project_evidence`
(`:5684`), and `disposition` (`:4318`), which now attaches an
`evidence_ref` to every dispute so the pre-existing disposition tests keep
landing.

## Unit tests (`crates/daemon/src/kernel_routes/dispute.rs`)

| Line | Test | What it asserts | Records | Status |
|---|---|---|---|---|
| 238 | `the_documented_rendering_parses_back` | the documented rendering for one reference | reason | unaudited |
| 252 | `render_and_parse_are_inverse_over_reserved_and_non_ascii_bytes` | round trip over reserved and non-ASCII bytes for both events | reason, form | unaudited |
| 286 | `reserved_and_multi_byte_values_encode_to_uppercase_escapes` | fixed witness: `encode` writes reserved and multi-byte input as uppercase escapes and `decode` reverses it | reason, form | unaudited |
| 298 | `every_accepted_reference_parses_back_to_its_inputs` (proptest) | every accepted path (no NUL, 1 to 64 chars) and optional id renders an ASCII reason within the cap that parses back to the inputs | reason, form | unaudited |
| 320 | `malformed_references_are_invalid_input` | 12 malformed references refused; a 64-hex oid accepted | form | unaudited |
| 352 | `a_path_the_secret_scanner_detects_is_invalid_input` | the test token is detected and a path holding it is refused | form | unaudited |
| 364 | `git_object_ids_and_ordinary_paths_carry_no_detection` | 64 seeded 40- and 64-hex ids beside an ordinary path carry no detection | form, reason | unaudited |
| 380 | `a_dispute_needs_evidence_and_other_events_take_none` | the requirement at the function level | requirement | unaudited |
| 400 | `a_reason_over_the_cap_is_invalid_input` | a reason over 4096 bytes is refused | reason | unaudited |
| 440 | `parse_refuses_every_rendering_render_does_not_produce` | 11 altered renderings parse to `None` | reason | unaudited |

## Kernel tests

| Location | Test | What it asserts | Records | Status |
|---|---|---|---|---|
| `crates/kernel/src/admission.rs:4532` | `a_dispute_with_evidence_never_raises_standing_across_the_grid` | over every consistent maturity pair, disposition, dispute event, and evidence flag: `Deny`, historical kept, effective not raised, restrictiveness not lowered, prior kept on a relaxing request | standing | unaudited |
| `crates/kernel/tests/kernel_slice.rs:2161` | `evidence_cited_in_offers_each_live_citing_scope_once_and_stops_at_the_first_accepted` | live decisions, observations, and decision events offer their scopes once each; unscoped and retired citers offer none; the walk ends at the accepted scope; retired and unknown ids are cited nowhere | liveness, visibility | unaudited |
| `crates/kernel/tests/kernel_slice.rs:2316` | `evidence_cited_in_matches_the_citation_model_under_seeded_citers` | over 24 seeded rounds of interleaved scoped, unscoped, live, and retired decisions, observations, and decision events, the answer equals the model for every accepted scope set; each scope is offered at most once and only when a live citer carries it; no scope before the last is accepted; a refusal offered every citing scope | visibility | unaudited |

## TypeScript tests (`packages/opencode-plugin/src/shared/memory-mark-command.test.ts`)

| Line | Test | What it asserts | Records | Status |
|---|---|---|---|---|
| 106 | a dispute takes `--evidence`, `--path` split at the last `@`, or both, in any order | parse shapes | form, identity | unaudited |
| 137 | a dispute without evidence, and evidence on another event, print usage | requirement at the parser | requirement | unaudited |
| 148 | a flag without a value, a repeated flag, or a malformed path is refused | parser refusals, including U+001F | form, identity | unaudited |
| 167 | the usage states the evidence requirement, the refusals, and the preview's limit | usage text | preview (G2 limit) | unaudited |
| 185 | the evidence rides the preview and the commit byte for byte under the user source kind | both bodies carry the operation and `source_kind: "user"`; `cause` names the flags | reporter, identity | unaudited |
| 212 | two disputes with different evidence in one session are two commits; the same evidence replays | identity | identity | unaudited |
| 271 | the fake refuses the reference shapes the route refuses | five path shapes the route refuses are refused at the preview step by the fake kernel | form (fake parity) | unaudited |
| 325 | a dispute the daemon refuses for its evidence is reported by name and writes nothing | refused preview, no receipt, one call | preview | unaudited |

The fake kernel mirrors the form rule, including the daemon's
confined-path rule and the percent-escape refusal (`disputeEvidenceWellFormed`,
`packages/opencode-plugin/src/shared/kernel-client-testing/fake-kernel.ts:335-351`;
`repositoryPathWellFormed`, `:270-311`); the secret scan, liveness, and
visibility stay with the daemon;
its contract test is
`packages/opencode-plugin/src/shared/kernel-client-testing/fake-kernel.contract.test.ts`.

## Production guards

| Location | Guard | Records | Status |
|---|---|---|---|
| `commit.rs:206-234` | `compose_reason`: dispute needs valid evidence; other events take none | requirement, form | unaudited |
| `dispute.rs:49-79` | `DisputeReason::validated`: presence, empty id, a detection in the raw `evidence_id` (`:58-59`, through `value_is_clean`, `:159-163`), form, cap, a detection in the composed reason (`:77-79`) | requirement, form, reason | unaudited |
| `dispute.rs:150-163` | `EvidenceRef::is_well_formed` | form | unaudited |
| `commit.rs:742-757` | `require_visible_evidence`: live and cited by a scope of the bound project | liveness, visibility | unaudited |
| `commit.rs:137`, `:168-171` | `deny_unknown_fields` with `reason` skipped, so a body cannot supply `reason` | reason | unaudited |
| `commit.rs:411-444` | `resolve_classes`: refuse an assertion above the derivation | reporter | unaudited |

## Tool tests (#1161)

| Location | Test | What it asserts | Records | Status |
|---|---|---|---|---|
| `packages/opencode-plugin/src/tools/eidnara-memory/tools.test.ts:1341` | is a gated mutation: tool-call identity, agent, and allowlist | no tool-call id, `context_researcher`, and an allowlist without `dispute` each refuse with no transport call | requirement, reporter | unaudited |
| `tools.test.ts:1359` | is in the exposed schema exactly when the allowlist names it | the `dispute` enum member, its fields, and the description paragraph appear only when allowed | requirement | unaudited |
| `tools.test.ts:1376` | sends the evidence under the tool identity, actor, cause, and assistant source kind | preview and commit bodies carry the operation, `source_kind: "assistant"`, no asserted class, `agent:opencode`, and the tool-call cause | reporter, identity, form | unaudited |
| `tools.test.ts:1454` | an aborted tool call sends nothing | a pre-aborted signal refuses at the preview with no send | preview | unaudited |
| `tools.test.ts:1464` | an abort while the commit is unbound stops the rebind and the resend | one commit attempt, no rebind, object unchanged | preview, identity | unaudited |
| `tools.test.ts:1491` | never relaxes a disposition through the memory's stored approval | `denied`, disposition kept, no commit call | reporter | unaudited |
| `tools.test.ts:1514` | a retry with the same tool-call id replays; a new tool-call id is a second write | same id replays without `visibilityChanged`; a new id adds one receipt | identity | unaudited |
| `tools.test.ts:1534` | refuses missing evidence and unsupported events before sending; renders denials and refusals | five invalid shapes refuse with no send; denied and refused results render | requirement, form | unaudited |
| `packages/pi-plugin/src/tools/eidnara-memory.test.ts:422` | is absent from the default schema and refused by the default allowlist | the default allowlist omits `dispute` | requirement | unaudited |
| `eidnara-memory.test.ts:433` | refuses an action outside the canonical set even when a caller's allowlist names it | the canonical-set check runs beside the allowlist | requirement | unaudited |
| `eidnara-memory.test.ts:449` | files a dispute under agent:pi and the assistant source kind when allowed | preview and commit bodies carry `agent:pi`, `source_kind: "assistant"`, and no asserted class | reporter, identity | unaudited |
| `eidnara-memory.test.ts:487` | needs a stable tool-call identity and sends an evidence id as evidence_id | no identity refuses; `evidenceId` rides as `evidence_id` | identity, form | unaudited |
| `eidnara-memory.test.ts:509` | refuses a dispute without evidence before sending anything | the same five invalid shapes refuse with no send | requirement | unaudited |

The shared-runner tests this PR adds (`memory-mark-command.test.ts:601`,
`:633`, `:660`, `:672`, `:706`) are listed in
`../disposition-path/existing-checks.md` under "PR A checks".

## None found

- No test constructs a retirement queued on the writer while a dispute
  holds it. For the cited evidence row this state is not constructible:
  `retire_evidence` refuses evidence a live row cites
  (`crates/kernel/src/slice/write.rs:585-608`). The reachable form is a
  citer retirement queued behind the dispute; `kernel_routes.rs:6416`
  constructs its sequential form only.
- No test cites evidence through an asserted-edge citer, and no Rust code
  under `crates/` writes `asserted_edges`.
- No test sends a raw `evidence_id` holding a secret-scanner detection to
  the raw-id scan added in `511cd6d73` (`dispute.rs:58-59`, `:159-163`).
- No test reads `durable_text_redactions` after a landed dispute.

## Suspiciously quiet areas

- `evidence_id` values are checked for presence, emptiness,
  decode-idempotence, a detection in the raw value, and a detection in
  the composed reason; their shape is otherwise unchecked before the
  Kernel read, which answers `not_found` for anything not stored.
- `Envelope::evidence_cited_in` (`crates/kernel/src/envelope.rs:520-598`)
  stops at the first accepted citer, but a refused dispute still visits
  every live citer of the id inside the writer transaction, one table
  seek per citer. Bounding it by distinct citing scopes needs an index
  over `(evidence_id, scope_id)`, a Kernel schema decision.
- `evidence_ref.path` accepts C0 and bidi control characters; they are
  stored percent-encoded. Recorded as a review nit, not fixed.
- The route documents `class_over_declared` and `invalid_input` for the
  reporter; the TypeScript runner only ever sends `source_kind: "user"`,
  so the command arm never reaches the raised-assertion branch.

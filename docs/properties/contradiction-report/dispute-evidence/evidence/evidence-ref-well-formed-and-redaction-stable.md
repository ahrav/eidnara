# evidence-ref-well-formed-and-redaction-stable

## Discovery trigger

Plan P4 rule A4: a repository reference must be followable and must
survive the redactor unchanged. Portfolio gap G3 asked for a token the
scanner detects and for the scanner's result on a hex object id and an
ordinary path.

## Evidence trail

- `EvidenceRef` is strict: `path` and `git_object_id`, `deny_unknown_fields`
  (`crates/daemon/src/kernel_routes/dispute.rs:20-26`).
- `is_well_formed` requires `kernel::is_lower_hex_oid(git_object_id)`,
  `path.len() <= MAX_EVIDENCE_PATH_BYTES` (1024, `dispute.rs:12`),
  `confined_path::check(path)`, `decodes_to_itself(path)`, and no
  `redact_durable_text` detection in the path (`dispute.rs:150-163`).
- `is_lower_hex_oid` accepts exactly 40 or 64 lowercase hex bytes; PR A
  made it `pub` (`crates/kernel/src/scope.rs:710-716`) and re-exported it
  (`crates/kernel/src/lib.rs:149`).
- `confined_path::check` refuses NUL, empty, `.`, and `..` components,
  protected components, and more than `MAX_COMPONENTS` components
  (`crates/daemon/src/confined_path.rs:29-45`). An absolute path has an
  empty first component and is refused.
- `decodes_to_itself` refuses any `%` followed by two hex digits
  (`dispute.rs:202-208`), so a stored value has one reading.
- `validated` scans the raw `evidence_id` (`dispute.rs:58-59` through
  `value_is_clean`, `:159-163`; added in `511cd6d73`) and the composed
  reason (`dispute.rs:75`). The raw scan covers an `evidence_id` whose
  secret the percent-encoding would break up in the reason.
- The route stores the received bytes and does not look the object up
  (`crates/daemon/src/kernel_routes/commit.rs:26-28`).

## Failure scenario

A client sends `path: "keys/<token>.txt"`. Without the scan, the Kernel's
`redact_lossy` would rewrite the reason, the stored path would hold a
placeholder, and the reference would point at no file. A client sending
an uppercase or 39-byte oid would store a reference no git command can
resolve.

## Timing windows and dependencies

None in time. The result depends on the scanner's embedded rule set
(`crates/secret-scanner/src/rules.rs:31`, `:299-306`); a rule change can
move a path from accepted to refused.

## What a test must construct

All constructed:

- Valid paths with `;`, `=`, `%`, `@`, spaces, and non-ASCII bytes at 40-
  and 64-hex oids, stored byte for byte:
  `a_dispute_citing_a_repository_file_records_it_in_the_reason`
  (`crates/daemon/tests/kernel_routes.rs:5880`).
- Refusals with `invalid_input` from preview and commit, nothing written:
  uppercase oid, 39- and 63-byte oids, `src/../a.rs`, `.git/config`, NUL,
  a path over 1024 bytes, a path holding the test token (same test).
- Unit refusals of empty oid, absolute path, empty path,
  `src/.eidnara/x`, and `double%2Fencoded`:
  `malformed_references_are_invalid_input` (`dispute.rs:315`).
- Both fields on one dispute land and parse back, and a malformed
  reference beside an id that would answer `not_found` answers
  `invalid_input`, so the form check runs before the visibility check
  (`a_dispute_judges_every_citer_and_both_evidence_fields_together`,
  `kernel_routes.rs:6285`).
- Every accepted path renders and parses back to itself (proptest
  `every_accepted_reference_parses_back_to_its_inputs`, `dispute.rs:293`);
  `encode` on reserved and multi-byte input has a fixed witness
  (`dispute.rs:281`).
- The fake kernel refuses five path shapes the route refuses
  (`packages/opencode-plugin/src/shared/memory-mark-command.test.ts:271`).
- Command parser: `--path` split at the last `@`; a path without `@`, an
  empty path or oid, and a path holding U+001F print usage
  (`packages/opencode-plugin/src/shared/memory-mark-command.test.ts:106`,
  `:148`).

## Investigation log

### Q: Which token does the scanner detect, and do object ids and ordinary paths pass (G3)?

- Sources examined: `a_path_the_secret_scanner_detects_is_invalid_input`
  (`dispute.rs:343-350`); `git_object_ids_and_ordinary_paths_carry_no_detection`
  (`dispute.rs:355-368`); `crates/secret-scanner/conservative_overlay.yaml:5-7`;
  `crates/secret-scanner/default_rules.yaml:410-411`;
  `context_core::redaction::redact_durable_text`
  (`crates/context-core/src/redaction.rs:462-464`).
- Findings: the test asserts `redact_durable_text` detects
  `sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678` and that a path
  holding it is `invalid_input`. By the rule regexes, the conservative
  overlay rule `magic-anthropic-api-key`
  (`\bsk-ant-(?:api03-|admin01-)?[A-Za-z0-9_-]{32,}`) matches it, and the
  default `anthropic-api-key` rule does not (it requires 93 characters and
  a trailing `AA`). The second test renders 64 seeded SHA-256 digests at
  40 and 64 hex beside `src/kernel_routes/commit.rs` and asserts no
  detection in the composed reason.
- Missing evidence: the detector id the scanner reports for the token was
  not observed; the rule attribution is inferred from the regexes.
- Conclusion: resolved with answer for detection and for the hex and path
  cases; the rule attribution is unverified.

### Q: Does the route verify the blob exists at the oid?

- Sources examined: `commit.rs:26-28`.
- Findings: no, by contract.
- Missing evidence: none.
- Conclusion: resolved with answer: out of scope for PR A.

### Q: Can the command cite a path that holds a space (portfolio R2)?

- Sources examined: `parseMemoryMarkArgs`
  (`packages/opencode-plugin/src/shared/memory-mark-command.ts:94-97`),
  the usage text (`memory-mark-command.ts:52`), `kernel_routes.rs:5880`.
- Findings: no. The parser splits its arguments on `/\s+/`, so a path
  with a space becomes two words. The route accepts such paths. At
  `511cd6d73` the usage names the limit ("a relative repository path
  without spaces").
- Missing evidence: an owner decision on quoted or escaped paths.
- Conclusion: needs human input.

### Q: Does the path rule refuse control characters?

- Sources examined: `confined_path::check`
  (`crates/daemon/src/confined_path.rs:29-45`), `encode`
  (`dispute.rs:166-176`).
- Findings: only NUL among control bytes. C0 and bidi control characters
  pass, and `encode` stores them percent-encoded, so the stored reason
  stays ASCII. A review round recorded this as a nit, not fixed in
  #1160.
- Missing evidence: none for the behavior.
- Conclusion: resolved with answer: accepted and stored encoded; recorded
  as a nit.

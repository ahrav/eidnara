# anchor-is-substring-of-stored-p1

## Discovery trigger

Specification #835 Property Catalog record `anchor-is-substring-of-stored-p1`,
derived at `265df096`, with constraint C-9 (at every seat the anchor is a
substring of the trimmed `p1`) and D-5 (the store scans each value and
anchor as its own field, never the JSON blob, and nulls an anchor the
redacted trimmed `p1` no longer contains). Milestone ticket #838 carries it
as the acceptance criterion that every stored non-null anchor lies in the
trimmed `p1`, including when a scanner secret straddles the anchor boundary.

## Evidence trail

`crates/memory-store/src/lib.rs::prepare_history_segment` redacts `p1`
through `write.content("p1", ..)` and then calls `prepare_claims` with the
redacted `p1`, trimmed. Every insert path prepares its segments through
`prepare_history_segments`: `replace_history_segments`,
`append_history_segments`, `publish_history_summarizer_chunk`, and
`prepare_state_sync`.

`crates/memory-store/src/lib.rs::prepare_claims` scans the anchor under
`claim_anchor`, the key under `claim_key`, the value under `claim_value`,
and the `key = value` pair under `claim_pair`, each as a field of its own.
It keeps the redacted anchor only when it is non-empty and the redacted
trimmed `p1` contains it. It drops a claim whose pair the scanner changes.
`insert_history_segment_tx` then writes the prepared claims through
`claims_cell`.

`crates/memory-store/src/lib.rs::MemoryStore::descend_lineage` copies rows by
`INSERT ... SELECT`, re-scanning `p1` with `redact_transaction_text`. It
copies `claims` only when `p1` is NULL or the re-scan leaves it unchanged,
and writes `'[]'` otherwise.

`crates/memory-store/tests/production_redaction.rs::history_segment_content_redacts_and_new_message_identities_reject`
stores a `p1` holding a keyword secret and four `db.port` claims whose
anchors lie outside, inside, and across each edge of the secret, plus an
`api.key = abc` claim. It asserts the exact redacted `p1`, four stored
claims (the `api.key` pair is dropped), the first value redacted and its
anchor `port 5432` kept, and for every claim that the value and anchor hold
no `secret` and every anchor lies in `p1.trim()`.

`crates/memory-store/tests/production_redaction.rs::active_scan_audit_expires_with_its_session_note_owner`
appends one segment with one claim and asserts `field_scans: 9` with one
detection: session id, two message ids, title, content, and the claim's
key, value, anchor, and pair.

`crates/memory-store/src/lib.rs::lineage_descent_tests::descent_copies_verbatim_ranges_and_session_notes_without_replay_duplicates`
seeds claims on source rows, gives row 2 a legacy `p1` secret, descends, and
asserts copied claim counts `[1, 0, 0]` with the first anchor `history`.

The validator enforces the same containment before storage in
`crates/daemon/src/history_summarizer_citations.rs::check_claim_set`,
covered by `tests::each_claim_rule_drops_only_its_own_claim`.

## Failure scenario

`p1` reads `port 5432 then password=hunter2 then done` and a claim anchors
`5432 then password=hun`. Redacting `p1` alone leaves an anchor outside the
stored `p1`, which never splices and footers forever. Scanning the JSON
cell as one string could leave an unredacted anchor beside a redacted `p1`.

## Timing windows and dependencies

The lineage copy re-scans `p1` in the copying transaction under the
scanner rules current at copy time. A rule change between the original
insert and the copy is what makes the copy's `p1` differ.

## What a test must construct

A `p1` holding a secret the production scanner detects, and anchors inside
it, straddling each edge, and outside it. Assert containment of every
stored anchor in the stored trimmed `p1`, not exact redacted anchor strings.
For lineage, a source row whose stored `p1` the re-scan rewrites.

## Investigation log

### Q: Is a straddling anchor stored as `None` or as a redacted substring?

- Sources examined: `prepare_claims`, the production redaction test.
- Findings: the anchor is redacted on its own, then kept only if contained.
  The test asserts containment and absence of `secret`, not which of the
  two outcomes each straddling anchor takes, so it survives scanner changes.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the lineage copy re-check containment directly?

- Sources examined: the `descend_lineage` insert statement.
- Findings: it does not re-scan or re-check claims. It relies on the claims
  having been checked at their original insert and drops all claims when
  `p1` changes, which preserves containment.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Do the named tests pass at HEAD?

- Sources examined: nextest run at `c38af85a`.
- Findings: all four tests pass.
- Missing evidence: none.
- Conclusion: resolved with answer.

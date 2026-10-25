# sc-v1-receipt-replays-legacy

## Discovery trigger

Part B obligation from #1066 comment 2, decision D7. At baseline the record
conflicted with code: the receipt lookup returned `Conflict` on a digest
mismatch, mapped to `IdentityReused`, and a batch holding one failed whole.
The U5 ticket (#1085) resolves OQ9 as a v2-then-v1 probe under the same key
and names the faults: v1 receipts in a store under the v2 publisher, and a
v1 evidence-only receipt.

## Evidence trail

`crates/daemon/src/harness_sources.rs` - `REQUEST_DIGEST_FORMAT` is
`"source-request.v2"` and `LEGACY_REQUEST_DIGEST_FORMAT` is
`"source-request.v1"`. `digest_inputs(unit, format)` length-prefixes the
format tag, text, domain, scope, role, and sensitivity and appends the scope
flag and egress byte; the v1 digest (`legacy_request_digest`) is these bytes
alone. `request_digest` appends a presence byte and the length-prefixed
`ToolOutcome::encode` bytes. `Prepared` carries both `request_digest` and
`legacy_digest`.

`stored_receipt(preview, intent, legacy_digest)` calls
`Envelope::stored_receipt` with the v2 digest; on `Conflict` it retries with
the legacy digest and returns the receipt with the digest that matched, or
`Conflict` when neither matches. `publish_within` uses it for both the
descriptor and the evidence intent: a descriptor match returns
`replayed: true` before any ingest; an evidence-only match makes the ingest
run under the digest that wrote it, so the Kernel replays the evidence and
the descriptor commits under v2. `publish_batch` uses the same helper in its
preview for both receipts. The receipt key (`receipt_key`) is unchanged.

Landed check:
`crates/daemon/tests/harness_sources.rs::receipts_written_before_the_outcome_record_replay_under_v2`
pins the v1 layout with a golden digest
(`9ff69ac0...68f3723`), publishes a message and a tool unit without an
outcome, and rewrites both receipts' `request_digest` to the v1 digest with
`store_receipt_as` (drops the `operation_receipts` triggers for one
statement and reinstalls them). It then asserts the message and the tool
unit, now carrying an outcome, replay on the single path and through
`publish_screened` with the tip unchanged and the stored tool descriptor
still without a record; changed text and changed egress refuse
`IdentityReused`; a v1 evidence-only receipt left by a scope refusal
completes with a new descriptor on both paths after the scope is inserted,
the batch path staging no artifact, and the completing descriptor receipt
holds the v2 digest; and a unit first published after the upgrade publishes
and replays under v2.

## Failure scenario

A v2 tag under the same key with no legacy probe turns every pre-upgrade
capture into `IdentityReused`, and one such unit refuses its whole batch, so
a window with any old unit never publishes again.

## Timing windows and dependencies

The probe runs in the receipt preview and again in the commit's receipt
check. A v1 evidence-only receipt is the window left by a partially completed
single-unit publication: evidence retained, descriptor refused. Under v2 the
descriptor completes and its receipt records v2, so the unit's two receipts
then carry different digest versions; each probe answers for its own key.

## What a test must construct

A store holding v1 descriptor and evidence receipts, the v2 publisher over
the same units, a changed payload and a changed context under a legacy key,
and a v1 evidence-only receipt. The fault is F4 applied to the digest tag.
The legal witness is a post-upgrade unit publishing and replaying under v2.

## Investigation log

### Q: Probe v1 then v2, or move to a new key (OQ9)
- Sources examined: `ticket-1085.md` Design Decisions; `stored_receipt`;
  `receipt_key`.
- Findings: the code probes v2 first, then v1, under one key for both
  receipts, and keeps mismatch rejection for both.
- Missing evidence: none.
- Conclusion: resolved with answer - v2-then-v1 probe under the same key,
  landed in U5.

### Q: Does a legacy replay ever enrich the stored descriptor?
- Sources examined: the replay branch of `publish_within`; the test's
  `stored_tool.detail.outcome == None` assertion.
- Findings: a descriptor-receipt match returns before any write.
- Missing evidence: none.
- Conclusion: resolved with answer - no enrichment; a pre-upgrade tool
  descriptor keeps no outcome record.

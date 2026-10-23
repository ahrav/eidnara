# sc-receipt-replays-equal-digest-conflicts-otherwise

Abbreviations follow [`../catalog.md`](../catalog.md): `hs.rs`, `hs-tests`,
`sc.rs`, and `sc-tests` name the harness-source and source-capture modules
and their tests.

## Discovery trigger

Part A record on receipt idempotency. D2 makes Kernel receipts the durable
record of capture, and D7 moved the digest to `source-request.v2` (OQ9).
Re-verified at HEAD `5a842d998`.

## Evidence trail

- `crates/daemon/src/harness_sources.rs`:
  - `receipt_key` hashes `occurrence_id`, `revision`, and representation; the
    digest is not part of the key.
  - `SourcePublisher::request_digest` hashes `digest_inputs` led by
    `REQUEST_DIGEST_FORMAT` (`source-request.v2`): text, domain, scope, role,
    asserted sensitivity (each length-prefixed), a scope-present byte, an
    egress byte, then the encoded outcome record or a 0 byte.
    `legacy_request_digest` is `digest_inputs` under
    `LEGACY_REQUEST_DIGEST_FORMAT` (`source-request.v1`) alone.
  - `stored_receipt` probes the v2 digest, and on `Conflict` probes the v1
    digest; a key holding neither returns `Conflict`.
  - `publish_within` previews both receipts, maps `Conflict` to
    `IdentityReused`, and returns `replayed: true` from a stored descriptor
    receipt before offering bytes. After the commit, `(Ok(receipt), None)` is
    the arm for a receipt committed between preview and commit.
  - `publish_batch` refuses two units under one key with different digests,
    previews every receipt, and returns `BatchRefusal { Conflict, unit: None }`
    when the main key replayed after the preview. A per-unit receipt
    `Conflict` inside the commit leaves `culprit` unset.
- Capture: `CaptureJob::publish` memo-keys by store incarnation, session, and
  receipt key; `classify` maps `IdentityReused` to `Disposition::Terminal`.
- Tests read at HEAD (`crates/daemon/tests/harness_sources.rs` unless noted):
  - `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically`:
    replay keeps `tip()` and `staged_artifacts_for_test()`; changed text and
    another scope return `IdentityReused` with both unchanged.
  - `publication_receipt_conflicts_on_provider_egress_change`,
    `publication_receipt_distinguishes_delimiters_inside_fields`.
  - `publication_receipt_digest_layout_is_pinned`: three v2 digests read from
    `operation_receipts`.
  - `receipts_written_before_the_outcome_record_replay_under_v2`: frozen v1
    digest, v1 descriptor and evidence receipts replay with no enrichment,
    changed text or egress is `IdentityReused`, a v1 evidence-only receipt
    completes under v2 without restaging.
  - `capture_batch_names_the_unit_whose_receipt_conflicts`.
  - `crates/daemon/src/source_capture_tests.rs`
    `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss`
    (replay after memo loss; the `replay_after_memo_loss` marker is recorded
    in `a_rerun_keeps_one_live_generation_across_a_restart`): after `forget_memo_for_test` the tip
    is unchanged and `units_replayed` equals the first pass's publications.
  - `sc-tests` `changed_bytes_under_one_identity_are_terminal_and_never_retried`.
  - `sc-tests` `a_concurrent_publication_of_a_unit_replays_or_conflicts`: the
    `before_publish` hook publishes the unit from a second `SourcePublisher`
    before the job's preview; equal context counts `units_replayed`, another
    sensitivity counts `units_identity_conflict`, and the memo answers later.

## Failure scenario

A layout change to `digest_inputs` without a new tag reinterprets stored
receipts, so every published unit conflicts. A missing digest input lets
changed bytes replay as the old object. Falsifier: `Published { replayed: true }`
for changed text, or `IdentityReused` for an unchanged unit after restart.

## Timing windows and dependencies

- Restart or memo loss between passes: the Kernel answers from receipts.
- Another writer between the job's handoff and its preview: constructed.
- Another writer between the preview and the commit: the code returns a
  replay (`publish_within`) or an unattributed `Conflict` (`publish_batch`);
  not constructed.

## What a test must construct

Two publications of one unit with an unchanged and a changed digest input,
asserting replay or `IdentityReused` with the tip and staged count unchanged.
For the remaining F5 window, a commit hook or a second store handle that
commits the same unit after the preview and before the commit.

## Investigation log

### Q: How U5 reads a v1 receipt under a v2 digest (OQ9)
- Sources examined: `ticket-1085.md` (Resolved OQ9), `pr-1085.md`,
  `stored_receipt`, `receipts_written_before_the_outcome_record_replay_under_v2`.
- Findings: probe v2 then v1 for both receipts under one key; a match on
  neither is `IdentityReused`.
- Missing evidence: none.
- Conclusion: resolved with answer; landed in U5.

### Q: Is F5 constructed?
- Sources examined: `a_concurrent_publication_of_a_unit_replays_or_conflicts`,
  `CaptureJob::publish` (`before_publish` hook runs before `publish_locked`).
- Findings: the hook runs before the preview, so the preview sees the stored
  receipt. The preview-to-commit arms are not reached by any test read.
- Missing evidence: a test reaching `(Ok(receipt), None)` or the main-key
  replay in `publish_batch`.
- Conclusion: unresolved, needs a commit hook or second store handle.

### Q: Do baseline citations hold?
- Findings: baseline `hs.rs:589-597`, `:1051-1054`, and `hs-tests` line numbers
  moved; the record names functions and tests.
- Conclusion: resolved with answer.

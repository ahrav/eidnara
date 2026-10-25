# Cited capture anchors: catalog

Method: `../METHOD.md`. The records come from the Property Catalog of the
evidence-cited memory capture specification
([#1130](https://github.com/ahrav/eidnara/issues/1130)), authored against
Eidnara `a0e3f2993`. The specification's long-form bundle is unavailable, so
the U1 ticket ([#1133](https://github.com/ahrav/eidnara/issues/1133))
reconstructs every record from the specification body and re-verifies every
reference against the branch that adds this part, based on `3a3b916b7`. U2
([#1134](https://github.com/ahrav/eidnara/issues/1134)), U3
([#1135](https://github.com/ahrav/eidnara/issues/1135)), and U4
([#1136](https://github.com/ahrav/eidnara/issues/1136)) re-verify every
reference at their own branches. A reference `path:line` names the first line of
the function, constant, or test it cites at the latest of those branches.

## Scope

Surfaces with behavior at the branch tip:

1. S1, the Kernel anchor vocabulary and `reachable_from` resolution:
   `crates/kernel/src/anchor.rs` (`AnchorCondition::decode`),
   `crates/kernel/src/applicability/engine.rs`
   (`anchor_row_exceeds_bounds`, `AnchorCacheKey`), and
   `crates/kernel/src/applicability/resolve.rs` (`ResolutionLadder`).
2. S2, the Kernel decision insert with anchor and evidence parents, the
   sensitivity fold, and evidence retention:
   `crates/kernel/src/slice/write.rs` and `crates/kernel/src/local_file.rs`.
3. S3, memory-capture validation and publish:
   `crates/daemon/src/memory_capture.rs`,
   `crates/daemon/src/memory_capture/native.rs`, and
   `crates/memory-store/src/memory_capture.rs`.
4. S4, `messages` descriptor identity and lookup:
   `crates/daemon/src/harness_sources.rs` and
   `crates/daemon/src/source_capture.rs`.
5. S5, the bounded git open: `crates/daemon/src/git_sources.rs`, and the
   capture HEAD observation that uses it,
   `crates/daemon/src/memory_capture/head.rs` (`observe_head`), added by
   #1134.
6. N1, the typed anchor write: `crates/kernel/src/anchor_write.rs`
   (`Envelope::insert_anchor`, `validate_anchor`, `validate_anchor_row`),
   added by #1133.
7. N2, N3, N5, and N6, the anchor, citation, reasons, and counters on
   capture decisions: `crates/daemon/src/memory_capture/cite.rs`
   (`Citation`, `locate`, `target`, `anchor_spec`), the publish closure in
   `crates/daemon/src/memory_capture.rs` (`publish_captured_memories`), and
   the capture-decision counters in `crates/daemon/src/source_capture.rs`
   (`record_capture_decisions`), added by #1135.
8. N4, the settlement record and the submit's bounded wait:
   `crates/daemon/src/source_capture/settlement.rs` (`Settlements`,
   `settle`), its writers in `crates/daemon/src/source_capture.rs`
   (`into_job`, `refuse_handoff`, `CaptureJob::end`, `ActiveSlot`), and the
   wait in `crates/daemon/src/memory_capture/native.rs`
   (`capture_settlements`), added by #1136.

## Status vocabulary

`Status:` uses METHOD's `active` for the 14 existing-behavior records and for
each new record a landed ticket made checkable. The remaining records use
`Status: proposed`, the value the specification assigns to a designed
obligation whose code does not exist yet. A proposed record names the ticket
that converts it to `active`, carries `Exercised: not yet`, and states the
intended reachability. Every existing check named in this part has status
`unaudited` in `existing-checks.md`.

## Reachability classes

- `default-production`: reached with the shipped configuration. Memory
  capture runs when `memory_enabled`, `auto_promote`, and
  `memory_auto_capture` are on (`crates/daemon/src/memory_capture.rs:578`),
  and all three default to on (`crates/daemon/src/config.rs:146`).
- `explicit-config-only`: reached only when source capture is enabled;
  `source_capture_enabled` defaults to off (`crates/daemon/src/config.rs:146`).
- `test-only`: no production caller at the branch tip. Nothing outside
  `crates/kernel` calls `evaluate_batch` or builds an `ApplicabilityRequest`.

## Index

| Slug | Type | Surface | Status | Semantics | Reachability |
| --- | --- | --- | --- | --- | --- |
| `p2-undecodable-or-oversized-anchor-row-is-uncertain` | safety | S1 | active | `always` | test-only |
| `p2-reachable-from-holds-on-ancestor-of-head` | safety | S1 | active | `always` | test-only |
| `p2-reachable-from-negative-needs-complete-evidence` | safety | S1 | active | `always` | test-only |
| `p2-anchor-verdict-cache-key-covers-row-head-and-boundary` | safety | S1 | active | `always` | test-only |
| `p2-decision-parents-must-be-live-at-insert` | safety | S2 | active | `always` | default-production |
| `p2-citing-decision-carries-at-least-its-evidence-class` | safety | S2 | active | `always` | default-production |
| `p2-cited-evidence-is-never-retired-or-swept` | safety | S2 | active | `always` | explicit-config-only |
| `p2-capture-commit-replays-by-job-key-and-frozen-digest` | safety | S3 | active | `always` | default-production |
| `p2-capture-quote-is-an-exact-substring-of-its-own-batch-source` | safety | S3 | active | `always` | default-production |
| `p2-capture-job-and-decision-identity-derive-from-the-source-tuple` | safety | S3 | active | `always` | default-production |
| `p2-capture-publish-and-session-delete-are-serialized` | safety | S3 | active | `always` | default-production |
| `p2-messages-descriptor-identity-is-per-block-and-evidence-id-is-the-receipt-key` | safety | S4 | active | `always` | explicit-config-only |
| `p2-deferred-source-units-publish-on-a-later-pass` | reachability | S4 | active | `sometimes` | explicit-config-only |
| `p2-git-open-is-isolated-and-allocation-bounded` | safety | S5 | active | `always` | default-production |
| `p2-new-anchor-row-decodes-on-write` | safety | N1 | active | `always` | default-production |
| `p2-new-anchor-and-registry-share-one-envelope` | safety | N1 | active | `always` | default-production |
| `p2-new-capture-anchor-identity-is-deterministic` | safety | N2 | active | `always` | default-production |
| `p2-new-head-observed-once-and-frozen` | safety | N2 | active | `always` | default-production |
| `p2-new-head-unavailable-keeps-decision-admitted` | safety | N2 | active | `always-or-unreached` | default-production |
| `p2-new-citation-is-alias-only` | safety | N3 | active | `always` | explicit-config-only |
| `p2-new-citation-block-target-rule` | safety | N3 | active | `always` | explicit-config-only |
| `p2-new-citation-folds-sensitivity-and-pins-evidence` | safety | N3 | active | `always` | explicit-config-only |
| `p2-new-settlement-wait-is-bounded-and-gate-free` | liveness | N4 | active | `always` | explicit-config-only |
| `p2-new-lookup-runs-only-after-settlement` | safety | N4 | active | `always` | explicit-config-only |
| `p2-new-miss-reason-matches-outcome` | safety | N5 | active | `always` | default-production |
| `p2-new-miss-counters-track-rationale` | safety | N5 | active | `always` | default-production |
| `p2-new-anchor-and-citation-add-no-authority` | safety | N6 | active | `always` | default-production |
| `p2-new-cited-decision-pair-observed` | reachability | N2 to N5 | active | `sometimes` | default-production |

Counts: 28 records; 14 existing-behavior records with `unaudited` existing
checks and 14 active new records (2 from U1, 2 from U2, 8 from U3, 2 from
U4). Semantics distribution: `always` 25, `always-or-unreached` 1,
`sometimes` 2, `reachable` 0, `unreachable` 0. Reachability among the 28
active records: 4 test-only, 16 default-production, 8 explicit-config-only. The specification's
5 test-only records shrink by one when U2 routes capture through the git
open; the N1 records become default-production when U3 writes capture
anchors.

## Records

### p2-undecodable-or-oversized-anchor-row-is-uncertain

Type: safety
Reachability: test-only - the engine is called only from `crates/kernel` tests; no production code builds an `ApplicabilityRequest`
Status: active
Exercised: yes - Kernel anchor and engine tests construct unknown kinds, missing values, conflicting columns, corrupt payloads, oversized payloads, and duplicate captures
Guarantee: An anchor row that `AnchorCondition::decode` refuses, or whose text columns and payload exceed `MAX_OBJECT_PAYLOAD_BYTES`, evaluates `Uncertain` and is never `Current`.
Check: `always` - for every candidate whose anchor row fails decode or exceeds the bound, the batch reports `Uncertain` for that candidate and every other candidate's verdict is unchanged; evaluated on every batch because rows come from storage
Fault/timing angle: none
Required faults and enabling state: an unknown `anchor_kind`; a git kind with an empty or uppercase oid; a row populating another kind's column; a payload above the bound; two captures of one commit
Confidence: high - [evidence](evidence/p2-undecodable-or-oversized-anchor-row-is-uncertain.md). Verified the bound check runs before decode at `crates/kernel/src/applicability/engine.rs:537` and that decode refuses each named shape at `crates/kernel/src/anchor.rs:272`
Existing check: `crates/kernel/tests/kernel_anchor.rs:252`, `:289`, `:304`; `crates/kernel/tests/kernel_applicability_engine.rs:241`, `:2108`, `:4143` (unaudited)
Impact: a malformed anchor that evaluated `Current` would inject a memory whose condition was never checked
Open questions: None.

### p2-reachable-from-holds-on-ancestor-of-head

Type: safety
Reachability: test-only - the resolution ladder runs only under the engine, which has no production caller
Status: active
Exercised: yes - `reachable_from_holds_on_descendants_and_fails_on_foreign_branches` builds a history with descendants and a foreign branch
Guarantee: A `reachable_from` anchor holds when its commit is an ancestor of or equal to HEAD, so a memory anchored at commit X stays `Current` through any number of descendant commits.
Check: `always` - for a readable repository and an unexhausted budget, `ResolutionLadder::evaluate(ReachableFrom { oid })` is `Holds` exactly when `oid` is an ancestor of or equal to HEAD in the fixture's graph
Fault/timing angle: none
Required faults and enabling state: a fixture with HEAD equal to the anchor, HEAD a descendant, HEAD on a foreign branch, a criss-cross merge, and a rebase merged through a second parent
Confidence: high - [evidence](evidence/p2-reachable-from-holds-on-ancestor-of-head.md). Read `resolve_commit` at `crates/kernel/src/applicability/resolve.rs:230`: a present anchor that is an ancestor-or-equal returns `Reachable` before any fallback rung
Existing check: `crates/kernel/tests/kernel_anchor_resolution.rs:44`, `:159`, `:557`; `crates/kernel/tests/kernel_applicability_engine.rs:806`; `crates/kernel/tests/kernel_anchor_write.rs:719` for a row `insert_anchor` wrote with no capture payload (unaudited)
Impact: a `reachable_from` anchor that failed after a descendant commit would turn every captured memory `Historical` after one commit, the U3 stop condition
Open questions: None.

### p2-reachable-from-negative-needs-complete-evidence

Type: safety
Reachability: test-only - as above
Status: active
Exercised: yes - the anchor-resolution tests construct absent objects, truncated windows, unreadable blobs, shallow boundaries, and budget exhaustion
Guarantee: `reachable_from` reports `DoesNotHold` only when the anchored commit is present, the ancestry test decided `false`, and the fallback window scan was complete with no match; every undecided step yields `Uncertain`.
Check: `always` - whenever the ladder returns `DoesNotHold { historical: false }` for a `ReachableFrom` condition, the anchor object was present, ancestry returned `Some(false)`, and `window_scan_complete` held
Fault/timing angle: budget exhaustion or interrupt during the ancestry walk or window scan
Required faults and enabling state: an absent anchor commit; a candidate window truncated at `CANDIDATE_WINDOW`; unreadable blobs; a shallow boundary; an exhausted `EvalBudget`
Confidence: high - [evidence](evidence/p2-reachable-from-negative-needs-complete-evidence.md). Read the `WindowMatch::None` guard in `resolve_commit` (`crates/kernel/src/applicability/resolve.rs:230`) and `window_scan_complete` at `:270`
Existing check: `crates/kernel/tests/kernel_anchor_resolution.rs:283`, `:325`, `:507`, `:601`, `:990`, `:1130`, `:1254`, `:1289`, `:1345`, `:1406`, `:1455` (unaudited)
Impact: a negative verdict on incomplete evidence would label a current memory `Historical` and drop it from injection
Open questions: None.

### p2-anchor-verdict-cache-key-covers-row-head-and-boundary

Type: safety
Reachability: test-only - as above
Status: active
Exercised: yes - the engine tests move HEAD, change the shallow boundary and sparse patterns, and share anchor ids across rows
Guarantee: A cached anchor verdict is reused only for a request whose checkout identity, anchor row fingerprint, HEAD, repository state, and patch-id algorithm all equal the cached entry's.
Check: `always` - for two evaluations that differ in exactly one `AnchorCacheKey` component (`checkout_identity`, `row_fingerprint`, `head`, `repository_state`, `patch_id_algorithm`), the second performs its own resolution; tightened by refinement R4 from "differ in a key component"
Fault/timing angle: HEAD or the shallow boundary moves between two batches
Required faults and enabling state: two batches across a HEAD move; a shallow-boundary change; two rows sharing an `anchor_id` but not a payload
Confidence: high - [evidence](evidence/p2-anchor-verdict-cache-key-covers-row-head-and-boundary.md). Read `AnchorCacheKey` at `crates/kernel/src/applicability/engine.rs:205`
Existing check: `crates/kernel/tests/kernel_applicability_engine.rs:148`, `:206`, `:518`, `:1029`, `:1064`, `:2621`, `:2659`, `:2881`, `:3080`, `:3434`, `:3676`, `:3943`, `:4024`, `:4095` (unaudited)
Impact: a stale cached `Holds` after a reset that dropped the anchored commit would keep a historical memory current
Open questions: None.

### p2-decision-parents-must-be-live-at-insert

Type: safety
Reachability: default-production - every memory-capture publish calls `insert_decision`, and since U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135)) every decision of a job with a readable frozen HEAD names the job's anchor
Status: active
Exercised: yes - the domain and evidence clauses by Kernel slice tests; the anchor clause by `a_decision_inserted_before_its_anchor_is_not_found_and_the_commit_rolls_back` and, in production order, by every anchored capture publish (`captured_memories_cite_the_blocks_holding_their_quotes_and_anchor_at_head`)
Guarantee: `insert_decision` refuses with `NotFound` a decision whose domain, proposition, scope, anchor, or evidence parent is absent or invalidated in the envelope's transaction.
Check: `always` - every committed decision row names a live domain and, for each set parent column, a parent row live at the decision's `created_commit_seq`. The `anchor_id` clause was `always-or-unreached` under refinement R1 while no production input reached it; U3 inserts the job's anchor before its first decision, so the clause is `always` and the marker `sometimes(decision_insert_named_an_anchor_id)` fires on every anchored publish
Fault/timing angle: a parent retired in an earlier commit, or inserted later in the same envelope
Required faults and enabling state: a decision naming a missing domain; a decision naming an anchor inserted after it in one envelope
Confidence: high - [evidence](evidence/p2-decision-parents-must-be-live-at-insert.md). Read `require_parents` at `crates/kernel/src/slice/write.rs:1095`
Existing check: `crates/kernel/tests/kernel_slice.rs:196`; `crates/kernel/tests/kernel_envelope.rs:669`; for anchors `crates/kernel/tests/kernel_anchor_write.rs:399` (unaudited)
Impact: a decision citing a missing anchor is refused `NotFound`, which fails the envelope and with it the whole capture job
Open questions: None.

### p2-citing-decision-carries-at-least-its-evidence-class

Type: safety
Reachability: default-production - the fold runs on every `insert_decision` and `insert_observation` that sets `evidence_id`, including review acceptance
Status: active
Exercised: yes - Kernel slice and review-acceptance tests cite `Sensitive` evidence from a `Normal` decision
Guarantee: A decision or observation whose row sets `evidence_id` stores the more restrictive of its asserted class and the live evidence's class.
Check: `always` - for every live decision with `evidence_id = e`, `rank(decision.sensitivity_class) >= rank(evidence_meta[e].sensitivity_class)`
Fault/timing angle: none
Required faults and enabling state: evidence stored `Sensitive` cited by a decision asserted `Normal`
Confidence: high - [evidence](evidence/p2-citing-decision-carries-at-least-its-evidence-class.md). Read `fold_cited_evidence_class` at `crates/kernel/src/slice/write.rs:980`
Existing check: `crates/kernel/tests/kernel_slice.rs:1453`, `:1526`; `crates/kernel/tests/kernel_review_acceptance.rs:988` (unaudited)
Impact: a decision classified below its evidence leaks the evidence's class through reads; for capture, the same fold would hide every row-cited memory from auto-inject (specification K4, OQ2-10)
Open questions: None.

### p2-cited-evidence-is-never-retired-or-swept

Type: safety
Reachability: explicit-config-only - `harness_capture` evidence exists only when source capture is enabled
Status: active
Exercised: yes - descriptor and local-file tests retire and sweep cited and uncited evidence
Guarantee: `retire_evidence` refuses, and `expire_harness_captures` keeps, an evidence row that a live observation, decision, decision event, or asserted edge cites.
Check: `always` - after any retirement or sweep, every evidence row cited under `EVIDENCE_CITED_SQL` is live
Fault/timing angle: a citation committed after the sweep selected the row for expiry
Required faults and enabling state: `harness_capture` evidence past `retain_until` with one live citing decision event; a pin added after selection
Confidence: high - [evidence](evidence/p2-cited-evidence-is-never-retired-or-swept.md). Read `EVIDENCE_CITED_SQL` at `crates/kernel/src/slice/write.rs:42`, `retire_evidence` at `:585`, and `expire_harness_captures` at `crates/kernel/src/local_file.rs:245`
Existing check: `crates/kernel/tests/kernel_source_descriptors.rs:1895`; `crates/daemon/tests/harness_sources.rs:3482`; `crates/kernel/src/local_file.rs:791`, `:889`; `crates/daemon/src/source_capture_tests.rs:3783` for a capture citation event (unaudited)
Impact: retired evidence would break every citation that names it
Open questions: None.

### p2-capture-commit-replays-by-job-key-and-frozen-digest

Type: safety
Reachability: default-production - every native capture submit publishes through `publish_captured_memories`
Status: active
Exercised: yes - `correction_supersedes_and_replay_or_duplicate_creates_no_extra_memory` replays a publish; the envelope test replays a receipt and changes its digest
Guarantee: The capture commit's operation key is the project's `memory_capture` key for the job id and its request digest is the SHA-256 of the frozen prepared plan, so a replay of the same plan returns the stored receipt with no new rows and a changed plan under the same key is `Conflict`.
Check: `always` - for every receipt of producer `memory_capture`, a second commit with the same key and digest is `replayed` and adds no row; one with a different digest returns `Conflict`. The receipt-conflict coverage marker is test-only (refinement R7): production never changes a frozen plan under its key
Fault/timing angle: the first commit's reply is lost and the job is retried
Required faults and enabling state: a publish retried after success; a frozen plan altered under the same key
Confidence: high - [evidence](evidence/p2-capture-commit-replays-by-job-key-and-frozen-digest.md). Read `publish_captured_memories` at `crates/daemon/src/memory_capture.rs:1202` and the frozen-plan read in `publish` at `:993`
Existing check: `crates/daemon/src/memory_capture.rs:2001`; `crates/kernel/tests/kernel_envelope.rs:160`; `crates/daemon/src/lib.rs:24502` (unaudited)
Impact: a non-replaying retry would duplicate memories; a digest that omitted plan fields would let a changed plan replay the old receipt
Open questions: None.

### p2-capture-quote-is-an-exact-substring-of-its-own-batch-source

Type: safety
Reachability: default-production - every capture output passes `parse_capture_batch`
Status: active
Exercised: yes - memory-capture unit tests and the native lease test submit malformed, foreign, and invented sources
Guarantee: A captured memory is accepted only when its `message_id` names a message of the batch and its quote is a nonempty substring of at most 2048 bytes of that same message's text; an unknown or duplicate source id refuses the whole batch.
Check: `always` - for every prepared memory, `source.text.contains(quote)` for the batch message whose id the memory's row names, and every row's `message_id` names exactly one batch message
Fault/timing angle: none
Required faults and enabling state: a quote taken from another message of the batch; an invented `message_id`; a duplicate `message_id`; an empty quote
Confidence: high - [evidence](evidence/p2-capture-quote-is-an-exact-substring-of-its-own-batch-source.md). Read `parse_capture_batch` at `crates/daemon/src/memory_capture.rs:212` and `validate_capture_memories` at `:274`; the prompt states the rule at `:134` (offset corrected by refinement R8)
Existing check: `crates/daemon/src/memory_capture.rs:1393`, `:1426`, `:1458`, `:1499`, `:1506`; `crates/daemon/src/lib.rs:24897` (unaudited)
Impact: a quote from another message would cite evidence for a statement the source never made
Open questions: None.

### p2-capture-job-and-decision-identity-derive-from-the-source-tuple

Type: safety
Reachability: default-production - every enqueue and every publish
Status: active
Exercised: yes - store and daemon tests enqueue the same source twice, hash out-of-grammar ids, and publish batches
Guarantee: A capture job id is the SHA-256 of `(project, harness, session_id, message_id, role, redacted text)` with length prefixes, and each written decision's object id is derived from `(job_id, memory index)`, so a retried job writes the same ids.
Check: `always` - for every capture job, `job_id == job_id(source, redacted_text)`, and for every decision it writes, `object_id == "mem_" + sha256(job_id + ":" + index)[..32]`
Fault/timing angle: none
Required faults and enabling state: the same source enqueued twice; a fragment id outside the store grammar
Confidence: high - [evidence](evidence/p2-capture-job-and-decision-identity-derive-from-the-source-tuple.md). Read `job_id` at `crates/memory-store/src/memory_capture.rs:112`, the hashed fragment id in `native_capture_fragments` at `crates/daemon/src/memory_capture.rs:319`, and the object id at `:1267`
Existing check: `crates/memory-store/src/memory_capture.rs:755`; `crates/daemon/src/memory_capture.rs:1525`, `:1574`, `:1885`, `:1907`, `:2001`; `crates/daemon/src/lib.rs:24502`, `:25813` (unaudited)
Impact: unstable ids would duplicate memories on retry and break the deterministic anchor id U3 derives from the job id
Open questions: None.

### p2-capture-publish-and-session-delete-are-serialized

Type: safety
Reachability: default-production - every capture publish, checkpoint, and session delete takes `capture_commit_gate`
Status: active
Exercised: yes - daemon tests race checkpoints and publishes against session delete
Guarantee: Capture publication, capture checkpointing, source-capture publish, and session deletion each hold `capture_commit_gate` for their whole store and Kernel write, so a deleted session's sources land wholly before or wholly after the sweep.
Check: `always` - no capture or source publish commit for a session has a `commit_seq` after that session's delete sweep committed; observed by holding the gate from a test thread and asserting the competing operation has not committed (refinement R5 adds this gate-hold observation method)
Fault/timing angle: a checkpoint or publish admitted before a delete that runs after it
Required faults and enabling state: a session delete issued while a checkpoint or publish waits on the gate
Confidence: high - [evidence](evidence/p2-capture-publish-and-session-delete-are-serialized.md). Read the gate in `publish` at `crates/daemon/src/memory_capture.rs:1000`, the delete path at `crates/daemon/src/lib.rs:6918`, and `capture_after_response` at `crates/daemon/src/lib.rs:9166`
Existing check: `crates/daemon/src/lib.rs:25045`, `:25383`, `:25588`, `:26136`; `crates/daemon/src/lib.rs:24794`; `crates/daemon/src/source_capture_tests.rs:1037` (unaudited)
Impact: an interleaving would leave a deleted session's memory or descriptor live
Open questions: None.

### p2-messages-descriptor-identity-is-per-block-and-evidence-id-is-the-receipt-key

Type: safety
Reachability: explicit-config-only - `messages` descriptors are published only when source capture is enabled
Status: active
Exercised: yes - source-capture tests publish Pi windows; Kernel descriptor tests read by identity
Guarantee: Each text block of a captured message is one `messages` unit with identity `(project_id, harness, session_id, message_id, block_index)`, and its evidence id is `srcev:` followed by the unit's receipt key, which a capture job cannot recompute from the text.
Check: `always` - for every live `messages` descriptor, its identity has exactly those five fields and `detail.evidence_id == "srcev:" + unit.key`
Fault/timing angle: none
Required faults and enabling state: a Pi message with two text blocks; a re-published revision of one message
Confidence: high - [evidence](evidence/p2-messages-descriptor-identity-is-per-block-and-evidence-id-is-the-receipt-key.md). Read `SessionIdentity::message_unit` at `crates/daemon/src/harness_sources.rs:214` and `evidence_request` at `:1017`
Existing check: `crates/daemon/src/source_capture_tests.rs:867`, `:1259`; `crates/kernel/tests/kernel_source_descriptors.rs:494`, `:2484`, `:2593`, `:2717`, `:2795`, `:2871`; `crates/daemon/tests/harness_sources.rs:1706` (unaudited)
Impact: a descriptor keyed per message rather than per block, or an evidence id derived from text, would break the U3 citation join
Open questions: None.

### p2-deferred-source-units-publish-on-a-later-pass

Type: reachability
Reachability: explicit-config-only - source capture only
Status: active
Exercised: yes - the 33-unit window test and the starting-kernel test publish the deferred unit on a later pass
Guarantee: A unit the memo records as `Pending` can publish on a later pass that offers it again, so a completed publish can leave a unit deferred.
Check: `sometimes` - a campaign constructs a window whose memo holds a `Pending` entry and a later pass that publishes it; narrowed by refinement R6 to publication from the memo's `Pending` entry
Fault/timing angle: the Kernel is starting or the window exceeds one pass's unit bound
Required faults and enabling state: a 33-unit window; a Kernel still starting during the first pass
Confidence: high - [evidence](evidence/p2-deferred-source-units-publish-on-a-later-pass.md). Read `Disposition::Pending` at `crates/daemon/src/source_capture.rs:183`
Existing check: `crates/daemon/src/source_capture_tests.rs:181`, `:429`, `:465`, `:1073`, `:1234`; `crates/daemon/src/source_capture_tests.rs:4451` for the settlement witness (unaudited)
Impact: without the witness, U4's rule that settlement means job end rather than descriptor presence is untested
Open questions: None.

### p2-git-open-is-isolated-and-allocation-bounded

Type: safety
Reachability: default-production - every native capture submit opens the project root through `git_sources::open` in `observe_head` (`crates/daemon/src/memory_capture/head.rs:21`), added by U2 ([#1134](https://github.com/ahrav/eidnara/issues/1134)); before U2 the only caller was `GitReconciler`, which only tests construct (relabelled per METHOD rule 4)
Status: active
Exercised: yes - git-source tests refuse selections and an oversized delta base; the head-observation tests open repositories through the same function
Guarantee: The daemon's repository open skips installation, user, and system configuration, ignores replacement refs, and installs `max_object_bytes` as gix's allocation limit after the repository's own configuration.
Check: `always` - every repository the daemon opens through `git_sources::open` has `objects.ignore_replacements` set and the allocation limit equal to the bound; a packed delta whose base exceeds it is refused
Fault/timing angle: none
Required faults and enabling state: a delta-packed commit against an oversized base; a repository declaring an unsupported `extensions.objectFormat`
Confidence: high - [evidence](evidence/p2-git-open-is-isolated-and-allocation-bounded.md). Read `isolated_options` at `crates/daemon/src/git_sources.rs:128` and `open` at `:138`
Existing check: `crates/daemon/tests/git_sources.rs:549`, `:827`; `crates/daemon/src/memory_capture/head.rs:139` (unaudited)
Impact: an unisolated open would let repository or user configuration raise the bound or substitute objects
Open questions: None.

### p2-new-anchor-row-decodes-on-write

Type: safety
Reachability: default-production - every memory-capture publish of a job with a readable frozen HEAD calls `Envelope::insert_anchor` (`crates/daemon/src/memory_capture.rs:1261`), added by U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135))
Status: active
Exercised: yes - `every_anchor_kind_commits_a_row_the_engine_decodes_and_a_decision_can_reference` writes all seven kinds; `invalid_anchors_are_refused_as_invalid_input_with_a_typed_cause` refuses the required faults
Guarantee: Every `anchors` row committed through `insert_anchor` decodes with `AnchorCondition::decode`, holds values only in the columns its kind owns, carries a lowercase hex oid for git kinds, and fits within `MAX_OBJECT_PAYLOAD_BYTES`.
Check: `always` - after any commit that inserted an anchor, `validate_anchor_row(row)` is `Ok` for every stored row, which runs the engine's bound check and `AnchorCondition::decode` (and with it the unowned-column check); asserted at the end of each writing test because the writer is the only path that adds rows
Fault/timing angle: none
Required faults and enabling state: an uppercase oid, a 39-character oid, a captures payload above the bound, a non-half-open interval, an empty value, an unparseable range, duplicate captures, a capture naming another commit, a secret in a value or a capture, and a raw row with an unowned column
Confidence: high - [evidence](evidence/p2-new-anchor-row-decodes-on-write.md). Read `validate_anchor` at `crates/kernel/src/anchor_write.rs:263` and `insert_anchor_inner` at `:320`; the typed `AnchorValue` (`:35`) makes an unowned column unrepresentable, and the round trip at `:136` compares the decoded condition to the request
Existing check: `crates/kernel/tests/kernel_anchor_write.rs:311`, `:454`, `:606`, `:378` (unaudited)
Impact: a capture decision anchored to an undecodable row evaluates `Uncertain` forever and revalidation fails silently
Open questions:
- The specification asked whether the writer accepts a typed enum or a raw `AnchorRowSpec`; #1133 resolved it as the typed `AnchorValue` enum, with `validate_anchor_row` exposed for raw-row checks only.

### p2-new-anchor-and-registry-share-one-envelope

Type: safety
Reachability: default-production - as above
Status: active
Exercised: yes - `a_decision_failure_after_the_anchor_rolls_back_both` and `a_duplicate_anchor_id_is_a_conflict_within_and_across_commits` construct the required fault and the duplicate cases
Guarantee: An `anchors` row and its `object_registry` row are visible together or not at all, and a decision referencing the anchor is committed in the same or a later `commit_seq`.
Check: `always` - for every live `anchors` row there is an `object_registry` row of kind `anchor` with the same `object_id` and `created_commit_seq`, and every `decisions.anchor_id` names a row with `created_commit_seq <= decision.created_commit_seq`
Fault/timing angle: a refusal inside the same envelope after the anchor insert
Required faults and enabling state: `insert_decision` fails after `insert_anchor` succeeded inside one closure; a duplicate `anchor_id` within one envelope and across two commits
Confidence: high - [evidence](evidence/p2-new-anchor-and-registry-share-one-envelope.md). Read `insert_anchor_inner` at `crates/kernel/src/anchor_write.rs:320` and `Envelope::guarded` at `crates/kernel/src/envelope.rs:348`
Existing check: `crates/kernel/tests/kernel_anchor_write.rs:429`, `:654`, `:399`; `crates/daemon/tests/claim_sources.rs:1508` (unaudited)
Impact: an orphan anchor is unreachable and inflates the registry; a decision inserted before its anchor is refused `NotFound` and fails the job
Open questions: None.

### p2-new-capture-anchor-identity-is-deterministic

Type: safety
Reachability: default-production - every anchored capture publish
Status: active - activated by U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135))
Exercised: yes - `one_job_and_plan_always_write_the_same_anchor_and_decision_rows` publishes one job and plan into two fresh Kernels; `a_retry_after_a_rolled_back_publish_writes_one_anchor_and_the_same_rows` refuses a publish after its anchor and first decision were written, then retries into the same Kernel; `a_replayed_publish_adds_no_rows_and_no_counts` replays a committed publish
Guarantee: The anchor id for a capture job is a pure function of `(job_id, head_oid)`, so a retry after a rolled-back commit produces identical bytes and converges on one row, and two jobs at one oid produce two rows.
Check: `always` - for every capture decision with `anchor_id = Some(a)`, `a == "anchor_" + sha256(job_id + ":" + oid)[..32]` for the job's frozen head, every decision of the job names `a`, and exactly one live `anchors` row has that id; a retry rebuilds byte-identical anchor and decision rows, and its citation events match in kind and evidence id while `recorded_at` is the retry's clock
Fault/timing angle: a retry after a rolled-back or lost commit
Required faults and enabling state: the same job and plan published into two fresh Kernels; a committed publish whose completion the store refuses, then a retry
Confidence: high - [evidence](evidence/p2-new-capture-anchor-identity-is-deterministic.md). Read `anchor_spec` at `crates/daemon/src/memory_capture/cite.rs:362` and the lazy insert in `publish_captured_memories` at `crates/daemon/src/memory_capture.rs:1202`
Existing check: `crates/daemon/src/memory_capture.rs:1734`, `:1772`; `crates/daemon/src/source_capture_tests.rs:3822`; `crates/daemon/src/source_capture_tests.rs:3496` (one anchor per job); `crates/kernel/tests/kernel_anchor_write.rs:654` for the Kernel refusal of a second row (unaudited)
Impact: duplicate anchors per job, or `Conflict` on retry that permanently fails the job
Open questions:
- Whether anchors should be shared across jobs at one oid (needs human input; the model keeps per-job identity because `insert_anchor` refuses duplicates and no receipt covers a cross-job row).

### p2-new-head-observed-once-and-frozen

Type: safety
Reachability: default-production - every `memory.capture.submit` whose output parses observes HEAD in `accept_native_output` (`crates/daemon/src/memory_capture/native.rs:609`)
Status: active - activated by U2 ([#1134](https://github.com/ahrav/eidnara/issues/1134)); U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135)) adds the anchor-equals-frozen-head clause
Exercised: yes - `a_submission_freezes_one_checkout_head_in_every_plan_and_a_retry_keeps_it` freezes one head in both jobs of a submission for five roots and keeps it across a later commit and a retry; `captured_memories_cite_the_blocks_holding_their_quotes_and_anchor_at_head` and `misses_carry_one_closed_reason_each_and_the_counters_follow_them` check each anchor's oid against the fixture HEAD, detached included
Guarantee: The checkout HEAD is read once per `memory.capture.submit`, before any plan of that submission is frozen, and the observation is part of the frozen `prepared_json`, so the receipt digest covers the anchor's value.
Check: `always` - every job frozen by one submission carries the identical `head`, equal to the commit the fixture's HEAD named at submit, and a retry publishes that frozen value; after U3, for every capture decision with an anchor, `anchors.reachable_from_oid == parse(prepared_json).head`
Fault/timing angle: a `git checkout` between the submit and its retry
Required faults and enabling state: a commit on the checked-out branch between the submit and the retry, with completion refused so the frozen plans stay in the store
Confidence: high - [evidence](evidence/p2-new-head-observed-once-and-frozen.md). Read `accept_native_output` (`crates/daemon/src/memory_capture/native.rs:594`): one `observe_head` call precedes the loop that freezes each job's plan, and the receipt digest covers every frozen byte (`crates/daemon/src/memory_capture.rs:1217`)
Existing check: `crates/daemon/src/lib.rs:24632`; `crates/daemon/src/memory_capture/head.rs:123`; `crates/daemon/src/source_capture_tests.rs:3496`, `:3646` (unaudited)
Impact: the anchor stops describing the revision at which the quote was observed, and the receipt no longer covers the anchor's value
Open questions: None.

### p2-new-head-unavailable-keeps-decision-admitted

Type: safety
Reachability: default-production - capture in an unborn, unreadable, unsupported-format, or non-repository root
Status: active - activated by U2 ([#1134](https://github.com/ahrav/eidnara/issues/1134)); completed by U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135)) with the rationale reason, the counter, and the no-anchor clause
Exercised: yes - U2 freezes `head_unavailable` for unborn, plain, unsupported-format, missing, and non-commit roots; U3's `misses_carry_one_closed_reason_each_and_the_counters_follow_them` admits decisions at a plain and an unborn root with no anchor row and `anchor: "head_unavailable"`, and `a_plan_frozen_without_a_head_publishes_as_before` does so for a plan without `head`
Guarantee: When HEAD is unborn or unreadable, the object format is unsupported, or the root is not a repository, the decision is still admitted with `anchor_id = None` and rationale `anchor: "head_unavailable"`, and no `anchors` row is written for the job.
Check: `always-or-unreached` - the path runs only when the head observation is unavailable; whenever it runs, the committed decision has `anchor_id IS NULL`, the rationale carries the reason, and no anchor row names the job
Fault/timing angle: none
Required faults and enabling state: a root with no `.git`; an unborn branch; an `objectFormat` gix refuses
Confidence: medium - [evidence](evidence/p2-new-head-unavailable-keeps-decision-admitted.md). The owner decided on 2026-10-25 (OQ2-11) that a detached HEAD anchors at its commit, which removes detached HEAD from this path; `observe_head` (`crates/daemon/src/memory_capture/head.rs:21`) maps every other refusal to `HeadUnavailable`
Existing check: `crates/daemon/src/memory_capture/head.rs:139`; `crates/daemon/src/lib.rs:24632`, `:24742`; `crates/daemon/src/source_capture_tests.rs:3646` (unaudited)
Impact: capture would stop in fresh checkouts
Open questions: None.

### p2-new-citation-is-alias-only

Type: safety
Reachability: explicit-config-only - citations exist only when source capture publishes `messages` descriptors
Status: active - activated by U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135))
Exercised: yes - `captured_memories_cite_the_blocks_holding_their_quotes_and_anchor_at_head` compares every citation event with the descriptor of the job's own message; `an_invented_source_id_is_refused_before_any_citation_lookup` refuses an output naming a source outside the batch
Guarantee: Every citation a capture decision carries is the `detail.evidence_id` of a live `messages` descriptor whose identity is the job's `(project_id, harness, session_id, message_id)`, read inside the publish envelope; no byte of the model output reaches it.
Check: `always` - for every capture citation event, the evidence id equals the `detail.evidence_id` of a descriptor of the job's lineage tuple live at the decision's commit, and `PreparedCapture` (which denies unknown fields) has no evidence field
Fault/timing angle: none
Required faults and enabling state: model output naming a source outside the batch; a message published in another session with the same id
Confidence: high - [evidence](evidence/p2-new-citation-is-alias-only.md). Read `Citation::lookup` at `crates/daemon/src/memory_capture/cite.rs:260`, which derives each lineage from the job row alone (`message_lineage`, `:148`), and the call inside the publish envelope at `crates/daemon/src/memory_capture.rs:1265`
Existing check: `crates/daemon/src/source_capture_tests.rs:3496`, `:3737` (unaudited)
Impact: a model-emitted id would let the extraction forge its own evidence link
Open questions: None.

### p2-new-citation-block-target-rule

Type: safety
Reachability: explicit-config-only - as above
Status: active - activated by U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135)), restated for `block`, `span`, and `first` (ADR-P2-001)
Exercised: yes - the end-to-end test cites a one-block message (`block`), the third of four items (`block`, that block only), a quote across two items (`span`, two events ascending), a quote starting at a separator (`span` of the blocks on both sides), and a message whose first block source capture refused for a secret, so no block locates (`first`, the lowest live block); unit and property tests cover inner newlines, empty blocks, a stopped walk, and random quotes of fully located messages
Guarantee: A cited capture decision cites the located block holding the first occurrence of its quote (`block`), the run of located adjacent blocks the occurrence crosses, a separator it touches joining the blocks on both sides (`span`), or else the lowest live block (`first`), and records the rule, the ascending block indexes, and their evidence ids in its rationale.
Check: `always` - for every cited capture decision, `citation_block` is `block` or `span` exactly when the cited blocks' bytes joined with `\n` contain the quote's first occurrence, `first` cites the lowest live `block_index`, and `citation_blocks` and `citation_evidence` list the cited blocks in ascending order, parallel to the citation events
Fault/timing angle: none
Required faults and enabling state: a multi-block message with the quote in a later item; a quote across the `\n` join; a quote starting at a separator; a block source capture refused, so the redacted job text locates no block
Confidence: high - [evidence](evidence/p2-new-citation-block-target-rule.md). Read `locate` at `crates/daemon/src/memory_capture/cite.rs:196` and `target` at `:226`
Existing check: `crates/daemon/src/source_capture_tests.rs:3496`; `crates/daemon/src/memory_capture/cite.rs:401`, `:412`, `:504`, `:518` (unaudited)
Impact: revalidation reports a false mismatch for correctly captured memories
Open questions: None.

### p2-new-citation-folds-sensitivity-and-pins-evidence

Type: safety
Reachability: explicit-config-only - as above
Status: active - activated by U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135)); under the decision-event placement the owner chose for OQ2-10 the guarantee is the evidence pin, and the decision's class is its own
Exercised: yes - `cited_capture_evidence_survives_expiry_and_refuses_retirement` sweeps past the retention deadline and attempts the retirement; the end-to-end test checks the cited decision keeps class `normal` and an empty `decisions.evidence_id`
Guarantee: The evidence a capture decision cites survives `expire_harness_captures` while the decision is live, retiring it while cited is refused, and the decision row's own class and `evidence_id` stay as an uncited decision's.
Check: `always` - for every live cited capture decision, each cited evidence row is live after every sweep, `retire_evidence` on it returns `Conflict`, `decisions.evidence_id IS NULL`, and `sensitivity_class` is the class the capture asserted
Fault/timing angle: a sweep running after the evidence's `retain_until`
Required faults and enabling state: cited evidence past `retain_until` with one live citing decision; a retirement attempt
Confidence: high - [evidence](evidence/p2-new-citation-folds-sensitivity-and-pins-evidence.md). `append_citations` (`crates/daemon/src/memory_capture/cite.rs:338`) writes decision events, which `EVIDENCE_CITED_SQL` counts (`crates/kernel/src/slice/write.rs:42`)
Existing check: `crates/daemon/src/source_capture_tests.rs:3783`, `:3496` (unaudited)
Impact: retired evidence breaks every citation
Open questions: None.

### p2-new-settlement-wait-is-bounded-and-gate-free

Type: liveness
Reachability: explicit-config-only - the wait runs only with source and memory consent on, for an output submit, and for a job whose source id can join
Status: active - activated by U4 ([#1136](https://github.com/ahrav/eidnara/issues/1136))
Exercised: yes - `a_running_entry_waits_for_its_end_or_its_budget_deadline` releases a waiter exactly at `accepted_at + 5 s`; `on_the_paused_clock_the_wait_is_zero_when_settled_and_ends_exactly_at_the_deadline` holds a real submit pending at the deadline minus 1 ms and releases it exactly at the deadline on the paused tokio clock; `a_publish_past_its_budget_releases_the_submit_at_the_deadline` does so on the real clock; `a_submit_during_the_publish_waits_outside_the_gate_and_then_cites` takes the capture commit gate from the test while the submit waits
Guarantee: With source capture enabled, the submit path proceeds to publish within `accepted_at + 5 s` plus one blocking-pool scheduling slack, whatever the source job does, and never holds the capture commit gate while waiting.
Check: `always` - the wait returns within the bound measured from the record's `accepted_at`, the bound being `CAPTURE_BUDGET` (`crates/daemon/src/source_capture.rs:41`), which also seeds the job's `EvalBudget` from the same instant in `into_job` (`:711`); a submit awaits each distinct job session in turn, and one publication slot means at most one entry is `Running`, so the waits end by one deadline; the commit gate is acquired only after the waits return, in the blocking hop, and the job records its end after releasing the gate
Fault/timing angle: a source job that stalls past its budget
Required faults and enabling state: a parked source job under a paused tokio clock; a parked job with a real clock
Confidence: high - [evidence](evidence/p2-new-settlement-wait-is-bounded-and-gate-free.md). Read `settle` at `crates/daemon/src/source_capture/settlement.rs:168` and `capture_settlements` at `crates/daemon/src/memory_capture/native.rs:123`
Existing check: `crates/daemon/src/source_capture/settlement.rs:236`; `crates/daemon/src/source_capture_tests.rs:4004`, `:4069`, `:4173` (unaudited)
Impact: the native lease is consumed by waits, or the daemon deadlocks on the capture gate
Open questions: None.

### p2-new-lookup-runs-only-after-settlement

Type: safety
Reachability: explicit-config-only - as above
Status: active - activated by U4 ([#1136](https://github.com/ahrav/eidnara/issues/1136))
Exercised: yes - `a_submit_during_the_publish_waits_outside_the_gate_and_then_cites` counts zero publications that may run a lookup while the entry is `Running`, then cites after the job ends; `a_submit_on_another_session_waits_on_each_job_session_it_carries` waits on the parked session's entry for a job another session's submit carries; `a_completed_publish_with_a_deferred_unit_reports_descriptor_not_found` uses the deferred unit as the witness that settlement is job end
Guarantee: The citation lookup of a submission runs only after the settlement record of the job's own session is ended, refused, absent, or past its deadline.
Check: `always` - at the instant the publish envelope opens for a job, the record for the job's `(project_id, harness, session_id)` is not running within its deadline; the submit handler awaits `capture_settlements` for every job session before the blocking hop that opens the envelope, and a replayed prepared plan reads the record without waiting and treats a running entry as `BudgetExpired`
Fault/timing angle: a slow source job and a fast harness submit, including a submit on another session whose lease carries the job
Required faults and enabling state: a parked source job and a submit inside its window; a second session bound to the same project
Confidence: high - [evidence](evidence/p2-new-lookup-runs-only-after-settlement.md). Read the wait at `crates/daemon/src/memory_capture/native.rs:389`, before `kernel_routes::blocking`, and `settlement_key` at `crates/daemon/src/memory_capture/cite.rs:121`
Existing check: `crates/daemon/src/source_capture_tests.rs:4069`, `:4134`, `:4451`; `crates/daemon/src/source_capture/settlement.rs:208`, `:360` (unaudited)
Impact: avoidable `descriptor_not_found` misses that hide a race as a missing source
Open questions: None.

### p2-new-miss-reason-matches-outcome

Type: safety
Reachability: default-production - every capture decision
Status: active - activated by U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135)); completed by U4 ([#1136](https://github.com/ahrav/eidnara/issues/1136)) with the `publish_budget_expired` construction
Exercised: yes - `cited`, `source_capture_off`, `source_id_unjoinable`, `descriptor_not_found`, `anchored`, and `head_unavailable` since U3; U4 adds `publish_budget_expired` for a job past its budget and for a refused handoff, and `descriptor_not_found` for a completed publish that deferred the unit
Guarantee: Every capture decision carries exactly one `citation` value from `cited | source_capture_off | source_id_unjoinable | publish_budget_expired | descriptor_not_found` and one `anchor` value from `anchored | head_unavailable`, chosen by the specification's precedence.
Check: `always` - for every capture decision, `(no citation event) == (citation != "cited")` and `(anchor_id IS NULL) == (anchor == "head_unavailable")`; consent off gives `source_capture_off` with no lookup, an empty lookup for a hashed id gives `source_id_unjoinable`, an empty lookup after a refused, deadlined, or still-running publish gives `publish_budget_expired`, and any other empty lookup gives `descriptor_not_found`
Fault/timing angle: a source publish refused or past its budget at submit time
Required faults and enabling state: source capture off; a hashed source id; an unpublished message; a plain and an unborn root; a parked source job; a refused handoff; a 33-unit window
Confidence: high - [evidence](evidence/p2-new-miss-reason-matches-outcome.md). Read `Citation::lookup` at `crates/daemon/src/memory_capture/cite.rs:260`, `is_hashed_source_id` at `:132`, and `settle` at `crates/daemon/src/source_capture/settlement.rs:168`
Existing check: `crates/daemon/src/source_capture_tests.rs:3496`, `:3646`, `:4173`, `:4227`, `:4451`; `crates/daemon/src/memory_capture/cite.rs:439`; `crates/daemon/src/memory_capture.rs:1574`; `crates/daemon/src/source_capture/settlement.rs:208` (unaudited)
Impact: revalidation cannot tell "never captured" from "captured late" from "lookup failed"
Open questions: None.

### p2-new-miss-counters-track-rationale

Type: safety
Reachability: default-production - as above
Status: active - activated by U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135))
Exercised: yes - the end-to-end and miss tests compare the seven counters with the committed decisions; `a_replayed_publish_adds_no_rows_and_no_counts` replays a committed publish
Guarantee: Within one daemon process, each of the seven capture-decision counters equals the number of committed capture decisions carrying that reason since process start, and a rolled-back or replayed publish adds nothing.
Check: `always` - after each publish settles, `counter[reason] == committed(reason)` in this process; the closure records the reasons, and `record_capture_decisions` adds them only after `commit_before` returns a receipt that was not replayed, so `committed <= counter <= attempted` holds under loss per METHOD effect accounting
Fault/timing angle: a publish that rolls back after its reasons were computed; a replayed receipt
Required faults and enabling state: a replayed publish; a refused commit
Confidence: high - [evidence](evidence/p2-new-miss-counters-track-rationale.md). Read `record_capture_decisions` at `crates/daemon/src/source_capture.rs:662` and its call in `CaptureWork::publish`
Existing check: `crates/daemon/src/source_capture_tests.rs:3496`, `:3646`, `:3822`; `crates/host-runtime/src/control.rs:1960` for the wire allowlist (unaudited)
Impact: operators tune the budget on numbers that count failures as misses
Open questions: None.

### p2-new-anchor-and-citation-add-no-authority

Type: safety
Reachability: default-production - as above
Status: active - activated by U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135))
Exercised: yes - the end-to-end test admits four cited decisions and one uncited sibling in one session and compares their admission rows and served visibility
Guarantee: A capture decision's admission record carries `ModelInference` and `AssistantInference` and the same maturity, and it serves with the same visibility and class, whether or not it has an anchor or a citation.
Check: `always` - sibling capture decisions that differ only in citation or anchor presence have equal `source_class`, `taint_class`, `maturity`, `effective_maturity`, and `elevated_support`, and equal visibility on every surface
Fault/timing angle: none
Required faults and enabling state: one cited and one uncited job in one session
Confidence: high - [evidence](evidence/p2-new-anchor-and-citation-add-no-authority.md). Admission classes are fixed at `crates/daemon/src/memory_capture.rs:1309`, and neither the anchor nor the citation event is an admission input
Existing check: `crates/daemon/src/source_capture_tests.rs:3496` (unaudited)
Impact: a model could raise its own authority by quoting a captured message
Open questions: None.

### p2-new-cited-decision-pair-observed

Type: reachability
Reachability: default-production - campaign coverage
Status: active - activated by U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135)); completed by U4 ([#1136](https://github.com/ahrav/eidnara/issues/1136)), which constructs the last state
Exercised: yes - an anchored and cited decision, and decisions for `source_capture_off`, `source_id_unjoinable`, `descriptor_not_found`, and `head_unavailable` since U3; U4 commits `publish_budget_expired` decisions
Guarantee: A test campaign produces at least one capture decision with both an anchor and a citation, and at least one decision for each miss reason, so every legal state is constructed.
Check: `sometimes` - states, not code points: a committed decision with an anchor and a citation, and one per miss reason
Fault/timing angle: none
Required faults and enabling state: a published message and a repository fixture with one commit; source capture off; a hashed id; a plain root; a parked source job past its budget
Confidence: high - [evidence](evidence/p2-new-cited-decision-pair-observed.md)
Existing check: `crates/daemon/src/source_capture_tests.rs:3496`, `:3646`, `:4173`, `:4227` (unaudited)
Impact: without these witnesses, the safety records above are vacuous
Open questions: None.

## Relationships

- `p2-new-anchor-row-decodes-on-write` keeps rows out of the uncertain path of
  `p2-undecodable-or-oversized-anchor-row-is-uncertain`: the writer runs the
  same bound check and decoder the engine runs.
- `p2-new-anchor-and-registry-share-one-envelope` and the anchor clause of
  `p2-decision-parents-must-be-live-at-insert` together order an anchor before
  every decision that names it.
- `p2-new-capture-anchor-identity-is-deterministic` depends on
  `p2-capture-job-and-decision-identity-derive-from-the-source-tuple` and
  `p2-capture-commit-replays-by-job-key-and-frozen-digest`.
- `p2-new-citation-folds-sensitivity-and-pins-evidence` reduces to
  `p2-cited-evidence-is-never-retired-or-swept` under event placement, and
  sidesteps `p2-citing-decision-carries-at-least-its-evidence-class`.
- `p2-new-citation-is-alias-only` depends on
  `p2-messages-descriptor-identity-is-per-block-and-evidence-id-is-the-receipt-key`
  and `p2-capture-quote-is-an-exact-substring-of-its-own-batch-source`.
- `p2-new-lookup-runs-only-after-settlement` uses
  `p2-deferred-source-units-publish-on-a-later-pass` as its witness that
  settlement differs from descriptor presence.
- `p2-new-head-unavailable-keeps-decision-admitted` relies on
  `p2-git-open-is-isolated-and-allocation-bounded` once U2 routes capture
  through that open.

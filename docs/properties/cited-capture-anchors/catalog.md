# Cited capture anchors: catalog

Method: `../METHOD.md`. The records come from the Property Catalog of the
evidence-cited memory capture specification
([#1130](https://github.com/ahrav/eidnara/issues/1130)), authored against
Eidnara `a0e3f2993`. The specification's long-form bundle is unavailable, so
the U1 ticket ([#1133](https://github.com/ahrav/eidnara/issues/1133))
reconstructs every record from the specification body and re-verifies every
reference against the branch that adds this part, based on `3a3b916b7`. A
reference `path:line` names the first line of the function, constant, or
test it cites at that branch.

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
5. S5, the bounded git open: `crates/daemon/src/git_sources.rs`.
6. N1, the typed anchor write: `crates/kernel/src/anchor_write.rs`
   (`Envelope::insert_anchor`, `validate_anchor`, `validate_anchor_row`),
   added by #1133.

New behavior the specification designs and later tickets land: N2 the anchor
on capture decisions, N3 citation derivation, N4 ordering and settlement,
N5 miss accounting, and N6 unchanged authority.

## Status vocabulary

`Status:` uses METHOD's `active` for the 14 existing-behavior records and for
the two N1 records #1133 makes checkable. The other 12 records use
`Status: proposed`, the value the specification assigns to a designed
obligation whose code does not exist yet. A proposed record names the ticket
that converts it to `active`, carries `Exercised: not yet`, and states the
intended reachability. Every existing check named in this part has status
`unaudited` in `existing-checks.md`.

## Reachability classes

- `default-production`: reached with the shipped configuration. Memory
  capture runs when `memory_enabled`, `auto_promote`, and
  `memory_auto_capture` are on (`crates/daemon/src/memory_capture.rs:570`),
  and all three default to on (`crates/daemon/src/config.rs:146`).
- `explicit-config-only`: reached only when source capture is enabled;
  `source_capture_enabled` defaults to off (`crates/daemon/src/config.rs:146`).
- `test-only`: no production caller at the branch tip. Nothing outside
  `crates/kernel` calls `evaluate_batch` or builds an `ApplicabilityRequest`,
  and `GitReconciler`, the only caller of `git_sources::open`, is constructed
  only by tests. `Envelope::insert_anchor` has no production caller until
  U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135)).

## Index

| Slug | Type | Surface | Status | Semantics | Reachability |
| --- | --- | --- | --- | --- | --- |
| `p2-undecodable-or-oversized-anchor-row-is-uncertain` | safety | S1 | active | `always` | test-only |
| `p2-reachable-from-holds-on-ancestor-of-head` | safety | S1 | active | `always` | test-only |
| `p2-reachable-from-negative-needs-complete-evidence` | safety | S1 | active | `always` | test-only |
| `p2-anchor-verdict-cache-key-covers-row-head-and-boundary` | safety | S1 | active | `always` | test-only |
| `p2-decision-parents-must-be-live-at-insert` | safety | S2 | active | `always`; anchor clause `always-or-unreached` | default-production |
| `p2-citing-decision-carries-at-least-its-evidence-class` | safety | S2 | active | `always` | default-production |
| `p2-cited-evidence-is-never-retired-or-swept` | safety | S2 | active | `always` | explicit-config-only |
| `p2-capture-commit-replays-by-job-key-and-frozen-digest` | safety | S3 | active | `always` | default-production |
| `p2-capture-quote-is-an-exact-substring-of-its-own-batch-source` | safety | S3 | active | `always` | default-production |
| `p2-capture-job-and-decision-identity-derive-from-the-source-tuple` | safety | S3 | active | `always` | default-production |
| `p2-capture-publish-and-session-delete-are-serialized` | safety | S3 | active | `always` | default-production |
| `p2-messages-descriptor-identity-is-per-block-and-evidence-id-is-the-receipt-key` | safety | S4 | active | `always` | explicit-config-only |
| `p2-deferred-source-units-publish-on-a-later-pass` | reachability | S4 | active | `sometimes` | explicit-config-only |
| `p2-git-open-is-isolated-and-allocation-bounded` | safety | S5 | active | `always` | test-only |
| `p2-new-anchor-row-decodes-on-write` | safety | N1 | active | `always` | test-only |
| `p2-new-anchor-and-registry-share-one-envelope` | safety | N1 | active | `always` | test-only |
| `p2-new-capture-anchor-identity-is-deterministic` | safety | N2 | proposed (#1135) | `always` | default-production (intended) |
| `p2-new-head-observed-once-and-frozen` | safety | N2 | proposed (#1134) | `always` | default-production (intended) |
| `p2-new-head-unavailable-keeps-decision-admitted` | safety | N2 | proposed (#1134) | `always-or-unreached` | default-production (intended) |
| `p2-new-citation-is-alias-only` | safety | N3 | proposed (#1135) | `always` | explicit-config-only (intended) |
| `p2-new-citation-block-target-rule` | safety | N3 | proposed (#1135) | `always` | explicit-config-only (intended) |
| `p2-new-citation-folds-sensitivity-and-pins-evidence` | safety | N3 | proposed (#1135) | `always` | explicit-config-only (intended) |
| `p2-new-settlement-wait-is-bounded-and-gate-free` | liveness | N4 | proposed (#1136) | `always` | explicit-config-only (intended) |
| `p2-new-lookup-runs-only-after-settlement` | safety | N4 | proposed (#1136) | `always` | explicit-config-only (intended) |
| `p2-new-miss-reason-matches-outcome` | safety | N5 | proposed (#1135) | `always` | default-production (intended) |
| `p2-new-miss-counters-track-rationale` | safety | N5 | proposed (#1135) | `always` | default-production (intended) |
| `p2-new-anchor-and-citation-add-no-authority` | safety | N6 | proposed (#1135) | `always` | default-production (intended) |
| `p2-new-cited-decision-pair-observed` | reachability | N2 to N5 | proposed (#1135, #1136) | `sometimes` | default-production (intended) |

Counts: 28 records; 14 existing-behavior records with `unaudited` existing
checks, 2 active N1 records, 12 proposed records. Semantics distribution:
`always` 25, `always-or-unreached` 1, `sometimes` 2, `reachable` 0,
`unreachable` 0. The anchor clause of
`p2-decision-parents-must-be-live-at-insert` is a second,
`always-or-unreached` check inside an `always` record and is not counted
separately. Reachability: 7 test-only, 6 default-production, 3
explicit-config-only among the 16 active records; the specification's count
of 5 test-only records grows by the two N1 records, whose writer has no
production caller yet.

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
Existing check: `crates/kernel/tests/kernel_anchor_resolution.rs:44`, `:159`, `:557`; `crates/kernel/tests/kernel_applicability_engine.rs:806` (unaudited)
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
Reachability: default-production - every memory-capture publish calls `insert_decision`; the `anchor_id` clause is reached only by tests until U3 passes an anchor id
Status: active
Exercised: partial - the domain and evidence clauses are exercised by Kernel slice tests; the anchor clause by `a_decision_inserted_before_its_anchor_is_not_found_and_the_commit_rolls_back`
Guarantee: `insert_decision` refuses with `NotFound` a decision whose domain, proposition, scope, anchor, or evidence parent is absent or invalidated in the envelope's transaction.
Check: `always` - every committed decision row names a live domain and, for each set parent column, a parent row live at the decision's `created_commit_seq`. The `anchor_id` clause is `always-or-unreached` (refinement R1): production passes `anchor_id: None` at the branch tip, so the clause carries the precondition marker `sometimes(decision_insert_named_an_anchor_id)` and holds whenever reached
Fault/timing angle: a parent retired in an earlier commit, or inserted later in the same envelope
Required faults and enabling state: a decision naming a missing domain; a decision naming an anchor inserted after it in one envelope
Confidence: high - [evidence](evidence/p2-decision-parents-must-be-live-at-insert.md). Read `require_parents` at `crates/kernel/src/slice/write.rs:1095`
Existing check: `crates/kernel/tests/kernel_slice.rs:196`; `crates/kernel/tests/kernel_envelope.rs:669`; for anchors `crates/kernel/tests/kernel_anchor_write.rs:334` (unaudited)
Impact: a decision citing a missing anchor would fail the foreign key and the whole capture job would fail
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
Existing check: `crates/kernel/tests/kernel_source_descriptors.rs:1895`; `crates/daemon/tests/harness_sources.rs:3482`; `crates/kernel/src/local_file.rs:791`, `:889` (unaudited)
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
Confidence: high - [evidence](evidence/p2-capture-commit-replays-by-job-key-and-frozen-digest.md). Read `publish_captured_memories` at `crates/daemon/src/memory_capture.rs:1154` and the frozen-plan read in `publish` at `:968`
Existing check: `crates/daemon/src/memory_capture.rs:1645`; `crates/kernel/tests/kernel_envelope.rs:160`; `crates/daemon/src/lib.rs:24489` (unaudited)
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
Confidence: high - [evidence](evidence/p2-capture-quote-is-an-exact-substring-of-its-own-batch-source.md). Read `parse_capture_batch` at `crates/daemon/src/memory_capture.rs:204` and `validate_capture_memories` at `:266`; the prompt states the rule at `:126` (offset corrected by refinement R8)
Existing check: `crates/daemon/src/memory_capture.rs:1277`, `:1310`, `:1342`, `:1383`, `:1390`; `crates/daemon/src/lib.rs:24703` (unaudited)
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
Confidence: high - [evidence](evidence/p2-capture-job-and-decision-identity-derive-from-the-source-tuple.md). Read `job_id` at `crates/memory-store/src/memory_capture.rs:112`, the hashed fragment id in `native_capture_fragments` at `crates/daemon/src/memory_capture.rs:311`, and the object id at `:1188`
Existing check: `crates/memory-store/src/memory_capture.rs:755`; `crates/daemon/src/memory_capture.rs:1409`, `:1458`, `:1558`, `:1580`, `:1645`; `crates/daemon/src/lib.rs:24489`, `:25619` (unaudited)
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
Confidence: high - [evidence](evidence/p2-capture-publish-and-session-delete-are-serialized.md). Read the gate in `publish` at `crates/daemon/src/memory_capture.rs:975`, the delete path at `crates/daemon/src/lib.rs:6909`, and `capture_after_response` at `crates/daemon/src/lib.rs:9157`
Existing check: `crates/daemon/src/lib.rs:24851`, `:25189`, `:25394`, `:25942`; `crates/daemon/src/lib.rs:24600`; `crates/daemon/src/source_capture_tests.rs:1036` (unaudited)
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
Confidence: high - [evidence](evidence/p2-messages-descriptor-identity-is-per-block-and-evidence-id-is-the-receipt-key.md). Read `SessionIdentity::message_unit` at `crates/daemon/src/harness_sources.rs:189` and `evidence_request` at `:992`
Existing check: `crates/daemon/src/source_capture_tests.rs:866`, `:1258`; `crates/kernel/tests/kernel_source_descriptors.rs:494`, `:2484`, `:2593`, `:2717`, `:2795`, `:2871`; `crates/daemon/tests/harness_sources.rs:1706` (unaudited)
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
Confidence: high - [evidence](evidence/p2-deferred-source-units-publish-on-a-later-pass.md). Read `Disposition::Pending` at `crates/daemon/src/source_capture.rs:158`
Existing check: `crates/daemon/src/source_capture_tests.rs:180`, `:428`, `:464`, `:1072`, `:1233` (unaudited)
Impact: without the witness, U4's rule that settlement means job end rather than descriptor presence is untested
Open questions: None.

### p2-git-open-is-isolated-and-allocation-bounded

Type: safety
Reachability: test-only - `git_sources::open` is called only by `GitReconciler`, which only tests construct; U2 ([#1134](https://github.com/ahrav/eidnara/issues/1134)) adds a default-production path through capture
Status: active
Exercised: yes - git-source tests refuse selections and an oversized delta base
Guarantee: The daemon's repository open skips installation, user, and system configuration, ignores replacement refs, and installs `max_object_bytes` as gix's allocation limit after the repository's own configuration.
Check: `always` - every repository the daemon opens through `git_sources::open` has `objects.ignore_replacements` set and the allocation limit equal to the bound; a packed delta whose base exceeds it is refused
Fault/timing angle: none
Required faults and enabling state: a delta-packed commit against an oversized base; a repository declaring an unsupported `extensions.objectFormat`
Confidence: high - [evidence](evidence/p2-git-open-is-isolated-and-allocation-bounded.md). Read `isolated_options` at `crates/daemon/src/git_sources.rs:128` and `open` at `:138`
Existing check: `crates/daemon/tests/git_sources.rs:549`, `:827` (unaudited)
Impact: an unisolated open would let repository or user configuration raise the bound or substitute objects
Open questions: None.

### p2-new-anchor-row-decodes-on-write

Type: safety
Reachability: test-only - `Envelope::insert_anchor` has no production caller until U3
Status: active
Exercised: yes - `every_anchor_kind_commits_a_row_the_engine_decodes_and_a_decision_can_reference` writes all seven kinds; `invalid_anchors_are_refused_as_invalid_input_with_a_typed_cause` refuses the required faults
Guarantee: Every `anchors` row committed through `insert_anchor` decodes with `AnchorCondition::decode`, holds values only in the columns its kind owns, carries a lowercase hex oid for git kinds, and fits within `MAX_OBJECT_PAYLOAD_BYTES`.
Check: `always` - after any commit that inserted an anchor, `validate_anchor_row(row)` is `Ok` for every stored row, which runs the engine's bound check and `AnchorCondition::decode` (and with it the unowned-column check); asserted after every commit because the writer is the only path that adds rows
Fault/timing angle: none
Required faults and enabling state: an uppercase oid, a 39-character oid, a captures payload above the bound, a non-half-open interval, an empty value, an unparseable range, duplicate captures, and a raw row with an unowned column
Confidence: high - [evidence](evidence/p2-new-anchor-row-decodes-on-write.md). Read `validate_anchor` at `crates/kernel/src/anchor_write.rs:235` and `insert_anchor_inner` at `:289`; the typed `AnchorValue` (`:34`) makes an unowned column unrepresentable, and the round trip at `:114` compares the decoded condition to the request
Existing check: `crates/kernel/tests/kernel_anchor_write.rs:252`, `:389`, `:507`, `:313` (unaudited)
Impact: a capture decision anchored to an undecodable row evaluates `Uncertain` forever and revalidation fails silently
Open questions:
- The specification asked whether the writer accepts a typed enum or a raw `AnchorRowSpec`; #1133 resolved it as the typed `AnchorValue` enum, with `validate_anchor_row` exposed for raw-row checks only.

### p2-new-anchor-and-registry-share-one-envelope

Type: safety
Reachability: test-only - as above
Status: active
Exercised: yes - `a_decision_failure_after_the_anchor_rolls_back_both` and `a_duplicate_anchor_id_is_a_conflict_within_and_across_commits` construct the required fault and the duplicate cases
Guarantee: An `anchors` row and its `object_registry` row are visible together or not at all, and a decision referencing the anchor is committed in the same or a later `commit_seq`.
Check: `always` - for every live `anchors` row there is an `object_registry` row of kind `anchor` with the same `object_id` and `created_commit_seq`, and every `decisions.anchor_id` names a row with `created_commit_seq <= decision.created_commit_seq`
Fault/timing angle: a refusal inside the same envelope after the anchor insert
Required faults and enabling state: `insert_decision` fails after `insert_anchor` succeeded inside one closure; a duplicate `anchor_id` within one envelope and across two commits
Confidence: high - [evidence](evidence/p2-new-anchor-and-registry-share-one-envelope.md). Read `insert_anchor_inner` at `crates/kernel/src/anchor_write.rs:289` and `Envelope::guarded` at `crates/kernel/src/envelope.rs:348`
Existing check: `crates/kernel/tests/kernel_anchor_write.rs:364`, `:555`, `:334`; `crates/daemon/tests/claim_sources.rs:1508` (unaudited)
Impact: an orphan anchor is unreachable and inflates the registry; a decision committed without its anchor fails the foreign key and fails the job
Open questions: None.

### p2-new-capture-anchor-identity-is-deterministic

Type: safety
Reachability: default-production (intended) - every anchored capture publish after U3
Status: proposed - owner: U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135)
Exercised: not yet - no production code writes a capture anchor
Guarantee: The anchor id for a capture job is a pure function of `(job_id, head_oid)`, so a retry after a rolled-back commit produces identical bytes and converges on one row, and two jobs at one oid produce two rows.
Check: `always` - for every capture decision with `anchor_id = Some(a)`, `a == anchor_id_for(job_id, frozen.head.oid)`, and the count of live `anchors` rows with that id is exactly one
Fault/timing angle: a retry after `kernel_write_failed`
Required faults and enabling state: `kernel_write_failed` on the first attempt, then a successful retry
Confidence: medium - [evidence](evidence/p2-new-capture-anchor-identity-is-deterministic.md). The Kernel half, `Conflict` on a duplicate id, is checked by `a_duplicate_anchor_id_is_a_conflict_within_and_across_commits`
Existing check: none for the derivation; `crates/kernel/tests/kernel_anchor_write.rs:555` for the Kernel refusal (unaudited)
Impact: duplicate anchors per job, or `Conflict` on retry that permanently fails the job
Open questions:
- Whether anchors should be shared across jobs at one oid (needs human input; the model keeps per-job identity because `insert_anchor` refuses duplicates and no receipt covers a cross-job row).

### p2-new-head-observed-once-and-frozen

Type: safety
Reachability: default-production (intended) - every capture submit after U2
Status: proposed - owner: U2 ticket [#1134](https://github.com/ahrav/eidnara/issues/1134); U3 completes the anchor-equals-frozen-head clause
Exercised: not yet - the prepared plan has no `head` field
Guarantee: The checkout HEAD is read once per `memory.capture.submit`, before any plan of that submission is frozen, and the observation is part of the frozen `prepared_json`, so the receipt digest covers the anchor's value.
Check: `always` - for every capture decision with an anchor, `anchors.reachable_from_oid == parse(prepared_json).head.oid`, and every job frozen by one submission carries the identical `head`
Fault/timing angle: a `git checkout` between the submit and its retry
Required faults and enabling state: a checkout of another commit between submit and retry
Confidence: medium - [evidence](evidence/p2-new-head-observed-once-and-frozen.md). The receipt digest already covers every frozen byte (`crates/daemon/src/memory_capture.rs:1167`)
Existing check: none
Impact: the anchor stops describing the revision at which the quote was observed, and the receipt no longer covers the anchor's value
Open questions: None.

### p2-new-head-unavailable-keeps-decision-admitted

Type: safety
Reachability: default-production (intended) - capture in an unborn, unreadable, or non-repository root
Status: proposed - owner: U2 ticket [#1134](https://github.com/ahrav/eidnara/issues/1134) to `partial`; U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) to `active`
Exercised: not yet - no head observation exists
Guarantee: When HEAD is unborn or unreadable, the object format is unsupported, or the root is not a repository, the decision is still admitted with `anchor_id = None` and rationale `anchor: "head_unavailable"`, and no `anchors` row is written for the job.
Check: `always-or-unreached` - the path runs only when the head observation is unavailable; whenever it runs, the committed decision has `anchor_id IS NULL`, the rationale carries the reason, and no anchor row names the job
Fault/timing angle: none
Required faults and enabling state: a root with no `.git`; an unborn branch; an `objectFormat` gix refuses
Confidence: medium - [evidence](evidence/p2-new-head-unavailable-keeps-decision-admitted.md). The owner decided on 2026-10-25 (OQ2-11) that a detached HEAD anchors at its commit, which removes detached HEAD from this path
Existing check: none
Impact: capture would stop in fresh checkouts
Open questions: None.

### p2-new-citation-is-alias-only

Type: safety
Reachability: explicit-config-only (intended) - citations exist only when source capture publishes descriptors
Status: proposed - owner: U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135)
Exercised: not yet - no capture decision carries a citation
Guarantee: Every citation a capture decision carries equals the `detail.evidence_id` of a live `messages` descriptor whose identity matches the job's `(project_id, harness, session_id, message_id)`, read inside the same envelope; no byte of the model output reaches the field.
Check: `always` - for every cited capture decision, a descriptor with that evidence id and the job's identity tuple was live at the decision's `created_commit_seq`, and `PreparedCapture` deserialization refuses an `evidence_id` key
Fault/timing angle: none
Required faults and enabling state: model output containing an `evidence_id` key; a descriptor of another session with the same `message_id`
Confidence: medium - [evidence](evidence/p2-new-citation-is-alias-only.md). The validated `message_id` is the batch's own id (`crates/daemon/src/memory_capture.rs:204`)
Existing check: none
Impact: a model-emitted id would let the extraction forge its own evidence link
Open questions: None.

### p2-new-citation-block-target-rule

Type: safety
Reachability: explicit-config-only (intended) - as above
Status: proposed - owner: U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135), restated there for `block`, `span`, and `first`
Exercised: not yet
Guarantee: When a message has several text blocks, the cited descriptors are the located block or run of blocks whose text contains the quote, falling back to the lowest live `block_index` with the rule recorded in the rationale.
Check: `always` - for every cited capture decision, the cited blocks' text joined with `\n` contains the quote, or the rationale records `first` and no located block contains it
Fault/timing angle: none
Required faults and enabling state: a two-block message with the quote in the second block; a quote spanning the `\n` join (`crates/daemon/src/memory_capture.rs:311`)
Confidence: medium - [evidence](evidence/p2-new-citation-block-target-rule.md)
Existing check: none
Impact: revalidation reports a false mismatch for correctly captured memories
Open questions: None.

### p2-new-citation-folds-sensitivity-and-pins-evidence

Type: safety
Reachability: explicit-config-only (intended) - as above
Status: proposed - owner: U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135); under the event placement the owner chose for OQ2-10 the guarantee reduces to the evidence pin
Exercised: not yet
Guarantee: The evidence a capture decision cites survives `expire_harness_captures` while the decision is live, and retiring it while cited is refused.
Check: `always` - for every live cited capture decision, the cited evidence row is live after each sweep
Fault/timing angle: a sweep running after the evidence's `retain_until`
Required faults and enabling state: evidence past `retain_until` with one live citing decision
Confidence: medium - [evidence](evidence/p2-new-citation-folds-sensitivity-and-pins-evidence.md). The pin rests on `EVIDENCE_CITED_SQL`, which counts decision events (`crates/kernel/src/slice/write.rs:42`)
Existing check: none for capture citations
Impact: retired evidence breaks every citation
Open questions: None.

### p2-new-settlement-wait-is-bounded-and-gate-free

Type: liveness
Reachability: explicit-config-only (intended) - the wait runs only with source capture enabled
Status: proposed - owner: U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136)
Exercised: not yet - no settlement record exists
Guarantee: With source capture enabled, the submit path proceeds to publish within `accepted_at + 5 s` plus one blocking-pool scheduling slack, whatever the source job does, and never holds the capture commit gate while waiting.
Check: `always` - the wait returns within the bound measured from the record's `accepted_at`, the bound being `CAPTURE_BUDGET` (`crates/daemon/src/source_capture.rs:35`); the commit gate is acquired only after the wait returns
Fault/timing angle: a source job that stalls past its budget
Required faults and enabling state: a parked source job under a paused tokio clock
Confidence: medium - [evidence](evidence/p2-new-settlement-wait-is-bounded-and-gate-free.md)
Existing check: none
Impact: the native lease is consumed by waits, or the daemon deadlocks on the capture gate
Open questions: None.

### p2-new-lookup-runs-only-after-settlement

Type: safety
Reachability: explicit-config-only (intended) - as above
Status: proposed - owner: U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136)
Exercised: not yet
Guarantee: The citation lookup of a submission runs only after the session's settlement record is ended, refused, absent, or past its deadline.
Check: `always` - at the instant the publish envelope opens, the record for `(project_id, harness, session_id)` is not running within its deadline
Fault/timing angle: a slow source job and a fast harness submit
Required faults and enabling state: a parked source job and a submit inside its window
Confidence: medium - [evidence](evidence/p2-new-lookup-runs-only-after-settlement.md)
Existing check: none
Impact: avoidable `descriptor_not_found` misses that hide a race as a missing source
Open questions: None.

### p2-new-miss-reason-matches-outcome

Type: safety
Reachability: default-production (intended) - every capture decision after U3
Status: proposed - owner: U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135); U4 ([#1136](https://github.com/ahrav/eidnara/issues/1136)) adds the `publish_budget_expired` state
Exercised: not yet
Guarantee: Every capture decision carries exactly one citation reason and one anchor reason from the closed vocabularies, chosen by the specification's precedence, and a cited or anchored decision carries the matching `cited` or `anchored` value.
Check: `always` - `(no citation) == (citation != "cited")` and `(anchor_id IS NULL) == (anchor == "head_unavailable")` for every capture decision, each value in the fixed set
Fault/timing angle: none
Required faults and enabling state: each miss condition constructed separately
Confidence: medium - [evidence](evidence/p2-new-miss-reason-matches-outcome.md)
Existing check: none
Impact: revalidation cannot tell "never captured" from "captured late" from "lookup failed"
Open questions: None.

### p2-new-miss-counters-track-rationale

Type: safety
Reachability: default-production (intended) - as above
Status: proposed - owner: U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135)
Exercised: not yet
Guarantee: Within one daemon process, each per-reason counter equals the number of committed capture decisions carrying that reason since process start, at most the number of attempted publishes carrying it.
Check: `always` - after each publish settles, `counter[reason] >= committed(reason)` and `counter[reason] <= attempted(reason)`, with the increment after the receipt returns and only for a receipt that was not replayed
Fault/timing angle: a publish that rolls back after the reason was computed
Required faults and enabling state: a rolled-back publish; a replayed receipt
Confidence: medium - [evidence](evidence/p2-new-miss-counters-track-rationale.md)
Existing check: none
Impact: operators tune the budget on numbers that count failures as misses
Open questions: None.

### p2-new-anchor-and-citation-add-no-authority

Type: safety
Reachability: default-production (intended) - as above
Status: proposed - owner: U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135)
Exercised: not yet
Guarantee: A capture decision's admission record carries `ModelInference` and `AssistantInference` and the same maturity whether or not it has an anchor or a citation.
Check: `always` - sibling capture decisions that differ only in anchor or citation presence have equal `source_class`, `taint_class`, `maturity`, `effective_maturity`, and `elevated_support`
Fault/timing angle: none
Required faults and enabling state: one cited and one uncited job in one session
Confidence: medium - [evidence](evidence/p2-new-anchor-and-citation-add-no-authority.md). Admission classes are fixed at `crates/daemon/src/memory_capture.rs:1201`
Existing check: none
Impact: a model could raise its own authority by quoting a captured message
Open questions: None.

### p2-new-cited-decision-pair-observed

Type: reachability
Reachability: default-production (intended) - campaign coverage
Status: proposed - owner: U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) to `partial`; U4 ticket [#1136](https://github.com/ahrav/eidnara/issues/1136) to `active`
Exercised: not yet
Guarantee: A test campaign produces at least one capture decision with both an anchor and a citation, and at least one decision for each miss reason, so every legal state is constructed.
Check: `sometimes` - states, not code points: a committed decision with an anchor and a citation, and one per miss reason
Fault/timing angle: none
Required faults and enabling state: a controllable source job and a repository fixture with one commit
Confidence: medium - [evidence](evidence/p2-new-cited-decision-pair-observed.md)
Existing check: none
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

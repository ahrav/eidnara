# Curator Operations

Status: current system document for the Curator lifecycle boundary
Scope: activation, status, review reads, backup and restore, exposure bounds, evaluation evidence, and property traceability for the proposal-only milestone

This document describes how a deployment owner and an operator run the Curator as it exists: a proposal-only reviewer that stages proposals and never accepts one. Acceptance has no code path in this milestone. `docs/host-wire-protocol.md` is the normative wire contract; where this document names a wire field, that document governs.

## 1. What runs and what never runs

The Curator worker is one task per data home. Each pass it reads the deployment owner's activation record, and while that record matches the live deployment it claims Ready review jobs of every project whose memories authority is MODULE, runs each through the shared coordinator, and settles it: a staged proposal selected by a completed receipt, or a content-free abstention, or an unknown outcome. Every disclosure to the remote model goes through the Kernel's egress policy and the Memory Store's committed attempt marker first; nothing is sent without both.

Two producers feed the queue. The History Summarizer reserves a job for each accepted fact set before it publishes the chunk, and activates the job as part of that publication; the Memory Classifier enqueues bounded selection pages of canonical memories. Both paths are exercised by real-store tests named in section 8.

Nothing in this milestone writes a canonical memory, promotes a proposal, or invents provenance. A default-`Sensitive` subject, which every History Summarizer subject is, never reaches the remote model: the run abstains by policy before any request and records that abstention.

## 2. Activation (Q36, Q38)

The gate is closed until the deployment owner writes a runtime identity record at

```
<data home>/curator-activation/runtime-identity.json
```

The directory must be mode `0700` and the file mode `0600`, both owned by the daemon's user. A record another user could read is refused, not read. The record is JSON with exactly these fields; an unknown field makes the record malformed.

| Field | Meaning |
| --- | --- |
| `schema` | `1`. |
| `model` | The canonical model id every request names. The provider's reported model is checked against it at response time and a mismatch ends the attempt without releasing text. |
| `prompt_template_version` | The revision of the `extracted_facts` question template compiled into the daemon. |
| `step_schema_version` | The Curator step schema version compiled into the daemon. |
| `scanner_ruleset_version` | The redaction detector revision compiled into the daemon. |
| `egress_policy_version` | The Curator policy union version compiled into the daemon. |
| `kernel_baseline_digest` | SHA-256 of the Kernel schema baseline compiled into the daemon. |
| `memstore_baseline_digest` | SHA-256 of the Memory Store schema baseline compiled into the daemon. |
| `kernel_incarnation` | The Kernel database incarnation id of this deployment. |
| `memstore_incarnation` | The Memory Store incarnation id of this deployment. |
| `provider` | `<host>/v1/messages@<anthropic-version>`, as the sender identifies the endpoint it dials. |
| `credential` | The startup-envelope credential name the sender dials with, for example `ANTHROPIC_API_KEY`. |
| `credential_fingerprint` | Lower-hex SHA-256 over `eidnara-curator-credential-v1`, a NUL byte, the credential name, a NUL byte, and the secret. |
| `provider_retention` | `{attested_by, attested_on, retention_terms, finite_work_exposure_acknowledged}`: the owner's statement about the provider account's data retention and an explicit `true` acknowledging that every run spends bounded provider work. |

Every term is compared against the live deployment on every pass and again before every job. Any mismatch closes the gate naming the field, and the worker runs nothing and sends nothing. The record therefore has to be rewritten when the binary changes (any compiled version or baseline digest), when either store is replaced (its incarnation changes), when the provider or endpoint changes, or when the secret is rotated (its fingerprint changes). That is the Q38 rule made mechanical: owner approval of provider retention and finite work exposure precedes the first remote call for exactly this deployment, and no approval carries over to a deployment it did not name.

The compiled versions and digests are what the worker compares against; the simplest way to fill them in is to read the closed reason from the status block after writing a draft record (`identity_mismatch` names the field) and correct one field at a time. Nothing reads the record except the worker; there is no operator command that writes it, and the daemon never writes one.

The credential is read from the startup envelope's credentials, never from the process environment. The host always hands the daemon its Model Execution supervisor and the envelope's credentials; a record naming a credential the envelope does not carry closes the gate as `unknown_credential`.

## 3. Status

`host.status` carries `metrics.curator` (wire document 7.6). Its `curator_state` and counters are the sampler's; its `activation_state` is the worker's: `open`; `unknown` before the first pass; `stale` once the last evaluation is older than 300 s, which means the worker stopped evaluating and nothing vouches for `open`; or the closed reason's kind (`missing`, `refused`, `unreadable`, `malformed`, `identity_mismatch`, `unacknowledged`, `unknown_credential`, `unavailable`, `store`). Every counter is a count or a byte total; nothing on this surface names a project, a payload, or a person.

The scheduler enqueues Memory Classifier review selections only while the gate is `open`, so a closed gate does not fill the queue with jobs nothing may run. Jobs already queued expire at their queue deadline and the sweep records them as expired; they are never silently dropped.

## 4. Review reads

`review.list` and `review.read` (wire document 7.8, protocol 3) are the only ways to see what the Curator concluded. Both are local reads of the bound project's completed outcomes and the proposals their receipts select. A proposal is readable only while its receipt is complete, its selection matches, and its review hold is live; the review hold's expiry is the only review deadline, and selection never refreshes it. An abstention lists with its reason and has nothing to read. Neither operation depends on the activation gate: an outcome already recorded stays readable while new disclosure is closed.

## 5. Backup, restore, and mismatch refusal

The Kernel and the Memory Store are two SQLite families that reference each other only by recorded incarnation ids. A Curator receipt binds the Kernel incarnation and the Memory Store incarnation it was written under; a Kernel hold binds both; the activation record names both. That is the whole restore contract: the two stores are backed up as one pair and restored as one pair, and every mismatch is refused where it is met.

Backup:

1. Stop the daemon. The worker is joined before the stores are released, so a stopped daemon has no run in flight; a receipt still `in_progress` at that point is taken over on the next pass after restart.
2. Kernel: `KernelStore::backup` publishes a verified copy into a private directory and returns its path and the captured commit sequence. This is the only supported Kernel backup; a file copy of a live Kernel family is not.
3. Memory Store: copy the closed `memory.sqlite` family (the database and any write-ahead log beside it). The store publishes no backup of its own.
4. Record the pair together with the binary's version and both baseline digests. A backup pair belongs to the binary that wrote it.

Restore of the pair:

1. Stop the daemon.
2. Kernel: open the target directory and call `KernelStore::restore` with the backup path. The restored database keeps its incarnation id, its staged rows, its holds, and its purge tombstones; a backup missing a purge this store committed is refused before anything is displaced.
3. Memory Store: replace the file family with the copied one. It keeps its incarnation id and every receipt.
4. Start the daemon. A proposal selected before the backup reads again through `review.read` while its review hold is live, and the owner's activation record still matches because neither incarnation changed.

Mismatch refusal, as the rehearsal test `crates/daemon/tests/curator_backup_restore.rs` proves over real files:

| Restored | Beside | What happens |
| --- | --- | --- |
| Memory Store from backup | a replaced (fresh) Kernel | `review.read` answers `incarnation_mismatch`; resuming the receipt is refused as a binding mismatch; the activation gate closes on `kernel incarnation`. The receipt row stays exactly as backed up. |
| Kernel from backup | a replaced (fresh) Memory Store | No receipt exists, so `review.read` answers `not_selected`; the gate closes on `memory store incarnation`. The Kernel's staged proposal is not served on anyone's say-so. |
| either store | a different binary | The gate closes on the baseline digest or compiled version that differs. Receipts and staged rows are untouched. |

Nothing reconciles a mismatched pair. No migration rewrites incarnation ids, no reset clears receipts, and no code path adopts a staged proposal whose receipt is missing.

Rollback rule: rolling back to an earlier release means the retained old binary together with the backup pair that binary wrote. If no matching pair exists, the only alternative is an owner-authorized fresh baseline: both stores start empty, the owner writes a new activation record for the new incarnations, and the pending proposals of the abandoned pair are documented as nonportable, because they are bound to incarnations that no longer exist and cannot be carried into the new pair. There is no supported path that keeps one store and replaces the other.

## 6. Exposure bounds (Q38)

Provider exposure is bounded by receipts and quotas, not by a spending cap; the daemon holds no billing ledger and none is added.

- One job runs at most `CURATOR_MAX_ATTEMPTS` (4) attempts, each at most `CURATOR_MAX_REQUEST_BYTES` (256 KiB) of request body, within `CURATOR_RUN_DEADLINE_MS` (120 s) less a 10 s settlement reserve. Every attempt is a committed marker in the Memory Store before a byte is sent; a marker that cannot commit sends nothing.
- Every job charges `CURATOR_RECEIPT_CHARGE_BYTES` (4 KiB) of permanent receipt metadata, and permanent charges plus temporary allowances are bounded per project (64 MiB) and per host (256 MiB). A project or host at its quota admits no new job; the sampler reports `receipt_charge_bytes`, `allowance_bytes`, `metadata_quota_bytes`, and `metadata_headroom_bytes` so headroom is read, not inferred.
- Every job carries at most `MAX_CURATOR_HOLD_REFERENCES` (512) held evidence references, and a project holds at most `MAX_ACTIVE_CURATOR_HOLDS_PER_PROJECT` (64) active holds; a run that ends without settling releases its hold rather than leaving it to its cutoff.
- The requested model profile (`model`, `max_tokens`) is what the attempt marker records; the provider's reported model id is checked against the requested one before any text is released, and a reported model that differs ends the attempt as a failed terminal with nothing released.

Finite cleanup is not evidence of sustainable service. The sampler's per-pass counts say what was swept; the growth of `receipt_charge_bytes` against `metadata_quota_bytes` over time is the measure an operator watches, and section 7 reports what has and has not been measured.

## 7. Evaluation evidence for milestone one

The parent contract asks for supported-proposal precision, useful yield, unnecessary abstention, missed contradictions, and physical requests per useful proposal, each with counts and explicit denominators, `N/A` at a zero denominator, failures and nonadmissions retained, citation validity scored separately from support for the claim, scripted proof kept distinct from actual-model observation, and no statistical quality claim. This section reports against that contract as of this document.

### 7.1 Authorization status

No deployment-owner authorization for live model calls exists, and no Q38 disclosure approval has been written for any deployment. No activation record exists in any environment. Therefore no actual-model run has been made, and every actual-model metric below is reported as missing, not as a pass. Writing an activation record is the owner's act; this document does not authorize one and no fixture stands in for one.

### 7.2 Scripted proof (not a quality claim)

The scripted corpus (`crates/daemon/tests/support/curator_corpus.rs`) is eight hand-labeled cases whose model turns are fixed text. It proves the coordinator's delivery, citation binding, and outcome ledger and says nothing about model quality.

| Measure | Count | Denominator | Value |
| --- | --- | --- | --- |
| Cases | 8 | | |
| Published proposals | 7 | 8 cases | scripted |
| Abstentions | 1 (`model_declined`, injected instructions) | 8 cases | scripted |
| Relations covered | supports, contradicts, supersedes, shared origin, decisive evidence outside initial context, protected source, incomplete search, injected instructions | 8 relations | each once |
| Citation validity | every cited alias in a published case names disclosed bytes; a citation outside disclosed bytes is refused before binding | 7 published cases | 7 of 7 valid, by construction |
| Contradictions cited | the contradiction case cites its contradicting source | 1 case with a contradicting source | 1 of 1, by construction |

These figures are the corpus's own labels replayed; they are not precision, yield, or recall of a model.

### 7.3 Actual-model observations

| Measure | Numerator | Denominator | Value |
| --- | --- | --- | --- |
| Supported-proposal precision | supported published proposals | published proposals | N/A (0 published) |
| Useful yield | useful published proposals | jobs run | N/A (0 run) |
| Unnecessary abstention | abstentions a reviewer judged unnecessary | abstentions | N/A (0) |
| Missed contradictions | contradicting sources in evidence not cited | proposals with a contradicting source in evidence | N/A (0) |
| Physical requests per useful proposal | committed attempt markers | useful published proposals | N/A (0) |
| Citation validity | citations naming disclosed bytes | citations | N/A (0) |
| Failures and nonadmissions retained | | | none observed; the ledger keeps every `failed`, `unknown`, `expired`, and nonadmitted outcome when they occur |

The comparison arm the contract names, one-shot proposals under the same model, profile, sampling, question, and initial eligible evidence, has likewise not run. It is an evaluation arm, not a production mode, and it needs the same owner authorization.

### 7.4 Operational measurements

- Permanent receipt growth and quota headroom: measured only in tests. The worker test observes one job moving from `jobs_ready` to `jobs_abstained` and `receipts_complete` from 0 to 1; the lifecycle test checks the byte identity between `receipt_charge_bytes`, `allowance_bytes`, and `metadata_headroom_bytes` against the quota. No long-running measurement exists.
- Temporary charge reclamation: the lifecycle test observes an expired job's allowance released and an expired staged input abandoned by one maintenance pass; the same test observes a second pass reclaiming nothing further.
- Queue delay and expiry: recorded per job as `queue_deadline_ms` and as the `expired` and `expired_unseen` counts; no production series exists.
- Compaction impact: not measured.
- Guard contention: the disclosure tests prove the network wait begins only after both store owners release and that a held owner stalls the attempt rather than the store; no production contention series exists.
- Producer nonadmission facts: recorded per session as `curator_nonadmission` with a closed code and surfaced as `nonadmissions` and `latest_nonadmission_*` counts; the handoff tests observe a capacity refusal recorded as the nonadmission.

None of these measurements supports a claim of sustainable service, and this document makes none.

## 8. Property traceability

The parent property portfolio was not published before dependent implementation, and no record ids exist to cite. This section therefore traces each milestone claim to the test that proves it and does not invent ids. When the portfolio is published, its ids attach to these rows; nothing here is recovered after the fact.

| Claim | Proof |
| --- | --- |
| Staged review inputs are immutable, reference-bound, and private until selected | `crates/kernel/tests/kernel_review_staging.rs`; `curator_settlement::kernel_results_stay_private_until_the_receipt_selects_them` |
| Evidence retention is bounded by execution and review holds; an empty hold is admitted and grows by extension | `crates/kernel/tests/kernel_curator_holds.rs` |
| Jobs are reserved once per causal identity and activated with progress | `history_summarizer_handoff_tests::reservation_precedes_staging_and_publication_activates_with_progress`, `identical_inputs_neither_duplicate_a_job_nor_reopen_a_settled_one` |
| Attempts are fenced and receipts co-commit execution | `curator_disclosure::nothing_is_sent_without_approval_under_cancellation_or_when_the_marker_cannot_commit`, `a_post_commit_lapse_stays_charged_and_sends_nothing` |
| Every read is policy-validated through one broker | `curator_broker::scope_staleness_and_expiry_refuse_before_any_disclosure`, `render_check_refuses_secrets_and_placeholders_without_redacting` |
| Project text is confined, finite, and owned | `crates/daemon/tests/curator_project_text.rs` |
| One verified request per attempt; a mismatched model releases no text | `curator_disclosure::provider_failure_and_a_mismatched_model_end_the_attempt_without_a_second_send` |
| Internal runs carry no public session authority | `curator_coordinator::capacity_is_acquired_before_anything_is_spawned_and_never_queued` |
| Proposals publish and read only through completed receipt selection | `curator_settlement::a_completed_receipt_selects_the_staged_proposal_and_reads_pass_the_kernel`, `a_selected_result_is_readable_only_through_its_live_review_hold` |
| Takeover fences the losing generation | `curator_settlement::a_takeover_fences_the_losing_generation_and_selects_only_its_own_result` |
| Restart republishes a reserved firing without a model run | `history_summarizer_handoff_tests::restart_republishes_a_reserved_firing_without_a_model_run`, `a_production_reservation_republishes_after_a_restart_and_a_stale_one_is_not_carried` |
| Default-Sensitive subjects abstain without a request, a canonical write, or invented provenance | `curator_coordinator::a_staged_subject_is_policy_blocked_for_a_remote_model_and_abstains`; `curator_worker::a_closed_gate_runs_nothing_and_an_open_gate_runs_the_job_to_policy_blocked_abstention` |
| A genuinely eligible memory becomes a published proposal | `curator_coordinator::a_selected_eligible_memory_becomes_a_published_proposal_through_the_shared_path` |
| The activation gate closes on every identity mismatch and opens only for an attested owner-only record | `curator::activation` unit test; `curator_worker` |
| Status counters are bounded and content-free | `curator::lifecycle` tests; `host-runtime` `curator_block_tests` |
| Both-store backup and restore, and mismatch refusal | `crates/daemon/tests/curator_backup_restore.rs` |
| Shutdown joins owned work before store release | `curator_coordinator::an_unknown_attempt_outcome_completes_unknown_and_cancellation_joins_the_attempt`; `curator_worker::a_cancelled_worker_loop_returns_before_the_stores_are_released`; the daemon's `shutdown_cancels_and_joins_tracked_*` tests |
| Cleanup is finite and kind-specific | `curator::lifecycle` maintenance test |
| Review outcomes list and read only through the receipt-selected path | `crates/daemon/tests/curator_wire.rs` |

Acceptance activation remains blocked: there is no acceptance code, no acceptance protocol version, and no owner-reviewed actual-evidence corpus gate, and this milestone adds none.

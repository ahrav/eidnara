# MemoryReviewer Operations

Status: current system document for the MemoryReviewer lifecycle boundary
Scope: activation, status, review reads, backup and restore, exposure bounds, evaluation evidence, and property traceability for the proposal-only milestone

This document describes how a deployment owner and an operator run the MemoryReviewer as it exists: a proposal-only reviewer that stages proposals and never accepts one. Acceptance has no code path in this milestone. `docs/host-wire-protocol.md` is the normative wire contract; where this document names a wire field, that document governs.

## 1. What runs and what never runs

The MemoryReviewer worker is one task per data home. Each pass it reads the deployment owner's activation record, and while that record matches the live deployment it claims Ready review jobs of every project whose memories authority is MODULE, runs each through the shared coordinator, and settles it: a staged proposal selected by a completed receipt, or a content-free abstention, or an unknown outcome. Every disclosure to the remote model goes through the Kernel's egress policy and the Memory Store's committed attempt marker first; nothing is sent without both.

Two producers feed the queue. The History Summarizer reserves a job for each accepted fact set before it publishes the chunk, and activates the job as part of that publication; the Memory Classifier enqueues bounded selection pages of canonical memories. Both paths are exercised by real-store tests named in section 8.

Nothing in this milestone writes a canonical memory, promotes a proposal, or invents provenance. A default-`Sensitive` subject, which every History Summarizer subject is, never reaches the remote model: the run abstains by policy before any request and records that abstention.

## 2. Activation (Q36, Q38)

The gate is closed until the deployment owner writes a runtime identity record at

```
<data home>/memory_reviewer-activation/runtime-identity.json
```

The directory must be mode `0700` and the file mode `0600`, both owned by the daemon's user. A record another user could read is refused, not read. The record is JSON with exactly these fields; an unknown field makes the record malformed.

| Field | Meaning |
| --- | --- |
| `schema` | `1`. |
| `model` | The canonical model id every request names. The provider's reported model is checked against it at response time and a mismatch ends the attempt without releasing text. |
| `prompt_template_version` | The revision of the `extracted_facts` question template compiled into the daemon. |
| `step_schema_version` | The MemoryReviewer step schema version compiled into the daemon. |
| `scanner_ruleset_version` | The redaction detector revision compiled into the daemon. |
| `egress_policy_version` | The MemoryReviewer policy union version compiled into the daemon. |
| `kernel_baseline_digest` | SHA-256 of the Kernel schema baseline compiled into the daemon. |
| `memstore_baseline_digest` | SHA-256 of the Memory Store schema baseline compiled into the daemon. |
| `kernel_incarnation` | The Kernel database incarnation id of this deployment. |
| `memstore_incarnation` | The Memory Store incarnation id of this deployment. |
| `provider` | `<host>/v1/messages@<anthropic-version>`, as the sender identifies the endpoint it dials. |
| `credential` | `ANTHROPIC_API_KEY`: the one startup-envelope credential the sender's protocol writes into its request. Any other name closes the gate as `unknown_credential`, even when the envelope carries it. |
| `credential_fingerprint` | The identity the daemon's committed harness selection, `<data home>/eidnara/harness-closures/active-selection.json`, records under `credential_identities` for that credential: an HMAC keyed under this start's connection key, never the secret itself. The key is fresh at every daemon start, so the value changes at every start. |
| `provider_retention` | `{attested_by, attested_on, retention_terms, finite_work_exposure_acknowledged}`: the owner's statement about the provider account's data retention and an explicit `true` acknowledging that every run spends bounded provider work. |

Every term is compared against the live deployment on every pass and again before every job. Any mismatch closes the gate, and the worker runs nothing and sends nothing. The record therefore has to be rewritten when a binary changes any compiled version or baseline digest the record names (the gate binds those terms, not a build identity, so a binary that changes none of them is admitted by the existing record), when either store is replaced (its incarnation changes), when the provider or endpoint changes, when the secret is rotated, and after every daemon start, because the credential identity is derived under that start's connection key. That is the Q38 rule made mechanical: owner approval of provider retention and finite work exposure precedes the first remote call for exactly the deployment the record's terms describe, and no approval carries over to a deployment they do not name.

The status block reports only the closed kind (`identity_mismatch`), never which term differs. The term and the deployment's live value are written to the daemon's log, `<data home>/.eidnara-coordination/eidnara.log`, once per distinct reason: `daemon: memory_reviewer activation gate closed: activation record names another kernel incarnation; the live value is <id>`. Every identity term is reported this way, so a draft record can be corrected one term at a time from the log. The credential is not: a name the sender does not dial with closes the gate as `unknown_credential`, and a mismatched identity logs only `activation record names another credential`, with no live value. The owner copies the identity from the selection file named above. The compiled versions and digests are not published on any other operator surface. Nothing reads the record except the worker; there is no operator command that writes it, and the daemon never writes one.

The credential is read from the startup envelope's credentials, never from the process environment. The host always hands the daemon its Model Execution supervisor and the envelope's credentials; a record naming a credential the sender does not dial with closes the gate as `unknown_credential`. Until the daemon has committed its harness selection for this start, no credential identity exists and the gate reports `unreadable`.

### Response budgets are the job's

A job may consume at most 1 MiB of raw provider response bytes and 64 KiB of decoded assistant text across every round, physical attempt, recovery, and generation; the per-response constants in the sender and decoder are the same numbers, so no single response can spend more than the job has left. Every attempt row records what its response consumed, written in the same statement as its terminal: the raw body the transport delivered and the text the decoder measured, on a completed response, a provider error body, a malformed body, and a cancelled or timed-out read alike. A `not_dispatched` attempt records zero. An attempt that is unterminated, `unknown`, or terminal without recorded bytes counts as the whole ceiling, because nothing durable says it consumed less; a failed terminal write therefore spends the job's remainder rather than granting fresh headroom.

The remaining allowance is computed inside the marker transaction, before the marker is inserted, over every generation of the job. A job with nothing left in either ceiling is refused there as attempt exhaustion, so no marker is charged and no request byte is sent; the run settles `budget_exhausted`. The two remaining values travel with the handed dispatch: the collector charges each chunk against the raw remainder before appending it and refuses the response where a chunk crosses it, recording the whole remainder as consumed; a declared content length past the remainder refuses before the body is read; the decoder charges each decoded text length against the text remainder before allocating it and refuses the same way. A compressed body is still refused before decompression, and nothing reads extra bytes to measure them. No store owner is held across a network await, and no durable write happens per chunk.

Adding the two usage columns changed the Memory Store baseline digest. A database created under the previous baseline refuses to open under this binary, and the previous activation record fails closed on `memstore_baseline_digest` until the deployment owner writes a renewed record; there is no migration, silent reset, or change to the Kernel baseline.

## 3. Status

`host.status` carries `metrics.memory_reviewer` (wire document 7.6). Its `memory_reviewer_state` and counters are the sampler's; its `activation_state` is the worker's: `open`; `unknown` before the first pass; `stale` once the last evaluation is older than 300 s, which means the worker stopped evaluating and nothing vouches for `open`; or the closed reason's kind (`missing`, `refused`, `unreadable`, `malformed`, `identity_mismatch`, `unacknowledged`, `unknown_credential`, `unavailable`, `store`). Every counter is a count or a byte total; nothing on this surface names a project, a payload, or a person.

The scheduler enqueues Memory Classifier review selections only while the gate is `open`, so a closed gate does not fill the queue with jobs nothing may run. Jobs already queued expire at their queue deadline and the sweep records them as expired; they are never silently dropped.

### Production selection gate

Selection over the production classes, `canonical_claims` and `promoted_memory`, is closed by the constant `PRODUCTION_SELECTION_OPEN` in `crates/daemon/src/memory_reviewer/selection.rs`. The gate is separate from activation: an open activation gate schedules no production selection while this constant is `false`.

The coordinator does resolve both classes. A canonical or promoted descriptor names its originating decision in its leading identity field (`object_id` or `decision_object_id`), and the coordinator reads it through `ReferenceExpectation::CanonicalSource`, binding the descriptor's revision together with the decision's live revision. The broker judges both on every read, so a retracted, superseded, hidden, or re-revised decision revokes every form derived from it. Multiple descriptors of one decision share that decision as their owner and do not count as independent corroboration.

A proposal over such a descriptor targets the originating decision, not the descriptor: `ProposalTarget::Memory` carries the decision's object id, source revision, the snapshot read, and the decision's last change as `commit_token`. The descriptor stays a causal input under `ReviewTarget::Memory`, so the job's causal identity is unchanged. A decision whose revision moved after the run bound it refuses the proposal instead of retargeting.

The gate stays closed because no production representation of a decision is eligible for the Remote destination. Kernel ingest stores any artifact without affirmative repository provenance as `Sensitive`, only Git commit publication issues that provenance, and Remote egress refuses `Sensitive`. A production-class job that did run would abstain `owner_sensitive` before any request, which `memory_reviewer_selection::the_production_classes_are_walked_in_order_and_unproven_canonical_descriptors_are_not_selected` and the broker's Remote refusal tests witness. Opening the gate requires two things the owner has not supplied: an approved decision representation that is Remote-eligible under existing policy without caller-supplied provenance, and a positive production-class witness that reaches a selected, readable proposal through the real selector, coordinator, and stores. Resolver wiring alone is not eligibility, and the test-only Git commit class is not a substitute.

## 4. Review reads

`review.list` and `review.read` (wire document 7.8, protocol 3) are the only ways to see what the MemoryReviewer concluded. Both are local reads of the bound project's completed outcomes and the proposals their receipts select. A proposal is readable only while its receipt is complete, its selection matches, and its review hold is live; the review hold's expiry is the only review deadline, and selection never refreshes it. An abstention lists with its reason and has nothing to read. Neither operation depends on the activation gate: an outcome already recorded stays readable while new disclosure is closed.

### Private results and the queue deadline

A run's Kernel result and its execution-to-review hold transfer commit before the Memory Store completes the receipt, and only the completed receipt selects the result for publication. The staged proposal row keeps the job's 24-hour queue deadline through the transfer; the transfer proves retention, not selection, and never moves a deadline. Until a receipt selects it, the row is private: `review.read` answers `not_selected`, and a Kernel read of the row by identity expires at the queue deadline even though its review hold runs for up to seven days.

A result the Memory Store selects before the queue deadline is read against its selection time, which completion fences against that same deadline. Such a result stays readable after the queue deadline exactly as long as its review hold is live and every disclosed input still passes local policy; the hold's expiry is what ends it. A receipt the sweep closed as `expired`, `unknown`, or `cancelled`, a rejected or losing generation, or a late completion fenced by a terminal cannot publish the row or refresh either deadline.

### Recovery without the run

A run's broker, alias table, and transcript live in the worker process. A worker that resumes a claim after a restart, or a successor that takes a receipt over, has none of them. Every proposal row therefore carries a dependency record beside its payload: the complete policy union the run disclosed, with each member's kind, id, revision, owner, and owner revision, and the attempt marker whose response the proposal is. The record lives in the row's witness, not in the payload, so the payload bytes, their digest, and the `review.read` wire shape are unchanged.

Before a resumed run resolves its subject or takes any evidence work, it reads the job's attempt markers. A generation with no marker, or with only `not_dispatched` markers, proceeds as a fresh run under the original identity, deadlines, and remaining attempts, unless an earlier generation left an unterminated or `unknown` marker: that marker already makes the job's outcome unknown, so a successor that took the receipt over sends nothing and completes it `unknown` rather than paying for an answer its settlement would discard. Any other marker at the run's own generation means a request was dispatched whose answer this process never saw, and the run sends nothing:

- A sealed proposal row at the generation's provisional identity is revalidated from its record: the union bytes must re-encode to their digest; the marker must be a completed attempt at that generation with the recorded body and union digests; every member is rebuilt from the live store and judged as a live run would judge it, including uncited members and each canonical member's originating decision and its revision; the members must produce exactly the payload's disclosed inputs and ancestry. A row that passes is published exactly as a live settlement would, under the review hold the lost run transferred or, when it had not, under this run's execution hold, extended over the record's inputs first because a hold recovered or replaced after an unsettled exit may cover nothing. Zero model requests.
- A row that fails revalidation, or a row staged without a record, ends the receipt `abstained` with the reason revalidation names. Nothing infers lineage from the payload, and no migration rewrites old rows.
- When no sealed row exists, the receipt completes `unknown`. A committed marker with an unknown outcome is not authority to send again.

A takeover advances the generation, so a successor adopts only a result of its own generation; the losing generation's row stays sealed and private. The selected read runs the same revalidation for a local reader on every `review.read`, so a proposal whose cited or uncited lineage moved after selection answers `dependency_refused` while the receipt still selects it.

The record widens the witness a proposal row is stored under. A binary from before this release decodes a witness only in its own shape, so it refuses every proposal row this release stages (`scope_mismatch` on read, and a settlement retry on that binary cannot recover its own sealed row). No schema baseline changes, so the store opens, but the rollback rule in section 5 still applies: an earlier binary runs only against the backup pair it wrote, never against a live family this release has staged into.

The sweep closes an in-progress receipt at the earlier of its run deadline and its job's queue deadline. Each lifecycle pass then reconciles the review holds of the live Memory Store incarnation: a hold whose result no completed receipt selects and no in-progress receipt can still select is released, so the hold and its quota charge end at the next pass rather than at the review expiry. The transfer had already moved each MemoryReviewer capture's `retain_until` to that expiry, and the release does not move it back: the capture's evidence and artifact bytes stay until the capture sweep retires them after `retain_until`, whether or not a hold still pins them. Selected results keep their holds; holds of another store incarnation, such as a restored Kernel's beside a replaced store, are left alone; nothing is rescued, refreshed, or accepted by this pass.

### Observational routes

A route bound with harness `cli` is observational. It authorizes and resolves its project exactly as any route does, so `review.list` and `review.read` work through it under the existing project authorization, and its session is torn down on close like any other. `host.status` is unaffected: it is a bearer-authenticated channel-0 operation (wire document 7.6) that reads no route binding. It takes no part in background work: the MemoryReviewer worker's project list, the scheduler's root, harness, and schedule selection, and the search maintenance roster read one participating view from which observational bindings are removed before the newest binding on each root is chosen. An observer opened beside a live scheduled harness therefore never replaces that harness's binding, a root bound only by observers contributes nothing, and closing an observer reveals no binding the views were not already reporting. A MODULE-authority project whose only live routes are observational stays dormant: a Ready job waits for a participating route, not for the gate.

`harness` is a claim the client makes on `route.open` and remains scoping metadata; the `cli` value selects observation and grants nothing.

### Exact integers at the client seam

The TypeScript host client decodes a response body exactly when the caller asks for it: `hostStatus` always does, and a routed `request` does under `{ exactIntegers: true }`. Those bodies go through a source-aware reviver (`parseExactJson` in `@eidnara/opencode`'s host client). A safe integer, within plus or minus 2^53 minus 1, stays a plain `number`; any other integer lexeme up to twenty digits becomes a `bigint` carrying its exact value; a longer lexeme, which no 64-bit field can produce, refuses the whole body as invalid JSON. Fractions, exponents, and every other value decode as `JSON.parse` does. If the runtime's `JSON.parse` withholds the lexeme from the reviver, an integer-valued double past the safe range refuses the body rather than passing as exact.

Every other body decodes with plain `JSON.parse`. Module payloads such as the context module's `transform` recipe embed session message values verbatim and are handed back to OpenCode, whose `JSON.stringify` rejects a `bigint`; on that path a model-written integer past 2^53 rounds as it always has instead of refusing the recipe.

Consumers of an exact body apply their field's Rust domain to the decoded value rather than to a rounded double: `exactCount` admits the status counters' unsigned domain through 2^53 inclusive, so `9007199254740992` is accepted and `9007199254740993` is refused instead of rounding into it, and an invalid counter leaves its valid siblings untouched; `exactU64` and `exactI64` cover generations, spans, revisions, commit tokens, and expiries, and return the `bigint` for any in-range value outside the safe integer range. A `number` counts as a wire integer only when it is a safe integer other than `-0`: serde reads `-0` and every decimal or exponent spelling as `f64`, and a double past the safe range may already be rounded (`9007199254740993e0` decodes as 2^53), so such a value is refused rather than presented as exact. Human output prints the exact decimal (`formatExactInteger`); JSON output emits the digits as a raw number token (`rawJsonInteger`), never a string.

`routeOpen(..., { consumerIdentity: null })` omits the ambient `EIDNARA_MODULE_ID` and `EIDNARA_LAUNCH_NONCE` pair for that bind alone; every other caller's bind is unchanged, and no process environment is mutated.

### The review command

`eidnara review list`, `eidnara review show <causal-identity>`, and `eidnara review status` are the packaged consumers of these reads. Each invocation opens one connection through the installed host client, probes the catalog for the `context` module on that connection, binds one observational route (harness `cli`, session `eidnara-review:<uuid>`, no ambient consumer identity), sends one `review.list` or `review.read` envelope with an explicit `limit` (16 by default, at most 64), and closes; `status` calls `host.status` only and binds no route. Every answer is validated against the protocol vocabulary and the Kernel's byte caps before rendering: a page item outside the outcome or reason vocabulary, a span whose end does not follow its start, text past 32 KiB, an identifier past 512 bytes, or a `terminal` the operation does not define refuses the whole answer as malformed, and no field of a malformed answer is printed. Kernel states such as `invalid:project_mismatch` and `unavailable:store_starting` render as states; management codes such as `route_unbound` and `session_mismatch` render as codes without their messages. Integers keep their exact digits in both text and JSON output.

The status rendering reads the block where `host.status` publishes it, under the `context` component's metrics, and follows the wire document's sanitizer: a block whose state is not `ready` reports every counter unavailable, a counter that is absent, negative, fractional, `null`, or past 2^53 is unavailable rather than zero, a store or activation state the host did not report renders as `unreported` rather than as the wire's `unknown` or `unavailable`, and the counters stay the overlapping populations the sampler reports, with no total or ratio.

## 5. Backup, restore, and mismatch refusal

The Kernel and the Memory Store are two SQLite families that reference each other only by recorded incarnation ids. A MemoryReviewer receipt binds the Kernel incarnation and the Memory Store incarnation it was written under; a Kernel hold binds both; the activation record names both. That is the whole restore contract: the two stores are backed up as one pair and restored as one pair, and every mismatch is refused where it is met.

Both families live under `<data home>/eidnara/context/`: the Memory Store as `store.db` with its `-wal` and `-shm` sidecars, the Kernel as the `kernel/` root holding `kernel.sqlite` and the artifact object store `kernel/artifacts/objects/`. A `.lease` sidecar beside `store.db` records the store's writer epoch; it is not part of a backup, because an open issues its lease above the fence epoch the family itself records.

No operator command performs a Kernel backup or restore in this milestone. `KernelStore::backup` and `KernelStore::restore` exist as library operations on an open store and are what the rehearsal below drives; wrapping them in a command is not in this milestone. The procedure below states what a backup must contain and what a restore is refused for, so that the command, when it exists, and any interim tooling meet the same contract.

Backup:

1. Stop the daemon. The worker is joined before the stores are released, so a stopped daemon has no run in flight. A receipt still `in_progress` at that point is finished by a later pass: the first pass after restart resumes it under the same claim when the same binary reacquires its own slot claim before that claim lapses (`MEMORY_REVIEWER_TASK_LEASE_MS`, 40 s from its last renewal; the worker instance derives from the payload manifest digest); otherwise the claim fences the job until it lapses, and the first pass after that takes the receipt over at the next generation.
2. Kernel database: `KernelStore::backup` publishes a verified copy of `kernel.sqlite` into a private directory (owned by the daemon's user, mode `0700`, not a symlink; anything else is refused as an unsafe destination) and returns its path and the captured commit sequence. This is the only supported Kernel database backup; a file copy of a live Kernel family is not.
3. Kernel artifacts: the database backup carries no artifact bytes. Copy `kernel/artifacts/objects/` beside it. The capture pins the evidence the backup references in the live store, but only for 24 hours unless `capture_pin_expires_at` says otherwise; after the pin lapses, reclamation may remove an object the live database no longer references, and a backup whose evidence names an object the target root does not hold is refused at restore. The objects copy is what makes a backup restorable past that window and into a fresh root.
4. Memory Store: copy the closed `store.db` family (`store.db`, `store.db-wal`, `store.db-shm` when present). The destination must be a directory owned by the daemon's user with mode `0700`, and the copied files must stay `0600`; the store narrows its live family to `0600` on open but nothing guards a copy. The store publishes no backup of its own.
5. Record the pair together with the binary's version and both baseline digests. A backup pair belongs to the binary that wrote it.

Restore of the pair:

1. Stop the daemon.
2. Kernel: place the artifact objects under the target root's `kernel/artifacts/objects/`, open the target root with the same binary that wrote the backup, and call `KernelStore::restore` with the backup path. The restored database keeps its incarnation id, its staged rows, its holds, and its purge tombstones. The restore is refused before anything is displaced when the backup lacks a purge this store committed, or when its live evidence references an object the root does not hold or holds as bytes that fail verification.
3. Memory Store: replace `store.db` and its sidecars with the copied family. It keeps its incarnation id and every receipt.
4. Start the daemon. A proposal selected before the backup reads again through `review.read` while its review hold is live. Neither incarnation changed, so the owner's activation record needs only the new start's credential identity, as after any start.

Mismatch refusal, as the rehearsal test `crates/daemon/tests/memory_reviewer_backup_restore.rs` proves over the data-directory layout above:

| Restored | Beside | What happens |
| --- | --- | --- |
| Kernel database from backup | a root without the referenced artifact objects | `KernelStore::restore` is refused as an invalid restore and the root is left as it was; carrying `kernel/artifacts/objects/` makes the same restore succeed and the evidence readable. |
| Memory Store from backup | a replaced (fresh) Kernel | `review.read` answers `incarnation_mismatch`; resuming the receipt is refused as a binding mismatch; the activation gate closes on `kernel incarnation`. The receipt row stays exactly as backed up. |
| Kernel from backup | a replaced (fresh) Memory Store | No receipt exists, so `review.read` answers `not_selected`; the gate closes on `memory store incarnation`. The Kernel's staged proposal is not served on anyone's say-so. |
| either store | a binary whose compiled versions differ but whose schema baselines match | The gate closes on the compiled version that differs (`identity_mismatch`; the log names the term). Receipts and staged rows are untouched. |
| either store | a binary whose schema baseline differs | The store refuses to open: the Kernel reports `kernel_state: unavailable` with `unavailable_reason: store_unsupported`, and the Memory Store's open fails on its baseline. Neither file is touched, and the activation gate is never evaluated because there is no store to evaluate it against. Nothing migrates a family to another baseline. |

Nothing reconciles a mismatched pair. No migration rewrites incarnation ids, no reset clears receipts, and no code path adopts a staged proposal whose receipt is missing.

Rollback rule: rolling back to an earlier release means the retained old binary together with the backup pair that binary wrote, restored by that binary. A live family the newer release has staged into is not a target for the old binary even when both binaries share a schema baseline: a row shape the old binary does not decode, such as the dependency record in a proposal row's witness (section 4), is refused where it is met rather than migrated. When the release being rolled back changed a schema baseline, the old binary cannot open the current live family to restore into it (the row above), so the live `kernel.sqlite` family and `store.db` family are moved aside first and the old binary restores into the emptied root, with the artifact objects carried as in Restore step 2. If no matching pair exists, the only alternative is an owner-authorized fresh baseline: both stores start empty, the owner writes a new activation record for the new incarnations, and the pending proposals of the abandoned pair are documented as nonportable, because they are bound to incarnations that no longer exist and cannot be carried into the new pair. There is no supported path that keeps one store and replaces the other.

## 6. Exposure bounds (Q38)

Provider exposure is bounded by receipts and quotas, not by a spending cap; the daemon holds no billing ledger and none is added.

- One job runs at most `MEMORY_REVIEWER_MAX_ATTEMPTS` (4) attempts, each at most `MEMORY_REVIEWER_MAX_REQUEST_BYTES` (256 KiB) of request body, within `MEMORY_REVIEWER_RUN_DEADLINE_MS` (120 s) less a 10 s settlement reserve. Every attempt is a committed marker in the Memory Store before a byte is sent; a marker that cannot commit sends nothing.
- Every job charges `MEMORY_REVIEWER_RECEIPT_CHARGE_BYTES` (32 KiB) of permanent receipt metadata, and permanent charges plus temporary allowances are bounded per project (64 MiB) and per host (256 MiB). Terminal rows keep their charge, so a store incarnation admits at most 2,048 jobs per project and 8,192 per host before reservation refuses for good. A project or host at its quota admits no new job; the sampler reports `receipt_charge_bytes`, `allowance_bytes`, `metadata_quota_bytes`, and `metadata_headroom_bytes` so headroom is read, not inferred.
- Every job carries at most `MAX_MEMORY_REVIEWER_HOLD_REFERENCES` (512) held evidence references, and a project holds at most `MAX_ACTIVE_MEMORY_REVIEWER_HOLDS_PER_PROJECT` (64) active holds; a run that ends without settling releases its hold rather than leaving it to its cutoff.
- The requested model profile (`model`, `max_tokens`) is what the attempt marker records; the provider's reported model id is checked against the requested one before any text is released, and a reported model that differs ends the attempt as a failed terminal with nothing released.

Finite cleanup is not evidence of sustainable service. The sampler's per-pass counts say what was swept; the growth of `receipt_charge_bytes` against `metadata_quota_bytes` over time is the measure an operator watches, and section 7 reports what has and has not been measured.

## 7. Evaluation evidence for milestone one

The parent contract asks for supported-proposal precision, useful yield, unnecessary abstention, missed contradictions, and physical requests per useful proposal, each with counts and explicit denominators, `N/A` at a zero denominator, failures and nonadmissions retained, citation validity scored separately from support for the claim, scripted proof kept distinct from actual-model observation, and no statistical quality claim. This section reports against that contract as of this document.

### 7.1 Authorization status

No deployment-owner authorization for live model calls exists, and no Q38 disclosure approval has been written for any deployment. No activation record exists in any environment. Therefore no actual-model run has been made, and every actual-model metric below is reported as missing, not as a pass. Writing an activation record is the owner's act; this document does not authorize one and no fixture stands in for one.

### 7.2 Scripted proof (not a quality claim)

The scripted corpus (`crates/daemon/tests/support/memory_reviewer_corpus.rs`) is eight hand-labeled cases whose model turns are fixed text. It proves the coordinator's delivery, citation binding, and outcome ledger and says nothing about model quality.

| Measure | Count | Denominator | Value |
| --- | --- | --- | --- |
| Cases | 8 | | |
| Published proposals | 7 | 8 cases | scripted |
| Abstentions | 1 (`model_declined`, injected instructions) | 8 cases | scripted |
| Relations covered | supports, contradicts, supersedes, shared origin, decisive evidence outside initial context, protected source, incomplete search, injected instructions | 8 relations | each once |
| Citation validity | every cited alias in a published case names disclosed bytes; a citation outside disclosed bytes is refused before binding | 7 published cases | 7 of 7 valid, by construction |
| Contradictions cited | both cases with a contradicting source (`contradiction`, `decisive evidence outside initial context`) cite it | 2 cases with a contradicting source | 2 of 2, by construction |

These figures are the corpus's own labels replayed; they are not precision, yield, or recall of a model.

### 7.3 Actual-model observations

| Measure | Numerator | Denominator | Value |
| --- | --- | --- | --- |
| Supported-proposal precision | supported published proposals | published proposals | N/A (0 published) |
| Useful yield | useful published proposals | jobs run | N/A (0 run) |
| Unnecessary abstention | abstentions a reviewer judged unnecessary | abstentions | N/A (0) |
| Missed contradictions | contradicting sources in evidence not cited | proposals with a contradicting source in evidence | N/A (0) |
| Physical requests per useful proposal | committed attempt markers not terminated `not_dispatched` (a marker that committed and sent nothing is proof of no disclosure, not a request) | useful published proposals | N/A (0) |
| Citation validity | citations naming disclosed bytes | citations | N/A (0) |
| Failures and nonadmissions retained | | | none observed; the ledger keeps every `failed`, `unknown`, `expired`, and nonadmitted outcome when they occur |

The comparison arm the contract names, one-shot proposals under the same model, profile, sampling, question, and initial eligible evidence, has likewise not run. It is an evaluation arm, not a production mode, and it needs the same owner authorization.

### 7.4 Operational measurements

- Permanent receipt growth and quota headroom: measured only in tests. The worker test observes one job moving from `jobs_ready` to `jobs_abstained` and `receipts_complete` from 0 to 1; the lifecycle test checks the byte identity between `receipt_charge_bytes`, `allowance_bytes`, and `metadata_headroom_bytes` against the quota. No long-running measurement exists.
- Temporary charge reclamation: the lifecycle test observes an expired job's allowance released and an expired staged input abandoned by one maintenance pass; the same test observes a second pass reclaiming nothing further.
- Queue delay and expiry: recorded per job as `queue_deadline_ms` and as the `expired` and `expired_unseen` counts; no production series exists.
- Compaction impact: not measured.
- Guard contention: the disclosure tests prove the network wait begins only after both store owners release and that a held owner stalls the attempt rather than the store; no production contention series exists.
- Producer nonadmission facts: recorded per session as `memory_reviewer_nonadmission` with a closed code and surfaced as `nonadmissions` and `latest_nonadmission_*` counts; the handoff tests observe a capacity refusal recorded as the nonadmission.

None of these measurements supports a claim of sustainable service, and this document makes none.

## 8. Property traceability

The parent property portfolio was not published before dependent implementation, and no record ids exist to cite. This section therefore traces each milestone claim to the test that proves it and does not invent ids. When the portfolio is published, its ids attach to these rows; nothing here is recovered after the fact.

| Claim | Proof |
| --- | --- |
| Staged review inputs are immutable, reference-bound, and private until selected | `crates/kernel/tests/kernel_review_staging.rs`; `memory_reviewer_settlement::kernel_results_stay_private_until_the_receipt_selects_them` |
| Evidence retention is bounded by execution and review holds; an empty hold is admitted and grows by extension | `crates/kernel/tests/kernel_memory_reviewer_holds.rs` |
| Jobs are reserved once per causal identity and activated with progress | `history_summarizer_handoff_tests::reservation_precedes_staging_and_publication_activates_with_progress`, `identical_inputs_neither_duplicate_a_job_nor_reopen_a_settled_one` |
| Attempts are fenced and receipts co-commit execution | `memory_reviewer_disclosure::nothing_is_sent_without_approval_under_cancellation_or_when_the_marker_cannot_commit`, `a_post_commit_lapse_stays_charged_and_sends_nothing` |
| Every read is policy-validated through one broker | `memory_reviewer_broker::scope_staleness_and_expiry_refuse_before_any_disclosure`, `render_check_refuses_secrets_and_placeholders_without_redacting` |
| Project text is confined, finite, and owned | `crates/daemon/tests/memory_reviewer_project_text.rs` |
| One verified request per attempt; a mismatched model releases no text | `memory_reviewer_disclosure::provider_failure_and_a_mismatched_model_end_the_attempt_without_a_second_send` |
| Internal runs carry no public session authority | `memory_reviewer_coordinator::capacity_is_acquired_before_anything_is_spawned_and_never_queued` |
| Proposals publish and read only through completed receipt selection | `memory_reviewer_settlement::a_completed_receipt_selects_the_staged_proposal_and_reads_pass_the_kernel`, `a_selected_result_is_readable_only_through_its_live_review_hold` |
| A resumed claim adopts a durable same-generation result without the broker and with zero requests; interrupted dispatched work without a result settles unknown; a selected read revalidates every persisted member | `memory_reviewer_settlement::a_resumed_claim_adopts_the_durable_result_without_the_broker_or_a_new_request`, `a_result_sealed_before_its_transfer_is_adopted_under_an_empty_execution_hold`, `a_durable_result_whose_lineage_moved_or_lacks_a_record_is_not_adopted`, `a_selected_read_refuses_when_an_uncited_member_no_longer_stands`; `memory_reviewer_coordinator::a_resumed_generation_adopts_the_result_a_lost_run_sealed_without_a_send`, `an_unknown_attempt_outcome_completes_unknown_and_cancellation_joins_the_attempt`, `a_not_dispatched_marker_alone_lets_the_run_proceed_with_a_new_attempt`, `a_resumed_generation_with_a_cancelled_marker_completes_unknown_without_a_send`, `a_hold_cap_refusal_on_resume_leaves_the_receipt_open_instead_of_abstaining` |
| Two individually legal responses cross the job's raw ceiling and the second is refused where it crosses the remainder with the remainder recorded; the text ceiling is enforced the same way; the next marker is refused before it is charged and nothing more is sent; cancelled and failed responses record what they consumed; usage survives takeover and reopen; the schema writes a terminal and its usage once | `memory_reviewer_coordinator::responses_consume_the_jobs_raw_ceiling_across_attempts_and_exhaustion_sends_nothing_more`; `memory_reviewer_disclosure::responses_consume_the_jobs_text_ceiling_across_attempts_and_exhaustion_sends_nothing`, `cancelled_and_failed_responses_record_the_bytes_they_consumed`; `memory_reviewer_model_request::responses_are_charged_against_the_jobs_remaining_allowance_and_refusals_record_known_consumption`; `memory_reviewer_ledger::response_usage_accumulates_across_attempts_and_generations_and_exhaustion_refuses_before_a_marker_is_charged`, `response_usage_columns_are_bounded_and_written_once_at_the_schema` |
| An observational route enrolls no project in the worker, selects no root or schedule for the scheduler, joins no maintenance roster, never replaces a live harness's binding, and reads its own project under route-local authorization; a pass over a view without the project leaves a Ready job unclaimed with the gate open | `daemon tests::an_observational_binding_reads_its_project_and_takes_no_part_in_background_work`; `memory_reviewer_worker::a_pass_over_a_view_without_the_project_leaves_its_ready_job_unclaimed`; `memory_reviewer_wire::an_observational_route_reads_the_same_outcomes_as_an_ordinary_route` |
| Integer lexemes a double cannot reproduce arrive exact through exact-integer routed and control responses while a default routed body decodes as `JSON.parse` does; 2^53 is accepted as a count and 2^53+1 refused; unsupported widths and a withheld lexeme refuse the body; the ambient consumer identity is omitted only when asked | `client.test.ts::integer lexemes a double cannot reproduce arrive exact through exact-integer routed and control responses`, `a default routed response decodes as JSON.parse does, so module payloads forwarded to OpenCode never carry a bigint`, `routeOpen omits the ambient consumer identity only when asked, for that bind alone`; `connection.test.ts::stream items decode exactly only under the exact_json response mode`; `exact-json.test.ts` |
| The review command binds one observational route per invocation with no ambient identity, sends one explicit request, validates every answer against the protocol vocabulary and byte caps, renders exact integers, keeps overlapping status counters and reports unavailable rather than zero, and closes its connection on every path; the shipped transport reaches a running daemon | `packages/cli/src/commands/review.test.ts`, `review.wire.test.ts`; `packages/e2e-tests/src/rust-runner/review-cli.test.ts`; `scripts/smoke-tarball-install.ts` |
| An unselected private result keeps its queue deadline through hold transfer, a timely selected result reads against its selection time under its live hold, and an orphaned hold is reconciled | `kernel_memory_reviewer_holds::review_transfer_acquires_before_releasing_and_moves_only_live_memory_reviewer_references`; `memory_reviewer_settlement::an_unselected_transferred_result_expires_with_its_queue_and_its_hold_is_reconciled`; `memory_reviewer_ledger::the_sweep_closes_an_in_progress_receipt_at_the_queue_deadline_before_its_run_deadline`; records under `docs/properties/evidence-backed-memory-review/` |
| Takeover fences the losing generation; a receipt left in progress is taken over once its claim lapses | `memory_reviewer_settlement::a_takeover_fences_the_losing_generation_and_selects_only_its_own_result`; `memory_reviewer_worker::a_receipt_left_in_progress_is_taken_over_at_the_next_generation_and_settled` |
| Restart republishes a reserved firing without a model run | `history_summarizer_handoff_tests::restart_republishes_a_reserved_firing_without_a_model_run`, `a_production_reservation_republishes_after_a_restart_and_a_stale_one_is_not_carried` |
| Default-Sensitive subjects abstain without a request, a canonical write, or invented provenance | `memory_reviewer_coordinator::a_staged_subject_is_policy_blocked_for_a_remote_model_and_abstains`; `memory_reviewer_worker::a_closed_gate_runs_nothing_and_an_open_gate_runs_the_job_to_policy_blocked_abstention` |
| A genuinely eligible memory becomes a published proposal | `memory_reviewer_coordinator::a_selected_eligible_memory_becomes_a_published_proposal_through_the_shared_path` |
| Canonical and promoted descriptors resolve through their originating decision, and the proposal targets that decision; an owner that is moved, retired, missing, wrong-kind, or out of scope when the subject is read refuses before disclosure, and an owner that moves after disclosure refuses at target binding | `memory_reviewer_broker::canonical_and_promoted_descriptors_resolve_to_their_originating_decision_and_target_it`, `a_moved_missing_stale_or_wrong_kind_owner_refuses_the_subject_and_the_target`; `memory_reviewer_coordinator::a_canonical_subject_resolves_through_its_decision_and_abstains_for_a_remote_model`; records under `docs/properties/evidence-backed-memory-review/` |
| Production selection over canonical classes stays closed until a Remote-eligible representation and positive witness exist | `memory_reviewer_selection::the_production_classes_are_walked_in_order_and_unproven_canonical_descriptors_are_not_selected`; `memory_reviewer_review_selection_is_not_scheduled_while_production_selection_is_closed` |
| The activation gate closes on every identity mismatch and opens only for an attested owner-only record | `memory_reviewer::activation` unit test; `memory_reviewer_worker` |
| A closed gate names the mismatched term and its live value in the log, once per distinct reason, and never the credential fingerprint | `memory_reviewer_worker::a_closed_gate_reports_the_mismatched_term_and_its_live_value_once_per_change`; `memory_reviewer::lifecycle::a_closed_reason_changes_once_per_distinct_reason_and_an_open_gate_clears_it` |
| Status counters are bounded and content-free | `memory_reviewer::lifecycle` tests; `host-runtime` `memory_reviewer_block_tests` |
| Both-store backup and restore over the data-directory layout, artifact objects required at restore, and mismatch refusal | `crates/daemon/tests/memory_reviewer_backup_restore.rs`; `kernel_backup::a_dangling_reference_is_refused_even_when_integrity_check_passes`, `a_backup_whose_evidence_was_purged_after_capture_cannot_be_restored` |
| A family whose schema baseline differs from the binary is refused at open and left untouched | `kernel_open::every_conclusive_kernel_mismatch_is_refused_and_left_untouched`; `storage::a_refused_foreign_database_keeps_its_bytes_and_gains_no_sidecars` |
| Shutdown joins owned work before store release | `memory_reviewer_coordinator::an_unknown_attempt_outcome_completes_unknown_and_cancellation_joins_the_attempt`; `memory_reviewer_worker::a_cancelled_worker_loop_returns_before_the_stores_are_released`; the daemon's `shutdown_cancels_and_joins_tracked_*` tests |
| Cleanup is finite and kind-specific | `memory_reviewer::lifecycle` maintenance test |
| Review outcomes list and read only through the receipt-selected path | `crates/daemon/tests/memory_reviewer_wire.rs` |

Acceptance activation remains blocked: there is no acceptance code, no acceptance protocol version, and no owner-reviewed actual-evidence corpus gate, and this milestone adds none.

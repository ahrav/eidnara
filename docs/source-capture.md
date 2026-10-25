# Source capture

Source capture retains the exact bytes of what an agent saw in a transform
window as Kernel source descriptors: user and assistant message text blocks,
tool outputs, and tool errors, plus the raw bytes of the configured
repository documents the pass read for its prompt (the prompt holds their
canonical form). Each capture is bound to the harness's native identity
and revision, carries a 30-day retention deadline when it holds message or
tool text, and enters the Kernel as a candidate observation under the
existing disclosure gates. The pass charges its window once the response is
built, and the capture job starts once the response is settled and its
output reservation is handed to the wire writer, which serializes the
bytes when it emits the frame, so publication never delays them.

The daemon modules are `crates/daemon/src/source_capture.rs` (the
supervisor, consent, handoff, job, memo, counters, document capture, and
session retirement), `crates/daemon/src/harness_sources.rs` (the adapters,
the publisher, screening, dispositions, receipts, and re-run
reconciliation), `crates/daemon/src/tool_outcome.rs` (the outcome record
and the summarizer digest), and `crates/daemon/src/project_docs.rs` (the
document reads). Expiry lives in `crates/kernel/src/local_file.rs`.

Related documents:

- Specification: <https://github.com/ahrav/eidnara/issues/1066>.
- Property catalog: [`properties/daemon/source-capture/`](properties/daemon/source-capture/README.md).
- Configuration key on the wire: [`host-wire-protocol.md`, Section 7.9](host-wire-protocol.md#79-native-automatic-memory-capture).
- Summarizer view of a tool result: [`compression-fidelity.md`, "Tool outcome digest"](compression-fidelity.md#tool-outcome-digest).

## Consent

`consents` admits a pass when all of these hold in the route's consent
snapshot:

- `source_capture.enabled` is true. The key is `UserOnly` and defaults to
  false; the daemon ignores a project-tier value with a warning, and each
  plugin strips it from the project tier.
- `memory.enabled` is true.
- The session is a primary session (`is_subagent` is false).
- The serializer profile serves native records and names the harness the
  route claims: `Pi` for `pi`, `OpencodeAiSdk` for `opencode`. This binds the
  `harness` identity field on every descriptor to the harness that
  `session.delete` retires by.

`memory.auto_capture` and `memory.auto_promote` gate model-extracted memory
facts and leave source capture unaffected. A pass that fails consent builds
no `Offer`, and the adapters run for consented passes only.

Repository documents join the offer when the pass also has
`memory_classifier.inject_docs` set (default true).

## Path of one pass

1. The transform handler builds an `Offer` holding the pass's native records
   (`parsed.native_messages`), the session and project binding, the current
   delete epoch, and a sink for document reads.
2. Once the response is built, `SourceCapture::offer` charges the retained
   window against the supervisor's 32 MiB byte budget
   (`MAX_RETAINED_WINDOW_BYTES`). The charge is twice the window and
   document bytes plus 256 KiB of artifact preparation
   (`ARTIFACT_PREPARATION_BYTES`, four times the 64 KiB batch). An offer
   above 32 MiB is refused as `refused_window_too_large`; a budget that
   cannot cover it is refused as `refused_resident_short`.
3. The offer rides the response as `AfterResponse` work. It runs once the
   response is settled and its output reservation is handed to the wire
   writer, which serializes a transform response when it emits the frame; a
   response that never settles drops it unrun, releasing the
   charge.
4. `CaptureHandoff::into_job` takes the store's single publication slot (a
   one-permit semaphore). A busy slot refuses the handoff as
   `refused_queue_full` and releases its charge. The job's 5 s cooperative
   budget (`CAPTURE_BUDGET`) starts at slot acquisition.
5. The daemon's task admission spawns the job. A closed admission (shutdown)
   refuses it as `refused_shutting_down`.
6. `CaptureJob::run` runs `publish` on the blocking pool. The job owns the
   slot permit, its charges, and the window until the closure returns,
   including on panic (`worker_panicked`). Cancellation interrupts the
   budget, and `run` returns after the closure physically completes.

Inside `publish`, in order:

- A cancelled job ends `discarded`; an unavailable Kernel ends
  `kernel_unavailable`; an exhausted budget ends `deadline`; an adapter-copy
  charge the budget cannot cover ends `refused_resident_short`.
- `units` runs `opencode_units` or `pi_units` over every record. A record
  the adapter refuses contributes no units and counts once in
  `adapter_refused`. The adapters' copies beyond the window charge are
  charged before use.
- Each unit's memo key is the Kernel store's incarnation id, the project,
  harness, and session, and the unit's receipt key. Units the memo holds as
  pending go first, then units the memo lacks. A unit the memo holds as
  published or terminal under an equal request digest is skipped
  (`units_memo_skipped`), and a repeated key within the window is dropped.
- The job takes the capture commit gate, the same mutex `session.delete`
  and checkpointing hold. Under it, `publish_locked` rechecks the delete
  epoch (`session_invalidated` on mismatch), materializes the
  `harness-capture` domain and the project scope under a replayable
  receipt, rechecks the budget, and calls `publish_screened` for the window
  batch. `capture_documents` then publishes the document batch in its own
  commit under the same gate.

### Screening and batch bounds

`publish_screened` decides each unit in order before the Kernel
transaction:

| Bound | Value | Source |
| --- | --- | --- |
| Unit text | 64 KiB | `MAX_SOURCE_UNIT_BYTES` (`MAX_CAPTURE_SOURCE_BYTES`) |
| Units per batch | 32 | `MAX_SOURCE_BATCH_UNITS` (`MAX_CAPTURE_MESSAGES`) |
| Text per batch | 64 KiB | `MAX_SOURCE_BATCH_BYTES` (`MAX_CAPTURE_INPUT_BYTES`) |
| Receipt preview wait | 30 s, cut to the budget | `RECEIPT_WAIT` |
| Document preview wait | 5 s, cut to the budget | `PREVIEW_WAIT` |
| Session-retirement preview wait | 5 s | `PREVIEW_WAIT` |
| Native revision lead over observation | 1 h | `MAX_REVISION_LEAD_MS` |

A unit over 64 KiB is refused whole, so every retained artifact equals the
complete native string. Identity, revision, and scope refusals are decided
per unit and leave siblings admissible. The first unit that passes screening
and would exceed 32 units or 64 KiB, and every later unit within the size
cap, is deferred. A replayed unit counts toward the bounds, which is why pending
units are offered first. The Kernel transaction stays all-or-nothing over
the admitted units.

### Dispositions

`classify` maps each `UnitDisposition` to a counter and a memo entry:

| Disposition | Counter | Memo entry |
| --- | --- | --- |
| `Published`, new | `units_published` | published |
| `Published`, replayed receipt | `units_replayed` | published |
| `UnitTooLarge` | `units_unit_too_large` | terminal |
| `Deferred` | `units_deferred` | pending, keeping the first wait time |
| `Refused(IdentityReused)` | `units_identity_conflict` | terminal |
| `Refused`, `is_permanent()` | `units_refused_terminal` | terminal |
| `Refused`, retryable | `units_refused_retryable` | pending |
| `Superseded` | `units_superseded` | terminal |

When the whole batch is refused, the unit `BatchRefusal::conflicting_unit`
names (a receipt under another request digest) becomes terminal and counts
in `units_identity_conflict`. Every other keyed unit is recorded `pending`,
keeping the time it first waited, so the memo offers it ahead of unknown
units on the next pass that carries it and it ages
`oldest_deferred_age_ms` until it publishes or the memo evicts it. The job
then ends `deadline` or `kernel_error`.

`PublishError::is_permanent` reports artifact refusals for size, purge or
pending-unlink re-admission blocks, secrets, scan limits, rewritten bytes,
unsupported shape, text fields over their cap, invalid input, and digest
collision, plus `IdentityReused` and every descriptor refusal except
`HeadMismatch`, as permanent. Capacity, storage, Kernel, missing admission,
revision-lead, and head-mismatch refusals are
retryable. In capture, per-unit `Refused` dispositions come from identity,
revision, scope, and key screening. Artifact refusals arise during batch
ingest and reject the whole batch, as the paragraph above describes.

### Memo, receipts, and replay

Kernel receipts are the durable record of capture. Each unit has two
receipts, `source-descriptor:{key}` and `source-evidence:{key}`, where the
key digests the occurrence id, revision, and representation. The request
digest binds the text, domain, scope, role, sensitivity, provider egress,
and the outcome record under the tag `source-request.v2`. `stored_receipt`
probes the `v2` digest and then the frozen `source-request.v1` digest, so a
receipt written before the outcome record replays as written. A key holding
neither digest is a conflict: the batch is refused, and
`BatchRefusal::conflicting_unit` names the unit, which becomes terminal under
`units_identity_conflict`.

The memo is process-local and bounded at 4 MiB (`MEMO_BUDGET_BYTES`),
charging each entry two key copies, its digest, and 192 bytes of
bookkeeping (`MEMO_ENTRY_OVERHEAD_BYTES`); it evicts oldest first. An entry
holds a digest, a disposition, and, for a pending unit, the time it first
waited. It holds no window bytes. A memo hit with an equal digest
suppresses the Kernel probe; a changed digest under the same key reaches
the Kernel's conflict check. After a restart, a poisoned lock, or a
`session.delete` whose memory-store delete succeeded, the memo is empty and
the Kernel answers from its
receipts (`replayed: true`).

The supervisor's retained memory is bounded by `RETAINED_BYTES_BOUND`,
32 MiB plus 4 MiB, which the daemon adds to its resident budget.

## What each class holds

| Class | Identity | Revision | Representation | Admission pair |
| --- | --- | --- | --- | --- |
| `messages` | `project_id`, `harness`, `session_id`, `message_id`, `block_index` | native timestamp | `text` | user: `ExplicitUser`/`UserExplicit`; other roles: `ModelInference`/`AssistantInference` |
| `raw_tool_spans` | `project_id`, `harness`, `session_id`, `parent_message_id`, `tool_call_id`, `result_revision`, `block_index` | `result_revision` | `tool_output` or `tool_error` | `TrustedToolResult`/`ToolUntrustedOutput` |
| `repository_documents` | `project_id`, `repository_id`, `relative_path` | historical head plus one | `text` | `UntrustedRepoText`/`RepoUntrustedText` |

`project_id` is the bound project's digest. For documents, the project
digest also stands in as `repository_id`.

### Messages

- OpenCode: user text publishes at `time.created`; assistant text publishes
  at `time.completed` and waits while the message has no completion time.
  Text parts marked `ignored`, and every part kind other than `text` and
  `tool`, contribute nothing. `block_index` is the part position.
- Pi: a row carries its session entry's `type` and `parentId` beside `id`
  and `message`. Only `type: "message"` entries with role `user`,
  `assistant`, or `toolResult` yield units, at `message.timestamp`. A bare
  string content is block 0; otherwise each text item is a block at its
  position. A row with a missing, null, or empty `type` is refused as
  `MissingIdentity("type")`, and another `type` as `UnsupportedShape`.

### Tool results and the outcome record

- OpenCode: a `tool` part with `state.status` of `completed` or `error`
  yields one unit, block 0, whose text is `state.output` or `state.error`,
  at revision `state.time.end`. A part whose `state.metadata.interrupted` is
  true waits until its turn has `time.completed`.
- Pi: a `toolResult` message yields one unit per text content item, under
  `parentId`, `toolCallId` (verbatim, including a composite `call|item`
  spelling), and `timestamp`. `isError` selects `tool_error`.

Each tool descriptor carries a `ToolOutcome` record, at most 1 KiB of
stored JSON (`MAX_TEXT_FIELD_BYTES`), version `TOOL_OUTCOME_VERSION` (1):

- `status`: `completed` or `error`, agreeing with the representation.
- `tool_name` and `byte_length` (the unit's UTF-8 length).
- `truncated`: OpenCode `state.metadata.truncated`; Pi
  `details.truncation.truncated`.
- `exit_code`: the integer `state.metadata.exit` of an OpenCode `bash`
  call. Pi records carry status and truncation only.
- `exception_ref`: for an interrupted OpenCode call, the `name` of the
  assistant message's `info.error`, when it is an ASCII identifier of at
  most 64 bytes.

An absent or malformed optional field is unknown and stays unset. Equal
bytes from calls with different outcomes form two descriptors, and the
record enters the request digest. The summarizer's digest of a tool result
is specified in [`compression-fidelity.md`](compression-fidelity.md#tool-outcome-digest).

### Repository documents

The documents are the fixed `PROJECT_DOC_FILES` list, `ARCHITECTURE.md`
then `STRUCTURE.md`, read when the pass composes its m0 prompt block.
`read_document` opens each without following a symlink, caps the read at
256 KiB, and decodes UTF-8. `leave_reads` hands the raw bytes to the
offer's sink; a failed pass and each retried composition clear the sink, so
capture sees the bytes of the composition the pass sent. The prompt keeps
its canonical block and hash; the descriptor holds the raw bytes, so an edit
that changes only a BOM, line endings, or trailing whitespace publishes a new
revision while the prompt hash stays equal.

`plan_document` decides each read against the project's live document rows:

| Read | Step | Counter |
| --- | --- | --- |
| Text equal to the live revision's artifact | skip | `documents_unchanged` |
| Other text up to 64 KiB | publish at historical head plus one | `units_published` or `units_replayed` |
| Text over 64 KiB, or a file over 256 KiB | skip | `documents_oversize` |
| `Missing` with a live revision | retire the live revision | `documents_retired` |
| `Missing` with no live revision | skip | `documents_missing` |
| Symlink or other non-regular file | skip | `documents_not_regular` |
| Bytes that are not UTF-8 | skip | `documents_not_utf8` |
| Other read failure | skip | `documents_unreadable` |

`Missing` means the file is absent from a directory that exists; a vanished
project root reads `Unreadable`. Only `Missing` retires. A deadline, a failed
preview, a failed retirement, or a refused publication inside the document
batch counts every remaining read in `documents_refused`. A job that ends
before the document batch (`discarded`, `kernel_unavailable`, `deadline`,
`refused_resident_short`, `session_invalidated`, an incarnation-read
failure, or a scope failure) drops its reads, and the drop counts each of
them in `documents_refused` as well. Document evidence is `canonical` and
carries no retention deadline. `session.delete` leaves document lineages in
place.

## Identity and revision rules

- Identity is native or the unit is refused. Every identity field comes from
  a named native field; a record missing one is refused as
  `MissingIdentity`. An OpenCode
  record naming another session is refused as `SessionMismatch`.
- `messages` and `raw_tool_spans`: a descriptor's revision exceeds its
  lineage's live revision, and a newer revision invalidates its predecessor
  in the same commit. Changed bytes under the same identity and revision
  conflict as `IdentityReused`. OpenCode user text and completed assistant
  text have no separate edit revision, so an edit of their bytes is a
  terminal conflict.
- Tool re-runs. `result_revision` is part of the frozen `raw_tool_spans`
  identity, so each result generation is its own lineage. `publish_screened`
  groups tool units by call (`project_id`, `harness`, `session_id`,
  `parent_message_id`, `tool_call_id`). A call's newest generation, in the
  batch or live in the store, is its head. The batch retires the call's
  older live generations in the commit that publishes the head, under a
  `source-rerun-retire:{digest}` receipt that binds every call, successor,
  and sorted retirement set. The retirement re-reads the call's live
  generations inside the commit and refuses the batch as `Conflict` when
  they differ from the plan. An older generation that never published is
  `Superseded`; one that replays its receipt retires nothing. Distinct
  `tool_call_id`s coexist. These decisions hold because the capture commit
  gate serializes every capture publication of the store.
- `repository_documents`: the revision is the lineage's historical head,
  the highest revision ever published including retired rows, plus one,
  starting at 1. The descriptor commit checks that head as `expected_head`.
  A revision at or below the historical head refuses as
  `RevisionNotAdvanced`; a moved head refuses as `HeadMismatch`. Both count
  in `documents_refused`, and the next composing pass reads the head again.
  A recreated file after a retirement therefore publishes above every
  earlier revision.

## Retention and expiry

`messages` and `raw_tool_spans` evidence is ingested with
`retention_class = harness_capture` and
`retain_until = observed_at + 30 days` (`HARNESS_CAPTURE_RETENTION_MS`,
equal to `CAPTURE_REPLAY_RETENTION_MS`). The Kernel ingest requires a
finite deadline for this class. The evidence receipt's request digest
excludes the clock, so a replay returns the first row and its deadline.
Equal-text units in one batch share one evidence row and its deadline.

`KernelStore::expire_harness_captures` runs in the MemoryReviewer lifecycle's
`kernel_slice`, after the MemoryReviewer capture sweep
(`expire_local_file_captures`) and before
`run_staging_maintenance`, while the Kernel is ready. The lifecycle repeats
every 30 s (`SAMPLE_INTERVAL`), or every 5 s (`SAMPLE_RETRY_INTERVAL`) after
a pass that advanced or failed. Each call:

- selects at most 64 expired captures (`MAX_EXPIRED_CAPTURES_PER_CALL`),
  oldest `retain_until` first, excluding evidence a live capture pin holds;
- commits the page in one transaction under the reserved
  `harness-capture-expiry` producer; the commit's receipt is the first
  capture's, and every later capture in the page records its own receipt
  inside it, each rechecking the deadline and pins under the writer;
- retires the capture's live source descriptors first, then its evidence;
- keeps evidence another live row still cites (`retained`), and drops that
  capture from later pages until the citation is gone;
- rolls the page back when the store refuses a capture, then commits the
  page again without it.

The daemon adds each pass's counts to `expiry_retired` and
`expiry_retained`. Canonical, git, document, and every other retention class
are outside this sweep.

Artifact GC reclaims the bytes afterwards only when no live reference names
the digest, no active pin, live ingestion reservation, or unacknowledged
descriptor replay holds it, and 14 days (`REFERENCED_GRACE_MS`) have
passed since the evidence was invalidated or its pin released, whichever is
later, and `retain_until` has passed. Retired descriptors stay readable as
history until then.

## Disclosure

- The capture publisher asserts `Sensitivity::Sensitive` and
  `ProviderEgress::LocalOnly`, and every capture evidence request carries no
  provenance: the field forwards `git_sources::commit_provenance`, which is
  `None` for every class except `git_commits`. The Kernel stores the evidence
  as `Sensitive`.
- Exact ingest refuses bytes in which the redactor detects a secret
  (`ExactBytesRewritten`) and retains nothing. In a capture batch, this
  refusal rejects the whole batch and names no unit. The job ends
  `kernel_error`, the memo records every keyed unit of the batch as pending,
  and the next pass that carries the unit offers the same batch again.
- `Sensitive` evidence is hidden from the `AutoInject` and `AutoSearch`
  surfaces, and the `Remote` destination receives the `ProviderSensitive`
  eligibility verdict (`denies_destination`).
- Each descriptor's admission is recorded under `EventKind::Other` with the
  fixed class pair in the table above. `EventKind::Other` leaves maturity
  unchanged and promotes nothing; promotion goes through explicit review.
  Capture writes no memory decision and no `decision_kind`.
- The CI "Invented provenance" step fails a change that constructs
  `RepositoryProvenance` or populates a `provenance` field in
  `harness_sources.rs`, `source_capture*`, or `project_docs.rs`, except
  `provenance: None` and the one line that forwards the git publisher's
  commit provenance.

## Deletion

`session.delete` runs both steps on one blocking task under the capture
commit gate:

1. `retire_captured_sources` retires every live `messages` and
   `raw_tool_spans` descriptor whose `project_id`, `harness`, and
   `session_id` name the bound session, in pages of at most 256 descriptors
   (`SESSION_RETIREMENT_PAGE`). Each page is one commit keyed by the
   `source-session-retire.v1` tag, the session's identity fields, and the
   page's sorted object ids. A page that replays means the inventory listed
   already-retired rows and fails as `Conflict`. Retired rows stay readable
   as history.
2. The memory store deletes the session's rows. Once that delete succeeds,
   `invalidate_sessions`
   bumps the delete epoch and empties the memo, so every capture whose pass
   sampled the epoch before the delete, for any session, ends
   `session_invalidated` when it reaches the gate. A delete that ends
   `kernel_unavailable`, `source_retirement_failed`, or
   `store_write_failed` leaves the epoch and the memo unchanged.

A deployment without a Kernel skips step 1. Outcomes, each safe to retry:

| Outcome | Meaning |
| --- | --- |
| `{ok: true, deleted_rows}` | Both steps completed. |
| `kernel_unavailable` | The Kernel is starting or unavailable, or an inventory or page commit failed transiently. The memory store is untouched; a retry continues from what is still live. |
| `source_retirement_failed` | A corrupt row, invalid input, or a replayed page. The memory store is untouched; a retry meets the same fault until the Kernel is repaired. |
| `store_write_failed` | The memory-store delete failed after step 1 committed; a retry completes step 2. |
| `store_unavailable` | No store was open, or the blocking work did not report back; a retry converges. |

## Observability

The daemon health report carries a `source_capture` object in `metrics`.
It holds every counter below by name, `active` and `draining` (the active
slot reports as `draining` once shutdown begins), and
`oldest_deferred_age_ms` (the age of the memo's oldest pending unit, or
null).

| Group | Names |
| --- | --- |
| Handoff | `admitted`, `refused_queue_full`, `refused_resident_short`, `refused_window_too_large`, `refused_shutting_down` |
| Job end | `completed`, `discarded`, `session_invalidated`, `kernel_unavailable`, `deadline`, `kernel_error`, `worker_panicked` |
| Units | `adapter_refused`, `units_published`, `units_replayed`, `units_memo_skipped`, `units_deferred`, `units_unit_too_large`, `units_identity_conflict`, `units_refused_terminal`, `units_refused_retryable`, `units_superseded` |
| Documents | `documents_unchanged`, `documents_retired`, `documents_missing`, `documents_not_regular`, `documents_oversize`, `documents_not_utf8`, `documents_unreadable`, `documents_refused` |
| Expiry | `expiry_retired`, `expiry_retained` |
| Gauges | `retained_bytes`, `publish_ms_last`, `publish_ms_max` |

`publish_ms_last` and `publish_ms_max` time the window batch's
`publish_screened` call. Log lines go to stderr in the fixed form
`daemon: source capture {stage}: {reason} ({count})`, built from a fixed
stage string, a counter name, and a count. `SourceUnit`'s `Debug` output
prints identities and sizes. Unit text, paths, and digests stay out of
logs and counters.

## Loss conditions

Capture retains what reaches a committed batch. These conditions leave an
observed unit uncaptured or unrecoverable:

| Condition | Effect |
| --- | --- |
| Refuse-newest (C7) | A handoff that meets a busy slot, a short budget, an oversized window, or shutdown is refused and its window dropped. The next pass that carries the same records captures them; a window no later pass carries is lost. |
| Unsettled response | A response that never reaches the wire writer drops its capture unrun. |
| Deferred and retryable units | A unit past the batch bounds or refused retryably waits as pending for the next pass that carries it. A pass that no longer carries it leaves it uncaptured. |
| Delete epoch | A `session.delete` that completes both steps ends every earlier handoff as `session_invalidated`, including other sessions' handoffs; their next passes offer the windows again. |
| Documents | A read dropped by a refusal, a busy slot, or a deadline waits for the next pass that composes the prompt block. That pass reads afresh, so the lineage converges on the latest read and intermediate versions are absent. |
| Terminal units | A unit over 64 KiB, an edit under an unchanged identity and revision, and a superseded tool generation are refused and retain nothing. |
| Secret-bearing bytes | Ingest refuses the whole batch that admits the unit, and the job ends `kernel_error`. The admitted siblings in that batch publish nothing on any pass that admits the unit again. |
| Post-delete republication (OQ4) | A harness session that keeps running after `session.delete` publishes again on its next pass: stored receipts answer for its earlier units, and new messages publish fresh descriptors. |
| Pending unlink (OQ20) | Ingest refuses a digest with a pending unlink. In a capture batch, that refusal rejects the whole batch as `Conflict` and names no unit, so the job ends `kernel_error`, the memo records every keyed unit of the batch as pending, and each later pass that admits the unit is refused the same way while the block holds. Every pending unlink is written with a purge tombstone, so the block outlives the unlink. |
| Tombstone after purge (OQ21) | A purge (`ArtifactDeletionKind::Purge`) writes an artifact purge tombstone, and ingest refuses identical bytes under any later identity as `ReAdmissionBlocked` for as long as the tombstone exists; in a capture batch the refusal rejects the whole batch as `Conflict` with no named unit. Expiry retires rows and GC reclaims bytes through the reclaim path, which writes no tombstone. |
| Retention | Expired, uncited, unpinned captures retire after 30 days, and GC may reclaim their bytes 14 days later. |

## Enabling gate

Retirement keeps descriptor and receipt rows, so 30-day retention bounds
bytes and leaves row counts growing with every captured unit. OQ6 is the
entry gate for setting `source_capture.enabled` in any environment: the
capacity owner supplies a per-store row allowance and the admission behavior
at that allowance. Until OQ6 is decided, the key stays at its default,
false.

## Open owner decisions

Each question below is open; this document records it and resolves none.

| Question | Owner | Decision | Affects |
| --- | --- | --- | --- |
| OQ2 | #708 and the disclosure policy owner | Whether a future read tool may disclose captured `Sensitive` bytes, such as tool errors, to the model, and under which gate. | Disclosure; the model-facing read of captured sources. |
| OQ6 | Capacity owner | The per-store row allowance for descriptor and receipt rows and the admission behavior at it, such as refusing new publication while replaying existing receipts. | The enabling gate; expiry drain; store growth. |
| OQ19 | Performance owner | The latency-equality criterion for a capture-enabled pass against a capture-disabled one. The recommendation is #708's rule: at least 10 interleaved runs per workload, medians and spread reported, and a repeatable p95 regression above 5 percent blocks. | The response-independence acceptance check. |
| OQ21 | Kernel owner | Whether identical bytes observed after their artifact was purged stay refused by the purge tombstone, or whether `harness_capture` purges leave the digest re-admissible. | Retention; re-capture of repeated tool output. |

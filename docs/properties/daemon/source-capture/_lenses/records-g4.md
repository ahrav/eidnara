# Source capture records, group g4

Records for the U1 and U2 capture-path obligations. Every citation was checked
against HEAD `5a842d998` on branch `source-capture/1090-property-catalog`.
Reachability was checked per record. `ConfigKey::SourceCaptureEnabled` in
`crates/daemon/src/config.rs` is `TierClass::UserOnly`.
`DaemonConfig::default` sets `source_capture_enabled: false`. The test
`hostile_project_tier_cannot_change_privileged_values_and_warns_per_key`
asserts that a project tier cannot enable it. `source_capture::consents` reads
the key on every pass that reaches the capture supervisor.

### sc-consent-gate-admits-capture

Type: safety
Reachability: explicit-config-only - the transform pass builds a capture
`Offer` only when `source_capture::consents` holds, and that requires
`source_capture_enabled`, which defaults to false and is user-tier only.
Status: active
Exercised: yes - `crates/daemon/src/source_capture_tests.rs`
`capture_off_memory_off_or_a_subagent_session_runs_no_adapter` runs key off,
`memory_enabled` off, and `is_subagent` true against a real Kernel store and
asserts every counter is zero and the tip unchanged;
`a_route_claiming_another_harness_captures_nothing` asserts a `pi` route
serving an OpenCode window admits no job.
Guarantee: An adapter runs and bytes are copied only when the pass's consent
snapshot has `source_capture.enabled` and `memory_enabled` set,
`is_subagent` clear, a native serializer profile, and a profile harness equal
to the route's claimed harness.
Check: `always` - on every handoff, adapter invocation implies that the
snapshot predicate held. It is evaluated per handoff, because the predicate
is a per-pass snapshot.
Fault/timing angle: a config reload between the pass and the job. The job
carries the `Offer` and never re-reads config, so it runs under the snapshot
that admitted it.
Required faults and enabling state: F28 (key off, `memory_enabled` off,
`is_subagent` true, route harness mismatch); F29 (consent changed between
pass and job) for the timing angle.
Confidence: high - [evidence](../evidence/sc-consent-gate-admits-capture.md).
I read `consents`, the `Offer` construction in the transform pass,
`SourceCapture::offer`, and `CaptureJob::units`, which is the only capture
caller of the adapters. I also read both tests.
Existing check: `source_capture_tests.rs`
`capture_off_memory_off_or_a_subagent_session_runs_no_adapter`
(`unaudited`); `a_route_claiming_another_harness_captures_nothing`
(`unaudited`); `config.rs`
`hostile_project_tier_cannot_change_privileged_values_and_warns_per_key` and
module `source_capture_parity` (`unaudited`).
Impact: exact user text and tool bytes are retained without the distinct grant
the plan requires.
Open questions:
- Two switches or the existing three (OQ15). Resolved in U1: two switches
  plus the subagent skip. `consents` never reads `memory_auto_capture` or
  `auto_promote`.
- No test constructs a config reload between the pass and the job (F29).

### sc-response-independent-of-publication

Type: safety
Reachability: explicit-config-only - the after-response capture work exists
only when `consents` admitted the pass; with the key at its default the pass
attaches none.
Status: active
Exercised: partial - `source_capture_tests.rs`
`the_response_is_identical_whatever_capture_and_the_kernel_are_doing`
compares capture-on and capture-off responses byte for byte with the Kernel
Ready, Starting, writer held by another thread's commit, and slot full;
`a_spent_budget_before_the_writer_commits_nothing` asserts a response time
bound under `Deadline` but no byte comparison;
`document_capture_leaves_the_response_and_respects_inject_docs` and
`pi_entry_identity_on_the_row_decides_capture_and_never_the_response` compare
responses; `transform_unit/host_tests.rs`
`the_host_starts_source_capture_after_the_response_settles` drives the
transport path.
Guarantee: A capture-enabled pass returns the same response bytes as a
capture-disabled pass, and the request path awaits neither publication, Kernel
phase, nor publication outcome.
Check: `always` - a predicate over every returned response: its bytes equal
the capture-off response for the same input. The property must hold on every
response, so it is not a coverage marker.
Fault/timing angle: the window between response construction and wire
handoff. It holds only a synchronous budget charge. The slot is taken with
`try_acquire_owned`, and the job is spawned unawaited after
`settle_prepared`.
Required faults and enabling state: F6 (Kernel Starting, `Deadline`), F22
(slot full), F30 (writer held by a foreground commit).
Confidence: high - [evidence](../evidence/sc-response-independent-of-publication.md).
I read `capture_after_response`, the transport handler's after-response
ordering, `CaptureHandoff::into_job`, `CaptureJob::run`, and the tests.
Existing check: the five tests named in `Exercised` (`unaudited`).
Impact: a Kernel stall becomes a transform stall, or the response varies with
Kernel state.
Open questions:
- Latency-equality threshold for AC7 (OQ19) (needs human input).
- The `Deadline` case asserts a time bound but compares no bytes with a
  capture-off baseline.

### sc-one-outstanding-batch-refuse-newest

Type: safety
Reachability: explicit-config-only - the slot is taken only by a
`CaptureHandoff`, which only a consented pass creates.
Status: active
Exercised: yes - `source_capture_tests.rs`
`the_response_is_identical_whatever_capture_and_the_kernel_are_doing`, slot
full section: the `before_publish` hook holds the first batch, and a second
pass is refused `refused_queue_full == 1` with `retained_bytes` unchanged and
`active == 1`. The section carries the coverage witness
`handoff_refused_slot_busy`.
`a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one`
asserts 32 published and one `units_deferred`.
`shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot`
asserts that a panic releases the slot and charges and that shutdown waits.
`a_window_past_the_remaining_budget_is_refused_resident_short` and
`crates/daemon/tests/harness_sources.rs`
`capture_batch_defers_units_beyond_its_bounds` cover the remaining bounds.
Guarantee: At most one capture job, publishing batches of at most 32 units and
64 KiB of text, is outstanding per daemon store. A handoff while one is
outstanding is refused with `refused_queue_full` and counted, and it retains
nothing once refused.
Check: `always` - `active <= 1`, and `retained_bytes` after a refused
handoff equals its value before that pass. The bound must hold at every
instant, and the zero-retention claim is checkable at refusal.
Fault/timing angle: two passes racing for the slot. The charge is taken at
response construction and the slot at settlement. The permit lives in the
`spawn_blocking` closure and drops only when the closure returns or unwinds.
Required faults and enabling state: F22 (a second pass while the first batch
is held inside publication).
Confidence: high - [evidence](../evidence/sc-one-outstanding-batch-refuse-newest.md).
I read `SourceCapture::default`, `offer`, `into_job`, `CaptureJob::run`,
`RetainedCharge`, `ActiveSlot`, and `publish_screened`'s bounds, plus the
tests.
Existing check: the tests named in `Exercised` (`unaudited`).
Impact: unbounded resident bytes from retained windows, or concurrent writer
pressure from capture.
Open questions:
- The ticket says the slot is acquired before any window reference is
  retained past the response. The code holds the charge and `Arc`
  references from response construction to settlement and takes the slot
  afterward. The net retention of a refused handoff is zero and it makes no
  copy, but the wording and the code differ.
- One job can run two serial batches: the window's, then the documents'
  (`capture_documents`). The outstanding bound counts jobs.

### sc-unit-cap-refuses-whole-never-prefix

Type: safety
Reachability: explicit-config-only - `publish_screened` has two production
callers, the capture job's window batch and its documents batch, and both run
only under consent.
Status: active
Exercised: yes - `crates/daemon/tests/harness_sources.rs`
`capture_batch_refuses_an_oversized_unit_whole_and_publishes_its_siblings`
publishes a 65536-byte unit with equal artifact bytes. It then sends two
siblings around a 65537-byte unit and asserts `UnitTooLarge`, both siblings
published, evidence `(3, 65536)`, four new receipts, none for the oversized
unit, and an unchanged tip on retry.
`capture_batch_refusals_take_no_admission_slot` asserts that an oversized
unit takes no batch slot. `source_capture_tests.rs`
`unreadable_documents_are_counted_and_retire_nothing` asserts that a 70 KiB
document counts `documents_oversize`.
Guarantee: Every retained capture artifact equals the complete native string.
A unit above 64 KiB has no evidence object and a `unit_too_large`
disposition, and its siblings still publish.
Check: `always` - every capture evidence object equals its unit text and is
at most 65536 bytes. This is a property of every stored object.
Fault/timing angle: none.
Required faults and enabling state: F31 (unit text above 64 KiB beside
sibling units).
Confidence: high - [evidence](../evidence/sc-unit-cap-refuses-whole-never-prefix.md).
I read the length screen in `publish_screened`, which runs before `prepare`,
`evidence_request`'s whole-text payload, the adapters (no truncation),
`classify`, `plan_document`, and the tests.
Existing check: the three tests named in `Exercised` (`unaudited`);
`MAX_SOURCE_UNIT_BYTES` aliases `MAX_CAPTURE_SOURCE_BYTES` in
`crates/memory-store/src/memory_capture.rs` (`unaudited`).
Impact: a spliced prefix is presented as an exact source, or one oversized
unit silences its window.
Open questions:
- No transform-path test sends an oversized unit, so the supervisor's
  `units_unit_too_large` counter and its terminal memo entry are untested.

### sc-terminal-disposition-never-retried

Type: safety
Reachability: explicit-config-only - the memo and the dispositions exist only
inside consented capture jobs.
Status: active
Exercised: partial - `source_capture_tests.rs`
`changed_bytes_under_one_identity_are_terminal_and_never_retried` edits an
OpenCode user message in place under the same `time.created`. It asserts one
`units_identity_conflict` across two edited passes, an unchanged tip, and one
live row with the original bytes.
`a_concurrent_publication_of_a_unit_replays_or_conflicts` asserts that the
memo answers for the terminal unit. `harness_sources.rs`
`capture_batch_names_the_unit_whose_receipt_conflicts` asserts that the
conflict names its unit and commits nothing. No test re-probes a terminal
unit after memo loss.
Guarantee: A terminal disposition for an unchanged `(receipt key, request
digest)` is never retried while its memo entry lives. After a memo loss the
Kernel's re-probe commits nothing, and the prior live descriptor stays live.
Check: `always` - a terminal memo entry with equal key and digest is never a
publication candidate and never becomes `Pending`, and the tip is unchanged
by any later attempt of that unit. Both must hold on every pass.
Fault/timing angle: a memo loss (restart, any `session.delete` through
`invalidate_sessions`, eviction past 4 MiB, poison). The next pass offers
the unit again, the Kernel answers `Conflict` from its receipt before any
commit, and the siblings in that batch wait one pass.
Required faults and enabling state: F4 (an OpenCode user message edited in
place keeping `time.created`); F3 (memo loss) for the re-probe.
Confidence: high - [evidence](../evidence/sc-terminal-disposition-never-retried.md).
I read `is_permanent`, `BatchRefusal::conflicting_unit`, the error arm of
`publish_locked`, `classify`, candidate selection in `CaptureJob::publish`,
`Memo`, and the tests.
Existing check: the tests named in `Exercised` (`unaudited`);
`PublishError::is_permanent` (`unaudited`, no table test).
Impact: retry amplification on the Kernel writer, or a minted revision that
evades `IdentityReused`.
Open questions:
- Daemon-minted revision for proven edits (OQ5). Resolved in U2: D2 stands.
  Changed bytes are a terminal `IdentityReused`, and the code has no minting
  path.
- The re-probe after memo loss has no test, so the `replay_after_memo_loss`
  precondition is not paired with a terminal unit.

### sc-pending-unit-publishes-next-ready-pass

Type: liveness
Reachability: explicit-config-only - pending state exists only in the memo of
consented capture jobs.
Status: active
Exercised: yes - `source_capture_tests.rs`
`a_unit_pending_behind_a_starting_kernel_publishes_on_the_next_ready_pass`
runs pass N with no Kernel open (`kernel_unavailable == 1`), opens the
Kernel, and asserts one live descriptor after pass N+1.
`a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one`,
`a_revision_lead_leaves_the_unit_pending`,
`a_delete_after_the_handoff_publishes_nothing_for_the_queued_window`, and
`shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot`
cover the deferral, retryable refusal, invalidation, and panic paths.
Guarantee: A pending unit whose input is unchanged and that the next window
carries is offered to `publish_screened` on the first later pass with the
Kernel Ready and the slot free. It publishes or becomes terminal there,
unless more than one batch of pending and unknown units precedes it or its
refusal is retryable.
Check: `always` - after Kernel Ready, a free slot, and one settled pass that
carries the unit, the unit is not pending, given at most 32 candidates and
64 KiB ahead of it. The bound is one pass, counted in passes, because the
code gives no wall-clock bound.
Fault/timing angle: Kernel Starting on pass N and Ready on pass N+1. A
refused or invalidated job records nothing, so its units stay candidates.
Required faults and enabling state: F6 (`kernel_store()` returns Starting on
one pass); F25 (a retryable revision lead); F22 (a refused window).
Confidence: high - [evidence](../evidence/sc-pending-unit-publishes-next-ready-pass.md).
I read the early `kernel_unavailable` end in `CaptureJob::publish`, the
pending-first candidate order, `classify`, the error arm of
`publish_locked`, `Memo::record`, and the tests.
Existing check: the tests named in `Exercised` (`unaudited`).
Impact: a window observed once and never captured, with no signal.
Open questions:
- Pending set per unit or per window. Resolved in U2: per unit, keyed in the
  memo, and units are re-derived from each window. Windows older than the
  submitted head are outside the guarantee (C7).
- No test sends a third pass after a slot-full refusal to observe that the
  refused window's units publish.

### sc-capture-creates-no-decision

Type: safety
Reachability: explicit-config-only - capture admissions are written only by
the consented capture job's `SourcePublisher`.
Status: active
Exercised: partial - `source_capture_tests.rs`
`captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen`
publishes user text and tool results, reopens the Kernel from disk, and
asserts nothing captured is visible to `Surface::AutoInject` and the
decision count is unchanged.
`an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss`
asserts `(TrustedToolResult, ToolUntrustedOutput)` and `Sensitive` for a
`read` output.
`a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires`
asserts `(UntrustedRepoText, RepoUntrustedText)`, `Sensitive`, and no new
decision for documents. The CI "Invented provenance" step scans the capture
modules. No test reads a captured row's provenance.
Guarantee: No capture creates a memory decision, carries a `decision_kind`,
or carries `RepositoryProvenance`. Every capture admission is
`EventKind::Other` with the fixed class pair and stores at least
`Sensitive`.
Check: `always(!X)` - the forbidden state (a capture decision, a capture
provenance, a capture classified below `Sensitive`) has no dedicated
detection point.
Fault/timing angle: none; a Kernel reopen is the state that must preserve it.
Required faults and enabling state: a published unit; a Kernel reopen (F3);
a `RepositoryDocuments` unit.
Confidence: high - [evidence](../evidence/sc-capture-creates-no-decision.md).
I read `SourcePublisher::apply` (`EventKind::Other`, no candidate),
`evidence_request`, `git_sources::commit_provenance` (`None` off
`GitCommits`), the capture publisher's `Sensitive`, the CI step, and the tests.
Existing check: the tests named in `Exercised` (`unaudited`); CI step
"Invented provenance" in `.github/workflows/ci.yml` (`unaudited`);
`surface_visibility` in `crates/kernel/src/admission.rs` (`unaudited`).
Impact: captured bytes become model-visible under a weaker policy.
Open questions:
- No row-level assertion shows captured evidence carries no provenance. U2
  deferred it as TS7.

### sc-capture-logs-carry-no-text

Type: safety
Reachability: explicit-config-only - every capture log line and counter is
emitted by the supervisor, which only consented passes reach. The report
reads zero when the key is off.
Status: active
Exercised: yes - `source_capture_tests.rs`
`the_capture_report_carries_no_unit_text` drives an identity conflict, an
adapter refusal whose session id is a sentinel, and a window too large. It
asserts that neither the report nor `health().metrics` contains the sentinel,
`msg_sentinel`, or the project digest. The `source_capture.rs` test
`log_lines_carry_only_fixed_codes_and_counts` renders every `Count` and
asserts the characters.
Guarantee: Every log line and counter the capture path emits carries a stage,
a reason code, and counts only, never unit text, paths, identities, or
digests.
Check: `always` - over every emitted record, the record consists of a
`'static` stage, a `Count` name, and integers. Every record must comply.
Fault/timing angle: error paths that would format the offending value.
`kernel_end` collapses every `KernelError` to a code, and no
`BatchRefusal` or `PublishError` is formatted.
Required faults and enabling state: each refusal and Kernel error path: F4
(identity conflict), F1 (adapter refusal), F32 (window above the supervisor
budget), F6 (Kernel unavailable or `Deadline`).
Confidence: high - [evidence](../evidence/sc-capture-logs-carry-no-text.md). I
read the `counts!` vocabulary, `log_line` (the module's only `eprintln!`),
`report`, `kernel_end`, and a grep of the capture-path modules for other
logging, plus both tests.
Existing check: the two tests named in `Exercised` (`unaudited`); `impl
std::fmt::Debug for SourceUnit` (`unaudited`; it prints identity values).
Impact: secret-bearing tool output, session ids, or paths appear in logs
outside every disclosure gate.
Open questions:
- `SourceUnit`'s `Debug` prints identity values, including a document's
  `relative_path`, so it is no guard against paths. No log formats it at HEAD.
- The `session.delete` error response formats a `KernelError` display. That
  is a response, not a capture log; its display strings were not audited.

## Existing checks

| Check | Covers | Status |
| --- | --- | --- |
| `crates/daemon/src/source_capture_tests.rs` `capture_off_memory_off_or_a_subagent_session_runs_no_adapter` | consent off, memory off, subagent: no job, tip unchanged | `unaudited` |
| `source_capture_tests.rs` `a_route_claiming_another_harness_captures_nothing` | route harness mismatch admits no job | `unaudited` |
| `crates/daemon/src/config.rs` `hostile_project_tier_cannot_change_privileged_values_and_warns_per_key`, module `source_capture_parity` | key is user-only, default false | `unaudited` |
| `source_capture_tests.rs` `the_response_is_identical_whatever_capture_and_the_kernel_are_doing` | response bytes across Ready, Starting, writer held, slot full; `refused_queue_full` retains nothing (witness `handoff_refused_slot_busy`) | `unaudited` |
| `source_capture_tests.rs` `a_spent_budget_before_the_writer_commits_nothing` | `Deadline` before the writer commits nothing; response bound | `unaudited` |
| `source_capture_tests.rs` `document_capture_leaves_the_response_and_respects_inject_docs` | document capture leaves the response unchanged | `unaudited` |
| `source_capture_tests.rs` `pi_entry_identity_on_the_row_decides_capture_and_never_the_response` | Pi response independent of row identity | `unaudited` |
| `crates/daemon/src/transform_unit/host_tests.rs` `the_host_starts_source_capture_after_the_response_settles` | transport starts capture after settlement | `unaudited` |
| `source_capture_tests.rs` `a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one` | batch unit bound; deferred unit publishes next pass | `unaudited` |
| `source_capture_tests.rs` `shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot` | slot and charges released on panic; shutdown drain | `unaudited` |
| `source_capture_tests.rs` `a_window_past_the_remaining_budget_is_refused_resident_short` | budget refusal retains nothing | `unaudited` |
| `crates/daemon/tests/harness_sources.rs` `capture_batch_defers_units_beyond_its_bounds` | 32-unit and 64 KiB batch bounds | `unaudited` |
| `harness_sources.rs` `capture_batch_refuses_an_oversized_unit_whole_and_publishes_its_siblings` | 65536 publishes; 65537 refused whole; siblings publish | `unaudited` |
| `harness_sources.rs` `capture_batch_refusals_take_no_admission_slot` | oversized unit takes no batch slot | `unaudited` |
| `source_capture_tests.rs` `unreadable_documents_are_counted_and_retire_nothing` | oversized document counted, nothing retained | `unaudited` |
| `source_capture_tests.rs` `changed_bytes_under_one_identity_are_terminal_and_never_retried` | edited message terminal, never re-offered | `unaudited` |
| `source_capture_tests.rs` `a_concurrent_publication_of_a_unit_replays_or_conflicts` | memo answers for a terminal unit | `unaudited` |
| `harness_sources.rs` `capture_batch_names_the_unit_whose_receipt_conflicts` | conflict names its unit, commits nothing | `unaudited` |
| `crates/daemon/src/harness_sources.rs` `PublishError::is_permanent` | terminal versus retryable classification | `unaudited` |
| `source_capture_tests.rs` `a_unit_pending_behind_a_starting_kernel_publishes_on_the_next_ready_pass` | pending behind Starting publishes on pass N+1 | `unaudited` |
| `source_capture_tests.rs` `a_revision_lead_leaves_the_unit_pending` | retryable unit re-offered | `unaudited` |
| `source_capture_tests.rs` `a_delete_after_the_handoff_publishes_nothing_for_the_queued_window` | invalidated window captured next pass | `unaudited` |
| `source_capture_tests.rs` `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen` | AE6: no decision, hidden from AutoInject after reopen | `unaudited` |
| `source_capture_tests.rs` `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` | AE3 class pair and `Sensitive`; replay after memo loss | `unaudited` |
| `source_capture_tests.rs` `a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` | document class pair, `Sensitive`, no decision | `unaudited` |
| `.github/workflows/ci.yml` step "Invented provenance" | no provenance construction in capture modules | `unaudited` |
| `crates/kernel/src/admission.rs` `surface_visibility` | surface visibility by class and sensitivity | `unaudited` |
| `source_capture_tests.rs` `the_capture_report_carries_no_unit_text` | report and health carry no sentinel, id, or digest | `unaudited` |
| `crates/daemon/src/source_capture.rs` test `log_lines_carry_only_fixed_codes_and_counts` | every log line is fixed characters | `unaudited` |
| `harness_sources.rs` `impl std::fmt::Debug for SourceUnit` | debug omits text (prints identity) | `unaudited` |

## Fault map rows

New ids F28 to F32 name the consent, writer, size, and budget faults these
records need. F1 to F25 have no row for them.

| fault id | injection at HEAD | availability | records |
| --- | --- | --- | --- |
| F1 missing or malformed native identity | a record naming another session (`the_capture_report_carries_no_unit_text`) | available | logs |
| F3 restart / memo loss | `SourceCapture::forget_memo_for_test`; drop handler and `KernelStore::open` | available; not paired with a terminal unit | terminal disposition; no decision |
| F4 digest input change under one key | same OpenCode id and `time.created`, changed text | available | terminal disposition; logs |
| F6 Kernel closed / `Deadline` | handler built without a Kernel; `before_publish` sleeping past `CAPTURE_BUDGET` | available | response independence; pending unit; logs |
| F22 slot busy / second pass racing | `SourceCapture::before_publish` hook (cfg(test)) blocking the first job | available | one outstanding batch; response independence; pending unit |
| F25 native timestamp ahead of daemon clock | `time.created` two hours ahead | available | pending unit |
| F28 consent predicate false at the pass (new: no existing id covers config gates) | `capture_config(false)`, `memory_enabled: false`, `is_subagent: true`, route harness mismatch | available | consent gate |
| F29 consent changed between pass and job (new: snapshot timing) | none; needs a rebind while the `before_publish` hook holds the job | constructible, no test | consent gate |
| F30 Kernel writer held by a foreground commit (new: distinct from Starting) | `KernelStore::commit` holding its closure on a thread | available | response independence |
| F31 unit text above 64 KiB beside siblings (new: size fault) | `"b".repeat(MAX_SOURCE_UNIT_BYTES + 1)` in `publish_screened`; 70 KiB document | available at the publisher seam; no transform-path test | unit cap |
| F32 window above the supervisor byte budget (new: window bound) | a 2 MiB or 9 MiB JSON string window through `SourceCapture::offer` | available | one outstanding batch; logs |

## Notes for the lead

- Slot timing, contract versus code. Ticket #1082 says the slot is acquired
  before any window reference is retained past the response.
  `capture_after_response` instead charges the window and holds `Arc`
  references from response construction until settlement, then
  `into_job` takes the slot. A refused handoff releases both at refusal and
  copies nothing, so the record's check reads "retained 0 bytes once
  refused".
- One job runs up to two serial `publish_screened` batches: the window's,
  then the documents' (`capture_documents`). R14's "one outstanding batch"
  holds per slot, not per job.
- Liveness bound qualification. "First later pass" holds only when at most 32
  candidates and 64 KiB precede the unit and the next window carries it. A
  retryable refusal (`RevisionAhead`) stays pending across passes. The record
  states both conditions.
- Terminal disposition after memo loss. Any `session.delete` empties the
  whole memo (`invalidate_sessions`), as do restart, eviction, and poison. A
  terminal unit then re-probes, fails its batch with `Conflict`, and delays
  its siblings one pass. No test covers this.
- `impl std::fmt::Debug for SourceUnit` prints identity values, including
  `relative_path` for documents. The source record listed it as the existing
  check for "no paths", but it does not meet that clause. No log formats it
  at HEAD.
- Missing checks: a byte comparison under `Deadline`; a transform-path
  oversize unit; a row-level provenance-null assertion (U2 deferral TS7); a
  third pass after a slot-full refusal; a consent reload between pass and job.
- Moved baseline citations: `hs.rs:90-102`, `:141-154`, `:436-458`,
  `:631-698`, `:758-770`, `:903`, `:911-915`, `:960-968`, and
  `lib.rs:1826-1834` no longer match. The records name `SourceUnit`'s
  `Debug`, `AdapterRefusal`, `is_permanent`, `prepare`, `apply`, and
  `publish_screened` instead. `memory_capture.rs` `capture_enabled` (570) and
  the subagent skip (694) still match. The batch-wide unnamed prepare failure
  the baseline cited no longer exists.
- OQ resolutions verified in ticket and code: OQ15 (U1, two switches), OQ5
  (U2, no minted revision), OQ20 (U2, `ReAdmissionBlocked` terminal). OQ19
  stays open (needs human input).

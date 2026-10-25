# Loss conditions and fail-closed dispositions

This file mirrors two closed lists from the corpus module and records how the
document reader and the capture pre-screen fail closed at HEAD. Code
references are verified at `41d889ea4`.

## Loss-condition enumeration

Source: `LossCondition` in `crates/daemon/src/source_corpus.rs:596-623`. That
enumeration is authoritative (OQ22d, resolved in #1106). This table mirrors it
in declaration order; a difference between the two is a review finding, and a
change to the enumeration after a measurement run invalidates the run (#1109).
The module's `closed_names!` macro (`source_corpus.rs:548-571`) gives each
variant one snake-case name and lists every variant once in `ALL`.

| Name | Variant | What it accounts for | Daemon counter or adapter step |
| --- | --- | --- | --- |
| `adapter_refused` | `AdapterRefused` | the adapter refused the unit's whole native record | `adapter_refused`, added once per refused record by `CaptureJob::units` (`source_capture.rs:1125-1155`, the add at `:1150-1153`) |
| `unit_too_large` | `UnitTooLarge` | the unit exceeds the per-unit byte cap | `units_unit_too_large`, from `UnitDisposition::UnitTooLarge` (`harness_sources.rs:1542-1545`, `source_capture.rs:1204`) |
| `deferred` | `Deferred` | the unit was still deferred after the last replayed pass | `units_deferred` (`source_capture.rs:1205`) |
| `secret_bearing` | `SecretBearing` | the Kernel refused the unit as secret-bearing | `units_refused_terminal`, from `exact_text_refusal` in the pre-screen (`harness_sources.rs:1590-1593`) |
| `re_admission_blocked` | `ReAdmissionBlocked` | the Kernel refused the unit's artifact as re-admission blocked | `units_refused_terminal`, from the pre-screen's re-admission check (`harness_sources.rs:1594-1599`); a purge committed between the screening preview and the commit reaches batch ingest, which reports `kernel_error` for that pass (below) |
| `refused_retryable` | `RefusedRetryable` | the unit was refused with a retryable cause on the last pass | `units_refused_retryable` (`source_capture.rs:1212`) |
| `identity_conflict` | `IdentityConflict` | the unit's identity held a different digest | `units_identity_conflict` (`source_capture.rs:1206-1208`, and `:851-854` for the unit a batch conflict names) |
| `superseded` | `Superseded` | a later revision of the same identity replaced the unit | `units_superseded` (`source_capture.rs:1213`) |
| `window_refused` | `WindowRefused` | the window was refused before any unit was read | `refused_queue_full`, `refused_resident_short`, `refused_window_too_large`, `refused_shutting_down` |
| `pass_failed` | `PassFailed` | the pass ended before publication | `kernel_error`, `deadline`, `kernel_unavailable`, `worker_panicked`, `discarded`, `session_invalidated` |

The replay oracle maps a screened disposition to a condition in
`settled` (`crates/daemon/tests/support/corpus_replay.rs:245-270`): the four
secret-scan artifact kinds map to `secret_bearing`, `ReAdmissionBlocked` to
`re_admission_blocked`, `IdentityReused` and descriptor refusals to
`identity_conflict`, and any other permanent refusal to an unnamed outcome
that the reconciliation reports as unclassified. A deferred unit, or one whose
last refusal is retryable, stays open for the next pass and becomes `deferred`
or `refused_retryable` only after the last pass.

## Non-unit reasons

Source: `NonUnitReason` in `source_corpus.rs:573-594`. These classify native
content that is never a unit, so it is neither admitted nor lost.

| Name | What it classifies |
| --- | --- |
| `non_text_part` | an OpenCode part whose kind is neither `text` nor `tool` |
| `ignored_text` | an OpenCode text part marked `ignored` |
| `unsettled_text` | OpenCode assistant text of a message without `time.completed` |
| `unsettled_tool` | an OpenCode tool part still `pending` or `running`, or interrupted in an incomplete turn |
| `non_text_item` | a Pi `toolResult` holding a non-text item; the whole record yields no unit |
| `skipped_item` | a Pi user or assistant content item other than `text` |
| `non_message_entry` | a Pi entry whose `type` is not `message` |
| `uncaptured_role` | a Pi message role other than `user`, `assistant`, and `toolResult` |

## Document reader dispositions at HEAD

The user-tier `project_docs.files` list names the documents
(`crates/daemon/src/config.rs:1300`), admitted by `admit_document_paths`
(`crates/daemon/src/project_docs.rs:64`) and defaulting to
`DEFAULT_PROJECT_DOC_FILES` (`project_docs.rs:16`). `read_document`
(`project_docs.rs:196`, through `read_document_with` at `:205`) opens the
parent and then the final component with `openat2` under `RESOLVE`
(`project_docs.rs:29`: beneath the root, no symlinks, no magic links, no mount
crossing) and checks the type, size, and UTF-8 on the opened descriptor.
`plan_document` (`source_capture.rs:1037`) maps each read to one capture step.

| Read | Arm | Capture step |
| --- | --- | --- |
| `Text` within the unit cap | `project_docs.rs:261` | publish head plus one, or `documents_unchanged` when the live digest matches |
| `Text` past the unit cap | `plan_document` | skip, `documents_oversize` |
| `Missing` | `project_docs.rs:240`: `ENOENT` on the final component in a parent that resolved beneath the root | retire the live capture-domain row, else `documents_missing` |
| `Unreadable` | `project_docs.rs:230` (the parent is absent, a symlink, across a mount, or unopenable), `:242` (any other final-component failure, a kernel without `openat2` included) | skip, `documents_unreadable` |
| `NotRegular` | `project_docs.rs:241` (`ELOOP`: a final-component symlink), `:248` (the descriptor is not a regular file) | skip, `documents_not_regular` |
| `Oversize` | `project_docs.rs:251` | skip, `documents_oversize` |
| `NotUtf8` | `project_docs.rs:261` | skip, `documents_not_utf8` |

A path that leaves the list is neither read nor retired.

Every arm except `Text` and `Missing` skips and retires nothing. The identity
is `[("project_id", P), ("repository_id", R), ("relative_path", name)]`
(`source_capture.rs:1057-1061`), with `R` the derived repository identity
(`crates/daemon/src/repository_identity.rs:62`) or `P` as its fallback. A
derivation the environment cuts short refuses every read as
`documents_refused`. A path whose read settles its current lineage first
retires every live capture-domain row under another `repository_id`
(`source_capture.rs:1435`, retired at `:921`).

## Capture pre-screen dispositions at HEAD

`publish_screened` (`harness_sources.rs:1512`) decides each unit inside one
screening preview (`harness_sources.rs:1537-1615`, through
`publish_screened_between` at `:1522`) before the batch reaches the Kernel, in
this order:

| Check | Location | Disposition | Counter (`classify`, `source_capture.rs:1195-1215`) |
| --- | --- | --- | --- |
| text past `MAX_SOURCE_UNIT_BYTES` | `harness_sources.rs:1542-1545` | `UnitTooLarge` | `units_unit_too_large`, terminal |
| batch already full | `:1546-1549` | `Deferred` | `units_deferred`, pending |
| `prepare` refuses identity or revision | `:1550-1556` | `Refused(refusal)` | terminal when `is_permanent` (`:573-592`), else retryable |
| scope refuses a project unit | `:1557-1571` | `Refused(Kernel NotFound)` | `units_refused_retryable` |
| second unit of an admitted lineage | `:1573-1582` | `Refused(IdentityReused)` when key equal and digest differs, else `Deferred` | `units_identity_conflict` or `units_deferred` |
| batch unit or byte bound reached | `:1583-1589` | `Deferred` | `units_deferred` |
| secret-bearing text | `:1590-1593` | `Refused(Artifact(kind))` | `units_refused_terminal` |
| no stored descriptor receipt and a purge tombstone or pending unlink names the payload digest | `:1594-1599`, through `readmission_blocked` (`:1160-1172`) and `Preview::artifact_readmission_blocked` (`crates/kernel/src/cas/ingest.rs:1470-1475`) | `Refused(Artifact(ReAdmissionBlocked))` | `units_refused_terminal` |

A refused unit takes no admission slot. A unit with a stored descriptor
receipt skips the re-admission check and replays. A digest whose only state is
a reclaiming reservation passes the check; batch ingest then refuses the whole
batch with `KernelError::Conflict` naming no unit
(`crates/kernel/src/cas/ingest.rs:822-827`; the batch contract at `:693`), the
pass counts `kernel_error` (`source_capture.rs:1476-1481`), every keyed unit
stays pending (`source_capture.rs:845-870`), so the reclaiming unit's siblings
wait with it, and the units publish once reclamation finishes. A purge committed after the screening preview and before
the commit takes the same path for one pass; the next pass's screening preview
refuses the purged unit and its siblings publish. The test seam between the two
is `SourceCapture::between_screen_and_commit` (`source_capture.rs:247-255`).

# Loss conditions and fail-closed dispositions

This file mirrors two closed lists from the corpus module and records how the
document reader and the capture pre-screen fail closed at HEAD. Code
references were verified at `41d889ea4` and re-checked at `aff5a00e6`, the
tip of the #1109 branch this part's branch starts from; the ranges in
`source_capture.rs` and `corpus_replay.rs` that moved between the two are
updated here.

## Loss-condition enumeration

Source: `LossCondition` in `crates/daemon/src/source_corpus.rs:596-623`. That
enumeration is authoritative (OQ22d, resolved in #1106). This table mirrors it
in declaration order; a difference between the two is a review finding, and a
change to the enumeration after a measurement run invalidates the run (#1109).
The module's `closed_names!` macro (`source_corpus.rs:548-571`) gives each
variant one snake-case name and lists every variant once in `ALL`.

| Name | Variant | What it accounts for | Daemon counter or adapter step |
| --- | --- | --- | --- |
| `adapter_refused` | `AdapterRefused` | the adapter refused the unit's whole native record | `adapter_refused`, added once per refused record by `CaptureJob::units` (`source_capture.rs:1158-1188`, the add at `:1183-1186`) |
| `unit_too_large` | `UnitTooLarge` | the unit exceeds the per-unit byte cap | `units_unit_too_large`, from `UnitDisposition::UnitTooLarge` (`harness_sources.rs:1557-1560`, `source_capture.rs:1237`) |
| `deferred` | `Deferred` | the unit was still deferred after the last replayed pass | `units_deferred` (`source_capture.rs:1238`) |
| `secret_bearing` | `SecretBearing` | the Kernel refused the unit as secret-bearing | `units_refused_terminal`, from `exact_text_refusal` in the pre-screen (`harness_sources.rs:1609-1612`) |
| `re_admission_blocked` | `ReAdmissionBlocked` | the Kernel refused the unit's artifact as re-admission blocked | `units_refused_terminal`, from the pre-screen's re-admission check (`harness_sources.rs:1613-1619`, answered at `:1635-1648`); a purge committed between the screening read and the commit reaches batch ingest, which reports `kernel_error` for that pass (below) |
| `refused_retryable` | `RefusedRetryable` | the unit was refused with a retryable cause on the last pass | `units_refused_retryable` (`source_capture.rs:1245`) |
| `identity_conflict` | `IdentityConflict` | the unit's identity held a different digest | `units_identity_conflict` (`source_capture.rs:1239-1241`, and `:870-873` for the unit a batch conflict names) |
| `superseded` | `Superseded` | a later revision of the same identity replaced the unit | `units_superseded` (`source_capture.rs:1246`) |
| `window_refused` | `WindowRefused` | the window was refused before any unit was read | `refused_queue_full`, `refused_resident_short`, `refused_window_too_large`, `refused_shutting_down` |
| `pass_failed` | `PassFailed` | the pass ended before publication | `kernel_error`, `deadline`, `kernel_unavailable`, `worker_panicked`, `discarded`, `session_invalidated` |

The replay oracle maps a screened disposition to a condition in
`settled` (`crates/daemon/tests/support/corpus_replay.rs:263-286`): the four
secret-scan artifact kinds map to `secret_bearing`, `ReAdmissionBlocked` to
`re_admission_blocked`, `IdentityReused` to `identity_conflict`, and any
other permanent refusal, a descriptor refusal included, to an unnamed outcome
that the reconciliation reports as unclassified (`corpus_replay.rs:282-283`;
`crates/daemon/tests/source_corpus_replay.rs:585`
`a_refused_outcome_record_is_unclassified_loss` constructs one). This matches
`classify` (`source_capture.rs:1239-1241`), which counts only
`IdentityReused` as `units_identity_conflict`. A deferred unit, or one whose
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

`read_document` (`crates/daemon/src/project_docs.rs:113-133`) reads one of the
fixed names in `PROJECT_DOC_FILES` (`project_docs.rs:9`) through
`read_bounded_bytes` (`config.rs:688`), which opens with
`O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC` and checks `is_file` on the
opened descriptor (`config.rs:696-719`). `plan_document`
(`source_capture.rs:1070-1122`) maps each read to one capture step.

| Read | Arm | Capture step | Revised by |
| --- | --- | --- | --- |
| `Text` within the unit cap | `project_docs.rs:115` | publish head plus one, or `documents_unchanged` when the live digest matches (`source_capture.rs:1107-1121`) | U8 (#1113) changes the name list |
| `Text` past the unit cap | `source_capture.rs:1079-1081` | skip, `documents_oversize` | none |
| `Missing` | `project_docs.rs:117-122`: `NotFound` and the parent's followed metadata is a directory | retire the live capture-domain row, else `documents_missing` (`source_capture.rs:1100-1106`) | U8 (#1113): beneath-root resolution must cover this classification, because `std::fs::metadata` follows a symlinked parent |
| `Unreadable` | `project_docs.rs:124`, `:130` | skip, `documents_unreadable` | U8 (#1113) |
| `NotRegular` | `project_docs.rs:128`: `symlink_metadata` is not a regular file | skip, `documents_not_regular` | U8 (#1113): `openat2` with beneath-root flags replaces the final-component `NOFOLLOW` |
| `Oversize` | `project_docs.rs:129` | skip, `documents_oversize` | none |
| `NotUtf8` | `project_docs.rs:115` | skip, `documents_not_utf8` | none |

Every arm except `Text` and `Missing` skips and retires nothing. The identity
is `[("project_id", P), ("repository_id", R), ("relative_path", name)]`
(`source_capture.rs:1090-1094`), with `R` the derived repository identity
(`crates/daemon/src/repository_identity.rs:62`) or `P` as its fallback. A
`.git` entry at the project root is opened directly, so a broken entry there
fails the derivation even inside an enclosing repository
(`repository_identity.rs:73`, `repository_identity_tests.rs:116`). A
derivation the environment cuts short refuses every read as
`documents_refused`. A path whose read settles its current lineage first
retires every live capture-domain row under another `repository_id`
(`source_capture.rs:1477`, retired at `:941`). The path's database
incarnation, project, and name are then recorded in `swept_paths`
(`source_capture.rs:241`, recorded at `:950`), and later passes of the daemon
incarnation skip the path's inventory, which scans every live
`RepositoryDocuments` descriptor of the store
(`source_capture_tests.rs:3022`).

A build that keys documents as `(P, P, path)` publishes beside a `(P, git:R,
path)` row and leaves it live, so running such a build against a database
this capture has written leaves two live rows for the path. The next daemon
incarnation of this capture retires the `(P, P, path)` row on its first pass
that settles the path and publishes the current text under `git:R`
(`source_capture_tests.rs:3051`). Until then, both rows are retrievable.

## Capture pre-screen dispositions at HEAD

`publish_screened` (`harness_sources.rs:1520`) decides each unit in the
screening loop (`harness_sources.rs:1551-1671`, through
`publish_screened_between` at `:1530`) before the batch reaches the Kernel, in
this order. The loop prepares and scans units with no Kernel reader held; it
reads the scope in one preview (`:1573-1579`) and the admitted units' receipts
and re-admission states together in another (`:1635-1645`). When that read
blocks a unit, screening resumes at the first blocked unit with its admission
slot released (`:1649-1670`).

| Check | Location | Disposition | Counter (`classify`, `source_capture.rs:1228-1248`) |
| --- | --- | --- | --- |
| text past `MAX_SOURCE_UNIT_BYTES` | `harness_sources.rs:1557-1560` | `UnitTooLarge` | `units_unit_too_large`, terminal |
| batch already full | `:1561-1564` | `Deferred` | `units_deferred`, pending |
| `prepare` refuses identity or revision | `:1565-1571` | `Refused(refusal)` | terminal when `is_permanent` (`:573-592`), else retryable |
| scope refuses a project unit | `:1572-1590` | `Refused(Kernel NotFound)` | `units_refused_retryable` |
| second unit of an admitted lineage | `:1592-1601` | `Refused(IdentityReused)` when key equal and digest differs, else `Deferred` | `units_identity_conflict` or `units_deferred` |
| batch unit or byte bound reached | `:1602-1608` | `Deferred` | `units_deferred` |
| secret-bearing text | `:1609-1612` | `Refused(Artifact(kind))` | `units_refused_terminal` |
| no stored descriptor receipt, no evidence receipt under another request, and a purge tombstone or pending unlink names the payload digest | `:1613-1619`, through `readmission_blocked` (`:1160-1180`) and `Preview::artifact_readmission_blocked` (`crates/kernel/src/cas/ingest.rs:1470-1475`) | `Refused(Artifact(ReAdmissionBlocked))` | `units_refused_terminal` |

A refused unit takes no admission slot. A unit with a stored descriptor
receipt skips the re-admission check and replays. A unit whose descriptor or
evidence receipt holds another request passes the check, and the batch preview
refuses it as an identity conflict (`harness_sources.rs:1294`, through
`conflicting_unit` at `:1491`). A digest whose only state is
a reclaiming reservation passes the check; batch ingest then refuses the whole
batch with `KernelError::Conflict` naming no unit
(`crates/kernel/src/cas/ingest.rs:822-827`; the batch contract at `:693`), the
pass counts `kernel_error` (`source_capture.rs:1517-1522`), every keyed unit
stays pending (`source_capture.rs:864-889`), so the reclaiming unit's siblings
wait with it, and the units publish once reclamation finishes. A purge committed after the screening read and before
the commit takes the same path for one pass; the next pass's screening read
refuses the purged unit and its siblings publish. The test seam between the two
is `SourceCapture::between_screen_and_commit` (`source_capture.rs:247-249`).

# p5dep-harness-sweep-retires-owners-before-evidence

HEAD `3a3b916b7`. Surface S6.

## Discovery trigger

Plan R4: a P5 sweep "drops rows ... whose descriptor is retired". P4 C6/C7:
`dispute` requires live evidence; a retired descriptor's evidence makes the
confirm `not_found`. `crates/kernel/src/schema.rs:135`: `harness_capture`
rows carry a finite `retain_until` (30 days) and "each has a Kernel
maintenance sweep that retires its expired, unpinned rows, owners first and
evidence second, and keeps evidence another live row cites." Lenses:
lifecycle transitions; data integrity; C12; L1.

## Evidence trail

- `crates/kernel/src/cas/mod.rs:155` `HARNESS_CAPTURE_RETENTION_CLASS =
  "harness_capture"`.
- `crates/daemon/src/harness_sources.rs:607-608`
  `HARNESS_CAPTURE_RETENTION_MS = CAPTURE_REPLAY_RETENTION_MS` (30 days,
  `crates/memory-store/src/memory_capture.rs:36`); `:991` doc: captured
  message text and tool results are `harness_capture` evidence owned for that
  long from `observed_at`; the deadline is set only when the evidence row is
  first created, so a replay keeps the first deadline.
- `crates/kernel/src/local_file.rs:26` `MAX_EXPIRED_CAPTURES_PER_CALL = 64`.
- `local_file.rs:28-43` `CaptureExpiry { retired, retained }`.
- `local_file.rs:46-70` `ExpiringClass`; `HARNESS_CAPTURES { retention_class:
  "harness_capture", owner_kind: SOURCE_DESCRIPTOR_KIND, producer:
  "harness-capture-expiry", cause: "harness capture retention expired",
  captures_per_commit: 64 }`. `SOURCE_DESCRIPTOR_KIND = "source_descriptor"`
  (`crates/kernel/src/source_descriptor.rs:30`).
- `local_file.rs:240-247` `expire_harness_captures(now)` doc and body.
- `local_file.rs:250-310` `expire_captures`: candidates under a reader
  (`:255-258`); per page under the writer: first capture via
  `retire_expired_capture` (recheck or `Conflict`), later captures via
  `still_expired` (skip if pinned) then `retire_capture` and a reserved
  receipt (`:272-284`); outcome handling: replayed receipt -> drop first;
  Ok -> drain page; `NotFound|Conflict|InvalidInput` -> remove the refused
  index and loop (`:304-306`); other errors propagate.
- `local_file.rs:447-458` `retire_expired_capture`; `:461-478`
  `still_expired` (reads `EXPIRED_UNPINNED_SQL`); `:481-495`
  `retire_capture`: `for observation in live_capture_observations(owner_kind,
  evidence_id) { retire_capture_observation }` then `if cited_elsewhere {
  "retained" } else { retire_evidence; "retired" }`.
- `local_file.rs:508-516` `EXPIRED_UNPINNED_SQL`: class match, live,
  `retain_until <= now`, no live unexpired pin.
- `local_file.rs:518-559` `expired_captures` and its SQL: live evidence
  object, and either a live owner observation exists or the evidence is not
  cited; `ORDER BY e.retain_until, e.evidence_id LIMIT 64`.
- `crates/kernel/src/slice/write.rs:569-577` `retire_capture_observation` ->
  `retire_slice_object(.., "observation", "observations")` (`:627-666`):
  `invalidate(tx, commit_seq, table, id)` and `object.invalidated_commit_seq
  = Some(commit_seq)`.
- Caller: `crates/daemon/src/memory_reviewer/lifecycle.rs:313-333`
  `kernel_slice`: `expire_local_file_captures` then
  `expire_harness_captures`, each guarded by `cancelled()`; `:358-430`
  `sweep_and_sample`; `:436-492` `run` loop with `SAMPLE_INTERVAL` 30 s and
  `SAMPLE_RETRY_INTERVAL` 5 s when a pass advanced (`:16-18`); spawned at
  `crates/daemon/src/lib.rs:3977-3985` once the store is installed, with the
  Kernel joining "only while it is ready" (`:3976`).
- Tests: `local_file.rs:791`
  `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept`; `:889`
  `a_refused_harness_capture_rolls_back_alone_inside_its_page_commit`; `:704`,
  `:722` receipt tests; `:1132` candidate cost; `lifecycle.rs:699`
  `the_maintenance_slice_expires_harness_captures_and_reports_progress`.

## Failure scenario

Evidence retired before its descriptors: `retire_evidence` would refuse
`Conflict` because the descriptors still cite it (`write.rs:606-608`), so the
order is enforced by the Kernel, not only by the sweep's sequence. If the
sweep skipped descriptors and retired only evidence, descriptors would cite
retired evidence; `kernel.read` of the `harness-capture` domain would show
live descriptors whose text is gone. For P5: a candidate row keyed on the
descriptor sees `invalidated_commit_seq` set after the sweep; one keyed on
the evidence sees it set unless the evidence is cited elsewhere (next
record).

## Timing windows and dependencies

Candidate selection under a reader, retirement under the writer; a pin or
citation added in between is seen by the writer recheck. Pages of 64; the
loop interval decides latency to retirement: with `pending_ahead` expired
captures older than a given one, retirement takes about
`ceil(pending_ahead / 64)` advancing passes at 5 s each.

## What a test must construct

- Publish descriptors through `SourcePublisher`/`publish_batch` with an
  `observed_at` 30 days in the past (or call `expire_harness_captures(now)`
  with `now` advanced); assert descriptor and evidence
  `invalidated_commit_seq` equal the sweep commit.
- 65 expired captures; assert one call retires 64 and the next retires 1.
- One capture pinned between selection and recheck (existing).

## Investigation log

### Q: L1. Should the bounded retirement latency be its own liveness record?

- Sources examined: `lifecycle.rs:16-18`, `:483-491` (interval choice),
  `local_file.rs:26`.
- Findings: the bound is expressible (pages of 64, 5 s retry while
  advancing, 30 s otherwise) and would need a fault-free window with the
  loop running against a real Kernel. No test measures it.
- Missing evidence: whether P5 depends on retirement latency (it depends on
  the eventual state, not the latency, as far as the plan says).
- Conclusion: needs human input; kept as an open question on this record.

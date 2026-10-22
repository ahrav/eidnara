# sc-expiry-descriptor-first-cited-survives

## Discovery trigger

Part B obligation from #1066 comment 2, decision D6 and constraint C11. The
U4 ticket (#1084) names the record with faults F12 (a pin acquired between
select and commit) and F13 (evidence cited by an independent row), and the
AC11 clause that cited or held evidence survives while canonical and git
evidence are untouched.

## Evidence trail

`crates/kernel/src/local_file.rs` - `KernelStore::expire_harness_captures`
calls `expire_captures(&HARNESS_CAPTURES, now)`. `HARNESS_CAPTURES` is an
`ExpiringClass` with `retention_class = HARNESS_CAPTURE_RETENTION_CLASS`,
`owner_kind = SOURCE_DESCRIPTOR_KIND` (`"source_descriptor"`), and the
reserved producer `harness-capture-expiry`. `expire_captures` reads candidates
under `lock_reader` through `expired_captures`, then commits each one in its
own `commit_with_writer` transaction keyed by `expiry_intent` (`{now}:{id}`
under `CommitIntent::RESERVED_PRODUCER_PREFIX`).

`retire_expired_capture` runs inside that transaction. It first re-evaluates
`EXPIRED_UNPINNED_SQL` for the one evidence id and returns `Conflict` when the
row is no longer expired or a live pin now covers it. It then retires every
live observation of `owner_kind` citing the evidence
(`live_capture_observations`, through `Envelope::retire_capture_observation`,
the path `retire_observation` takes for ids outside the local-file
namespace). It then asks `cited_elsewhere`, which evaluates
`EVIDENCE_CITED_SQL` (live observations, decisions, decision events, asserted
edges); a cited row returns `"retained"` with the evidence live. Otherwise it
calls `retire_evidence`, which re-checks the same predicate and refuses
`Conflict` while cited (`crates/kernel/src/slice/write.rs`,
`retire_evidence_inner`).

`EXPIRED_UNPINNED_SQL` matches `e.retention_class=?1` exactly, a live row, a
non-null `retain_until <= now`, and no `capture_pin_refs` row joined to an
unreleased, non-degraded, unexpired `capture_pins` row. Canonical rows carry
`retain_until: None` and another class string, so neither predicate arm
selects them.

Landed checks:
`crates/daemon/tests/harness_sources.rs::harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it`
asserts the expiry commit's `change_event` kinds are
`["observation_retire", "evidence_retire"]`, the descriptor's
`superseded_by` is `None`, the evidence's `invalidated_commit_seq` equals the
descriptor's, and the `git_commit` and repository-document rows stay
`("canonical", None, None)` after a sweep at `NOW + 32 d`.
`crates/daemon/tests/harness_sources.rs::harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages`
cites the first capture in page order through an independent `note`
observation (`cite`), takes a live `capture_source_hold` over every capture,
asserts a sweep retires nothing while the hold lives, and after the hold
lapses asserts `(63, 1)`: the cited capture loses its descriptor and keeps its
evidence. After `retire_observation` of the citer, a later sweep retires it.
`crates/kernel/src/local_file.rs::a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept`
selects the row with `expired_captures`, inserts a `capture_pins` and
`capture_pin_refs` row inside the writer transaction, and asserts
`retire_expired_capture` returns `Conflict`; the MemoryReviewer sweep in the
same store retires only its own capture; the unpinned commit returns
`"retired"`.

## Failure scenario

A sweep that retires evidence before its descriptors leaves a live
descriptor naming bytes that no longer exist, or trips `retire_evidence`
forever. A sweep that trusts the reader's candidate list retires evidence a
run pinned between select and commit. A sweep that matches the class by
prefix or skips the citation check removes canonical claims or evidence a
decision still cites.

## Timing windows and dependencies

The window is between `expired_captures` under the reader lock and the
per-capture writer commit. A hold or citation landing in that window is seen
by the writer recheck, because the recheck and the retirement share one
transaction. A pin taken after the commit has no effect on a row already
retired. A hold's own expiry (`p.expires_at > now`) is part of the predicate,
so a lapsed hold stops protecting the row at the sweep's `now`.

## What a test must construct

An expired `harness_capture` row with a live descriptor, a pin inserted
between selection and the writer commit, an independent observation citing
the evidence, and a `canonical` row and a git row in the same store. Assert
for every retired evidence row that its commit also retired its descriptors
and that no live citer or live pin existed at that commit. The legal witness
is a capture shared by a held descriptor that is skipped and retired on a
later sweep. Coverage witnesses `expired_capture_pinned_mid_sweep` and
`expired_capture_cited_elsewhere` mark the two preconditions; the lead names
them in parallel, and they were not present in the files at the HEAD read
for this record.

## Investigation log

### Q: Does the sweep retire descriptors through `retire_observation`?
- Sources examined: `ticket-1084.md` Scope; `retire_expired_capture`;
  `Envelope::retire_observation` and `retire_capture_observation`.
- Findings: the sweep calls `retire_capture_observation`;
  `retire_observation` forwards to it after refusing local-file ids.
- Missing evidence: none.
- Conclusion: resolved with answer - same retirement path; the ticket wording
  names the public wrapper.

### Q: Can a descriptor be retired in an earlier commit than its evidence?
- Sources examined: `retire_expired_capture`; `expired_captures_sql`.
- Findings: in the `retained` case the descriptors retire in one commit and
  the evidence in a later sweep after the citation goes; the candidate query
  re-admits the row only once it is uncited.
- Missing evidence: none.
- Conclusion: resolved with answer - the record's "that or an earlier commit"
  clause covers this order.

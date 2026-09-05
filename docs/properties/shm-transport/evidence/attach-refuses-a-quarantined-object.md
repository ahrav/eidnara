# attach-refuses-a-quarantined-object

## Discovery trigger

The catalog recorded this at medium confidence as a lead: the reported basis was
that `validate_lifecycle`'s snapshot tuple omits the `quarantined` field, but no
one had re-read the tuple directly. This file resolves that by direct read. The
lens is state-reachability: quarantine is documented as terminal, so every
entry point that creates a usable handle should observe it.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:2813-2831` is
  `validate_lifecycle`. Its snapshot reads exactly eight fields with
  `read_volatile` at `:560-567`, in order: `magic`, `layout_version`,
  `descriptor_depth`, `arena_bytes`, `max_leases`, `total_bytes`, `incarnation`,
  `lane`. The equality check at `:2819-2827` compares those same eight and
  returns `RingError::InvalidGrant` at `:2828`. **`quarantined` is not read and
  not compared.** This resolves the catalog's open question and lifts the basis
  from reported to verified (line numbers re-verified at post-#131 HEAD: the
  field reads are `:560-567` and the comparison is `:2819-2827`).
- In the source tree, `ring.rs:783-798` was `Ring::attach`: it computed the
  layout, converted `total_bytes`, mapped, called `validate_lifecycle`, and
  returned the `Ring` wiring the two transferred doorbells, with no quarantine
  check on the path; the only `is_quarantined` call sites were per-operation.
  At HEAD that gap is closed: `Ring::attach` (`ring.rs:1095-1150`) checks
  `ring.is_quarantined()` at `:1141-1143` and returns `RingError::Quarantined`
  before handing the ring out, and `attach_refuses_a_quarantined_ring`
  (`:3696-3702`) pins it. The record stands as a regression contract on that
  gate.
- `ring.rs:218` shows `quarantined: AtomicU8` is a `LifecyclePage` field, so it
  is present in the very page `validate_lifecycle` reads. The omission is a
  choice of fields, not an absence of data.
- `ring.rs:2797-2810` initializes a fresh lifecycle page and sets
  `quarantined: AtomicU8::new(0)` at `:2808`, so the field is meaningful from
  creation onward.
- `packages/shm-native/src/lib.rs:736-739` claims the process-wide grant
  reservation with `GrantReservation::claim`, and `:740-741` attaches the two
  rings afterwards. Ordering matters: the claim is consumed **before** the
  mapping is validated, so a failing attach releases it through
  `GrantReservation::drop` (`lib.rs:122-130`, removing both grants at `:126`)
  while a succeeding attach retains it.
- `lib.rs:747-762` inserts the `Channel` with `_reservation: Some(reservation)`
  (field declared at `lib.rs:89`), so the claim outlives attach and is held by
  the registry entry.
- `lib.rs:1545-1561` (`close`) and `:1578-1594` (`force_close`) remove the registry
  entry at `:1573` (both `close` and `force_close` route through `finish_close`), but only under the condition at
  `:1564-1566`: `channel.producers.is_empty() && channel.active
  .is_empty() && channel.stranded.is_empty()`. **Correction:** the catalog's
  impact claim that the grant reservation is "held for the process lifetime" is
  too strong. An alias-free quarantined channel can be closed and its claim
  released. The claim is pinned indefinitely only when a detach already failed
  and left entries in `stranded`, which is the same condition that raised
  quarantine in the first place (`lib.rs:301-302`).
  At HEAD: Attach also runs conservation_inner(true) after the quarantine gate (`:1148`), so a mapping whose cursors already disagree with its slots is refused too.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

1. A detach failure or a receive-validation failure sets the flag. The two
   reachable triggers are `packages/shm-native/src/lib.rs:301` and
   `ring.rs:1401`.
2. A reconnect or worker restart re-derives the same grant and calls
   `Ring::attach` (`ring.rs:1095`), directly or through `RingAttachment::attach`
   (`ring.rs:978-980`).
3. `validate_lifecycle` compares its eight fields, all of which still match, and
   returns `Ok(())`. A `Ring` is returned.
4. On the addon path the grant claim taken at `lib.rs:736` is now consumed and a
   channel id is issued at `lib.rs:747-762`. The caller receives a success.
5. Every subsequent operation fails: `try_reserve` returns
   `ProducerError::Quarantined` (`ring.rs:1275-1277`), `try_receive` returns
   `RingError::Quarantined` (`:1396-1398`), `release` returns
   `LeaseError::Quarantined` (`:1529-1531`), and `probe` returns
   `RingError::Quarantined` (`:1888-1890`).
   At HEAD: Attach refuses a flagged object at `:1141-1143`, so this scenario stops at step 2 and never returns a usable Ring.
   At HEAD: Descriptor validation returns RingError::Descriptor and the try_receive wrapper quarantines through quarantine_with, which is one of many quarantine_with call sites rather than a single inline enter_quarantine.

The consequence is a misleading success return plus a channel that can never do
work. If the original quarantine came from a failed detach, the surviving
`stranded` entries also block the registry cleanup that would release the grant
claim.

## Timing windows and dependencies

No timing window is involved. The property is a missing check on a synchronous
path, so it holds or fails deterministically for any attach against a flagged
object. It depends on `quarantine-authority-survives-peer-writes` only in the
sense that both assume the flag is trustworthy. Platform framing changed with
PR #131: the former Linux-only `/proc`-based descriptor transfer (pre-rewrite
`ring.rs:497-505`) is gone, and `RingAttachment` (`ring.rs:971-980`) now carries
already-owned descriptors with no `cfg(target_os)` gate of its own; the
remaining platform-specific code is confined to object creation and sealing
(`ring.rs:2109-2185` (source tree; not at HEAD) cfg arms).
At HEAD: ring.rs has no platform-specific cfg at HEAD, so there is no remaining platform-gated code to point at.

## What a test must construct

Create a ring, publish nothing, call `ring.enter_quarantine()`, then obtain the
grant with `ring.grant()` and duplicates of the three descriptors, and call
`Ring::attach(descriptors, grant)` (post-#131 signature, `ring.rs:1095`). Assert the attach returns `Err`. On the
addon side, drive the same sequence through `force_close` and a re-open of the
same descriptor pair, and assert both that no channel id is issued and that
`ACTIVE_GRANTS` (`lib.rs:98`) does not contain the grant afterwards. Reading
`ACTIVE_GRANTS` requires a test hook; the addon exposes channel counts but the
grant set is a private static, so the observable proxy today is the channel
count plus a second attach attempt expecting "shared-memory descriptor is
already attached" (`lib.rs:112`).

## Investigation log

### Q: Confirm by direct read that `validate_lifecycle` does not read `quarantined`.

- Sources examined: `ring.rs:2813-2831` read in full, including the eight
  `read_volatile` calls at `:560-567` and the eight-way equality check at
  `:2819-2827`; `ring.rs:208-219` for the field list of `LifecyclePage`;
  `ring.rs:783-798` for the whole attach path; and the complete
  `is_quarantined` call-site grep.
- Findings: the tuple reads eight fields and `quarantined` is not among them.
  No other read of the flag occurs anywhere on the attach path. The claim is
  confirmed.
- Missing evidence: none for this question.
- Conclusion: resolved for the source tree, where `validate_lifecycle` did not
  read `quarantined` and attach admitted a quarantined object. At HEAD the
  attach path has an explicit `is_quarantined()` gate (`ring.rs:1141-1143`)
  with a unit test (`:3696-3702`), so the defect is fixed and the record guards
  the gate rather than reporting a live admission.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 27, `:1020-1022` now `:1141-1143`: Attach also runs conservation_inner(true) after the quarantine gate (`:1148`), so a mapping whose cursors already disagree with its slots is refused too.
  - line 47, `:1328` now `:1525`: close and force_close share finish_close (`:1515-1528`), so the retention condition and the registry removal each exist once rather than twice.
  - line 60, `ring.rs:1098` now `ring.rs:1401`: Descriptor validation returns RingError::Descriptor and the try_receive wrapper quarantines through quarantine_with, which is one of many quarantine_with call sites rather than a single inline enter_quarantine.
  - line 62, `ring.rs:783` now `ring.rs:1095`: Attach refuses a flagged object at `:1141-1143`, so this scenario stops at step 2 and never returns a usable Ring.
  - line 89, `ring.rs:2109-2185`: ring.rs has no platform-specific cfg at HEAD, so there is no remaining platform-gated code to point at.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 89, `ring.rs:2109-2185` (platform cfg arms around object creation and sealing): No cfg(target_os) arms remain anywhere in ring.rs; creation and sealing are unconditionally Linux in create_linux_memfd (`:2856`) and validate_seals (`:2848`).
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

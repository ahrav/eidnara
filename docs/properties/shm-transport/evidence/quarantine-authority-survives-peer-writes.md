# quarantine-authority-survives-peer-writes

## Discovery trigger

The hostile-peer lens, applied field by field to the shared control pages. The
peer maps the whole object read-write and the required seal set omits
`F_SEAL_WRITE`, so every field in the mapping was checked for peer writability.
The quarantine flag is one of them, and it is the only field that carries a
terminal local decision.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:208-219` declares
  `struct LifecyclePage`, with `quarantined: AtomicU8` at `:218`. The flag lives
  in the shared mapping and nowhere else.
- `ring.rs:1001-1034` declares `struct Ring` with exactly seven fields: `mapping`,
  `layout`, `grant`, the two eventfd doorbells `data_ready` and `capacity_ready`
  (post-#131, replacing the former `scheduling` field), `owned_runtime_dir`, and
  `_not_send_or_sync: PhantomData<Rc<()>>`. There is no local mirror of the
  flag, so no local state can outlive a peer's overwrite of the shared byte.
- `ring.rs:1915-1922` is `enter_quarantine`. The store is at `:1918`:
  `unsafe { (*page).quarantined.store(1, Ordering::Release) }`. **Correction:**
  the catalog cites `ring.rs:1915`, which is the function signature.
- `ring.rs:1927-1940` is `is_quarantined`. It loads with `Ordering::Acquire` at
  `:1934` and ends `.unwrap_or(true)` at `:1935`, so a failed pointer
  computation reads as quarantined. That fail-closed behaviour covers a bad
  pointer, not a hostile value.
- The gates re-read the flag on every call: `:1275` (`try_reserve`), `:1396`
  (`try_receive`), `:1529` (`release`), `:1608` (`conservation`), `:1888`
  (`probe`). A repository-wide grep for `is_quarantined` across `crates/` and
  `packages/shm-native/src` returns exactly these five call sites plus the
  definition at `:1927` and one in-file unit-test assert (`ring.rs:2366` (source tree; not at HEAD)), so
  nothing latches the value.
- `ring.rs:462` (`Mapping::create`) and `ring.rs:481` (`Mapping::attach`) both
  pass `libc::PROT_READ | libc::PROT_WRITE` with `MAP_SHARED`.
- `ring.rs:2848-2854` (`validate_seals`) requires
  `F_SEAL_GROW | F_SEAL_SHRINK | F_SEAL_SEAL`, and `ring.rs:2865-2867`
  (`seal_object`) applies the same three. `F_SEAL_WRITE` appears nowhere in the
  file, so the lifecycle page stays writable through the peer's own mapping.
- `packages/shm-native/src/lib.rs:301` and `:308` show the addon raising
  quarantine on a failed alias detach, which is the trigger most likely to
  matter in practice: the flag is what keeps the storage condemned while a
  JavaScript view may still be attached.
  At HEAD: The protection and sharing flags moved out of `ring.rs`: `sys::mmap_shared` passes `libc::PROT_READ | libc::PROT_WRITE` with `libc::MAP_SHARED` at `crates/shm-transport/src/backend/sys.rs:94-95`.
  At HEAD: The gates still re-read the flag, but `is_quarantined` latches locally, and there are thirteen production gate sites in `ring.rs` (`:1141`, `:1202`, `:1228`, `:1275`, `:1396`, `:1404`, `:1478`, `:1529`, `:1608`, `:1888`, `:2248`, `:2382`, `:2541`), not five.
  At HEAD: `is_quarantined` returns the local latch first (`:1928-1930`) and latches any observed shared flag (`:1936-1938`), so a hostile value is fail-closed too, not only a bad pointer.
  At HEAD: `Ring` now has fifteen fields, the two doorbells are AF_UNIX socketpairs rather than eventfds, `owned_runtime_dir` is gone, and a local mirror of the flag exists: `quarantined: Cell<bool>` at `:1009`.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

1. The receiver validates a peer-authored descriptor, validation fails, and
   `try_receive` calls `enter_quarantine()` at `ring.rs:1401`. The shared byte
   becomes 1.
2. The local side now treats the direction as terminal. Charges are retained
   rather than returned, per `docs/shm-transport.md:79` (source tree; not at HEAD).
3. The peer stores `0` to the same byte through its own writable mapping. No
   seal and no page protection prevents this.
4. The next local `try_reserve` re-reads the flag at `ring.rs:1275`, observes
   zero, and admits a reservation into storage the local side condemned. The
   same holds for `try_receive`, `release`, `conservation`, and `probe`.
   At HEAD: This step is unreachable at HEAD: `is_quarantined` latches the observed flag, so a peer store of zero cannot make `try_reserve` (`:1275`) admit again.

The consequence is that storage whose alias state is unknown becomes reusable
again, which is exactly what quarantine exists to prevent
(`docs/shm-transport.md:79` (source tree; not at HEAD)).

## Timing windows and dependencies

The window is unbounded. Because every gate re-reads the shared byte rather
than latching it, a peer write at any time after `enter_quarantine()` takes
effect on the very next operation, and it stays in effect until something
re-raises the flag. There is no configuration that closes it: the seal set is
fixed in code, and both mapping paths request write access unconditionally.
Platform gating matters only for the seal check, which is Linux-only
(`ring.rs:2850` sits behind `validate_seals`, called from `Mapping::attach` under
`#[cfg(target_os = "linux")]`); on macOS there are no seals at all, so the
exposure is not narrower. This property is upstream of
`quarantine-gates-cover-every-storage-mutation` and
`attach-refuses-a-quarantined-object`: both assume the flag reads 1 once set.
At HEAD: Platform gating no longer varies: the ring backend is Linux-only through `compile_error!` at `ring.rs:18-19`, `validate_seals` carries no `cfg`, and there is no macOS path.

## What a test must construct

A second process, or a second mapping of the same descriptor in the same
process, that writes the flag byte directly rather than calling any `Ring`
method. Concretely: derive the byte address as
`mapping.base + layout.lifecycle + offset_of!(LifecyclePage, quarantined)`,
raise quarantine on the first handle, then store `0u8` through the second
handle, then assert that `try_reserve`, `try_receive`, `release`, and `probe` on
the first handle still return their `Quarantined` variant. A test that only
calls `enter_quarantine()` and then re-checks the gates, as
`crates/shm-transport/tests/ring.rs:177`
(`quarantine_rejects_all_operations_and_reports_conservation`) does at `:181`,
cannot fail regardless of the answer.

## Investigation log

### Q: Is the flag deliberately shared so the peer observes quarantine, and if so what protects the local decision?

- Sources examined: `ring.rs:208-219` (page layout), `:1001-1034` (`Ring`
  fields), `:1915-1940` (both flag accessors), all five gate sites, the addon
  quarantine call sites at `packages/shm-native/src/lib.rs:301`, `:308`,
  `:337`, `:345`, `:368`, `:375`, `:421-422`, `:461-462`, and
  `docs/shm-transport.md:79` (source tree; not at HEAD), `:21`.
- Findings: placing the flag in the shared page is the only way the peer can
  learn the direction is dead, and the addon quarantines both directions
  together at `lib.rs:421-422`, which reads as deliberate cross-side signalling.
  Nothing in the code or the document states that the flag is also the local
  authority, and no comment addresses the asymmetry.
- Missing evidence: no design note, commit message, or plan requirement states
  whether the flag is intended as a shared signal, a local latch, or both.
- Conclusion: unresolved, needs the design intent. The mechanism is fully
  established; the intent is not.

### Q: Does the `docs/shm-transport.md:116` (source tree; not at HEAD) non-guarantee about malicious peers extend to control pages, or only to payload bytes?

- Sources examined: `docs/shm-transport.md:116` (source tree; not at HEAD) in full.
- Findings: the sentence is "It does not protect against a malicious
  authenticated peer mutating mapped payload after publication, and tests or
  docs must not claim such immutability." It names payload only. The preceding
  sentence lists the trusted obligations as "lane ownership, no-transfer,
  no resizing, and post-publication immutability", none of which mention control
  pages either.
- Missing evidence: no statement anywhere in the document about control-page
  integrity.
- Conclusion: needs human input. The text is silent on control pages, and
  reading silence as either coverage or exclusion would be a fabricated answer.

### Q: Can a peer still revive a quarantined handle at HEAD? (added 2026-09-05)

- Checked: `Ring.quarantined: Cell<bool>` (`crates/shm-transport/src/backend/ring.rs:1009`); `enter_quarantine` (`:1915-1922`) sets it before storing the shared flag; `is_quarantined` (`:1927-1940`) returns the cell when set and latches any observed shared flag. `quarantine_survives_peer_clearing_shared_flag` (`:4261`) and `shared_quarantine_flag_latches_locally_when_observed` (`:3942`) clear the shared flag and assert `try_receive`, `try_reserve`, `trim`, and `arm_data_wait` stay quarantined.
- Conclusion: no. The record is refreshed to Exercised: yes; the trail above describes the source tree's unlatched gate.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 16, `ring.rs:719-727` now `ring.rs:1001-1034`: `Ring` now has fifteen fields, the two doorbells are AF_UNIX socketpairs rather than eventfds, `owned_runtime_dir` is gone, and a local mirror of the flag exists: `quarantined: Cell<bool>` at `:1009`.
  - line 24, `ring.rs:1381-1388` now `ring.rs:1927-1940`: `is_quarantined` returns the local latch first (`:1928-1930`) and latches any observed shared flag (`:1936-1938`), so a hostile value is fail-closed too, not only a bad pointer.
  - line 28, `:913` now `:1275`: The gates still re-read the flag, but `is_quarantined` latches locally, and there are thirteen production gate sites in `ring.rs` (`:1141`, `:1202`, `:1228`, `:1275`, `:1396`, `:1404`, `:1478`, `:1529`, `:1608`, `:1888`, `:2248`, `:2382`, `:2541`), not five.
  - line 34, `ring.rs:321` now `ring.rs:462`: The protection and sharing flags moved out of `ring.rs`: `sys::mmap_shared` passes `libc::PROT_READ | libc::PROT_WRITE` with `libc::MAP_SHARED` at `crates/shm-transport/src/backend/sys.rs:94-95`.
  - line 54, `ring.rs:913` now `ring.rs:1275`: This step is unreachable at HEAD: `is_quarantined` latches the observed flag, so a peer store of zero cannot make `try_reserve` (`:1275`) admit again.
  - line 70, `ring.rs:2131` now `ring.rs:2850`: Platform gating no longer varies: the ring backend is Linux-only through `compile_error!` at `ring.rs:18-19`, `validate_seals` carries no `cfg`, and there is no macOS path.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 32, `ring.rs:2366` (the single in-file unit-test assert): No single line replaces it: `ring.rs` now holds roughly thirty in-file test asserts on `is_quarantined` (for example `:3204`, `:3942`, `:4267`).
  - line 51, `docs/shm-transport.md:79` (documented charge retention on quarantine): The trimmed document states only that active and quarantined charges are reported separately (`:21`) and stay within the process bound (`:92`); the retention rule now lives on `AdmissionController` in `crates/shm-transport/src/profile.rs:380-384`.
  - line 60, `docs/shm-transport.md:79` (documented purpose of quarantine): Absent from the trimmed document.
  - line 98, `docs/shm-transport.md:79` (documented quarantine clause): Absent from the trimmed document.
  - line 109, `docs/shm-transport.md:116` (the malicious-peer non-guarantee sentence): The trimmed 98-line document contains no malicious-peer statement at all.
  - line 111, `docs/shm-transport.md:116` (the malicious-peer non-guarantee sentence): Gone with the trim; nothing replaces it.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

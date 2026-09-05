# reclamation-excludes-pages-with-live-wrapped-bytes

## Discovery trigger

Fix commit `b3e10a256` "keep reclamation off pages containing live wrapped
bytes". Before it, `removal_ranges` rounded its start *down* to a page
boundary, so `MADV_REMOVE` could zero the head of a page whose leading bytes
still belonged to an unreleased frame. Lead only; re-verified at HEAD.

## Evidence trail

- Reclamation is producer-driven: `reclaim_completed`
  (`crates/shm-transport/src/backend/ring.rs:2070-2151`) walks the
  contiguous completed prefix, validates each descriptor, and computes
  `[reclaimed, new_reclaimed)` as the logical byte run to return.
- `removal_ranges` (`:375-427`) converts that run to physical ranges. The
  fixed start rounds *up*: a `logical_start` not on a page boundary advances
  to the next boundary (`:398-404`); the end rounds *down* (`:405`); an
  empty result short-circuits (`:406-408`). At the arena wrap the run splits
  into at most two ranges (`:411-414`), both inside the arena.
- Consequence: `remove_pages` (`:433-441`, `MADV_REMOVE`) only ever touches
  pages every byte of which lies inside the released run. A page shared with
  a live neighbor — including the page holding the tail of a wrapped frame's
  first span — is left resident.
- The one trailing exception is explicitly guarded (`:2199-2220`): the
  partial page at `reclaimed` is removed only when
  `arena_write == new_reclaimed` (nothing live anywhere ahead, `:2199`) and
  the run crossed that page's boundary (`:1535-1536` (source tree; not at HEAD)). Under that guard the
  page contains only released bytes plus dead slack.
- Failure containment: a nonzero `madvise` return quarantines before any
  capacity is published (`:2194-2198` via the `remove` closure), and
  `arena_reclaimed` is stored only after every removal succeeded
  (`:2142-2144`, "capacity becomes visible only after every removal
  succeeds").
  At HEAD: The guard is `everything && live_end == reclaimed`, where `live_end` is `reserved_end` while a reservation is outstanding and `arena_write` otherwise (`ring.rs:2154-2156`).
  At HEAD: The trailing partial page is removed only by `trim`, which passes `everything = true` to `punch_dead_pages` (`ring.rs:2264`); a reclaim pass always passes `false` (`:2133`), so reclamation never touches a partial page.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

Frame A ends mid-page; frame B starts on the same page and is still leased.
A is released and reclaimed. With round-down, `MADV_REMOVE` covers the shared
page and B's leading bytes read back as zeros — silent data corruption in a
frame the receiver already validated, with no error anywhere. The wrap
variant is the same defect where B is the second span of a frame that
wrapped the arena end.

## Timing windows and dependencies

No interleaving is required: the defect class is arithmetic, reachable
single-threaded. Dependencies: the runtime page size (`system_page_size`,
`:443-449`) — a 16 KiB host makes partial-page sharing far more common — and
the FIFO ordering that `reclaim_completed` enforces
(`allocation_start == reclaimed + run_len`, `:2109-2114`), which is what
makes "everything before `new_reclaimed` is dead" a sound premise.

## What a test must construct

- The pure function: partial pages at both ends and a wrap split. Exists:
  `removal_ranges_exclude_partial_pages_and_split_once_at_wrap`
  (`ring.rs:4053-4070`) sweeps 4/16/64 KiB pages, asserts a sub-page run
  removes nothing, an unaligned run removes only its interior page, and a
  wrapping run splits into two exact ranges.
- The live-neighbor oracle: reclaim beside a held lease, then read the
  neighbor's bytes back. Exists:
  `partial_page_reclaim_preserves_live_neighbor` (`:4118-4133`).
- The eventual-progress complement: sub-page releases still converge to
  whole-page removal (`repeated_subpage_releases_eventually_remove_complete_pages`,
  `:2319-2335` (source tree; not at HEAD)) and removed pages read back as zeros
  (`reclaimed_pages_leave_residency_and_reuse_as_zeroes`, `:4073-4088`).
- Not yet constructed: the trailing-partial-page exception (`:2199-2220`)
  under a *wrapped* `arena_write` — every existing case reaches it with a
  linear cursor — and any of this on a non-4096-page host (fault class F11).
  At HEAD: Sub-page releases do not converge to whole-page removal on their own at HEAD: reclamation punches only once a quarter of the arena is dead (`ring.rs:2132-2134`, `punch_batch_bytes` at `:2159-2161`), and the rest comes back only through `trim`.

## Investigation log

### Q: is the trailing-page exception sound when the cursor wrapped?

- Sources examined: `:2199-2220`; `logical_page % arena_bytes` mapping at
  `:411-412`.
- Findings: the guard compares logical (unwrapped, monotone) values, and the
  physical offset is derived by modulo, so the mapping is consistent; but
  `arena_write == new_reclaimed` with both mid-lap is a state no test
  constructs, and the equality is between values a hostile peer cannot forge
  only because both pages are producer-owned.
- Missing evidence: no test constructs the `arena_write == new_reclaimed`
  state with a wrapped cursor.
- Conclusion: unresolved, needs a test that reaches the exception with a
  wrapped cursor.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 25, `:1533-1548` now `:2199-2220`: The trailing partial page is removed only by `trim`, which passes `everything = true` to `punch_dead_pages` (`ring.rs:2264`); a reclaim pass always passes `false` (`:2133`), so reclamation never touches a partial page.
  - line 27, `:1534` now `:2199`: The guard is `everything && live_end == reclaimed`, where `live_end` is `reserved_end` while a reservation is outstanding and `arena_write` otherwise (`ring.rs:2154-2156`).
  - line 66, `:2319-2335`: Sub-page releases do not converge to whole-page removal on their own at HEAD: reclamation punches only once a quarter of the arena is dead (`ring.rs:2132-2134`, `punch_batch_bytes` at `:2159-2161`), and the rest comes back only through `trim`.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 28, `:1535-1536` (check that the run crossed the page boundary): No such check remains; once `live_end == reclaimed` the punch window is widened outward to page boundaries (`ring.rs:2200-2204`), with a whole-arena shortcut at `:2205-2206`.
  - line 66, `:2319-2335` (repeated_subpage_releases_eventually_remove_complete_pages): That test is gone; `subpage_releases_stay_resident_until_trim` (`ring.rs:4091-4115`) now covers sub-page releases.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

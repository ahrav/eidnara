# layout-region-offsets-are-real-page-aligned

## Discovery trigger

`Layout::new` aligns two region offsets to `PAGE_SIZE` and then adds `PAGE_SIZE`
once more for the lifecycle page. `PAGE_SIZE` is a compile-time `4096`. The crate
also has a `system_page_size()` helper that reads `sysconf(_SC_PAGESIZE)`, and
exactly one caller uses it. So the layout's page arithmetic and the kernel's page
granularity are two different numbers on any host whose page is not 4096.

## Evidence trail

`crates/shm-transport/src/backend/ring.rs:46` declares
`const PAGE_SIZE: usize = 4096`. `Layout::new` (`:279-345`) uses `CACHELINE`
(128) for the three control pages and `PAGE_SIZE` for the rest: `arena =
align_up(slots + slot_bytes, PAGE_SIZE)` (`:318-323`), `lifecycle =
align_up(arena + arena_bytes, PAGE_SIZE)` (`:324-329`), and `total =
lifecycle.checked_add(PAGE_SIZE)` (`:330-332`). `system_page_size()`
(`:443-450`) exists and falls back to `PAGE_SIZE`, but its only caller is
`verify_prefaulted` (`:1009` (source tree; not at HEAD)), which uses it to size the `mincore` residency
vector. No layout arithmetic consults it.

`total` is what leaves the crate. `Ring::create_in` passes it to
`Mapping::create(layout.total)` (`:1061`), which `ftruncate`s the object to that
length (`:2859` on Linux, `:1784` (source tree; not at HEAD) on macOS) and `mmap`s exactly `len`
(`crates/shm-transport/src/backend/sys.rs:86-104`). It is also published in the grant as `total_bytes` (`:1059`) and
re-derived on the attaching side by `checked_layout`, which requires
`layout.total == total` (`:942-944`). So both sides agree on a number computed
from a constant neither of them checks against the kernel.

I computed the layout for the three profiles that exist in the tree —
`lease_limited_profile` depth 2 (`tests/ring.rs:18`), `qualified_test_profile`
depth 8 (`crates/host-runtime/src/ring_transport.rs:38`), and `ring_profile` depth 32
(`src/profile.rs:700`) — all with `arena_bytes = MIN_ARENA_BYTES = 67_108_864`
(`src/arena.rs:4-7`), using `size_of::<DescriptorSlot>() = 256` and 128 for each
of the three control pages. Depth 2 and depth 8 produce identical offsets because
both slot regions fit inside the first 4096 bytes.

| | depth 2 and 8 | depth 32 |
| --- | --- | --- |
| slots | 384 | 384 |
| arena | 4096 | 12288 |
| lifecycle | 67112960 | 67121152 |
| total | 67117056 | 67125248 |

Under a 4096-byte page every one of `arena`, `lifecycle`, and `total` is a
multiple of the page size, the lifecycle page occupies a real page alone, and the
arena ends exactly where the lifecycle page begins. Under a 16384-byte page, with
the same as-built offsets, `arena % 16384` is 4096 for depth 2 and 8 and 12288 for
depth 32; `lifecycle % 16384` is the same; and `total % 16384` is 8192 for depth 2
and 8 but 0 for depth 32. Depth 32 satisfies the total-is-a-page-multiple property
by coincidence, since 67125248 is exactly 4097 × 16384. Had `Layout::new` used
16384, arena would be 16384 and lifecycle 67125248 for every depth, and total would
be 67141632 — 24576 bytes larger than the depth-2 and depth-8 figure and 16384
larger than the depth-32 figure.

## Failure scenario

On a 16 KiB-page host the lifecycle page stops being a page. For depth 8, real
page 4096 spans bytes 67108864 to 67125248; the lifecycle structure sits at
67112960 for 128 bytes (`LifecyclePage` is 128 bytes, `ring.rs:208-219`), and the arena's
final 4096 bytes — bytes 67108864 to 67112960, peer-writable payload — occupy the
first quarter of that same real page. For depth 32 the shared prefix is 12288
bytes. The lifecycle page holds the magic, the layout version, the geometry
echoed by `validate_lifecycle`, the incarnation, the lane, and the `quarantined`
flag. Any reasoning or mechanism with page granularity — `mprotect` to make the
control page read-only to one role, `mincore` residency attributed per region,
`madvise` on the arena, or a hardware watchpoint — now covers arena payload and
lifecycle state as one unit and cannot separate them. The arena's start is
likewise not on a real page boundary, so it shares its first real page with the
three control pages and the entire descriptor-slot array.

The second divergence is the mapping tail. For depth 2 and 8, `total` of
67117056 is not a multiple of 16384, so `mmap` and `ftruncate` round the mapping
up to 67125248 and 8192 bytes are addressable past `mapping.len`. `ptr_at`
bounds-checks every typed access against `self.len` (`:489-498`), and both
prefault walks stop at `len`, so those 8192 bytes are mapped, writable, and never
initialised — reachable only through a pointer computed outside `ptr_at`.
`Mapping::drop` munmaps `len` (`:665-671`), which the kernel rounds up, so they
are released.

## Timing windows and dependencies

No fault and no race. This is fixed at construction and holds for the mapping's
lifetime. The enabling condition is purely environmental: a host whose
`sysconf(_SC_PAGESIZE)` is not 4096, which means Apple Silicon macOS or an
aarch64 Linux kernel built with 16 KiB or 64 KiB pages. It shares that condition
with `page-size-dependent-setup-runs-on-a-non-4096-page-host`, and on macOS it is
gated behind `macos-object-creation-outcome-is-attributed`, since creation must
succeed before a layout is mapped at all.

## What a test must construct

A ring created on a host whose page size is not 4096, asserting that `arena`,
`lifecycle`, and `total` are each multiples of `system_page_size()`. The offsets
are private, so this needs either an accessor or a unit test inside the module,
alongside the existing `residency_vector_len` test (`:4046-4051`). The stronger
form is a pure test that does not need special hardware: call the layout
computation with an injected page size of 16384 and 65536 and assert the same
three divisibility conditions, which fails today at HEAD because the page size is
not injectable. A depth sweep matters — depth 32 passes the `total` condition and
fails the other two, so a single-depth test can conclude the wrong thing. The tail
arm asserts `total % system_page_size() == 0`, which is what removes the 8192
addressable bytes past `len`.

## Investigation log

### Q: Is the layout total required to be a multiple of the real page size, or only of 4096?

- Sources examined: `ring.rs:45-46`, `:208-345`, `:367-450`, `:451-483`,
  `:489-513`, `:929-946`, `:1040-1091`, `:1009` (source tree; not at HEAD), `:1672-1677` (source tree; not at HEAD), `:2859`, `:1779` (source tree; not at HEAD),
  `:4046-4051`; `src/arena.rs:4-7`, `:225-236` (source tree; not at HEAD); `src/profile.rs:700-703`;
  `tests/ring.rs:14-38`; `crates/host-runtime/src/ring_transport.rs:38-40`; the diff
  of `a5568707` restricted to the page-size change. The offsets in the table were
  computed by replicating `Layout::new` with verified struct sizes
  (`size_of::<DescriptorSlot>() = 256`, 128 for each control page,
  `size_of::<SharedDescriptor>() = 120`), not read from a run.
- Findings: not required by any in-tree caller. `mmap` accepts a non-page-multiple
  length and rounds up; `ftruncate` accepts one; `munmap` accepts one; and
  `residency_vector_len` (`crates/shm-transport/src/backend/sys.rs:153-155`) already computes `div_ceil` against the
  runtime size, so `mincore`'s vector is correctly sized regardless. The
  consequences are the shared real page between the lifecycle structure and the
  arena tail, the non-page-aligned arena start, and the addressable slack past
  `len` — none of which is an immediate memory-safety violation, and all of which
  break the region separation the 4096 alignment was evidently there to create.
- Missing evidence: nothing in the repository states why `arena` and `lifecycle`
  are page-aligned rather than cacheline-aligned like the three control pages. If
  the intent is only cacheline separation then 4096 is over-alignment and the
  16 KiB divergence is harmless; if the intent is page separation then it is a
  defect. No comment, plan, or traceability entry records the intent.
- Conclusion: resolved with answer on the arithmetic, unresolved on the
  requirement. The divergence is exact and reproducible; whether page separation
  is a contract needs the design owner.

### Q: Does `Layout::new` still use a 4096 literal at HEAD? (added 2026-09-05)

- Checked: `Layout::new` (`crates/shm-transport/src/backend/ring.rs:279-345`) reads `page_size = system_page_size()` (`:443-450`), rejects an `arena_bytes` that is not a multiple of it, aligns `arena` and `lifecycle` to `page_size`, and sets `total = lifecycle + page_size`. No 4096 literal remains in the layout.
- Conclusion: no. The offsets are page multiples by construction; the divergence computed in the trail above applies to the source tree only. No test asserts this under a non-4096 page.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 14, `:141-182` now `:279-345`: At HEAD Layout::new reads page_size = system_page_size() (`:280`), rejects an arena_bytes that is not a multiple of it (`:283-285`), aligns arena and lifecycle to that runtime page size, adds one runtime page for total, and uses CACHELINE for five control pages rather than three.
  - line 16, `:158-163` now `:318-323`: The alignment argument is the runtime page_size local, not the PAGE_SIZE constant.
  - line 17, `:164-169` now `:324-329`: The alignment argument is the runtime page_size local, not the PAGE_SIZE constant.
  - line 18, `:170-172` now `:330-332`: The addend is the runtime page_size local, not the PAGE_SIZE constant.
  - line 20, `:1009`: system_page_size() has many callers at HEAD, including Layout::new (`:280`), punch_batch_bytes (`:2160`), punch_dead_pages (`:2185`), and punch_range (`:2294`), so the layout arithmetic does consult the kernel page size.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 20, `:1009` (verify_prefaulted): No verify_prefaulted exists; the mincore residency vector is sized in Mapping::resident_pages (`:583-596`).
  - line 25, `:1784` (the macOS ftruncate path): The macOS object-creation path was removed; only create_linux_memfd (`:2856-2863`) remains.
  - line 111, `:1009` (verify_prefaulted): Removed; Mapping::resident_pages (`:583-596`) is the only mincore caller.
  - line 111, `:1672-1677` (unnamed source range): The record does not say what this range held and no construct at HEAD corresponds to it; the neighbouring object checks are validate_object (`:2833-2846`) and validate_seals (`:2848-2854`).
  - line 111, `:1779` (the macOS ftruncate): The macOS creation path was removed.
  - line 112, `:225-236` (unnamed source range): crates/shm-transport/src/arena.rs ends at line 223 and the record does not say what this range held; ArenaCounts::conserves (`:208-222`) is the last item in the file.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

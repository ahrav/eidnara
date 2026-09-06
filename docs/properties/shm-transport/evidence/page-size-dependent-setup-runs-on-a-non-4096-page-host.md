# page-size-dependent-setup-runs-on-a-non-4096-page-host

## Discovery trigger

Commit `a5568707` fixed a page-size defect by replacing `self.mapping.len
.div_ceil(PAGE_SIZE)` with `residency_vector_len(self.mapping.len,
system_page_size())` in `verify_prefaulted`, and added a unit test for the new
helper. It changed nothing else. Three page-size notions survive in the tree and
only one of them reads the kernel, so the fix repaired the consumer of the page
size while leaving the producers on a constant.

## Evidence trail

The three notions, in `crates/shm-transport`:

1. `const PAGE_SIZE: usize = 4096` (`src/backend/ring.rs:46`), used by
   `Layout::new` at `:164` (source tree; not at HEAD), `:170` (source tree; not at HEAD), `:172` (source tree; not at HEAD), and by `prefault_read` at `:1673` (source tree; not at HEAD).
2. A bare literal `let page = 4096usize` (`src/arena.rs:229` (source tree; not at HEAD)), used by the
   `prefault` write walk at `:230` (source tree; not at HEAD). This one does not even reference the constant,
   so a change to `PAGE_SIZE` would not reach it.
3. `system_page_size()` (`src/backend/ring.rs:443-449`), which reads
   `sysconf(_SC_PAGESIZE)` and falls back to `PAGE_SIZE`. Its sole caller is
   `verify_prefaulted` at `:1009` (source tree; not at HEAD).
   At HEAD: `system_page_size` has several callers: `Layout::new` (`ring.rs:280`), `Mapping::resident_pages` (`:590`), the punch batch size (`:2160`), the reclaim page walk (`:2185`), and `removal_ranges` (`:2294`).
   At HEAD: `system_page_size` caches its result in a `OnceLock` and reads the kernel page size through `sys::page_size()` (`backend/sys.rs:185-189`), still falling back to `PAGE_SIZE`.
   At HEAD: `PAGE_SIZE` appears only as its own definition (`ring.rs:46`) and as the fallback inside `system_page_size` (`:446`), so no layout arithmetic is left on the constant.

`verify_prefaulted` is a hard gate on creation, not a diagnostic:
`Ring::create_in` returns `PrefaultFailed` if it reports false (`:586-588` (source tree; not at HEAD)). That
is why the pre-`a5568707` form was a real defect. With a 16 KiB page and the
depth-8 layout, `mapping.len` is 67117056; the old code sized the vector
`67117056 / 4096 = 16386` entries, `mincore` writes only
`ceil(67117056 / 16384) = 4097`, and the remaining 12289 entries stay zero, so
`residency.into_iter().all(|entry| entry & 1 == 1)` (`:1019` (source tree; not at HEAD)) is false and every
creation fails. At depth 32 the numbers are 16388 written down to 4097, leaving
12291. The current form computes 4097 directly and matches what `mincore` writes.

The two prefault walks are not defective in the same direction, and this is worth
recording because it is the reason the half-fix went unnoticed. Both step by 4096
over `0..len`. Where the real page is larger, stepping by a smaller amount touches
every real page several times: it over-touches, never under-touches. For the
depth-8 layout the last offset `prefault_read` visits is 67112960, which lies
inside real page 4096 (67108864 to 67125248), the same real page that contains
`len`. `arena::prefault` additionally writes `base.add(len - 1)` (`:235` (source tree; not at HEAD)). So
residency is complete and `verify_prefaulted` agrees. The cost is four times the
volatile accesses needed on a 16 KiB host, which is waste rather than
incorrectness. The direction that would break coverage — a real page smaller than
4096 — does not occur on either supported target.

No host in CI exercises any of this. `the source repository `ci.yml` workflow:132` builds
`[ubuntu-latest, macos-latest]`. The Linux step (`:159-166`) runs
`cargo nextest run -p shm-native -p shm-transport` with no target filter, so
it runs the lib target and therefore
`residency_vector_tracks_runtime_page_size` (`src/backend/ring.rs:4045-4050`) —
but on an x86-64 runner whose page size is 4096, where the assertion about 16384
is a property of the pure function and not of any mapping. The macOS step
(`:168-175`) runs `cargo nextest run -p shm-transport --test contract --test
fuzz_corpus`; `--test` selects integration targets, so the lib target is excluded
and even that pure test does not run on macOS. Neither step runs `tests/ring.rs`
on macOS, and `tests/contract.rs` and `tests/fuzz_corpus.rs` construct no `Ring`.

Update 2026-08-31: the CI paragraph above resolves against the pre-#131
workflow. PR #131 (merge `5d638e3e8`) removed the macOS matrix leg entirely and
deleted the Darwin npm packages (`packages/host-darwin-*`, removed in
`55f47ac64`); `the source repository `ci.yml` workflow` at HEAD `bdf72f46a` has only
`ubuntu-latest` jobs, so CI provisions no non-4096-page host of any kind. See
the dated investigation-log entry below.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

The only in-tree evidence that any page-size code is correct is a pure-function
assertion on a 4096-page host. The end-to-end path — `Layout::new`, `ftruncate`,
`mmap`, both prefault walks, `mincore`, and the `PrefaultFailed` gate — has never
run together on a host where the constant and the kernel disagree. That is the
exact configuration where the previous defect lived, and it is the configuration
where the residual defects in `layout-region-offsets-are-real-page-aligned` take
effect. A subsequent change that reintroduces a 4096 assumption into the residency
path, or that starts depending on the arena or lifecycle offsets being real pages,
is invisible to CI.

The gap is sharpened by where the 16 KiB page actually occurs. `macos-latest` maps
to Apple Silicon runners, whose page size is 16384, so CI already provisions a
16 KiB host every run — and it is precisely the host on which no `Ring` is ever
constructed, because `Ring::create` fails there for an unattributed reason. The
one platform that would have caught the original defect is the one platform where
the code does not execute.

## Timing windows and dependencies

No fault and no race; the condition is environmental and constant for a host. This
record is coverage for `layout-region-offsets-are-real-page-aligned`, which states
the substantive alignment property, and it is blocked on macOS by
`macos-object-creation-outcome-is-attributed`. On Linux it is not blocked: an
aarch64 runner with a 16 KiB or 64 KiB kernel would execute the whole path today.

## What a test must construct

Execution of `Ring::create` to completion on a host whose
`sysconf(_SC_PAGESIZE)` is not 4096, asserting that `verify_prefaulted` returns
true rather than that creation merely does not error — the gate already conflates
the two, so the assertion has to read the probe. The cheap construction is an
aarch64 Linux job with a 16 KiB or 64 KiB page kernel added to the CI matrix, which
needs no macOS work. The alternative is making the page size injectable so the
whole layout-and-prefault path can be driven at 16384 and 65536 on any host; that
also unblocks the pure arms in
`layout-region-offsets-are-real-page-aligned`. A second, independent arm is to add
`--test ring` to the macOS command, which does not by itself establish page-size
coverage but does put the 16 KiB host in contact with the code.

This is location and environment coverage, so the assertion must be that the path
executed and its own probe passed. It must not assert a page-size violation.

## Investigation log

### Q: Did `a5568707` leave any page-size-dependent code on the constant, and does any of it under-cover pages rather than over-cover them?

- Sources examined: `git show a5568707 -- crates/shm-transport/src/backend/ring.rs`
  in full; `src/backend/ring.rs:46`, `:279-345`, `:443-449`, `:574-593` (source tree; not at HEAD),
  `:1008-1022` (source tree; not at HEAD), `:1672-1677` (source tree; not at HEAD), `:4045-4050`; `src/arena.rs:221-236` (source tree; not at HEAD); every
  occurrence of `PAGE_SIZE`, `system_page_size`, `residency_vector_len`, and
  `4096` in the crate; `the source repository `ci.yml` workflow:126-177`; the import lists of
  `tests/contract.rs` and `tests/fuzz_corpus.rs`. Residency-vector and offset
  figures were computed, not observed.
- Findings: the fix touched `verify_prefaulted` and added `system_page_size` plus
  `residency_vector_len` and its test. `Layout::new` and both prefault walks were
  left on 4096, and `src/arena.rs:229` (source tree; not at HEAD) is a separate literal rather than the
  constant. Of the three residual sites, the two prefault walks over-touch and are
  therefore harmless in the direction that matters; the layout arithmetic is the
  load-bearing one. The `mincore` mismatch that the fix repaired reproduces exactly
  in arithmetic at both depths.
- Missing evidence: that `macos-latest` provisions arm64 with a 16384-byte page is
  external to this repository. I could not query the runner image from here, and
  the workflow pins only the label. If the label still resolved to an x86-64
  image, CI would have no 16 KiB host at all, which strengthens rather than weakens
  the record. Whether the arena and lifecycle offsets are contractually pages is
  the open design question, and it belongs to
  `layout-region-offsets-are-real-page-aligned`.
- Conclusion: resolved with answer. Three page-size sources, one of them reading
  the kernel, and no end-to-end execution on a host where they differ. The
  half-fix is confirmed from the diff rather than inferred from the current state.

### Q: Does any non-4096-page host remain after PR #131, and is Darwin still a supported release surface? (added 2026-08-31)

- Sources examined: `the source repository `ci.yml` workflow` at HEAD `bdf72f46a` (every
  `runs-on` is `ubuntu-latest`); `packages/` listing at HEAD;
  `git log --diff-filter=D -- packages/host-darwin-arm64/package.json`
  (`55f47ac64`, merged via `5d638e3e8`, PR #131);
  `crates/shm-transport/src/backend/ring.rs:1-2`, `:311-312`, `:2176`;
  `docs/shm-transport.md:5`, `:83`.
- Findings: PR #131 removed the macOS matrix leg, so CI provisions no 16 KiB
  host at all; the former open question about `macos-latest`'s page size is
  moot. The crate now compile-errors off Linux (`ring.rs:1-2`) and
  `create_macos_shm` (`:2176`) has no caller, so the arm in "What a test must
  construct" that adds `--test ring` to the macOS command no longer has a
  command to add to and would require restoring both the CI leg and the code
  path. The remaining constructions are unchanged: an aarch64 Linux large-page
  job, or an injectable page size.
- Missing evidence: whether Darwin is intended to return as a release surface;
  the packages and CI jobs are deleted while the `cfg(target_os = "macos")`
  code remains.
- Conclusion: needs human input on the Darwin surface; resolved with answer on
  coverage — at HEAD no CI host has a non-4096 page. Citation corrections: the
  `ci.yml` matrix and macOS step lines cited above (`:132`, `:159-166`,
  `:168-175`) no longer exist at HEAD.

### Q: Do the prefault mechanisms the check names exist at HEAD? (added 2026-09-05)

- Checked: `rg 'verify_prefaulted|PrefaultFailed|prefault' crates/shm-transport/src crates/host-runtime/src` returns nothing. `Layout::new` (`ring.rs:279-345`), `residency_vector_len` (`backend/sys.rs:153`), and the `mincore` accounting (`:583-596`) take the page size from `system_page_size()`. CI runs Linux x86-64 only.
- Conclusion: no. The check is re-stated against the runtime-aligned setup and reclamation path; the record stays active because no non-4096-page host executes it.
  At HEAD: `Ring::resident_arena_pages` (`ring.rs:1894-1899`) forwards to `Mapping::resident_pages` (`:583-596`), which sizes the vector with `sys::residency_vector_len(len, system_page_size())` and calls `sys::mincore`.
  At HEAD: The helper moved to `backend/sys.rs:153`, and its caller `Mapping::resident_pages` passes the page size in (`ring.rs:590`).

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 17, `:164`: At HEAD `PAGE_SIZE` appears only as its own definition (`ring.rs:46`) and as the fallback inside `system_page_size` (`:446`), so no layout arithmetic is left on the constant.
  - line 21, `src/backend/ring.rs:194-200` now `src/backend/ring.rs:443-449`: At HEAD `system_page_size` caches its result in a `OnceLock` and reads the kernel page size through `sys::page_size()` (`backend/sys.rs:185-189`), still falling back to `PAGE_SIZE`.
  - line 23, `:1009`: At HEAD `system_page_size` has several callers: `Layout::new` (`ring.rs:280`), `Mapping::resident_pages` (`:590`), the punch batch size (`:2160`), the reclaim page walk (`:2185`), and `removal_ranges` (`:2294`).
  - line 165, `:386` now `backend/sys.rs:153`: The helper moved to `backend/sys.rs:153`, and its caller `Mapping::resident_pages` passes the page size in (`ring.rs:590`).
  - line 165, `:1855-1863` now `:583-596`: At HEAD `Ring::resident_arena_pages` (`ring.rs:1894-1899`) forwards to `Mapping::resident_pages` (`:583-596`), which sizes the vector with `sys::residency_vector_len(len, system_page_size())` and calls `sys::mincore`.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 17, `:164` (PAGE_SIZE use in Layout::new): `Layout::new` (`ring.rs:279-345`) reads `system_page_size()` at `:280` and uses that value at `:322`, `:328`, and `:331`; it no longer names `PAGE_SIZE`.
  - line 17, `:170` (PAGE_SIZE use in Layout::new): The second layout alignment uses the runtime `page_size` value (`ring.rs:328`).
  - line 17, `:172` (PAGE_SIZE use in Layout::new): The lifecycle page addition uses the runtime `page_size` value (`ring.rs:331`).
  - line 17, `:1673` (prefault_read): No prefault read walk exists at HEAD and nothing replaced it.
  - line 18, `src/arena.rs:229` (bare 4096 literal in the arena prefault): `arena.rs` has no prefault function and no page literal at HEAD.
  - line 19, `:230` (arena prefault write walk): The write walk is gone and nothing replaced it.
  - line 23, `:1009` (verify_prefaulted as the sole caller of system_page_size): `verify_prefaulted` no longer exists.
  - line 26, `:586-588` (Ring::create_in returning PrefaultFailed): Neither `create_in` nor `PrefaultFailed` exists at HEAD; creation is `Ring::create` (`ring.rs:1040`) with no prefault gate.
  - line 31, `:1019` (residency all-resident predicate): It is replaced by a resident-page count, `residency.into_iter().filter(|entry| entry & 1 == 1).count()` in `Mapping::resident_pages` (`ring.rs:595`).
  - line 41, `:235` (arena prefault last-byte write): The arena prefault walk is gone and nothing replaced it.
  - line 115, `:574-593` (creation-side prefault gate): The creation path has no prefault gate at HEAD.
  - line 116, `:1008-1022` (verify_prefaulted): The function is gone; residency is only counted, in `Mapping::resident_pages` (`ring.rs:583-596`).
  - line 116, `:1672-1677` (prefault_read): No prefault read walk exists at HEAD.
  - line 116, `src/arena.rs:221-236` (arena prefault): `arena.rs` carries no prefault code at HEAD.
  - line 123, `src/arena.rs:229` (bare 4096 literal in the arena prefault): The literal and its walk are gone from `arena.rs`.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

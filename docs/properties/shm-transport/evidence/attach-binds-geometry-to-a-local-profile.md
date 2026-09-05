# attach-binds-geometry-to-a-local-profile

## Citation refresh, 2026-08-30

The ring-transport refactor (`0f336d3c`, `d8bde128`, `793a973e`, `ed487e11`)
renamed `crates/host-runtime/src/shm_provider.rs` to
`crates/host-runtime/src/ring_transport.rs` and deleted `provider_recovery.rs`,
`transport_negotiation.rs`, and `transport_provider.rs`. Host-side citations below
were re-anchored against `ring_transport.rs` at `e447c927`.

Where the cited construct survives, the citation names `ring_transport.rs` and a
line re-verified against that commit. Where it does not, the original reference is
kept and prefixed `former`, so it reads as pre-refactor evidence rather than a
current location. A `former` line number is never a claim about the tree today.
Every `provider_recovery.rs` reference is `former` by definition: that module has
no successor. See the refresh note in [../catalog.md](../catalog.md).

## Discovery trigger

`Ring::create_in` takes a `&TargetProfile` and derives the layout from it.
`Ring::attach` takes no profile at all. Admission charges a profile's geometry
before a candidate is prepared, so the attaching side charges for one geometry and
maps whatever the grant declares, with no step that compares the two.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1095-1150` — `Ring::attach(fd:
  OwnedFd, grant: RingGrant, scheduling: SchedulingMode)`. Three parameters, none
  of them a profile. The whole body is: `grant.checked_layout()` (`:1103`),
  `Mapping::attach` (`:1106`), `validate_lifecycle` (`:1107`), `prefault_read`
  (`:607` (source tree; not at HEAD)).
- `crates/shm-transport/src/backend/ring.rs:1040-1050` — the contrast.
  `Ring::create_in` rejects a profile whose backend, memory layout, or `max_spans`
  disagrees (`:1047-1049`) before deriving `Layout::new(profile.descriptor_depth(),
  profile.arena_bytes())` (`:1050`). All of that is skipped on attach.
- `crates/shm-transport/src/backend/ring.rs:929-946` — `checked_layout`, the
  only bound on attach geometry. It rejects `layout_version != LAYOUT_VERSION`,
  `descriptor_depth == 0`, `arena_bytes < MAX_FRAME_BYTES`, `max_leases == 0`,
  and `max_leases > descriptor_depth` (`:930-937`), then requires `layout.total ==
  total` (`:942-944`). In the source tree depth had a floor of 1 and no
  ceiling, so only `usize` overflow stopped a huge depth. At HEAD `Layout::new`
  refuses `depth == 0 || depth > MAX_DESCRIPTOR_DEPTH`
  (`crates/shm-transport/src/backend/ring.rs:286-287`, with
  `MAX_DESCRIPTOR_DEPTH = 4096` at `:52`) and `RingGrant::checked_layout`
  (`:929-946`) runs it before any grant is accepted, so an absolute ceiling
  exists; what is still missing is the match against a local profile.
- `crates/shm-transport/src/backend/ring.rs:2813-2831` — `validate_lifecycle`.
  It compares the mapped lifecycle page against the *grant*: magic, layout
  version, depth, arena bytes, max leases, total bytes, incarnation, and lane
  (`:2819-2827`). Every comparison is grant-versus-mapping. None is
  profile-versus-grant, so a self-consistent object plus a matching grant passes
  regardless of what the local profile charged.
- `crates/shm-transport/src/backend/ring.rs:1105-1106` — the mapping size comes
  straight from `grant.total_bytes`, so the grant chooses the `mmap` length.
- Every `Ring::attach` call site in the tree passes `(fd, grant, scheduling)` and
  no profile: `ring.rs:979` (`RingAttachment::attach`),
  `packages/shm-native/src/lib.rs:287` (the addon's `attach_ring`, which no
  longer opens `/proc/{pid}/fd/{fd}` and no longer takes a pid),
  the Rust test peer's `attach_ring`, deleted with `shm_provider.rs` in `ed487e11`,
  and `crates/shm-transport/tests/ring.rs:352`, `:379`, `:564`.
- `packages/shm-native/src/lib.rs:644-647` — the addon does check a profile,
  but only as a string: `profile != PROFILE` rejects. `PROFILE` is
  `"host-test-ring-v1"` (`:27`). A name match is not a geometry match.
- `crates/host-runtime/src/ring_transport.rs:1179-1184` —
  `qualified_test_profile_pins_client_grant_geometry` asserts
  `descriptor_depth() == 8`, `max_leases() == 8`, and `arena_bytes() ==
  MIN_ARENA_BYTES`. This pins the host's own profile object, on the creating side.
- `packages/plugin/src/shared/host-client/shm-grant.ts:161-171` (source tree; not at HEAD) — the only
  place any attaching side pins geometry, and it is in TypeScript: exact
  `descriptorDepth`, `arenaBytes`, `maxLeases`, and `reserved` (`geometry_mismatch`
  at `:167` (source tree; not at HEAD)), plus `totalBytes` bounded by `MAX_TOTAL_BYTES` (`:170` (source tree; not at HEAD)), which is
  `ARENA_BYTES + 1_048_576n` (`:76` (source tree; not at HEAD)).
  At HEAD: the attaching-side geometry bounds are in Rust: `MAX_DESCRIPTOR_DEPTH` enforced by `Layout::new`, and the addon's `grant_matches_profile` check against the local profile.
  At HEAD: Call sites now pass `(descriptors, grant)`, and the tree has more of them than this list: `packages/shm-native/src/lib.rs:772` and `:777`, plus `crates/host-runtime/src/ring_transport.rs:877` and `:879`, none of them passing a profile.
  At HEAD: Only `max_spans` is re-checked here; backend and memory-layout agreement is enforced by `TargetProfile::new` before `Ring::create` runs.
  At HEAD: `Ring::attach(descriptors: [OwnedFd; 3], grant: RingGrant)` takes two parameters, still none of them a profile; there is no `scheduling` argument and no prefault, and the body also sets CLOEXEC on all three descriptors, captures the cursor baselines, refuses a quarantined mapping (`:1141`), and runs `conservation_inner(true)` (`:1148`).

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

A grant declares depth 4096 with the arena floor. `checked_layout` accepts it:
depth is nonzero, the arena meets its floor, leases can be set to any value up to
depth, and `total_bytes` can be set to whatever `Layout::new(4096, 67_108_864)`
computes. `validate_lifecycle` accepts it, because the object was initialized from
that same grant. The attaching process maps roughly 1 MiB of extra control region
that its admission charge never accounted for, and the local accounting now
describes an object that was never mapped.

The larger version was the one the TypeScript cap existed for: in the source tree
nothing inside Rust placed a ceiling on depth or total bytes, so a self-consistent
grant with a very large depth reached `mmap` with only `shm-grant.ts:170` in the
way. At HEAD the ceiling is in Rust (`MAX_DESCRIPTOR_DEPTH`, `ring.rs:52`,
enforced at `:286-287`), so every caller that reaches `Ring::attach` — the
addon's own raw boundary, the Rust test peer, a future non-TypeScript client — is
bounded at 4096 descriptors; the record's remaining content is the absence of a
profile match below that bound.

## Timing windows and dependencies

No fault and no window. The gap is a missing parameter, so it holds at every
attach. Directly enables `one-profile-name-denotes-one-geometry`: because attach
never checks the profile's geometry, two artifacts can name one profile with two
geometries and nothing detects it. Also enables the geometry half of
`native-boundary-not-weaker-than-its-wrapper`, since the wrapper's
`geometry_mismatch` has no native counterpart.

## What a test must construct

A grant whose geometry differs from the admitted profile's, driven through the
attaching path. Two arms are worth separating. First, agreement: attach with a
correct grant and assert that the mapped depth, arena bytes, and lease cap equal
the local profile's values. `Ring::attach` is still not told the profile, so at
the Rust boundary this test needs a signature change; at the addon boundary the
comparison is unit-tested (`grant_matches_profile`,
`packages/shm-native/src/lib.rs:1268` and `:1278`), and what is missing is a test
that drives a mismatched grant through `attach` itself and asserts the
`descriptor_error()` refusal (`:710-711`). Second, a ceiling: assert
`Ring::attach` refuses a self-consistent grant whose depth exceeds
`MAX_DESCRIPTOR_DEPTH` (`crates/shm-transport/src/backend/ring.rs:52`, enforced
by `Layout::new` at `:286-287`); the `ring.rs` unit tests at `:3803` and `:3811`
construct exactly that grant, so this arm pins a bound HEAD enforces rather than
a gap.

## Investigation log

### Q: none recorded — the catalog lists "Open questions: None".

The record carries no open question. This log records the check that had to be
run before accepting the claim, since a single missed call site would refute it.

- Sources examined: `crates/shm-transport/src/backend/ring.rs:1095-1150` and
  `:929-946` and `:2813-2831` read in full; every `Ring::attach` and
  `attachment().attach()` call site found by grep across `crates/` and
  `packages/`, excluding `target/`, `node_modules/`, and `dist/`;
  `packages/shm-native/src/lib.rs:286-288` and, at `9c1eb4d1`,
  `crates/host-runtime/src/shm_provider.rs:779-788` for the two `attach_ring`
  wrappers, the host-side one since deleted by `ed487e11`;
  `packages/plugin/src/shared/host-client/shm-grant.ts:146-174` (source tree; not at HEAD) for the
  TypeScript geometry pin.
- Findings: the claim holds. Six call sites, none passing a profile. The two
  `attach_ring` wrappers carried, at `9c1eb4d1`, a `pid` and `fd` and opened
  `/proc/{pid}/fd/{fd}`, so they authenticate the object's provenance, but they
  pass the decoded grant straight through. `create_test_pair`
  (`packages/shm-native/src/lib.rs:931-945`) is the one path that does use a
  profile on both sides, and only because it creates both rings locally and
  attaches via `RingAttachment`, so the grant it attaches with was derived from
  that same profile moments earlier.
- Missing evidence: none. The catalog's citations `ring.rs:1095` and `:929` both
  resolve, and its statement that `checked_layout` "bounds depth only by `!= 0`
  plus layout arithmetic" is accurate; it omits the related
  `max_leases <= descriptor_depth` constraint at `:934`, which bounds leases
  relative to depth but places no absolute ceiling on either.
- Conclusion: resolved with answer. No attach path anywhere in the tree binds
  grant geometry to a local profile, and the only geometry ceiling in the system
  lives in TypeScript.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 27, `crates/shm-transport/src/backend/ring.rs:598-616` now `crates/shm-transport/src/backend/ring.rs:1095-1150`: At HEAD `Ring::attach(descriptors: [OwnedFd; 3], grant: RingGrant)` takes two parameters, still none of them a profile; there is no `scheduling` argument and no prefault, and the body also sets CLOEXEC on all three descriptors, captures the cursor baselines, refuses a quarantined mapping (`:1141`), and runs `conservation_inner(true)` (`:1148`).
  - line 34, `:552-557` now `:1047-1049`: Only `max_spans` is re-checked here; backend and memory-layout agreement is enforced by `TargetProfile::new` before `Ring::create` runs.
  - line 57, `packages/shm-native/src/lib.rs:250` now `packages/shm-native/src/lib.rs:287`: Call sites now pass `(descriptors, grant)`, and the tree has more of them than this list: `packages/shm-native/src/lib.rs:814` and `:819`, plus `crates/host-runtime/src/ring_transport.rs:877` and `:879`, none of them passing a profile.
  - line 61, `packages/shm-native/src/lib.rs:503-506` now `packages/shm-native/src/lib.rs:644-647`: The addon does bind grant geometry to the local profile at HEAD: `grant_matches_profile` (`packages/shm-native/src/lib.rs:257-266`) compares descriptor depth, arena bytes, and max leases against `host_test_ring_profile()`, and `attach` refuses a grant that fails it (`:710-711`), so the addon check is no longer name-only.
  - line 68, `packages/plugin/src/shared/host-client/shm-grant.ts:161-171`: At HEAD the attaching-side geometry bounds are in Rust: `MAX_DESCRIPTOR_DEPTH` enforced by `Layout::new`, and the addon's `grant_matches_profile` check against the local profile.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 31, `:607` (prefault_read): Attach performs no prefault at HEAD; the steps after `validate_lifecycle` are the cursor baseline reads, the quarantine check, and `conservation_inner(true)`.
  - line 68, `packages/plugin/src/shared/host-client/shm-grant.ts:161-171` (TypeScript geometry pin): The whole `packages/plugin` tree no longer exists, so no TypeScript geometry pin remains.
  - line 71, `:167` (geometry_mismatch rejection): `packages/plugin` was deleted; the file no longer exists.
  - line 71, `:170` (totalBytes bounded by MAX_TOTAL_BYTES): `packages/plugin` was deleted; the file no longer exists.
  - line 72, `:76` (MAX_TOTAL_BYTES definition): `packages/plugin` was deleted; the file no longer exists.
  - line 127, `packages/plugin/src/shared/host-client/shm-grant.ts:146-174` (TypeScript geometry pin): The `packages/plugin` tree was deleted, so the conclusion that the only geometry ceiling lives in TypeScript no longer holds.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

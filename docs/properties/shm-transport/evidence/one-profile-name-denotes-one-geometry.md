# one-profile-name-denotes-one-geometry

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

Commit `daf6e244`, "fix(shm): track the ring layout total in the raw descriptor
test grant", is the worked example. Its message records that a hardcoded
`total_bytes` of `arena + 12288` became wrong when the control region grew from
two pages to three, that `RingGrant::decode` recomputed the layout and rejected
the grant, and that only one test noticed because "the other boundary cases
expect a rejection and passed either way". That is a hand-maintained copy of a
derived constant silently weakening a test suite for a day. Searching for other
copies of the same geometry found seven artifacts naming one profile string,
`host-test-ring-v1`, with two different geometries.

## Evidence trail

The two subsections below are the trail: the geometry arithmetic recomputed
from the constants, then the artifact-by-artifact agreement check.

### The arithmetic, computed rather than asserted

`Layout::new` (`crates/shm-transport/src/backend/ring.rs:279-345`) is
deterministic in `(depth, arena_bytes)`. The inputs it depends on:
`CACHELINE = 128` and `PAGE_SIZE = 4096` (`:45-46`); `ProducerPage`,
`ConsumerPage`, and `ReclaimPage`, each `#[repr(C, align(128))]` holding two
`AtomicU64` (`:66-85`), so `size_of` is 128 each; and `DescriptorSlot`,
`#[repr(C, align(128))]` holding `AtomicU8`, two `AtomicU64`, and
`UnsafeCell<SharedDescriptor>` (`:146-154`), where `SharedDescriptor` is
`#[repr(C)]` (`:94-108`).

I compiled those exact declarations and evaluated the arithmetic rather than
computing it by hand. Results: `size_of::<SharedDescriptor>() == 120` and
`size_of::<DescriptorSlot>() == 256`.

Substituting into `Layout::new`, with `arena_bytes = 67_108_864` (the
`MIN_ARENA_BYTES == MAX_FRAME_BYTES == 64 MiB` floor,
`crates/shm-transport/src/arena.rs:4-7`), the control-region prefix is
`align_up(128 + 128 + 128, 128) == 384` in both cases, and:

| depth | `slots + slot_bytes` | `arena` offset | `total` | `total - arena_bytes` |
| --- | --- | --- | --- | --- |
| 8 | `384 + 2048 = 2432` | 4,096 (1 page) | 67,117,056 | **8,192** |
| 32 | `384 + 8192 = 8576` | 12,288 (3 pages) | 67,125,248 | **16,384** |

The overhead is the arena's page-aligned offset plus the trailing lifecycle page
(`crates/shm-transport/src/backend/ring.rs:318-332`), which is why depth 8 yields two pages and depth 32 yields four.

The TypeScript constant `GRANT_LAYOUT_OVERHEAD_BYTES` in
`packages/shm-native/tests/mechanism.ts:145` is `16_384n`. It is **correct for
depth 32 and wrong for depth 8**, understating nothing and overstating the
depth-8 overhead by 8,192 bytes. It is internally consistent, because the same
file declares `GRANT_DESCRIPTOR_DEPTH = 32n` (`:127`) and
`GRANT_MAX_LEASES = 32n` (`:130`). This also confirms the `daf6e244` story
arithmetically: the old value `12_288` is exactly the depth-32 overhead one page
short, matching "assumed the control region ahead of the arena fit in two pages.
It now needs three."

### Which artifacts agree, and which disagree

Depth 8, overhead 8,192:

- `crates/host-runtime/src/ring_transport.rs:38-40` — `qualified_test_profile`:
  `descriptor_depth: DESCRIPTOR_DEPTH` where `DESCRIPTOR_DEPTH = 8` (`:32` (source tree; not at HEAD)),
  `arena_bytes: MIN_ARENA_BYTES`, `max_leases: DESCRIPTOR_DEPTH`.
- `crates/host-runtime/src/ring_transport.rs:1244-1247` — an existing assertion that the
  encoded grant's total equals `(shm_transport::MIN_ARENA_BYTES + 8_192) as
  u64`. Independent confirmation of the depth-8 row above.
- `packages/plugin/src/shared/host-client/shm-grant.ts:67-69` (source tree; not at HEAD) —
  `DESCRIPTOR_DEPTH = 8n`, `ARENA_BYTES = 67_108_864n`, `MAX_LEASES = 8n`,
  enforced at `:160-168` (source tree; not at HEAD) as `geometry_mismatch`.
- `packages/plugin/src/shared/host-client/test-support/shm-grant-fixtures.ts:26-30` (source tree; not at HEAD)
  — `grantHex` defaults: `depth ?? 8n`, `arena ?? 67_108_864n`,
  `maxLeases ?? 8n`, `total ?? arena + 8_192n`.

Depth 32, overhead 16,384:

- `crates/shm-transport/src/profile.rs:703-706` — `ring_profile`:
  `descriptor_depth: 32`, `arena_bytes: MIN_ARENA_BYTES`, `max_leases: 32`.
- `packages/shm-native/tests/mechanism.ts:127-145` — `GRANT_DESCRIPTOR_DEPTH =
  32n`, `GRANT_MAX_LEASES = 32n`, `GRANT_LAYOUT_OVERHEAD_BYTES = 16_384n`,
  assembled at `:159-166`. Its comment at `:126` states the intent explicitly:
  "Geometry of the `host-test-ring-v1` profile (`ring_profile`)."
- `crates/shm-transport/fuzz/corpus/provider_grant/valid` — the golden grant
  fixture, pinned as a hex literal at `crates/shm-transport/tests/fuzz_corpus.rs:94-95`.
  Decoding it gives layout version 2, lane 0, depth 32, arena 67,108,864, leases
  32, total 67,125,248, reserved 0 — overhead 16,384. This fixture doubles as the
  fuzz `provider_grant` seed asserted to be *accepted*.

So four artifacts describe depth 8 and three describe depth 32, under one profile
name. Every artifact is internally consistent and every one of the seven agrees
with `Layout::new` for the depth it declares. The contradiction is entirely in the
name.

## Failure scenario

The name is the only thing a caller matches on: the addon's attach checks
`profile != PROFILE` (`packages/shm-native/src/lib.rs:603-605`) and nothing
more, and `PROFILE` is `"host-test-ring-v1"` (`:27`). Because
`attach-binds-geometry-to-a-local-profile` holds — no attach path compares grant
geometry to a local profile — a depth-32 grant carrying that name is accepted
natively while the host's admission charged for depth 8. The concrete recurrence
mode is the one `daf6e244` already exhibited: change a control-region struct,
and a hand-maintained overhead constant becomes stale in whichever family did not
get updated, degrading tests that expect rejection into tests that pass for the
wrong reason.

## Timing windows and dependencies

No fault, no window. This is static and live at `9c1eb4d1`. Depends on
`attach-binds-geometry-to-a-local-profile` for the disagreement to be
consequential rather than merely untidy; grouped with
`negative-tests-fail-for-their-stated-reason`, because a stale constant in this
cluster degrades negative tests specifically.

## What a test must construct

Nothing to inject — fault class F8, a cross-artifact equality assertion, which
does not exist anywhere in the repository. The assertion needed is: for the single
authoritative `(depth, arena_bytes, max_leases)` tuple, every artifact naming
`host-test-ring-v1` matches it, and every hardcoded overhead constant equals
`Layout::new(depth, arena_bytes).total - arena_bytes`. The second half is
mechanically checkable today from Rust; the first half requires deciding which of
the two geometries the name means.

## Investigation log

### Q: Is the depth-32 fixture a deliberate model of `create_test_pair` (which uses `ring_profile`), in which case the profile string is knowingly overloaded across two geometries?

- Sources examined: `packages/shm-native/tests/mechanism.ts:108-171` for the
  fixture and its `loadRawAddon` path; `packages/shm-native/src/lib.rs:889-953`
  for `create_test_pair`; `crates/shm-transport/src/profile.rs:699-711` for
  `ring_profile`; `git log -1 --format=%B daf6e244`;
  `crates/shm-transport/tests/fuzz_corpus.rs:92-146` for the golden fixture.
- Findings: the fixture is deliberate. `create_test_pair` calls
  `ring_profile(HardwareProfileId::new(PROFILE)?, ColdParkWake)`
  (`lib.rs:891`) where `PROFILE` is the same string the host uses, so the
  addon really does create depth-32 rings under that name, and the fixture's own
  comment at `:126` names `ring_profile` as its source. The golden fuzz seed is a
  third depth-32 artifact, so this is a family, not a one-off. What the sources do
  not establish is whether the overload is *intended* or is an accident of
  `ring_profile` reusing the host's profile string as a hardware-profile id.
  `HardwareProfileId::new(PROFILE)` passes a *profile name* into a *hardware
  profile* slot, which is at least a suspicious reuse.
- Missing evidence: any document or plan stating which geometry
  `host-test-ring-v1` denotes. `docs/shm-transport.md:88` tabulates
  "descriptors 16 total, 8 per direction" and "receive leases 16 total, 8 per
  direction", which matches the depth-8 family only, and never mentions a
  depth-32 variant.
- Conclusion: partially resolved. The fixture's depth-32 choice is deliberate and
  traceable; whether one name may denote two geometries needs human input.
  Independent of that answer, the document at `:88` describes only the
  depth-8 geometry, so the depth-32 family is undocumented.
- Correction recorded while verifying: the assertion message at
  `crates/shm-transport/tests/ring.rs:518-519` (source tree; not at HEAD) instructs a human to "update the
  copy of this hex in
  packages/plugin/src/shared/host-client/shm-transport-provider.test.ts too".
  No copy of that hex exists in `packages/` at `9c1eb4d1`; that test file builds
  grants through `grantHex()` from the depth-8 fixtures module instead. The
  instruction points at a file that can no longer be kept in sync, and following
  it would put a depth-32 literal into a depth-8 suite.

### Q: Do the artifacts still disagree at HEAD? (added 2026-09-05)

- Checked: host `ring_profile()` (`crates/host-runtime/src/ring_transport.rs:38-39`) returns `host_test_ring_profile` (`crates/shm-transport/src/profile.rs:679-692`, depth 8, eight leases); the addon fixture (`packages/shm-native/tests/mechanism.ts:127-130`) encodes depth 8, a 64 MiB arena, and eight leases under `host-test-ring-v1`; the depth-32 `ring_profile(hardware)` (`profile.rs:699-706`) takes an arbitrary id and is not a definition of that profile. `ring_profile_pins_per_connection_grant_geometry` is at `ring_transport.rs:1179`; `host_test_ring_profile_names_one_geometry` is at `crates/shm-transport/tests/profile.rs:202`. `packages/plugin` is not in this tree.
- Conclusion: no. The contradiction is resolved; the record stays active as the cross-artifact equality contract, which no test asserts against the TypeScript fixture.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 41, `:38-54` now `:66-85`: At HEAD the control-region prefix ahead of the slots is five cache lines, because two WakeEpoch pages (`:87-92`) follow the reclaim page, so the prefix is 640 bytes and not the 384 the next paragraph computes; each page also carries an explicit _padding UnsafeCell field. The page-aligned arena offsets and the totals in the table are unchanged, so the two overhead figures still hold.
  - line 64, `packages/shm-native/tests/mechanism.ts:110` now `packages/shm-native/tests/mechanism.ts:145`: At HEAD the constant is 8_192n, which is the depth-8 overhead and agrees with the depth-8 geometry the same file now declares, so it is no longer wrong for depth 8.
  - line 67, `:92` now `:127`: It is 8n at HEAD, not 32n.
  - line 68, `:95` now `:130`: It is 8n at HEAD, not 32n.
  - line 77, `crates/host-runtime/src/ring_transport.rs:47-50` now `crates/host-runtime/src/ring_transport.rs:38-40`: `qualified_test_profile` no longer exists; `ring_profile()` returns `shm_transport::profile::host_test_ring_profile()`, which sets the depth, arena, and lease bound itself.
  - line 92, `crates/shm-transport/src/profile.rs:706-709` now `crates/shm-transport/src/profile.rs:703-706`: At HEAD `ring_profile` takes a caller-supplied HardwareProfileId and is documented as a depth-32 caller-thread profile under an arbitrary id, so it is not a definition of host-test-ring-v1; `host_test_ring_profile` (`:683-697`) is the depth-8 definition of that name.
  - line 94, `packages/shm-native/tests/mechanism.ts:92-110` now `packages/shm-native/tests/mechanism.ts:127-145`: The fixture declares depth 8, eight leases, and an 8,192-byte overhead at HEAD, so it belongs to the depth-8 family and no longer contradicts the host.
  - line 96, `:91` now `:126`: The comment names host_test_ring_profile at HEAD, not ring_profile.
  - line 99, `crates/shm-transport/tests/ring.rs:513-514` now `crates/shm-transport/tests/fuzz_corpus.rs:94-95`: The frozen fixture encodes layout version 3, not 2; lane 0, depth 32, arena 67,108,864, leases 32, and total 67,125,248 are unchanged, so the overhead is still 16,384.
  - line 151, `lib.rs:633-636` now `lib.rs:891`: At HEAD `create_test_pair` calls `host_test_ring_profile()`, so the addon creates depth-8 rings under that name and no longer passes PROFILE into a hardware-profile slot.
  - line 153, `:80` now `:126`: The comment names host_test_ring_profile at HEAD.
  - line 160, `docs/shm-transport.md:99-104` now `docs/shm-transport.md:88`: The document states the per-connection charge in prose at HEAD, 16 ring descriptors and 16 receive leases, rather than in a table, and it still describes only the depth-8 geometry.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 78, `:32` (DESCRIPTOR_DEPTH = 8): The constant moved into the transport crate as HOST_TEST_RING_DEPTH (`crates/shm-transport/src/profile.rs:679`).
  - line 83, `packages/plugin/src/shared/host-client/shm-grant.ts:67-69` (TypeScript depth-8 geometry constants): `packages/plugin` is absent from this tree, as the 2026-09-05 log entry below records.
  - line 85, `:160-168` (geometry_mismatch enforcement): Same missing file.
  - line 86, `packages/plugin/src/shared/host-client/test-support/shm-grant-fixtures.ts:26-30` (grantHex depth-8 defaults): Same missing directory.
  - line 169, `crates/shm-transport/tests/ring.rs:518-519` (an assertion message telling a human to update a packages copy of the hex): The golden-fixture assertion at HEAD reads 'the checked-in fixture bytes moved unexpectedly' (`crates/shm-transport/tests/fuzz_corpus.rs:104-108`) and names no packages copy.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

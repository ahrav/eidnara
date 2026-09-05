# one-profile-id-names-one-ring-geometry-in-code

## Discovery trigger

U3 renamed the ring profile id to `host-test-ring-v1`. A renamed identity is an input to every geometry-bearing artifact, so the in-code half of `one-profile-name-denotes-one-geometry` needed its own record: does the id, in Rust, pin exactly one geometry?

## Evidence trail

- `host_test_ring_profile` (`crates/shm-transport/src/profile.rs:683-692`) builds the geometry from `HOST_TEST_RING_DEPTH` (8, `:679`) and `MIN_ARENA_BYTES`, with `max_leases` equal to the depth.
- `host_test_ring_profile_names_one_geometry` (`crates/shm-transport/tests/profile.rs:202`) asserts the id string, depth 8, eight leases, and sixteen descriptors against literals, and asserts the arena charge equals `2 * shm_transport::MIN_ARENA_BYTES`.
- The host reads the profile through `ring_profile()` (`crates/host-runtime/src/ring_transport.rs:38-39`) and every connection charges `profile.charges()`.
- The addon fixture (`packages/shm-native/tests/mechanism.ts:126-130`) and the addon setup code name the same id.

## Failure scenario

The id survives a geometry change. A peer sized from the id over- or under-runs the ring, and the admission charge per connection silently changes.

## Timing windows and dependencies

None. The profile is a constant.

## What a test must construct

- Present: literal assertions for id, depth, lease bound, and descriptor charge.
- Missing: a literal arena assertion. The test compares `arena_bytes` against `2 * MIN_ARENA_BYTES`, so a change to that constant moves both sides; a spelled 128 MiB (two 64 MiB arenas) would catch it.

## Investigation log

### Q: Does the test pin the arena dimension?

- Sources examined: `crates/shm-transport/tests/profile.rs:202-213`.
- Findings: The arena assertion reads the constant it checks; the other four dimensions are literals.
- Missing evidence: A literal arena value in the test.
- Conclusion: unresolved, needs the test to spell the arena literal; the record is `partial` until then.

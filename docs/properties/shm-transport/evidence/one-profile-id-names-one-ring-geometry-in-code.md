# one-profile-id-names-one-ring-geometry-in-code

## Discovery trigger

U3 renamed the ring profile id to `host-test-ring-v1`. A renamed identity is an input to every geometry-bearing artifact, so the in-code half of `one-profile-name-denotes-one-geometry` needed its own record: does the id, in Rust, pin exactly one geometry?

## Evidence trail

- `host_test_ring_profile` (`crates/shm-transport/src/profile.rs:683-692`) builds the geometry from `HOST_TEST_RING_DEPTH` (8, `:679`) and `MIN_ARENA_BYTES`, with `max_leases` equal to the depth.
- `host_test_ring_profile_names_one_geometry` (`crates/shm-transport/tests/profile.rs:202`) asserts the id string, depth 8, eight leases, sixteen descriptors, and an arena charge of 134,217,728 bytes (two 64 MiB arenas) against literals; none of the five reads the constant it checks.
- The host reads the profile through `ring_profile()` (`crates/host-runtime/src/ring_transport.rs:38-39`) and every connection charges `profile.charges()`.
- The addon fixture (`packages/shm-native/tests/mechanism.ts:106-110`) and the addon setup code name the same id.

## Failure scenario

The id survives a geometry change. A peer sized from the id over- or under-runs the ring, and the admission charge per connection silently changes.

## Timing windows and dependencies

None. The profile is a constant.

## What a test must construct

- Present: literal assertions for id, depth, lease bound, descriptor charge, and arena charge. A change to `MIN_ARENA_BYTES` or `HOST_TEST_RING_DEPTH` under an unchanged id fails the test.
- Missing: nothing for the in-code half; the cross-peer half belongs to `one-profile-name-denotes-one-geometry`.

## Investigation log

### Q: Does the test pin the arena dimension?

- Sources examined: `crates/shm-transport/tests/profile.rs:202-213`.
- Findings: All five dimensions are literals. The arena assertion compared against `2 * MIN_ARENA_BYTES` when the record was first written, so a change to that constant moved both sides; the test now names 134,217,728 bytes.
- Missing evidence: none.
- Conclusion: resolved; the test pins every dimension the id denotes.

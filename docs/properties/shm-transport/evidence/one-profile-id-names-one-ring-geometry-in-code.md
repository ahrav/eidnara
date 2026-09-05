# `one-profile-id-names-one-ring-geometry-in-code`

- **Discovery:** U3, when the profile id was renamed.
- **Primary evidence:** `host_test_ring_profile` (`crates/shm-transport/src/profile.rs`) builds the geometry from `HOST_TEST_RING_DEPTH` and `MIN_ARENA_BYTES`; `host_test_ring_profile_names_one_geometry` (`crates/shm-transport/tests/profile.rs:202`, added at U3) compares the id, depth, lease bound, and the derived charges against literals spelled in the test. The addon's setup fixture (`packages/shm-native/src/setup.rs`) and `packages/shm-native/tests/mechanism.ts` name the same id.
- **Existing evidence:** the tests named above; `crates/host-runtime/src/ring_transport.rs` reads the profile through `ring_profile()` and every connection charges `profile.charges()`.
- **Failure scenario:** the id survives a geometry change, so a peer sized from the id over- or under-runs the ring.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U3):** pass for the in-code half: the id, depth, lease bound, and charges are spelled in the test; the arena size is not asserted because it would compare `MIN_ARENA_BYTES` to itself. The cross-peer half (a peer that echoes the id exercises the geometry) is the sibling record `one-profile-name-denotes-one-geometry`, which keeps its source status.
- **Open-question log:** none.

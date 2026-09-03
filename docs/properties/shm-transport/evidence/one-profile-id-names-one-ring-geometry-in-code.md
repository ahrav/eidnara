# `one-profile-id-names-one-ring-geometry-in-code`

- **Discovery:** U3, when the profile id was renamed.
- **Primary evidence:** `host_test_ring_profile` (`crates/shm-transport/src/profile.rs`) builds the geometry from literals; `host_test_ring_profile_names_one_geometry` (added at U3) compares the id, depth, arena size, span bound, topology, and charges against literals spelled in the test. The addon asserts the same id in `grant_message_accepts_tagged_setup_envelope`, and the package test `packages/shm-native/tests/mechanism.ts` states the geometry of `host-test-ring-v1` independently.
- **Existing evidence:** the tests named above; `crates/host-runtime/src/ring_transport.rs` reads the profile through `ring_profile()` and every connection charges `profile.charges()`.
- **Failure scenario:** the id survives a geometry change, so a peer sized from the id over- or under-runs the ring.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U3):** pass for the in-code half: the test spells the literals rather than reading the constants under test. The cross-peer half (a peer that echoes the id exercises the geometry) is the sibling record `one-profile-name-denotes-one-geometry`, which keeps its source status.
- **Open-question log:** none.

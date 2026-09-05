# iceoryx-cross-process-pairing-is-reachable-or-declared

Record invalidated 2026-08-31: iceoryx2 backend removed in `0f336d3c`; absent at
HEAD `46278f47a` after PR #131 (merge `5d638e3e8`).

## Discovery trigger

The ring's cross-process story is explicit and checkable: a `RingGrant` carrying
the incarnation is encoded, transferred alongside a duplicated file descriptor,
and re-verified against the shared lifecycle page at attach. The iceoryx backend
has no grant, no descriptor transfer, no seals, and no lifecycle page. The
question "what authenticates the peer" therefore has to be asked before any
parity claim, and the answer turns out to be that no peer can exist: the backend
creates both ports itself, under a service name it never discloses.

## Evidence trail

- **The cited mechanism is gone.** `0f336d3c` ("refactor(shm): collapse to fixed
  ring transport") deleted `crates/shm-transport/src/backend/iceoryx.rs`,
  `crates/shm-transport/tests/iceoryx.rs`, and the `iceoryx` Cargo feature, so
  `backend/mod.rs` now declares only `ring` and `sample`. Every `iceoryx.rs`
  citation below is kept as a record of what the removed backend did and did not
  guarantee, and resolves against `9c1eb4d1`, not HEAD. No successor backend
  exists in the tree.

- `backend/iceoryx.rs:57-69` — the service name is built from 16 bytes of
  `getrandom` output formatted as `shm-` plus 32 hex characters, inside
  `create`. It is not a parameter, it is not stored on the struct
  (`:36-46`), there is no accessor, and no other function in the file mentions
  `ServiceName`. Nothing in the repository can learn or supply it, so
  `open_or_create` at `:82` always creates.
- `backend/iceoryx.rs:76-77` — `max_publishers(1)` and `max_subscribers(1)`.
  `:84-88` creates the subscriber and `:89-106` creates the publisher, both on
  the same instance, so both slots are consumed by the creator. In iceoryx2
  0.9.3 a second port beyond the bound fails at creation:
  `src/port/publisher.rs:567` returns
  `PublisherCreateError::ExceedsMaxSupportedPublishers` and
  `src/port/subscriber.rs:328` returns
  `SubscriberCreateError::ExceedsMaxSupportedSubscribers`. Since
  `IceoryxBackend::create` builds one of each, a second process that somehow
  guessed the name could not construct a backend against a live pair at all.
- `backend/iceoryx.rs:56` and `:163` — the incarnation is minted locally by
  `Incarnation::random()` (`descriptor.rs:127-131`), and `try_receive` builds the
  expected identity from `self.incarnation`. There is no accessor for it, no
  constructor that accepts one, and no encode or decode path. Compare
  `backend/sample.rs:82`: a sample whose prefix carries any other incarnation
  is rejected as `WrongIncarnation`. So even a hypothetical second participant
  would have every one of its samples refused.
- `backend/ring.rs:851-962` `RingGrant` with `encode` (`:876`), `decode`
  (`:893`), and `decode_slice` (`:924`); `:1153-1155` `grant()`; `:1158-1160`
  `raw_fd()`; `:1247-1261` `attachment()` duplicating the descriptor with
  `F_DUPFD_CLOEXEC`. That is the transfer channel the iceoryx backend lacks.
- `backend/ring.rs:1107` calls `validate_lifecycle`, which at `:2813-2831` reads
  eight fields from the shared page and compares all eight against the grant,
  including the incarnation (`snapshot.6` at `:2825`, compared at `:2825`). On
  Linux `create_in` also seals the object (`:1063-1064`) and
  `validate_seals` (`:2848-2854`) requires `F_SEAL_GROW|SHRINK|SEAL`. None of
  these three mechanisms — grant equality, incarnation equality against shared
  state, or seals — exists on the iceoryx path.
- `crates/shm-transport/tests/iceoryx.rs` — all seven tests construct exactly
  one `IceoryxBackend` and use it as both producer and receiver (`:79`, `:108`,
  `:123`, `:141`, `:289`; the two decoder tests at `:164` and `:233` call
  `SamplePrefix` directly and touch no backend at all).
  `benches/hardware_envelope.rs:564` (source tree; not at HEAD) does the same, and the bench report
  classifies the arm accordingly: `loopback_smoke_arms: ["iceoryx_0_9_3"]`
  (`:141` (source tree; not at HEAD)), against nine `paired_process_arms` at `:289` that include `ring`.
- `crates/shm-transport/tests/ring.rs:489`
  `two_process_zero_copy_exchange_uses_authenticated_grant` is the ring's
  two-process test. There is no iceoryx analogue, and none can be written against
  this API.
- `crates/shm-transport/Cargo.toml:9-10` (source tree; not at HEAD) — `default = ["iceoryx"]`. The
  backend is compiled by default *for the transport crate*.
  `crates/host-runtime/Cargo.toml:25` and `packages/shm-native/Cargo.toml:17` both
  depend with `default-features = false`, so neither the host nor the shipped
  addon contains it.
  At HEAD: Neither dependent sets default-features = false any more, because shm-transport no longer declares any features.
  At HEAD: the report emits only paired_process_arms with six arms, h0_metadata_cacheline_ping_pong, the four ownership ablations, and ring, and no loopback classification exists.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

Nothing breaks at runtime; the loss is evidential. The iceoryx backend is
`selectable` in the release-gate manifest
(`benches/manifests/v1.json:107-110` (source tree; not at HEAD)) as one of two candidate providers for a
transport whose entire purpose is moving frames between the host process and a
JavaScript runtime process. Every observation that exists about it was produced
by one process talking to itself.

The concrete consequence for the properties this catalog already holds: a
same-instance exercise structurally cannot construct the second participant, so
it cannot prove any property whose predicate ranges over two address spaces or
two incarnations. That covers publication visibility across a real
release-acquire edge, peer authentication at attach, geometry binding, restart
reconciliation, and stale-cursor handling — five of the ring's groups. It is not
that these are untested on iceoryx; it is that no test written against
`IceoryxBackend::create(profile, lane)` can reach them, because the API admits
neither an inbound service name nor an inbound incarnation. The loopback also
suppresses the failure mode of
`iceoryx-receive-expectation-tracks-the-delivered-stream`: with one instance
owning both cursors, the restart divergence that record derives cannot occur,
which is why that record is latent rather than live.

## Timing windows and dependencies

No window and no fault; this is a static property of the constructor. The one
runtime dependency worth recording is that the tests do run. Verified by
executing `cargo nextest list -p shm-transport -p shm-native` at
`4d781582`: the listing includes `shm-transport::iceoryx` with all seven
tests, because selecting the transport crate on the command line enables its
default features regardless of the two dependents' `default-features = false`.
That is the command at `the source repository `ci.yml` workflow:162`, guarded by
`if: runner.os == 'Linux'`, so the iceoryx suite executes in CI on Linux. The
macOS branch at `:172-173` selects only `--test contract --test fuzz_corpus`, so
it never runs there, while `cargo check -p shm-transport --features iceoryx`
at `:157` compiles it on both. This corrects
`existing-checks.md:56` (source tree; not at HEAD), which states the suite is "not executed anywhere in CI;
only `cargo check --features iceoryx` runs."

## What a test must construct

Two distinct processes exchanging one frame over iceoryx, with the receiving side
refusing a mismatched peer identity. That requires an API change first, so the
test cannot be written today: `create` must accept a service name and an
incarnation from an authenticated setup channel — the same way `Ring::attach`
takes a `RingGrant` — and expose the pair for the creating side to publish. Until
then the campaign obligation is the declaration, not the test: assert that no arm
whose evidence is loopback-only is marked `selectable`, and that the bench
report's `loopback_smoke_arms` and the manifest's `selectable` list do not
overlap. Today they do, on this arm. Coverage check to emit:
`shm_iceoryx_two_process_exchange`, which will not fire, and whose not firing is
the evidence.

## Investigation log

### Q: Given no grant, no descriptor, no seals, and no lifecycle page, what authenticates a peer on the iceoryx path?

- Sources examined: `backend/iceoryx.rs:48-118`, `:150-176`, `:36-46`;
  `crates/shm-transport/src/descriptor.rs:122-145`;
  `backend/sample.rs:74-127`; `backend/ring.rs:851-962`, `:1040-1150`,
  `:1153-1160`, `:2813-2831`, `:2848-2854`; `tests/iceoryx.rs` in full;
  `tests/ring.rs:489`; `benches/hardware_envelope.rs:289`, `:531-598` (source tree; not at HEAD);
  `benches/manifests/v1.json:100-155` (source tree; not at HEAD); the three `Cargo.toml` files;
  `the source repository `ci.yml` workflow:154-176`; and in iceoryx2 0.9.3,
  `src/port/publisher.rs:560-570` and `src/port/subscriber.rs:320-332`.
- Findings: nothing authenticates a peer, because the design admits no peer. The
  service name is locally random and undisclosed, both port slots are consumed by
  the creator under caps of one, and the expected incarnation is the local one.
  Three independent facts, each sufficient on its own. The backend is a loopback,
  and the bench's own report already says so at `:141` (source tree; not at HEAD) while the manifest calls
  the arm selectable at `:107-110` (source tree; not at HEAD).
- Missing evidence: whether loopback is the intended permanent shape of this
  candidate or a scaffold pending a grant-equivalent. The transport document
  scopes it as "a source-built candidate, not a selected backend"
  (`docs/shm-transport.md:120` (source tree; not at HEAD)), which is consistent with either reading.
- Conclusion: resolved with answer, and it reframes the gap. The parity question
  is not which ring guarantees the iceoryx backend fails to meet; it is that a
  whole class of them is unprovable on it by construction, and that the release
  gate does not currently notice.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 66, `:140` now `:289`: At HEAD the report emits only paired_process_arms with six arms, h0_metadata_cacheline_ping_pong, the four ownership ablations, and ring, and no loopback classification exists.
  - line 73, `packages/shm-native/Cargo.toml:16` now `packages/shm-native/Cargo.toml:17`: Neither dependent sets default-features = false any more, because shm-transport no longer declares any features.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 64, `benches/hardware_envelope.rs:564` (the iceoryx bench arm): No iceoryx arm remains in the bench; the file contains no occurrence of the name.
  - line 66, `:141` (loopback_smoke_arms in the bench report): The report has no loopback_smoke_arms key at HEAD.
  - line 71, `crates/shm-transport/Cargo.toml:9-10` (default = [iceoryx] feature default): The manifest has no [features] section at HEAD; the iceoryx feature was deleted with the backend.
  - line 81, `benches/manifests/v1.json:107-110` (the selectable iceoryx candidate provider): The manifest has no selectable key and its transport arm list is [ring] at `:95`.
  - line 113, `existing-checks.md:56` (the claim that the iceoryx suite is not executed in CI): The quoted sentence no longer appears anywhere in the inventory; line 56 is now a section header, and the retired suite is recorded at `:404-410`.
  - line 138, `:531-598` (the iceoryx bench arm body): The arm was deleted with the backend.
  - line 139, `benches/manifests/v1.json:100-155` (the manifest's candidate provider block): The manifest is 122 lines at HEAD and contains no candidate provider or selectable list.
  - line 146, `:141` (the bench report calling the arm loopback): No loopback_smoke_arms key exists at HEAD.
  - line 147, `:107-110` (the manifest calling the arm selectable): No selectable key exists in benches/manifests/v1.json at HEAD.
  - line 151, `docs/shm-transport.md:120` (the source-built candidate scoping sentence): The transport document is 98 lines at HEAD and no longer mentions a candidate backend.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

# U3 review notes

Per-file review evidence for the U3 receipt. Source is
`host@39e8230371b15966a5767771daf001e44d191aac`. Anchors below are the
`review_evidence` pointers in `receipt.json`.

## What the wave moved

Four source trees: the host runtime crate, the shared-memory transport crate
(with its fuzz workspace and hardware-envelope bench), the tokenizer crate, and
the native addon package, plus the wire-protocol and transport documents the
crates cite, the perf script the host tests parse, and the harness-closure
fixture the digest test reads. About 70,000 lines of Rust.

Every file went through one rename pass (an ordered replacement table applied
to text and to path components; the table spells the retired names, so it
lives outside this tree), then `cargo fmt`,
`cargo clippy --fix` for the nested `if` blocks Rust 2024 expresses as let
chains, and the hand edits listed under each anchor. A file whose destination
bytes equal the source blob is `verbatim`; a file that differs only by the
rename table and formatting is `renamed`; a file with any hand edit is
`adapted`. The receipt's `transformation` field records which.

## Doc-rigor: Rust sources

Read in full, with docs written or corrected where the lints required it:
`crates/shm-transport/src/{lib,descriptor,lease,lifecycle,harness}.rs`,
`crates/shm-transport/src/backend/sample.rs`, `crates/shm-transport/src/profile.rs`,
`crates/shm-transport/tests/{ring,fuzz_corpus}.rs`,
`crates/host-runtime/src/{instance,config,handler,auth}.rs`,
`crates/host-runtime/src/broca/{subprocess,backend,supervisor,protocol}.rs`,
`crates/host-runtime/tests/{protocol_vectors,harness_closure,synapse_protocol,perf_budget_runner,synapse_bundle}.rs`,
`crates/tokenizer/src/lib.rs`, `crates/tokenizer/tests/token_golden.rs`.

Read for the rename and the lint output only: every other Rust file in the
four trees. For those files the review established that the renamed
identifiers compile, that every retired name is gone (the registry gate scans
every tracked file), that rustdoc builds with `-D warnings`, and that the
tests pass; it did not re-derive each function's contract from its body.
Several source files carry doc fragments the source's earlier comment pass left
behind (a line that reads only `/// floor.` or `/// order.`). Those were
corrected where a lint flagged them and left otherwise; they are recorded here
so the next pass over `host-runtime` knows they exist.

What was checked and changed:

- `shm-transport` compiles with `#![warn(missing_docs)]`; the workspace lints
  with `-D warnings`, so its 77 undocumented public items each received a
  doc stating mechanism: descriptor validation order, lease span access rules,
  the close-state successor table, sample prefix layout, and the module docs.
- Twelve empty doc comments across `host-runtime` (`HostHandler`,
  `begin_stopping`, `run_route_gone`, `SCRATCH_RESERVED_BYTES`,
  `request_deadline`, `harness_unavailable_reason`,
  `HarnessDispatchBackend`, `constant_time_eq`, and the module docs of
  `runtime`, `config`, `broca::protocol`, and the transport's `fuzz_corpus`
  test) were written from the code they sit on.
- `max_resident_bytes` linked its docs to private constants; the links are
  plain text now.
- Rust 2024: `gen` is reserved. The locals in `connection.rs`, `dispatch.rs`,
  `runtime.rs`, `tests/dispatch.rs`, and the `gen` field in `routing.rs` are
  `generation`; the routing test helper that was named `generation` is
  `generation_core`. `std::env::set_var` is unsafe: `instance.rs` splits the
  data-root resolver into `default_data_root(DataRootEnv { xdg_data_home, home })`
  so its test passes values instead of mutating the process environment (the
  struct's named fields keep the two values from being swapped at the one
  call site), and the fixture child in `tests/broca_subprocess.rs` marks its
  writes `unsafe` with the single-thread argument.
- `tests/ring.rs`: the unsealed-object test counted every ring mapping in the
  process and raced other tests in the same binary (it failed about half the
  runs here); it now counts mappings of the object it created.
- `tests/synapse_protocol.rs`: `boundary_waiters_with_maximal_texts_are_all_admitted`
  opens 33 ring clients, but `MAX_RING_RESIDENT_BYTES` admits eight rings per
  process, so the ninth setup socket closes and the test hangs on the paused
  clock. Source CI never ran this file. Its ring-free half (the startup formula
  admits the boundary and refuses boundary-plus-one) is split out as
  `waiter_boundary_is_the_last_feasible_startup_configuration` and runs; the
  ring half is `#[ignore]` with that reason and the receipt records it under
  `known_red`. The fix belongs upstream.
- `tests/perf_budget_runner.rs` reads the perf script from the crate's own
  `scripts/` directory; the script's `ROOT` is three levels up.
- `tests/harness_closure.rs` reads the closure fixture from the crate's own
  `tests/fixtures/` directory; the digest test is `canonical_manifest_digest_is_pinned`,
  renamed from a name that claimed a cross-language agreement no test in this
  tree performs (the TypeScript side lands in U7).
- Two checks were added for renamed identities the source never pinned:
  `host_test_ring_profile_names_one_geometry` (`crates/shm-transport/src/profile.rs`)
  and `credential_fingerprint_matches_the_committed_vector`
  (`crates/host-runtime/src/broca/subprocess.rs`).
- The source's task references and repository paths were removed from every
  comment and doc the rename touched.

## Doc-rigor: TypeScript and scripts

`packages/shm-native/index.ts` and its four test files were read in full for
the loader contract (payload package selection, capability reporting, the
error taxonomy). `index.ts` and `tests/runtime.ts` typecheck under the
package's own `tsconfig.json`, which includes only those two; the Bun suite
passes against a release build of the cdylib. The tokenizer
generators resolve `ai-tokenizer` from the repository root, where it is a dev
dependency, and `gen-claude-vocab.ts` handles the two indexed reads the
stricter root `tsconfig.json` flags. `crates/host-runtime/scripts/perf-host.sh`
was read for its `ROOT` computation and its arm tables, which
`perf_budget_runner.rs` parses. `tests/fixtures/generate-synapse-tiny.py` was
read for `canonical_fingerprint`, which mirrors the Rust function.

## Doc-rigor: documents

`docs/host-wire-protocol.md` and `docs/shm-transport.md` were read in full.
Changes beyond the rename: the task references, the source repository's
historical document paths, and the sentence that explained why some literals
kept the predecessor's spelling are gone; the canonical `route.open` example
in section 6.4 is 167 bytes with header `a7 00 00 00 ...` because the module
id it targets is `context`.

## Doc-rigor: property catalogs

The seven source catalogs (transport; host lifecycle, ring datapath, setup
identity, client peer, request path, runtime config) were carried with the
rename table plus a short catalog-specific table: source tracker references
become prose, the source checkout path becomes
"the `host` source checkout", and the source workflow path is named in words.
The six host catalogs are merged into `docs/properties/host-runtime/catalog.md`
under one H1; each area keeps its existing-check inventory, fault map,
portfolio evaluation, and lenses under its own directory; every evidence file
is in one `evidence/` directory (no slug collides). Seven records that carried
the interim status `superseded-by-refactor` read `Status: invalidated` with an
`Invalidated:` field naming the removal. One `Exercised: no` reads `not yet`.
Two `Reachability:` fields that opened with bold prose open with the enum
value. Record bodies were not otherwise edited; their line citations name
source lines at the time each catalog was written, and their test names are
the stable anchors `property-impact.json` resolves.

The "Discovered at U3" sections and `docs/properties/tokenizer/catalog.md`
are authored here. The `core` records were written from the code and the
regeneration work; the Broca, Synapse, and addon records were written from the
existing test suites and enter at the status observed at discovery.

## Regenerated fixtures

Each of these has a renamed identity as an input, so under R18 it was
regenerated once with an oracle outside the implementation, and the registry
lists it as a `byte-stable` fixture pinned by this receipt:

| Fixture | Renamed input | Oracle |
| --- | --- | --- |
| `setup_auth::vectors` and the `auth.rs`, `protocol_vectors.rs`, and addon `setup.rs` literals | domain separators, `daemon_ver` prefix | Python HMAC over the documented transcript; reproduced the predecessor vectors from the predecessor strings |
| `tests/fixtures/harness-closures/pi-valid.json` digest | `schema` field | Python canonical JSON digest; reproduced the predecessor digest |
| `tests/fixtures/synapse-tiny/manifest.json` `fingerprint` | pre-image first line | the generator's Python `canonical_fingerprint`; reproduced the predecessor value; artifacts unchanged |
| `tests/broca_subprocess.rs` credential fingerprint literal | domain separator | Python HMAC derivation; reproduced the predecessor value |
| `protocol_vectors.rs` canonical `route.open` header | module id | byte count of the literal body |
| `crates/tokenizer/testdata/token-golden.json` | corpus texts | `gen/gen-token-golden.ts` against `ai-tokenizer@1.0.6` |

Behavior-only goldens (the transport fuzz corpus, the Synapse tiny model and
corpus, the vocabulary asset) are byte-identical to their source blobs. The
bench manifest's `experiment` name carried the crate prefix and is renamed.

## Binary and generated inputs

`model.onnx`, `embedding.bin`, `corpus.json`, and the tokenizer JSON files
under `tests/fixtures/synapse-tiny/` are outputs of `generate-synapse-tiny.py`;
the fuzz corpus seeds are the inputs `tests/fuzz_corpus.rs` replays;
`assets/claude.tiktoken` is the output of `gen-claude-vocab.ts`;
`benches/manifests/v1.json` is the bench's committed manifest. Each is
recorded as `generated` with its regeneration command. `Cargo.lock` and
`bun.lock` are lockfile output.

## Authored control records

`migration/waves/U3/{receipt,property-impact,architecture-impact,waivers}.json`
and this file implement the wave's requirements: the receipt pins every source
blob and destination hash and declares the four source trees as scope; the
impact closure classifies every record of the three catalogs over the wave's
Rust files (`core`, `invalidated`, or `carried-forward` with the source status
verbatim; the counts are in the file);
the architecture record holds the pre-port and post-integration reports with
three recorded, non-blocking candidates. The registry gains the renamed
identities, the addon's TypeScript classification, the fixtures, and the
authored files. `scripts/eidnara-migration/check.test.ts` and
`bun run check:repo` exercise them.

## Architecture review

The pre-port review ran over the source trees at the pinned commit and the
post-integration review over the destination modules, using the rubric in
`docs/runbooks/architecture-review.md`; the metric is public items over
implementation lines outside `cfg(test)`. Both reports are in the OS temp
directory and hashed in `architecture-impact.json`. No Strong candidate.
Three Worth-exploring candidates are recorded for after the wave: the unused
producer half of `frame_channel`, the two forwarding wrappers around the shared
setup proof, and the duplicated filesystem-policy helpers in
`harness_closure.rs`. None was applied in U3 because the wave's rule is no
behavior change beyond renames.

## Landing as a stacked decomposition

The wave landed as seventeen stacked pull requests instead of the four this
record was first written against: five for the transport and tokenizer (#18 to
#22, replacing #8) and twelve for the host runtime, addon, catalogs, and this
record (#31 to #42, replacing #9, #10, #11). The receipt's source blobs and
review evidence are unchanged; the destination hashes were recomputed over the
stacked tree. What differs from the four-PR tree:

- The shm-transport tree is the reviewed one from #18 to #21, not #8's. Thirty
  files under `crates/shm-transport` and `crates/tokenizer` therefore carry
  different destination bytes; seven that were `verbatim` against the source
  blob are `adapted` now (`fuzz/Cargo.lock`, the three `near-valid` corpora,
  `arena.rs`, `evidence.rs`, `backend/mod.rs`). The #11 review fixes to
  `shm-transport/src/{lease,lifecycle,profile}.rs`, `tests/ring.rs`, and
  `tokenizer/gen/gen-claude-vocab.ts` were superseded by #20 and #22 and are not
  carried.
- `shm_transport::setup_auth::compute_proof` takes `daemon_id` as
  `&[u8; DAEMON_ID_LEN]`; `host_runtime::auth::compute_proof` (two lines, one
  in its vector test) and the addon's `setup::proof` (one line) follow.
- `Ring::release(ReleaseIdentity)` is crate-private after #20. The addon keeps
  each `ReceiveLease<'static>` in `ActiveLease` behind a boxed `from_host` ring,
  mirroring the producer side, and releases through the lease
  (`packages/shm-native/src/lib.rs`, 41 lines). A mechanism test covers slot
  return and the full-ring rejection. The raw-addon readiness test guards one
  callback after close, because the socketpair doorbell makes a peer's exit
  visible to the consumer; the production loader already unregisters first.
- Seven `property-impact.json` check pointers moved with the tests #18 to #21
  relocated or replaced: `tests/contract.rs` cases to `tests/profile.rs` and
  `tests/evidence.rs`; `stale_lap_release_cannot_complete_recycled_slot` into
  `src/backend/ring.rs`; `host_test_ring_profile_names_one_geometry` to
  `tests/profile.rs`; the decoder-totality length sweep to the fuzz-corpus
  replay `frame_descriptor_corpus_replays_without_panic`; the FIFO reclamation
  test to `retained_oldest_lease_enforces_fifo_reclamation` (its
  release-validation half is `stale_lap_release_cannot_complete_recycled_slot`);
  and the doorbell attach gate to
  `doorbell_attachment_requires_connected_unix_stream_socket`. The transport
  catalog prose under `docs/properties/shm-transport/` still describes the
  eventfd doorbell and cites #8-era line numbers; that is a catalog revision
  for a later wave, not a check failure.
- The post-integration architecture review ran a second iteration against the
  stacked tree (`architecture-impact.json` `reports[1]`). The three
  Worth-exploring candidates stand; the lease-ownership change added one
  Speculative loop-created candidate (three copies of the `&'static Ring`
  borrow in the addon). No Strong candidate. Iteration 1 is superseded in the
  record because the checker verifies every post-integration report against
  HEAD; its report hash is named in the iteration-2 summary.

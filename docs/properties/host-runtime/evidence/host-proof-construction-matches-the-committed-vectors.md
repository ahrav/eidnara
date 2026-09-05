# host-proof-construction-matches-the-committed-vectors

## Discovery trigger

The authentication domain separators and the daemon version prefix were
renamed at U3, so every committed proof vector had to be regenerated. A
proof transcript is the one place where a symmetric change is invisible to
the code's own tests: if both the host and its in-crate client apply the same
wrong transcript, every handshake still succeeds. The protocol's fixture rule
at `docs/host-wire-protocol.md:955` requires an independent oracle for that
reason. The audit checked that the crate's `compute_proof` is the shared
transport transcript, that its output over the committed inputs equals what
an implementation outside the crate produces, and that every field is folded
into the digest.

## Evidence trail

All references are at `572315a`.

Production construction. `auth::compute_proof` (`auth.rs:119-134`) forwards
to `shm_transport::setup_auth::compute_proof`
(`crates/shm-transport/src/setup_auth.rs:40-59`), whose `transcript_mac`
(`:89-107`) is `HMAC-SHA256(key, domain || client_nonce || server_nonce ||
u32be(len(daemon_ver)) || daemon_ver || daemon_id)`. The domains are
`eidnara-server-v1` and `eidnara-client-v1` (`:28`, `:30`), re-exported into
`auth.rs:13` as `SERVER_PROOF_DOMAIN` and `CLIENT_AUTH_DOMAIN`. The server
side computes the server proof at `auth.rs:216-223` and the expected client
auth at `:239-246`, comparing in constant time at `:247`. The client side
verifies the server proof at `:298-307` before checking daemon id and daemon
version (`:310-317`), then computes its auth at `:319`. The host calls
`authenticate_server` from `connection.rs:91` and `setup_socket.rs:655`; the
managed client calls `authenticate_client` from `client.rs:334`.

Independent oracle. `tests/support/raw_client.rs:1-7` states that the module
reimplements framing and proof from the protocol's literal values and must
never call the crate's proof helpers. Its `proof` at `:251-269` builds the
same HMAC layout from its own constants `SERVER_DOMAIN` and `CLIENT_DOMAIN`
(`:24-25`). The doc string at `:250` spells the layout.

Committed vectors. `shm_transport::setup_auth::vectors` (`setup_auth.rs:112`
onward) pins `DAEMON_VER = "eidnara-host/0.1.0"` (`:117`), `SERVER_PROOF`
starting `89, 41, 95, 101` (`:120-123`), and `CLIENT_AUTH` starting
`140, 161, 69, 27` (`:126-129`), over key `00..1f`, client nonce `20..3f`,
server nonce `40..5f`, daemon id `60..6f` (`:131`). The audit reproduced
both values with a Python `hmac.new(key, ..., sha256)` over the documented
layout: `59295f65...4038` for the server domain and `8ca1451b...d79f` for the
client domain.

Existing checks, verified, all run under `cargo test --workspace
--all-targets` (`.github/workflows/ci.yml:118`, and again on stable at
`:126`):

- `committed_wire_vectors_pin_the_proof_construction`
  (`auth.rs:640-672`) calls the crate's `compute_proof` over the vector
  inputs (`:642-645`) for both domains and compares the hex to literals
  (`:647-656`). This test proves the crate matches the literals, not that the
  literals are right.
- `committed_auth_proof_vectors_pin_the_construction`
  (`tests/protocol_vectors.rs:33-72`) calls `raw_client::proof` over
  `vector_inputs` (`:23-31`) with `VECTOR_DAEMON_VER = "eidnara-host/0.1.0"`
  (`:20`) and compares to byte literals (`:36-43`). The literals equal the
  crate's, so the two implementations agree. It also asserts the two domains
  yield distinct proofs (`:71`).
- `proof_folds_every_input` (`:75-157`) flips one byte of the key, client
  nonce, server nonce, and daemon id, and changes the daemon version string,
  asserting each perturbation changes the oracle's proof (`:152-155`).
- `host_authenticates_against_the_independent_oracle` (`:221-227`) completes
  a handshake between the real host and the oracle client, so the two
  implementations agree at runtime, not only on the vector inputs.

## Failure scenario

1. A refactor changes `transcript_mac` to hash `daemon_ver` without its
   length prefix.
2. The host and the in-crate client both use `compute_proof`, so every
   handshake between them still succeeds, and the in-crate handshake tests
   stay green.
3. `committed_wire_vectors_pin_the_proof_construction` fails because the
   crate's output no longer matches the literal, and
   `host_authenticates_against_the_independent_oracle` fails because the
   oracle still computes the documented transcript.

The impact named in the record has two directions: a conforming external
client that cannot authenticate, or, if a weakened transcript were also
adopted by a rogue listener, an impostor that can.

## Timing windows and dependencies

None on the proof itself; it is a pure function. The vector tests depend on
the `hmac` and `sha2` crates in both the production path and the oracle, so
a defect in those crates would not be detected by this pair. The Python
reproduction is the only check independent of that dependency, and it is not
committed to the tree.

## What a test must construct

The existing fixed-vector pair is necessary but not the record's `always` check:
the two implementations are never called against each other, so a defect that
returns the committed literal for the fixture while mis-encoding another nonce,
identity, or version passes both. The check needs direct
`compute_proof(...) == raw_client::proof(...)` comparisons over generated and
single-field-perturbed input tuples, with distinct inputs required to give
distinct proofs. A second gap remains: the protocol document's own example vectors do not match the code
(see the second question below). A test that decodes the JSON examples in
`docs/host-wire-protocol.md` section 5.2 and compares them to
`shm_transport::setup_auth::vectors` would catch documentation drift of this
kind; today no test reads those bytes.

## Investigation log

### Q: Do the in-crate and independent vectors describe the same transcript?

- Sources examined: `auth.rs:640-672`; `tests/protocol_vectors.rs:20-72`;
  `tests/support/raw_client.rs:24-25`, `:250-269`;
  `crates/shm-transport/src/setup_auth.rs:28-30`, `:89-107`, `:112-131`.
- Findings: the crate literal `59295f650f2b...` is byte-for-byte the oracle
  literal `[89, 41, 95, 101, 15, 43, ...]`, and the client literals match
  the same way. Both use daemon version `eidnara-host/0.1.0`. A Python HMAC
  over the layout at `raw_client.rs:250` reproduced both values.
- Missing evidence: none.
- Conclusion: resolved. Both tests pin the same transcript, and an oracle
  written from the layout description alone reproduces it.

### Q: Do the protocol document's example proofs match the committed vectors?

- Sources examined: `docs/host-wire-protocol.md:207-226`;
  `crates/shm-transport/src/setup_auth.rs:112-131`; the Python reproduction.
- Findings: the document's `ServerProof` example at `:213` carries
  `server_proof: [64, 154, 84, 68, ...]` and its `ClientAuth` example at
  `:217` carries `[184, 138, 243, 55, ...]`. Neither equals the committed
  vectors (`[89, 41, 95, 101, ...]` and `[140, 161, 69, 27, ...]`). The
  prose at `:220` also says the proofs use daemon version
  `host-runtime/0.1.0`, while the JSON at `:213` and the code use
  `eidnara-host/0.1.0`. Recomputing with `host-runtime/0.1.0` under the
  current domains gives `0a05b00f...` and `b576c1bf...`, which match neither
  the document nor the code. The document's bytes therefore come from a
  transcript this tree does not contain, most likely the predecessor domain
  strings.
- Missing evidence: the predecessor domain and version strings, to confirm
  the document's bytes are the predecessor vectors rather than an error.
- Conclusion: unresolved, needs a documentation fix or the predecessor
  strings. The code and both tests agree with each other and with the
  independent reproduction; the protocol document's section 5.2 examples do
  not describe this implementation. Section 18 of the same document names
  `host-runtime` source and tests as authority (`:989-991`), so this is a
  stale example, not a contract the code violates.

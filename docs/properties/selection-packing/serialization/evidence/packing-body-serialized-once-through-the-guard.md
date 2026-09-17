# packing-body-serialized-once-through-the-guard

## Discovery trigger

RP2.8's packing contract requires every accepted preparation to be serialized
once and measured once through the existing exact measure-then-write guard,
under the approved serialized-bytes and token limits; the U4b ticket names the
guard as the only write path.

## Evidence trail

- `crates/daemon/src/packing/serialize.rs` `serialize` builds one
  `PreparedOutput` from the closed ledger's text, measures it, compares the
  measured length with the serialized-bytes limit, reserves a buffer of that
  length, and writes once through `MeasuredOutput::write_to`; `finalize` calls
  it once per pass and returns the first body it yields. It re-checks no
  accounting bound: `prepare_optional` (`crates/daemon/src/packing/mod.rs:746`)
  refuses a closed render past one before an admission exists, and a rebuilt
  ledger is a shorter prefix of an admitted render.
- `crates/daemon/src/dispatch.rs` `PreparedOutput::measure` refuses a body past
  `MAX_WIRE_BODY_BYTES`, and `write_to` reports a length mismatch, so the
  packer inherits both checks rather than re-implementing them; the
  `guard_calls` test-support counters record each call on the calling thread.
- `crates/daemon/tests/packing_serialize.rs` reads the counters around
  `finalize`, compares the body to the ledger's text and rendered length, and
  admits at a limit equal to the render under both the byte profile and the
  exact tokenizer.

## Failure scenario

A packer that writes the ledger's text directly, or measures once and writes
each candidate render until one fits, emits bytes the guard never measured;
the counters make either visible where the body alone would not.

## Timing windows and dependencies

None.

## What a test must construct

- A closed ledger and a serialized-bytes limit equal to its rendered length.
- The guard's call counters reset on the test thread before `finalize`.

## Investigation log

### Q: Can the packing path reach the guard's transport refusal?

- Sources examined: `crates/daemon/tests/support/packing.rs` (payloads are
  `&'static str` persisted under `PersistBounds` with `max_payload_bytes` of
  1 MiB); `crates/daemon/src/packing/serialize.rs` `serialize`, which maps a
  `measure` error to `SerializationBound::Transport`;
  `crates/daemon/tests/prepared_output.rs`, which measures a body of exactly
  `MAX_WIRE_BODY_BYTES` and refuses one past it.
- Findings: no packing fixture renders 64 MiB, so the `Transport` arm is
  reached only through the guard's own tests. `PackingLimits::from_manifest`
  refuses a rendered-bytes or serialized-bytes value past the transport
  maximum, so a manifest cannot admit a render the guard would refuse.
- Missing evidence: a packing render at the transport maximum.
- Conclusion: unresolved, needs a fixture that can persist a 64 MiB payload -
  the arm is typed and the guard's contract is covered where it is cheap.

### Q: What does the `guard_calls` counter oracle observe?

- Sources examined: `crates/daemon/src/dispatch.rs` `guard_calls`
  (thread-local `Cell` counters incremented in `measure` and `write_to`,
  reset by the test) and the three `finalize` tests that read them.
- Findings: the counters count every guard call on the test thread since the
  reset, whoever made it, so a second measurement or a write after a refusal
  anywhere on the packing path would show. They do not see calls on another
  thread, and the multi-thread test compares identities, not counts.
- Missing evidence: none.
- Conclusion: resolved with answer - the oracle is per-thread and covers the
  single-threaded path the record describes.

# packing-body-serialized-once-through-the-guard

## Discovery trigger

RP2.8's packing contract requires every accepted preparation to be serialized
once and measured once through the existing exact measure-then-write guard,
under the approved serialized-bytes and token limits; the U4b ticket names the
guard as the only write path.

## Evidence trail

- `crates/daemon/src/packing/serialize.rs` `serialize` checks the accounting
  bounds, builds one `PreparedOutput` from the closed ledger's text, measures
  it, compares the measured length with the serialized-bytes limit, reserves a
  buffer of that length, and writes once through `MeasuredOutput::write_to`;
  `finalize` calls it once per pass and returns the first body it yields.
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

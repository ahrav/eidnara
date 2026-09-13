# message-decode-allocation-gate-has-isolated-scope

System: test-only messages-decode resource budget invariant.
HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
[Source register](../source-register.md) defines P and B. No new tests run.

## Discovery trigger

P:L67 specifies at most 16 events per message from `messages`. P:L213 adds
peak live bytes below three times message JSON length, while naming a full
TransformRequest decode. Those numerator and denominator scopes need an
explicit connection before a result is meaningful.

## Evidence trail

1. P:L250 defines events as alloc plus realloc and peak as live high water.
2. P:L252-L260 reports body size 85,032 bytes, message-array size 43,780,
   full metered decode 4,181 events, messages decode 3,283 events, and a
   derive-only messages mirror at 427 events and 84,356 peak bytes. These
   different scopes cannot be combined into one candidate result.
3. `crates/daemon/tests/served_json_passthrough_allocations.rs:10-32` uses a
   process-global allocation-event counter; `:58-85` scopes serialization
   calls and compares per-block growth. It is not a decode test.
4. `crates/daemon/tests/parse_charge_covers_typed_decode.rs:25-88` supplies a
   requested-layout live/peak counter and serializes participating tests.
   The mutex alone does not exclude every unrelated process allocation.
5. `crates/daemon/src/lib.rs:1745-1766` accounts native Values and tail data
   outside messages. Their allocations must remain in the full resource
   ledger but not leak into a messages-only target numerator.

The threshold is a genuine test-only resource invariant: `E_msg <= 640` and
`P_msg < 3 * J`. Instrumentation validity is its observation contract, not a
separate runtime property. Invalid attribution supplies no passing result.
The measured implementation uses production wire types rather than a mirror.

## Failure scenario

A full-body counter includes native-message Values and rejects a decoder
that meets the message target. Alternatively, a counter starts after the
message tree exists and reports an artificially small peak. Integer division
introduces another false pass: 679 / 40 is 16 although 679 exceeds 640.

Competing explanation: more allocations are genuine decoder regressions.
Per-allocation scope attribution and a stable corpus distinguish this from
whole-body or harness noise. Do not subtract the maximum of a separate
empty-body run; maxima occur at different times.

## Timing windows and dependencies

Build and serialize the exact fixture before measurement. Record `J` from
its immutable raw message-array slice, not reserialized typed output after
unknown-field dropping or default omission. Include internal tagged-enum and
unescape workspace. End only after result materialization and peak capture.

Scope certification must account for all allocation API variants used by the
chosen counter, realloc semantics, isolation, and requested-size limits.
The threshold does not claim allocator residency or handler latency.

## What a test must construct

- Exactly 40 plugin-shaped messages with the declared mixed block distribution
  and 2 KiB tool results, using the production wire types.
- Immutable message JSON bytes and a recorded `J` including array envelopes.
- Valid event and live-byte attribution to the message decode interval.
- `E_msg <= 640` and `P_msg < 3 * J`, with no integer-division relaxation.
- An independent shape/value check outside the measurement interval.
- Full-body and decode-plus-projection measurements in their own scopes so
  the narrow gate cannot hide native payloads or construction workspace.

## Investigation log

### Q: How does a full TransformRequest run isolate messages costs?

- Sources examined: P:L67, P:L213, P:L252-L260; both allocation-test patterns.
- Findings: The proposal gives a whole-request call and a subtree threshold
  without a concrete attribution mechanism.
- Missing evidence: A scope certificate for the final allocation gate.
- Conclusion: needs human input. Either instrument the subtree inside the
  production decode or combine isolated production IngressMessages decoding
  with a proved integration equivalence. A mirror is insufficient.

### Q: Can the existing counter patterns be copied without validation?

- Sources examined: allocation API hooks, reset functions, and mutex above.
- Findings: They establish useful seams, not complete isolation or true
  resident-byte measurement. The served test uses integer per-block growth
  for a different claim; that arithmetic must not define the new threshold.
- Missing evidence: Counter completeness, isolation, and interval validation.
- Conclusion: unresolved, needs measurement-harness validation before results.

### Q: Is this only an evidence gate like the payoff requirement?

- Sources examined: P:L67 and P:L213; independent finding 2.
- Findings: Allocation count and peak bound a concrete operation's resources.
  Scope validity determines whether those quantities were observed correctly.
- Missing evidence: Final attributed measurements and the owner's attribution choice.
- Conclusion: resolved on category. R5 stays active; the two strict budget
  comparisons remain in Check, with instrumentation rules stated separately.

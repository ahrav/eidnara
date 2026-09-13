# served-serialization-is-single-pass-and-error-terminal

## Discovery trigger

Plan R3 and AE4 preserve one serialization and error return before copying.
Adding an identity return creates another finalization outcome; observing only
the copy branch would miss an erroneous return of an incomplete A.

## Evidence trail

- [Encode][encode] calls `value.serialize` once with `?`, then extracts A,
  sorts tables, allocates B, and copies. Error exits precede all finalization.
- [The counted test][counted] has two nested Counted values sharing a counter.
  It asserts two visits and literal bytes, but does not count root/child sites
  independently or inject an error. Status: unaudited.
- [Production construction][constructor] calls the WireMessage facade and
  expects serialization to succeed. Generic failing sources are private tests.
- [Prepared-output writer failure][writer-test] is a different boundary. It
  does not exercise a failure inside this serializer/formatter recording.

Numbered links were verified with `git show HEAD:<path>` at
`2e4433e6b511ae74944df8a9669c428e73915d29`. The plan's `4980f8af` baseline
does not establish exercise here. No test or failure injection ran.

## Failure scenario

A preparatory counting serialization can traverse a stateful source twice.
Ignoring an error or moving finalization ahead of its check can send incomplete
object tables to sorting, or return a partial buffer instead of the error.
A copy-only counter stays zero on that incorrect early-return path.

## Timing windows and dependencies

This is a synchronous emission cut, not an asynchronous cancellation scenario.
Inject an error before the first write and after a nonempty prefix while a
nested object remains open. Later child sites must remain unvisited.
Do not broaden this into allocator-failure, panic-recovery, or transport tests.

## What a test must construct

Extend the existing private generic test context. Count root and each distinct
child emission site separately. For canonical and disordered successful input,
the count vector must equal exactly one expected traversal.

For each failing source, define the expected prefix before invoking encode.
Check that exact count vector, absence of later visits, and the returned
injected error class/message. A private finalization observation immediately
after successful serialization must remain absent on error, before buffer
extraction, sorting, or either return choice. There must be no reorder work.

Observe the real encode, not a separately reconstructed serializer sequence.
Use nonallocating local state where possible and keep probes test-only. The
joint guarantee uses `always`, including `always(!finalization_after_error)`;
there is no named forbidden code location to justify `unreachable` semantics.

The private error input makes this record's reachability `test-only`. That
does not weaken the single-pass requirement for production WireMessage calls
or claim a host `encode_failed` result for an unreachable production error.

## Investigation log

### Q: Do existing success counters prove failure is terminal?

- Sources examined: the counted test and encode's `?` boundary.
- Findings: the source boundary is clear, but success-only visits do not
  construct incomplete span tables or test the early-return failure mode.
- Missing evidence: both injected error cuts and a finalization observation.
- Conclusion: unresolved, needs `/testing:test-strategy` to extend the private
  tests and execute them against the candidate.

### Q: Should downstream writer failures supply this coverage?

- Sources examined: public prepared-output tests and production constructor.
- Findings: destination failure happens after canonical message construction.
- Missing evidence: no additional transport proof is needed for this record.
- Conclusion: resolved with answer. Keep S7 and existing transport owners
  separate; route inspected tests to `/testing:invariant-test-review`.

[encode]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L121-L142
[counted]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L170-L193
[constructor]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/transform.rs#L164-L166
[writer-test]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/prepared_output.rs#L186-L204

# served-unchanged-span-copy-is-identity

## Discovery trigger

Returning A is equivalent to the old identity-order output only if copying
unchanged spans is itself identity. The initial independent findings identify
this missing bridge. A correct field-change flag does not prove it.

## Evidence trail

- [SpanWriter][writer] appends the serializer's bytes and updates position.
- [Formatter hooks][formatter] record object starts before `{`, ends after
  `}`, field starts after commas, and field ends after values. Key ends include
  their closing quote. Stack `expect` guards cover local nesting assumptions.
- [The copier][copy] copies gaps, reconstructs braces and commas, recursively
  copies fields, and resumes at the object's end to skip completed descendants.
- [Encode][encode] owns both A and the real tables after successful serialization.
  This is the observation point, not a second serializer setup in a test.
- [Scalar fixtures][tests] are unaudited; they compare external reference
  bytes, not unchanged tables directly against A.

Anchors are checked using `git show HEAD:<path>` at
`2e4433e6b511ae74944df8a9669c428e73915d29`, separate from plan baseline
`4980f8af`. No runtime identity observation exists in this pass.

## Failure scenario

An omitted comma, repeated child, or wrong resume offset can make the old
unchanged copier output differ from A. A new return-A branch then changes
behavior even with an accurate no-change flag. B1's output fixtures alone
do not isolate the reason.

## Timing windows and dependencies

Only completed hook-visible compact serialization is in the lemma's domain.
Do not fabricate arbitrary spans or RawValue fragments that bypass recording.
The unchanged tables remain in source order before sorting.

For a range without objects, the copier emits the exact slice. For an object,
the recorded fields plus compact braces/commas cover its original bytes.
Recursive identity for each unchanged field gives object identity. Resuming
at its end excludes already-emitted descendants; copied gaps complete the
enclosing range. This is source-based reasoning, not execution proof.

## What a test must construct

Observe A and unchanged tables from one actual successful encode. In a
private test-only observation, call the existing copier on those tables and
compare its result with A in bytes and length. Complete object/field ranges
can add focused checks where a failure needs localization.

The test-only observation remains useful after production bypasses the copier
on canonical input. It must not become a new production hook, parallel encode
implementation, or extra production serialization. Keep expected-copy work
outside the allocation measurement interval for S5.

Include `{}`, singleton objects, nested arrays of objects, scalar gaps, and
strings containing braces, commas, colons, quotes, backslashes, and Unicode.
Include `-0.0`, `1.0`, large/small exponents, signed minimum, unsigned maximum.
Default HarnessMeta supplies a production empty object; scalar-root probes
are private test cases. The algorithmic obligation remains default-production.

## Investigation log

### Q: Is a general span-geometry validator required?

- Sources examined: formatter boundaries, recursive copier, and module tests.
- Findings: compact punctuation reasoning and a direct real-table comparison
  establish the specific bridge without a second geometry subsystem.
- Missing evidence: the actual test-only comparison and its executed results.
- Conclusion: resolved with answer. Add only the smallest observation that
  discriminates; do not mandate a giant validator.

### Q: Does this duplicate B1?

- Sources examined: the B1 relationship and plan KTD1.
- Findings: this is a local identity lemma; B1 owns end-to-end byte/hash behavior.
- Missing evidence: candidate execution, not a new semantic contract.
- Conclusion: resolved with answer. Hand off to `/testing:test-strategy`;
  existing tests and production guards retain their separate audit owners.

[writer]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L24-L34
[formatter]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L42-L81
[copy]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L84-L109
[encode]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L121-L142
[tests]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L195-L215

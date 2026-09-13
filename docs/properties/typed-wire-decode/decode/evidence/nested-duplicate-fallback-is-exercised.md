# nested-duplicate-fallback-is-exercised

## Discovery trigger

R2 accepts a new Tree lane outcome for duplicate message/block fields under
derived serde. A safety comparison can pass without reaching that transition.
The failure-recovery and wildcard lenses require an independent witness.

System: `/local/home/ahrav/scratch/eidnara`, 2026-09-13.
HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
The accepted replacement is absent; this record imports no prior exercise.

## Evidence trail

- `crates/daemon/src/lib.rs:12923-12951` routes Invalid typed decode into
  tree parsing, but also reaches Tree without attempting typed decoding.
- `crates/daemon/src/lib.rs:19762-19964` constructs the current corpus.
  Its duplicate nested key is IngressMessage.mid at `:19809-19812`.
- `crates/daemon/src/lib.rs:20045-20052` expects exactly three tree-only
  raw-parser cases, all duplicates in existing derived envelopes.
- `crates/memory-store/src/lib.rs:131-133,255-257` currently collapses
  nested wire duplicates through Value before typed mirror conversion.
- `crates/daemon/src/lib.rs:20140-20181` pins the observed direct-lane list
  but does not separately observe the new CK/block failure preconditions.

The fallback branch is default-production and already has an ingress-mid
duplicate control. CK role and block kind duplicate failures are prospective
until KTD1 derives those structs. No claim is made that they fail typed decode
at HEAD, or that the new marker has already fired.

## Failure scenario

A generator uses json! or a map to create duplicate keys, which overwrites the
first value before serialization. Another fixture changes an escaped route
or adds a page field, reaching Tree without any typed failure. Both can make
the test appear to cover fallback while missing its new enabling situation.

A marker named for a mismatched output would be invalid: it could fire only
when the safety property fails. The markers here describe correct recovery
preconditions and therefore remain satisfiable on a correct implementation.

## Timing windows and dependencies

The repeated recognized field follows a valid prefix in the same byte body.
Admission must be nonbinding. The literal discriminator selects an unpaged
transform, and the compatibility gate must accept before the typed attempt.
Duplicate keys solely inside a retained Value map are a control, not a forced
derived-field error. Unknown duplicate keys are not interchangeable either.

## What a test must construct

The fixed marker `typed-wire-decode-message-duplicate-fallback` observes a
literal body whose ck contains two role fields with a valid final role.
The fixed marker `typed-wire-decode-block-duplicate-fallback` observes two
kind fields in one block envelope, with a valid final kind.
For each, require gate acceptance, typed Invalid, successful independent tree
conversion, and the handler's BodyLane::Tree. The paired safety record checks
the semantic result; the marker must not assert that the result is wrong.
Check `sometimes` independently for each marker. Maintain separate witnessed
bits and require both at campaign completion. Aggregate output must report
both results; a total count or OR across markers cannot mask an unfired one.

Keep a duplicate-ingress-mid control and a clean direct body. If a marker
fails, first verify literal construction and branch observations. An unfired
marker can mean a generator gap or changed reachability; it is not evidence
of a violated formal liveness guarantee.
No marker, test, or benchmark is implemented or run here.

## Investigation log

### Q: Does the existing nested duplicate already cover CK serde replacement?

- Sources examined: daemon lib.rs:19809-19812 and wire.rs:25-31.
- Findings: the field is IngressMessage.mid, whose struct is already derived.
- Missing evidence: CK role and WireBlock kind literal-byte witnesses.
- Conclusion: resolved with answer: it is a useful current-path control,
  not evidence of the new message/block fallback transition.

### Q: Is a public wire-visible lane marker required?

- Sources examined: daemon lib.rs:15783-15790 and :20140-20181.
- Findings: module-local tests can already observe BodyLane. The real host
  fixture does not expose the complete gate/typed-failure observation.
- Missing evidence: the selected seam for each final marker.
- Conclusion: unresolved, test strategy can use the existing local seam or
  a narrow test-only fixture extension; no wire field is authorized.

### Q: Can one fallback marker satisfy the aggregate record?

- Sources examined: this record and the supplied 2026-09-13 independent pass.
- Findings: message and block cases are separate required situations.
- Missing evidence: implementation and execution of both marker checks.
- Conclusion: resolved with answer: no; require each `sometimes` result
  independently and preserve each result in the aggregate report.

## Named handoff

`/testing:test-strategy` owns marker instrumentation and corpus construction.
Independent analyst `ses_f6756093fffeVjNp36S3E8pKrM` completes the supplied
portfolio pass on 2026-09-13; the prospective production-path label does not
claim either marker has fired. This property remains unexercised.
`/testing:invariant-test-review` owns the paired safety/coverage oracle audit.

# dec-a-cache-ttl-parse-is-total-over-arbitrary-strings

## Discovery trigger

The user-configured TTL reaches a string parser that performs floating-point
arithmetic before producing milliseconds. This record separates parser totality
from the scheduling consequences of accepted or rejected values. Source revision:
`74044960ee91641dec95c8552f15282844a18b13`. Rust paths below are relative to
`crates/daemon/src/`.

## Evidence trail

`parse_cache_ttl` trims its input, accepts case-insensitive `never`, and otherwise
parses bare milliseconds or lowercase `s`, `m`, and `h` units
(`scheduler.rs:365-398`). Empty or non-digit numeric components and unsupported
units return an error. The final-character slice subtracts that character's
UTF-8 width (`scheduler.rs:374-377`), so a multibyte suffix does not split a
character. Non-finite or oversized millisecond results become `u64::MAX`
(`scheduler.rs:389-397`).

`ttl_execute_fired` compares saturating elapsed time with a strict `>`;
`ttl_hard_expired` also requires a positive prior-response timestamp
(`scheduler.rs:400-407`). Neither predicate can exceed `u64::MAX`. A TTL of
zero satisfies the hard-expiry predicate only when the prior timestamp and
elapsed time are both positive.

`decide` computes idle expiry and uses it to request execution
(`scheduler.rs:689-707`), then applies band selection and boundary deferral
(`scheduler.rs:709-732`). Zero therefore does not unconditionally force the
final pass to `Execute`. The former every-pass claim omitted these gates.

The user-only config parser accepts non-empty trimmed TTL strings and map values
without validating their units (`config.rs:924-945`). Project TTLs are ignored
with a tier warning (`config.rs:672`, `config.rs:723-733`). An invalid accepted
user string such as `5S` becomes the scheduler default without a parse diagnostic
(`scheduler.rs:771-773`), rather than five seconds.

## Failure scenario

Parser totality is not identified as broken. The behavioral questions are the
zero-TTL policy and silent fallback for a malformed TTL value. A malformed JSONC
file is different: file-read warnings do exist, as the separate invalidated
silent-file-failure record explains.

## Timing windows and dependencies

Parsing has no timing dependency. Scheduling consumes caller-supplied timestamps.
To exercise zero expiry, use a positive prior-response time and a later `now_ms`.
Do not equate a second invocation with positive elapsed time, and account for
boundary deferral when asserting the final pass.

## What a test must construct

Check strings including zero, lowercase and uppercase units, a multibyte suffix,
empty input, and a long digit run. Assert both total return and the expected
`Result`. Separately check zero-TTL predicates and invalid-string fallback.

Existing checks, each `unaudited`: the scheduler golden runs its parser cases
(`scheduler.rs:1082-1085`); the `never` tests assert its sentinel, predicates,
and a deferred scheduler outcome (`scheduler.rs:1378-1405`). The config string
test (`config.rs:1163-1171`) checks a valid `45m` value, not parse diagnostics.

## Investigation log

### Q: Does zero mean every pass must execute?

- Sources examined: `scheduler.rs:400-407`, `scheduler.rs:689-732`.
- Findings: zero expires only with positive elapsed time and, for hard expiry,
  a positive prior timestamp. Later scheduler gates still apply.
- Missing evidence: a policy decision about accepting zero and reporting malformed
  TTL values; no policy change is made here.
- Conclusion: resolved with answer for implementation behavior; intended zero
  and diagnostic policy needs human input.

### Historical investigation

The [pre-refresh evidence](https://github.com/ahrav/eidnara/blob/74044960ee91641dec95c8552f15282844a18b13/docs/properties/daemon/decisions/evidence/dec-a-cache-ttl-parse-is-total-over-arbitrary-strings.md)
preserves the old execution report and configuration-document quotations. Its
coordinates and unconditional every-pass prediction are historical only.

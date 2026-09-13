# Transform source-guard precursor

## Scope and evidence boundary

This supplement covers the source-domain and recursive snapshot portion of
[#533](https://github.com/ahrav/eidnara/issues/533), under the parent
[#525](https://github.com/ahrav/eidnara/issues/525). It does not replace the
parent's reusable TE01-TE30 companion catalog. That companion is unavailable
in this checkout. The TE19 identifier and title below come from the parent's
record index; the predicate is scoped to this precursor's executable witnesses.

The existing native response format remains unchanged. The production hook
selects the guarded path when `transform_mode` is `rust`, so this record uses
`explicit-config-only` reachability. Direct callers also use the same guard.

The source predicate covers the captured input. It does not establish that a
separate destination remains unchanged or replaceable after an await. The
destination is inspected at entry; its publication-time contract is PR2 work.

| Record | Scope | Evidence |
| --- | --- | --- |
| TE19 | Hook-free source inspection and rechecks | [Evidence](evidence/referenceable-json-rejects-hooks-before-reading.md) |

### referenceable-json-rejects-hooks-before-reading

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: partial - focused guard, direct transform, hook and wrapper witnesses run; the parent ownership and publication contract is not exercised in full.
Guarantee: Unsupported initial message sources are refused before source hooks, preflight or dispatch, and guarded continuations refuse changed or unsupported sources before further source reads and publication.
Check: `always` - source-hook counters stay zero, an initial refusal has zero preflight and transform calls, and a failed continuation source check prevents further source reads, retries and publication; already-serialized pages may finish sending before that check.
Fault/timing angle: Entry inspection, directory lookup, ordinal scan completion, daemon response and full-sync retry.
Required faults and enabling state: Construct a supported input and an independent expected output; install a getter or replace membership at a named pause; observe that the pause was reached and count any earlier valid dispatch separately.
Confidence: medium - [evidence](evidence/referenceable-json-rejects-hooks-before-reading.md). Focused witnesses and repository checks pass; the supplied review findings have documented dispositions, while complete lifecycle coverage remains open.
Existing check: `packages/opencode-plugin/src/hooks/context/transform-capture.test.ts`, `rust-mode-transform.test.ts`, and `hook.test.ts`; execution and review repairs are recorded in the evidence and dispositions, without a full-parent adequacy claim.
Impact: A source getter or proxy trap can run arbitrary code, or a changed input can reuse an obsolete cache prefix or publish stale output.
Open questions:
- Complete parent acceptance requires PR2's ownership, admission and publication witnesses.
- Final acceptance of the repaired precursor remains with the controller.

## Guarded snapshot validation

`transform-capture.ts` owns descriptor traversal, source membership capture,
recursive field tapes and rechecks. A separate root tape includes array
bookkeeping and membership descriptors. Nested array tapes include named keys,
their order and descriptors, and explicit array terminators. Recording a member
temporarily replaces the root field visitor and restores it afterward. Tape
comparison checks bounds before indexing as defense in depth; the regression
also benefits from the tape's explicit terminators. It rejects proxies before reflection,
including revoked proxies. It inspects hidden accessors as well as enumerable
properties. Harmless hidden data is accepted and nested hidden data participates
in the guard. Optional undefined object fields are omitted; array holes and
undefined array entries are rejected. Dense readonly input arrays are accepted
when the destination is mutable. Neither capture nor recheck freezes or
deep-clones source payloads. Reads of absent optional fields, out-of-range
indexes and the returned array's `then` fall through to `Array.prototype` and
`Object.prototype`, so the root check refuses an accessor on either prototype
(other than `__proto__`) by descriptor inspection, without invoking it, and logs
that decline at warn because it repeats on every pass in the process. The guard
runs in the host's realm and calls built-in methods and `Object` reflection
functions itself, so a built-in replaced by a data property is a compromised
process, not a source hook it can detect.

Exact field tapes, not hashes, decide whether the existing inbound cache prefix
and terminal message can be reused. The old recursive traversal and snapshot
signature functions are removed. SHA-256 wire fingerprints remain part of the
existing transport contract, not the authority for field equality.

The walk has a 256 MiB cumulative conservative source/snapshot estimate and a
bounded ancestor depth. Strings retain their UTF-16 charge, and descriptors,
field slots and repeated visits to shared subtrees also count. This unit is
independent of the 64 MiB encoded-output limit. JSON below that encoded limit
can exceed the walk estimate and be refused; accepting every 64 MiB JSON value
is not promised. The walk limit is not aggregate capture admission, engine
enumeration allocation or process RSS. It cannot stand in for TE23 or TE24.

Source declines log `SourceRejected` at debug and `SourceWalkLimitExceeded` at
warn through existing loggers without incrementing daemon-failure counters. A
walk-limit decline repeats on every pass of that session until its history
shrinks, so it is the one decline an operator must be able to see. If a source
check rejects after delivery IDs are known, the existing NACK path handles
those IDs. Checks after `await assertCurrentRetryPass()` remain necessary:
even an already-resolved promise opens a microtask window.

## Deferred ownership and publication work

PR2 owns admission before asynchronous work, cancellation and settling charges,
pass-local ordinal state, state promotion before delivery awaits, output-owner
and invalidation fencing, and the destination's all-or-none replacement
contract. The ordinal resolver still updates shared memo state. The wrapper
still restores its shallow membership snapshot on thrown hook errors. Existing
splice-based publication and post-delivery cache promotion remain unchanged.

The wrapper validates the source tree before copying membership. The walker's
root entry and the wrapper share one exported predicate, `rootArrayRejection`,
which rejects proxies, unsupported prototype chains and an own or inherited
`then` property without invoking it. After the hook, only this cheap root check
runs. It protects promise assimilation, not publication: an `undefined` return
cannot undo changes the hook has already made to the host array. Source
rejection before the hook leaves host data unchanged. Supported calls retain
the existing array result. The root check is not an output-owner or
all-or-none publication check.

This is partial input for TE17, not completion of TE17, TE18, TE20-TE24 or TE30.
Recipe response validation and applied-output revisions belong to #538.
The [retained baseline](baseline.md) is historical measurement evidence, not a
U5 comparison or a performance acceptance result.

The [review dispositions](review-dispositions.md) separate verified repairs
from rejected shortcuts and deferred ownership work.

## Measured local cost

A local exploratory run of `bench-transform-client.ts --samples 5 --warmup 2`
measured a 38.5 ms median for the large warm-append case. Isolated probes on a
1,000-message, 2 KiB-per-message fixture measured approximately 2.46 ms for
capture and 1.95 ms for a full recheck. These are small local samples, not a
controlled treatment comparison or a performance acceptance result.

Repeated full-history checks are a material cost. Checks after an await remain
necessary even when its promise was already resolved; synchronous call sites
can be consolidated only when no intervening callback can change source data.
The completed ownership and recipe implementation must include these costs in
the parent's three-artifact comparison and meet its performance gate. This
precursor does not establish that the gate will pass.

The conservative walk estimate also depends on structure. Text-heavy shapes
measured roughly 2.0 to 3.2 estimated bytes per JSON byte. Persisted OpenCode
rows are metadata-heavy: each message carries identifier, timestamp, token and
tool-state fields, and each property is charged for its descriptor, three
attribute fields and a tape slot in addition to its bytes. Synthetic shapes with
200-, 800- and 3,000-character parts measured 11.9, 7.0 and 3.8 estimated bytes
per JSON byte; replaying three real sessions measured 3.5 to 6.3. At the earlier
64 MiB budget those ratios admitted roughly 5 to 18 MiB of JSON and declined
every pass of a 12,351-message, 19 MiB session. The 256 MiB budget admits the
19 MiB session and about 20 MiB of the shortest measured shape. The ratio is
illustrative, not a conversion rule or another limit.

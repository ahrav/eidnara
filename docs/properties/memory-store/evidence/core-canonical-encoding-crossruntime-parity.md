# core-canonical-encoding-crossruntime-parity

## Discovery trigger

`crates/context-core/src/canonical_json.rs:1-4` opens by declaring the encoding
"shared with the TypeScript runtime" with "golden cross-runtime cases" in
`testdata/canonical-json-contract-v1.json`. The source catalog raised this
record on that claim's predecessor at `claim_operation.rs:1-4` in the host
repository at `eb6da6109`, which named the TypeScript twin
(`packages/plugin/src/features/eidnara/memory/claim-operation-contract.ts`) and
stated "Both runtimes are proven against the golden corpus". A cross-runtime
byte-equality claim carries digest weight, so the first question was whether
the two implementations agree where naive implementations diverge: object key
ordering, where JavaScript's default comparison is UTF-16 code-unit order and
misorders astral-plane keys relative to high BMP keys, while Rust `str`
ordering is UTF-8 byte order, which equals code-point order.

At HEAD no TypeScript encoder reads the fixture (see the investigation log), so
the question the record can answer here is narrower: does the Rust encoder emit
one byte sequence per value, and does the Dreamer request digest follow the
documented formula over those bytes.

## Evidence trail

The Rust side at HEAD, `crates/context-core/src/canonical_json.rs`:

- `:10` documents "Objects sort keys by Unicode code point (Rust `str`
  ordering)".
- `:93` implements it as
  `let sorted: BTreeMap<&String, &Value> = entries.iter().collect();`.
  `BTreeMap` orders by `Ord for String`, which is lexicographic over UTF-8
  bytes. For well-formed UTF-8 this is exactly code-point order.
- `:55-68` escapes only `"`, `\`, and code points below `0x20`, the last as
  lowercase `\u00xx` via `write!(out, "\\u{:04x}", c as u32)` at `:62`.
- `:42-53` is the number vocabulary: an `i64` path range-checked to
  `±MAX_SAFE_INTEGER` at `:44-46`, and an `f64` path at `:48-52` rejecting
  non-finite, fractional, and out-of-range values. An integer literal above
  `i64::MAX` has no `as_i64` view and falls through to the `f64` path, where its
  magnitude fails the range check (`:40-41`).
- `:128-135` hashes `<protocol>`, `\n`, then the canonical bytes; `:139-141`
  binds the Dreamer protocol string to it.
- Recursion at `:88` (arrays) and `:101` (objects) means nested values encode
  through the same rules.

The TypeScript side, as the source catalog read it at `eb6da6109`; none of
these lines exist in this repository and they are kept because they explain
why the fixture's discriminating case looks the way it does:

- TS `:51-52` carried the comment "Unicode code-point comparison (== UTF-8 byte
  order, unlike JS `<` which compares UTF-16 code units and misorders
  astral-plane keys)".
- TS `:53-63` defined `compareCodePoints`, spreading both strings into code
  points and comparing `codePointAt(0)` pairwise, with a length fallback.
- TS `:120` used it: `Object.keys(record).sort(compareCodePoints)`.
- TS `:102` rejected non-safe integers with `Number.isSafeInteger`; TS
  `:107-108` noted `String(-0) === "0"`; TS `:81-83` threw on a lone surrogate;
  TS `:115-118` threw for a non-plain prototype.

The fixture pins the discriminating case. Decoding the `astral-key-order` entry
in `crates/context-core/testdata/canonical-json-contract-v1.json` gives keys
`U+0041` (`A`), `U+FFFD`, and `U+1F600` with the pinned canonical output
`{"A":3,"\u{FFFD}":1,"😀":2}`. That is code-point order
(0x41 < 0xFFFD < 0x1F600). UTF-16 code-unit order would place `😀` second,
because its lead surrogate is `0xD83D` (55357), which is below `0xFFFD`
(65533). A regression to a comparator with UTF-16 order would fail it.

Fixture breadth at HEAD, enumerated: 5 `canonicalization` cases
(`scalars-and-key-order`, `unicode-and-escapes`, `astral-key-order`, `numbers`,
`nesting`) and 5 `invalidCanonical` cases (`1.5`, `9007199254740993`,
`9007199254740992`, `-9007199254740992`, `18446744073709551615`).

Live consumers at HEAD:

- `crates/memory-store/src/dreamer_ledger.rs:685-688` wraps
  `compute_dreamer_request_digest`; `crates/daemon/src/lib.rs:9579-9588`
  digests the classify request's effect-defining inputs through it, and
  `:9624` passes the digest into `begin_dreamer_receipt`.
- `crates/memory-store/src/dreamer_ledger.rs:318-321` compares a stored
  receipt's `request_digest` against the incoming one and reports
  `DigestConflict` on a mismatch.
- `crates/memory-store/src/lib.rs:3206-3212` encodes a durable JSON value
  through `canonical_json_encode` and runs the secret scanner over the result.

## Failure scenario

A refactor replaces the `BTreeMap` collect at `canonical_json.rs:93` with a
`Vec` plus a sort under some other comparator, or the number path at `:42-53`
starts accepting a value it rejected. Either change produces different
canonical bytes for a value in the discriminating region, or accepts a value
whose digest is not stable across the runtimes that originally shared the
vocabulary.

Different canonical bytes mean a different SHA-256 through `protocol_digest`
(`:128-135`) and therefore a different `compute_dreamer_request_digest`
(`:139-141`). The consequence follows the digest's job in the Dreamer ledger.
`run_dreamer_task` computes the digest over the request's effect-defining
inputs (`crates/daemon/src/lib.rs:9579-9588`) and hands it to
`begin_dreamer_receipt` (`:9624`). If the same command's retry digests
differently, `dreamer_ledger.rs:318` reports `DigestConflict` and the daemon
returns `dreamer_request_conflict` (`lib.rs:9646-9652`) instead of replaying
the recorded outcome (`:9626-9628`). If two different requests digest the
same, the second is treated as a replay of the first and reads the first's
result. A byte change in the encoder also changes the text the durable-write
redaction scan sees (`crates/memory-store/src/lib.rs:3206-3212`), so a secret
that was detected under one encoding can be missed under another.

The ordering failure only manifests for keys in the discriminating region,
which is why a corpus that covers only ASCII keys would not see it. The
fixture does cover it, which is the strength this record exists to preserve.

## Timing windows and dependencies

None. This is a pure-function law, not a race. No fault injection, no
interleaving, no clock.

The dependency worth naming is on the fixture: the module header at
`canonical_json.rs:1-4` still calls the cases cross-runtime, but at HEAD only
the Rust tests read them. A case added here pins Rust bytes only until an
encoder under the same fixture exists in this repository.

## What a test must construct

A generator, not a fixed list. The fixture already carries the hand-picked
discriminating cases; the property test should widen coverage:

1. Object keys drawn from the discriminating regions: `U+E000`..`U+FFFF`
   (high BMP, single UTF-16 unit) and `U+10000`+ (astral, surrogate pair), plus
   pairs that share a prefix and differ only past it, plus keys where one is a
   prefix of the other (to exercise the `BTreeMap` shorter-is-less rule).
2. Integers at exactly `MAX_SAFE_INTEGER`, `-MAX_SAFE_INTEGER`,
   `MAX_SAFE_INTEGER + 1`, `-(MAX_SAFE_INTEGER + 1)`, `0`, `-0`, and floats
   with zero fraction such as `1e3` and `-1e3`.
3. Strings containing every code point below `0x20`, `U+007F`, `U+2028`,
   `U+2029`, `"`, `\`, and astral text.
4. Nested arrays and objects to depth 3 or more, since `encode_canonical_value`
   recurses at `:88` and `:101`.
5. For every generated object, a permutation of its key insertion order, and
   the assertion that both encode to the same bytes and the same Dreamer
   digest.
6. The acceptance boundary: `canonical_json_encode` succeeds if and only if
   every number is finite, integral, and within `±(2^53 - 1)`.
7. If a TypeScript encoder under the fixture is added here, both directions of
   the original comparison: equal output bytes when both accept, and equal
   accept/reject verdicts, restricted to values Rust can represent (Rust `&str`
   cannot hold a lone surrogate).

Semantics: `always`. Every Dreamer command digests through this path before
its receipt is written, so the law must hold at every evaluation. There is no
optional path and no situation to reach, only an input domain to cover.

## Investigation log

### Q: Is the `U+FFFD` key in the `astral-key-order` fixture deliberate?

- Sources examined: the decoded fixture keys (`U+0041`, `U+FFFD`, `U+1F600`),
  the pinned canonical string, and, as the source catalog read them at
  `eb6da6109`, TS `:66-78` (`isWellFormedUnicode`) and TS `:81-83` (the
  lone-surrogate rejection).
- Findings: `U+FFFD` is a legitimate assignable character and is an effective
  discriminator, since it sits above the surrogate range in code-point order but
  its single UTF-16 unit (65533) sits above a lead surrogate (55357). So the
  case works. The alternative reading is that an earlier generator emitted a
  lone surrogate or a `U+E000`-style private-use character and something in the
  toolchain replaced it with the replacement character. `U+E000` would
  discriminate identically, which makes the two hypotheses observationally
  equivalent from the fixture alone. Note that the TypeScript encoder would
  *throw* on a lone surrogate (TS `:81-83`), so the fixture could not have been
  generated with one in place.
- Missing evidence: the generator source and revision history for this fixture
  entry.
- Conclusion: unresolved, needs the fixture generator's history. The case
  discriminates correctly either way, so this is a maintenance clarity concern
  rather than a correctness one. Recording it so a future editor does not
  "clean up" the replacement character and silently destroy the discriminating
  power of the case.

### Q: Is the two-case rejection surface (`invalidCanonical`) intended to be that narrow?

- Sources examined, as the source catalog read them at `eb6da6109`: the
  fixture's two-case `invalidCanonical` array (`float` = `1.5`,
  `beyond-safe-integer` = `9007199254740993`), `claim_operation.rs:79-83`,
  `claim_operation.rs:737-747` (`non_canonical_numbers_are_rejected`), TS
  `:102-105`. At HEAD the number path is
  `crates/context-core/src/canonical_json.rs:42-53` and the test is `:235`.
- Findings: the two cases covered the fractional path and the magnitude path,
  which are the two rejection reasons `number_as_safe_integer` can produce for
  a value JSON can express. Non-finite values cannot appear in JSON text at
  all, so the `is_finite()` check (`canonical_json.rs:49` at HEAD) is defensive
  against a programmatically constructed `Value` rather than against parsed
  input. The `-(MAX_SAFE_INTEGER + 1)` boundary and the above-`i64::MAX` path
  were untested by the two-case fixture.
- Missing evidence: whether a programmatically constructed non-finite
  `serde_json::Number` is even representable (serde_json normally refuses to
  construct one).
- Conclusion: resolved with answer. At HEAD
  `crates/context-core/testdata/canonical-json-contract-v1.json` carries five
  `invalidCanonical` cases: `1.5`, `9007199254740993`, `9007199254740992`,
  `-9007199254740992`, and `18446744073709551615`, so both `±2^53` boundaries
  and the above-`i64::MAX` path are pinned, and
  `crates/context-core/src/canonical_json.rs:250`
  `integer_above_i64_max_is_not_canonical` covers the u64 path separately. The
  property test proposed above still adds the generated interior of each region.

### Q: Does a TypeScript encoder under the same fixture exist in this repository?

- Sources examined: a repository-wide search for `canonicalJsonEncode`,
  `compareCodePoints`, and `canonical-json-contract-v1.json` outside
  `crates/context-core`; `packages/e2e-tests/src/incident-pool/history.ts:29`
  (`canonicalJson`); the module header at
  `crates/context-core/src/canonical_json.rs:1-4`.
- Findings: no TypeScript encoder reads the fixture. The only TypeScript
  canonical-JSON function in the tree, `history.ts:29`, is an incident-pool
  harness helper: it sorts keys with JavaScript `<` (UTF-16 code-unit order),
  escapes through `JSON.stringify`, and accepts any number, so it is not the
  twin this record was raised against and shares no vocabulary with it. The
  module header still says the encoding is "shared with the TypeScript runtime";
  that names the host repository's encoder, which this repository does not
  carry. The Rust encoder's only consumers here are the Dreamer request digest
  (`crates/memory-store/src/dreamer_ledger.rs:686`, called from
  `crates/daemon/src/lib.rs:9579`) and the durable-write redaction scan
  (`crates/memory-store/src/lib.rs:3206`), both Rust.
- Missing evidence: none for the question as asked.
- Conclusion: resolved with answer. The record's cross-runtime clause has no
  second runtime to compare against at HEAD, so the catalog states the record as
  the Rust encoder's byte stability and the Dreamer digest formula, and keeps the
  cross-runtime clause as an open question for the day an encoder under the
  fixture is added here.

# status-sanitizer-preserves-inclusive-integer-domain

## Discovery trigger

Specification U3 acceptance: exact wire integer meaning is preserved or
refused before rounding can reach display; status supports its inclusive 2^53
domain. Owner decision OQ9 on #596: a source-aware reviver at the receive seam,
per-field domain validation, exact decimal in human output, raw JSON tokens in
JSON output, a route-local option to omit the ambient consumer identity.

## Evidence trail

`packages/opencode-plugin/src/shared/host-client/exact-json.ts` - the reviver,
the twenty-digit width bound, `exactIntegerWithin`, `exactCount`, `exactU64`,
`exactI64`, `formatExactInteger`, `rawJsonInteger`.

`connection.ts` - `consumeJson` calls `parseExactJson` when the pending
request's `responseMode` is `exact_json` and `JSON.parse` otherwise; an `Error`
body always decodes plainly. The UTF-8 fatal decoder, byte accounting, and lease
release around it are unchanged.

`client.ts` - `hostStatus` requests `exactIntegers`; `awaitRequest` maps
`RequestOptions.exactIntegers` to the `exact_json` response mode; `routeOpen`
reads `options.consumerIdentity === null`.

Tests named in the catalog record.

## Failure scenario

A `host.status` counter of 9007199254740993 renders as 9007199254740992 and
passes a validator written for the 2^53 domain; two receipts whose generations
differ by one past 2^53 list as the same generation.

## Timing windows and dependencies

None.

## What a test must construct

Raw response text with integer tokens at 2^53-1, 2^53, 2^53+1, 2^53+2, the
u64 maximum, the i64 extrema, `-0`, a fraction, an exponent, a digit string,
and a nested array, requested with `exactIntegers`; a control response with an
out-of-domain counter beside a valid one; a default routed `transform` recipe
whose inserted message value carries 2^53+1 and a nanosecond timestamp; a
stream under each response mode; a `JSON.parse` that hands the reviver no
source text; a route open with and without the ambient identity present in the
environment and with the route-local override.

## Investigation log

### Q: Must every integer lexeme become a `bigint`?

- Sources examined: every `typeof === "number"` check on decoded values in the
  host client and its consumers; the decision text.
- Findings: converting every integer would change the type every existing
  consumer sees and is the representation redesign the ticket says needs
  re-scoping. Converting only the lexemes outside the safe-integer range keeps
  every value existing consumers could represent identical and changes only
  the values that rounded before. The split is on `Number.isSafeInteger`, not
  on whether a particular double happens to be exact, because `Number`'s
  shortest-digits printing does not reveal exactness. The guarantees the
  decision names hold under this narrower conversion.
- Missing evidence: none.
- Conclusion: resolved with answer; bounded conversion.

### Q: What bounds the cost of a hostile lexeme?

- Sources examined: `BigInt(string)` cost growth; the response byte cap.
- Findings: no Rust integer field produces more than twenty digits, so a longer
  lexeme is refused as invalid JSON before conversion.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Should every response body decode exactly?

- Sources examined: every production caller of `client.request`; the context
  module's `transform` recipe (`crates/daemon/src/edit_recipe.rs`, `Insert`
  operations push message values verbatim); `parseRecipe` and
  `validateJsonValue` in `hooks/context/edit-recipe.ts`; the output path from
  `applyTransformRecipe` into the array OpenCode serializes.
- Findings: `validateJsonValue` admits only JSON-shaped values, so a `bigint`
  anywhere in a recipe refuses the whole recipe as `malformed`, and the failure
  is sticky because the offending message stays in the transcript. Even if the
  recipe admitted it, the inserted values are handed to OpenCode, whose
  `JSON.stringify` throws on a `bigint`. The plugin's own send path emits
  integers up to `u64::MAX` as plain lexemes, and the daemon returns them byte
  for byte, so a model-written value past 2^53 completes the round trip. No
  module payload consumer can use a `bigint`; the consumers that need exactness
  are `host.status` and the review command.
- Missing evidence: none.
- Conclusion: resolved with answer; exact decoding is a per-request response
  mode. `hostStatus` selects it; a routed `request` selects it with
  `exactIntegers`; everything else decodes as `JSON.parse` does.

### Q: What does a domain validator return for an unsafe integer-valued double?

- Sources examined: `exactIntegerWithin`; `String(2 ** 60)`;
  `parseExactJson("9007199254740993e0")`; `module-wire.ts`'s `wireIntegerText`.
- Findings: the reviver produces an unsafe integer-valued `number` from a
  decimal or exponent spelling, and that value may already be rounded:
  `9007199254740993e0` and `9007199254740993.0` both decode as 2^53, which
  `exactCount` formerly accepted as the exact boundary. A caller passing
  `2 ** 60` as a `number` formerly got a `bigint` back, though nothing on the
  wire can prove such a double was not rounded. Serde reads every such
  spelling, and `-0`, as `f64` rather than `i64`/`u64`, and the send path
  already refuses both. The validators now admit a `number` only when it is a
  safe integer other than `-0`; any value outside the safe range must arrive
  as a `bigint`, which only an integer lexeme produces.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: What happens when the runtime withholds the lexeme?

- Sources examined: the reviver's `context` parameter; the declared engine
  floor; the host process that loads the plugin.
- Findings: `engines` is a manifest hint, and the plugin runs inside the host's
  runtime. Without `context.source` the reviver cannot tell an integer-valued
  double past the safe range from a rounded one; returning it would let 2^53+1
  pass `exactCount` as 2^53. The reviver refuses such a value with
  `SyntaxError`, so the body is `invalid_response_body`; fractions and safe
  integers still decode. The test strips the context through a wrapped
  `JSON.parse`.
- Missing evidence: none.
- Conclusion: resolved with answer; fail closed.

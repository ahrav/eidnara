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

`connection.ts` - `consumeJson` calls `parseExactJson`; the UTF-8 fatal
decoder, byte accounting, and lease release around it are unchanged.

`client.ts` - `routeOpen` reads `options.consumerIdentity === null`.

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
and a nested array; a control response with an out-of-domain counter beside a
valid one; a route open with and without the ambient identity present in the
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

# System lens: product context

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope and P notation: [source register](../source-register.md).

The plan targets conversation transform decoding. It reports 40-message
allocation counts and 200-message scaling at P:L250-L267, with a derive-only
mirror as the proposed shape. These are historical reports, not evidence of
the final code's payoff. Native-message payloads remain Values (P:L105).

The host accepts framing before applying facade/transform caps
(`docs/host-wire-protocol.md:306-308`). A memory optimization that changes
terminal classification affects callers even if projected text is identical.
Canonical block text remains consumed and retained (P:L108-L109).

The 40/200 operation experiment cannot inherit W1's old 1,400/2,500-message
handler-level scope. Candidate: a narrowly named decode-plus-projection
measurement contract with an explicit within-noise stop.

# decode-and-projection-stay-within-resident-pool

System: complete request-derived memory interval.
HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
[Source register](../source-register.md) defines P and B. No peak is measured.

## Discovery trigger

P:L88 explicitly requires the declared pool to bound decode plus projection.
P:L261 reports a separate projection peak of 306,497 bytes, or 7.00 times
messages bytes. That historical report is not a simultaneous full-path bound.

## Evidence trail

1. `crates/host-runtime/src/handler.rs:549-565` requires reservation before
   request allocation and retention while the bytes remain resident.
2. `crates/daemon/src/lib.rs:12153-12170` runs the byte cap before meter
   creation, then holds the meter through dispatch and response settlement.
3. `:16144-16161` probes bodies larger than the facade cap before that meter.
   `:15837-15860` deserializes probe keys through serde string handling.
   The scoped allocation tests at
   `crates/daemon/tests/parse_charge_covers_typed_decode.rs:202-222,311-333`
   use sub-cap long keys, not the above-cap probe interval.
4. `crates/daemon/src/wire.rs:540-559` can rebuild and clone a canonical
   shell. `:731-736` allocates canonical text and hashes it.
5. `crates/daemon/src/served_json.rs:121-144` builds serialization bytes,
   object metadata, and another output buffer. The plan's new canonical
   block producer uses this serializer; its temporary overlap matters.
6. `crates/daemon/src/lib.rs:2871-2889` computes retained cache charge from
   an already built snapshot. Cache admission is not a preallocation proof.
7. `crates/host-runtime/src/config.rs:15-41,481-502` defines and tests separate
   ingress, egress, and scratch slices. Ring accounting is separate again.

The same configuration contract at `:65-72` explicitly covers named logical
payloads, not exact process RSS. Preserve its existing accounting and ownership
rules; requested-layout measurements remain supporting lower bounds.

A1 says admission charge precedes the entry probe at
`docs/properties/hot-path-optimization/latency-audit/catalog.md:159-165`.
The above-cap call order in item 3 contradicts that unqualified claim. This
ordering discrepancy is verified; allocation amount or budget exceedance is not.

Reachability is default-production. The owning scopes are the real handler,
projection builder, and retained-cache holders, not only isolated tests.

## Failure scenario

The new decoder fits a one-copy estimate, yet the typed request remains live
while projection creates canonical text, span buffers, and metadata. The
projected result can fit its retained cache limit even though construction
temporarily exceeds request scratch. Two requests can amplify that gap.

Competing explanation: another already reserved pool covers the transient
allocation. An allocation-to-owner ledger is the discriminating evidence;
an unused global capacity number is not a covering reservation.

## Timing windows and dependencies

Start before cap probing and keep one interval through decode, fallback,
projection, temporary cleanup, and handoff to retained holders. Track input
bytes under ingress and response bytes under egress separately. Include
remaining native Values and all request-owned fields in the full interval.

Track requested sizes alongside demand under the declared logical accounting
rules. Do not equate `sum(retained_bytes)` with a live peak or add separately
reset stage maxima. Preserve existing conservative per-holder charges and
local alias accounting; introduce no new global deduplication or RSS policy.

## What a test must construct

- Near-ceiling text plus nodes, plain and escaped, in both lanes.
- Above-1-MiB escaped keys before the cap probe, and sub-cap controls.
- Nonempty projection with retained typed input and canonical text live.
- Synthetic normalization that forces rebuilding, plus a reused prefix.
- Late errors and fallback, with temporaries observed before cleanup.
- A second holder consuming known scratch capacity.
- Time-correlated logical demand and existing covering charges/headroom for
  every pool, including transient workspace and other owners, without double
  counting reserved headroom or releasing required charges early.

## Investigation log

### Q: Is above-cap probe ordering inconsistent with A1?

- Sources examined: `crates/daemon/src/lib.rs:12153-12166,15837-15860,16144-16161`.
- Findings: The larger-body probe precedes the meter, contrary to A1's
  charge-before-probe claim. Not retaining a key does not establish scratch size.
- Missing evidence: Allocation amount and existing covering charge on this path.
- Conclusion: ordering discrepancy resolved with source evidence; owner
  disposition and a quantitative witness remain open. This is not a measured OOM.

### Q: What reserves projection workspace before construction?

- Sources examined: P:L88; wire builder; canonical serializer; cache admission.
- Findings: The meter remains live, but its magnitude is derived from decode.
  A retained-cache limit does not identify transient construction ownership.
- Missing evidence: Attribution to existing pool charges or reserved headroom.
- Conclusion: needs human input on the uncovered demand, then measurement.
  The catalog does not select a replacement ownership or allocator model.

### Q: Does the proposed new text ceiling fit the full path?

- Sources examined: P:L88 and P:L254-L267.
- Findings: Reported stage rows have different intervals and denominators.
- Missing evidence: Final candidate continuous combined peak near the ceiling.
- Conclusion: unresolved, needs measurement before a larger ceiling is claimed.

### Q: Does the refinement require a new exact RSS bound?

- Sources examined: `crates/host-runtime/src/config.rs:65-72`; independent finding 8.
- Findings: The original wording could be read as a new physical-memory model.
- Missing evidence: Candidate logical-pool and requested-layout observations only.
- Conclusion: resolved. R4 uses the existing declared logical budget and keeps
  the full decode-plus-projection interval, without an exact RSS promise.

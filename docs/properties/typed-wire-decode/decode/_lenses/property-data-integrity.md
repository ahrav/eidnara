# Property lens: data integrity

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

Candidate: `unknown-envelope-fields-do-not-erase-payload-values`.

Inject the same sentinel name at an unknown CK/block field and inside each
retained payload Value. Derived envelope serialization must discard the first
while retaining the second's JSON value. Cover null, empty collections,
escaped text, nested arrays, and provider namespaces. Use known-field
construction as the independent oracle, not the new serializer on both sides.

Evidence: `crates/memory-store/src/lib.rs:114-123,243-247,326-447` separates
envelope fields from payload types. `crates/daemon/src/transform.rs:700-708`
retains native messages and tail_delta. HEAD replay at `:150-152` and
`:271-273` in memory-store is the intentional contract difference.

Store corruption and durable write ordering are outside this representation
change. They are not inferred absent system-wide.

# Property lens: security boundaries

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md).

Untrusted bytes can be short on wire yet dense in heap nodes. Admission
checks the footprint floor before scratch allocation at
`crates/daemon/src/lib.rs:16084-16093`. Long escaped keys threaten probe
allocation before charge; an existing test targets that at
`crates/daemon/tests/parse_charge_covers_typed_decode.rs:311-333`.

Candidates fold into footprint and full-pool properties. Include oversize,
malformed, raw-token, and ignored-field inputs rather than only valid text.
Authentication, authorization, transport descriptor integrity, and key
rotation are N/A to the owned-wire-model change; their contracts are not
weakened or re-proven by smaller decode allocations.

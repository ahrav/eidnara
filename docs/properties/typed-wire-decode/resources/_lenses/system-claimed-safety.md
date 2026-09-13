# System lens: claimed safety guarantees

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope and P notation: [source register](../source-register.md).

P:L51-L52 promises unchanged refusals and accurate retained accounting.
P:L88 makes string copies a measured coefficient, starting at one, while node
copies remain two. The source uses three string copies and two node copies
at `crates/daemon/src/metered_decode.rs:33-64`.

P:L176 permits retuning a witness that becomes admissible. That conflicts
with P:L18, P:L70, P:L164, and the user's frozen-body/boundary constraint.
Changing expected capacity with the candidate can hide the conflict.

`docs/host-wire-protocol.md:304-314` establishes transport/application limits,
not a formula for typed heap ownership. Preserve the narrower source of each
claim. Candidates: fixed admission outcomes and pool-backed physical lifetime.

# Property lens: protocol contracts

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope and P notation: [source register](../source-register.md).

The application uses `invalid_params` and `queue_full` at
`crates/daemon/src/lib.rs:16125-16136`, beneath the host framing contract
(`docs/host-wire-protocol.md:304-314`). Byte caps are inclusive at
`crates/daemon/src/lib.rs:16144-16161`.

Candidate: freeze input bytes, capacities, pressure schedule, outcome, and
boundary neighbours across the optimization. Recomputing capacity using the
candidate at `:20188-20199` is a differential check, not frozen preservation.
P:L176's witness retuning conflicts with the requested frozen contract and
stays a decision. Duplicate-envelope-key lane changes need fallback evidence.

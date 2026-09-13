# Property lens: distributed coordination

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

Narrowly nonapplicable: this change decodes one received body into local owned
types. It changes no quorum, election, distributed lease, or remote commit.
`crates/daemon/src/lib.rs:12911-12951` contains the complete lane decision.

Page generation and route ownership exist around that decision. Their
replay, retirement, and fencing obligations stay in the daemon handlers and
host-runtime catalogs. Preserving their raw tree and digest inputs remains
applicable and is captured in the page record.

No distributed liveness property is invented. A process-local Arc and an
accepted type shape do not establish coordination safety outside this slice.

# System lens: wildcard, run last

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
This pass follows all 21 non-wildcard passes. It is not independent evidence.

The hidden system boundary is parser re-entry, not merely tree allocation.
`crates/daemon/src/lib.rs:19735-19741` explicitly demonstrates that a Value
containing a later-position raw-value token can parse from bytes but fail
when re-read from its sorted map. Message and block custom deserializers add
such re-reads at `crates/memory-store/src/lib.rs:131-133,255-257`.

Put that object under a discarded CK field. Removing the custom deserializer
could remove the rejection even though the outer gate still selects Tree.
The existing corpus's inside-message case targets an unknown IngressMessage
field, not the same field under `ck` (`crates/daemon/src/lib.rs:19937-19947`).
Baseline/final execution is missing; preserve this as a compatibility question.

The accepted plan is absent from HEAD and has a worktree SHA-256 recorded in
the catalog. Its source citations are navigation leads, not verified HEAD
locations. Source verification must not turn that local plan into committed
evidence or treat stale local main as its measured baseline.

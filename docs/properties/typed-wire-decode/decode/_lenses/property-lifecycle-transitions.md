# Property lens: lifecycle transitions

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

Candidate: `decoded-snapshots-own-and-share-prefixes`.

Relevant transitions are decode completion, body-owner drop, ready snapshot
retention, prefix reattachment, projection/native-cache eviction, and final
owner drop. `crates/daemon/src/lib.rs:4351-4400` retains CK/native prefixes
from projection or ready request; `:22886-23004` tests the snapshot fallback.

KTD3's owned outputs allow a later private-input owner to release bytes after
decode. HEAD `Handler::handle` still owns RequestCtx through settlement at
`crates/daemon/src/lib.rs:12153-12170`. Do not report early input release as
implemented, or infer it from dropping a standalone test Vec.

Thread-pool migration and transport lease teardown remain separate work.
This slice proves the output's ownership prerequisite, not those lifecycles.

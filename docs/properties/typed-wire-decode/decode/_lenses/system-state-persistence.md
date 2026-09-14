# System lens: state and persistence

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- `TransformSnapshot::Ready` and `SnapshotLease` own
  `Arc<TransformRequest>` at `crates/daemon/src/lib.rs:1803-1828`.
- `ready_delta_request` clones that Arc at
  `crates/daemon/src/lib.rs:2003-2007`. Delta expansion can recover CK shells
  from either projection or ready snapshot at `:4351-4367`.
- `crates/daemon/src/wire.rs:89-114` retains the ingress shell behind every
  projected block. Dropping a request variable does not drop all readers.
- Accepted KTD3 retains owned strings, payload Values, and shared shells.
  Removing envelope replay state must not introduce a request-buffer borrow.
- Local durable schema and history_summarizer fingerprint rules remain linked
  obligations, not additional records in this decode slice.

Candidate: `decoded-snapshots-own-and-share-prefixes`. Construct input-buffer
drop and cache-owner drop separately; neither proves the other boundary.

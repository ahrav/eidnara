# Property lens: idempotency and replay

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

Candidate: `typed-mutation-is-visible-and-copy-isolated`.

Serialization replay is the applicable meaning of replay here. HEAD public
role/origin/meta/extras mutations can leave `original` authoritative until
`mark_modified` (`crates/memory-store/src/lib.rs:145-160,211-228,266-278`).
KTD1 removes that second authority. Accessor calls with no edit must no longer
change equality or serialized value merely by clearing hidden state.

`WireBlock` equality currently includes its private original because the
whole struct derives PartialEq (`crates/memory-store/src/lib.rs:232-240`).
The accepted tuple is kind plus provider extras. Do not infer byte equality
from all Value number equalities; the identity workstream owns that caveat.

Remote request retry deduplication and historian receipts are outside scope.

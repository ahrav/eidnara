# Property lens: concurrency

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

Candidates: owned snapshot retention and copy-isolated typed mutation.

Hold a request, a projection, a reattached prefix, and a cloned block owner.
Edit one request shell through `Arc::make_mut`, then drop the original input
buffer and selected owners. Surviving owners must keep their prior typed
content. Unchanged synthetic state should share the same shell pointer under
the accepted replacement; a changed synthetic flag needs a distinct shell.

Evidence: `crates/daemon/src/wire.rs:89-114,540-559,1749-1792,1818-1860`.
The tested mobility bound is at `:1796-1800`.

No new atomic ordering or lock-free reclamation algorithm is introduced.
The vulnerable sequence is owner retention and copy-on-write, so a real
multi-thread race is not required merely to construct it.

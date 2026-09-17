# Existing checks and reuse assessment

System: `/local/home/ahrav/scratch/eidnara`. Base: `rp27/u3c-dense-lane` at
`f318c4a4`. Every check below is `unaudited`: source inspection establishes its
presence and assertions, not adequacy.

| Location and check | Asserted behavior | Status | Limitation for this part |
| --- | --- | --- | --- |
| `crates/daemon/tests/kernel_routes.rs`, idempotent `kernel.commit` replay | A commit retried under the same key replays; a different digest is refused. | unaudited | Kernel commit intents, not edit receipts; durable, not in-memory. |
| `crates/retrieval/tests/identity.rs`, preparation digest tests | Revision, representation, or span change alters the digest. | unaudited | Digest only; no receipt state. |
| `crates/daemon/tests/query_route_handler.rs`, disable tests | A route without an installed limit set answers `disabled`. | unaudited | The query route, not the edit routes. |
| `crates/daemon/tests/kernel_routes.rs`, `replayed_intents_return_one_receipt_and_projects_never_collide` | A commit key under one project does not answer another project's route. | unaudited | Kernel commit receipts prefixed by `ProjectBinding::operation_key`; the edit store follows the same scope id but is in memory. |

Suspiciously quiet areas: before this part nothing in the daemon distinguished
a prepared edit from an applied one, and no route answered `unknown`.

# System lens: bug history and density

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope and baseline comparison: [source register](../source-register.md).

History names `3de11b27` (charge before lane probe), `33511c5e` (unescape
buffer and no second classification parse), and `7edeb90f` (bounded held
bytes). Their titles are leads, not incident reproductions. The mechanisms
are visible in `crates/daemon/src/lib.rs:12153-12170,16084-16103` and
`crates/daemon/src/metered_decode.rs:242-303`.

Tests concentrate on successful dense-native tree peaks and direct text
peaks in `crates/daemon/tests/parse_charge_covers_typed_decode.rs:103-199`.
Failure gates have allocation checks at `:238-333`. Text-heavy combined tree
conversion and a continuous projection peak are quieter areas.

No additional incidents, issue reproductions, or external repositories are
available. HP1 issue titles do not establish measured defects.

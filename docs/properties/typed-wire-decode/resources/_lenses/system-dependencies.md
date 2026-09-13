# System lens: dependencies

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope: [source register](../source-register.md).

Serde supplies the visitor sequence. `visit_str` and `visit_borrowed_str`
have different scratch obligations at
`crates/daemon/src/metered_decode.rs:816-831`. Raw-value-token behavior has
explicit footprint tests at `:1097-1112`; generic JSON intuition is not an
independent oracle for those cases.

`GlobalAlloc` counters record requested sizes, not allocator size classes
(`crates/daemon/tests/parse_charge_covers_typed_decode.rs:28-29`). Criterion
and its harness configuration are measurement dependencies, not runtime
proof. `hot_path` requires `bench-internals`
(`crates/daemon/Cargo.toml:77-81`), which P:L214 omits.

Remote service outages, DNS, and broker dependencies are N/A to this scope.

# System lens: unproven assumptions

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope and P notation: [source register](../source-register.md).

One moved string at the end of `from_value` does not prove one copy at every
instant. P:L88 permits raising the coefficient after measuring both lanes.
Escaped scratch, internally tagged buffering, failed typed prefixes, and
canonical output coexistence require separate accounting.

An existing `Arc` does not imply zero charge: the holder policy explicitly
charges the whole shared ingress allocation
(`crates/daemon/src/retained_size.rs:263-271`). Nor does a declaration prove
that transient projection allocations acquired it before allocation.

W1 is invalidated at
`docs/properties/hot-path-optimization/latency-audit/catalog.md:1772-1773`.
Its retained text and plan references cannot silently make it active.
Missing: raw allocation trace, final binary, frozen boundary manifest, and
predeclared timing noise rule. No answer is inferred from test names.

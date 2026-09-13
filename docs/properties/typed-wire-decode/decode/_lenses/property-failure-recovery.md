# Property lens: failure recovery

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

Candidates: `typed-failure-preserves-tree-outcome` and
`nested-duplicate-fallback-is-exercised`.

Use literal bytes with duplicate recognized fields. A JSON Value fixture
cannot retain two occurrences. The compatibility gate must accept the bytes,
the typed attempt must fail, and the independent tree conversion must succeed.
The handler must report the tree answer without dispatching the failed parse.

Evidence: `crates/daemon/src/lib.rs:12921-12951,19967-20079,20140-20181`.
Duplicates inside retained Value maps need separate controls: not every
duplicate forces a derived struct to fail.

No process crash or storage recovery is required for parse fallback. Calling
the tree lane directly cannot witness recovery from a failed typed attempt.

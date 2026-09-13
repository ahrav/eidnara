# System lens: product context

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- The settled plan targets transform decode retention while preserving the
  OpenCode caller's content. KTD3 rejects borrowed request types and payload
  typing; KTD5 retains paging through trees.
- A lost tool input, opaque part, or media source changes what the model sees.
  These are kept payload Values, not disposable envelope fields.
- The producer emits `provider_executed` only for true at
  `packages/opencode-plugin/src/hooks/context/module-wire.ts:1013-1014`.
  Plan R5 accepts default-false omissions; identity details stay elsewhere.
- A stale serialized public-field edit can hide a real transformation.
  The current stale-meta behavior is pinned at
  `crates/daemon/src/transform.rs:13722-13734`.

No observed user incident is supplied. Consequences are failure scenarios,
not claims about production occurrence or measured latency improvement.

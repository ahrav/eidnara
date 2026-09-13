# System lens: claimed liveness guarantees

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- Invalid typed decoding reaches one tree attempt in
  `crates/daemon/src/lib.rs:12923-12951`; this is bounded control flow, not an
  eventual-progress guarantee needing a timer.
- Page collection and expiry are separate state machines. Existing handler
  catalogs own their progress obligations. Preserving the tree lane does not
  introduce a new page completion deadline.
- Issue 438 owns joined blocking work. Issue 524 owns private-input release.
  This slice preserves owned output needed by those plans but cannot claim
  either lifecycle has been implemented from a type assertion.

No new liveness record is justified in this scope. Independent reachability
of successful nested-duplicate fallback is required instead. No artificial
timeout or unbounded eventual assertion is proposed.

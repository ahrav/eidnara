# System lens: dependencies

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

- Serde derives and serde_json define duplicate recognized fields, missing
  defaults, internally tagged enums, ignored fields, and Value conversion.
  Their behavior is part of the compatibility experiment, not an assumption.
- `TransformRequestWire.serializer_profile` remains `Option<String>` and is
  defaulted into a String at `crates/daemon/src/transform.rs:810-818,917-927`.
  Replacing that wrapper would change the null boundary.
- Actual payload fields remain Value-bearing at
  `crates/memory-store/src/lib.rs:326-447` and
  `crates/daemon/src/transform.rs:700-708`.
- The plugin hashes page arrays before send at
  `packages/opencode-plugin/src/hooks/context/module-wire.ts:148-155`.
  Rust verifies the raw tree before typed conversion.

No external service dependency participates in envelope decoding. Dependency
outage, DNS, and broker faults are narrowly inapplicable; parser/version
compatibility remains applicable.

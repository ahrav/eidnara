# Eidnara

Eidnara is a Rust-first codebase that consolidates the capabilities of
`ahrav/commons` and `ahrav/magic-context` into one workspace. It starts from a
fresh Git history under the MIT license. The source repositories stay
authoritative until the migration's recorded point of no return.

## Layout

```
crates/                 Rust workspace members (arrive with waves U2 through U5)
packages/               TypeScript adapters, CLI, native addon, payload (U5, U7)
fixtures/vocabulary/    predecessor-captured schema vocabularies (U4)
migration/              registry, readiness, owners, per-wave receipts
docs/properties/        property catalogs and the pinned METHOD
docs/runbooks/          architecture review and cutover runbooks
release/                registry gate, payload index, publish journal, cutover record
scripts/eidnara-migration/  permanent migration tooling
```

## Migration controls

Every wave lands with a receipt, a property-impact record, an
architecture-impact record, and a waiver list under `migration/waves/<wave>/`.
`migration/registry.json` classifies every legacy identity, TypeScript file,
persistent family, and authored file. `migration/upstream-readiness.json`
names the beads that must be closed before a wave pins its source commit.
`migration/owners.json` names the role owners and the go/no-go rule.

Validate with:

```
bun install
bun run typecheck
bun run test:eidnara-migration
bun run eidnara:check registry migration/registry.json
bun run eidnara:check waivers migration/waves/<wave>/waivers.json
bun run eidnara:check receipt migration/waves/<wave>/receipt.json
bun run eidnara:check property-impact migration/waves/<wave>/property-impact.json
bun run eidnara:check architecture-impact migration/waves/<wave>/architecture-impact.json
bun run eidnara:check property-catalog docs/properties/<part>/index.json
```

Receipt checks read the pinned source commits with `git ls-tree`. By default the
checker looks for the source repositories as sibling checkouts of this
repository (`../commons`, `../magic-context`); override with
`--checkout <repo>=<dir>`.

`bun run eidnara:registry-audit` captures raw npm registry responses for every
`@eidnara/*` and `@cortexkit/*` name into `release/registry-gate.json` with
response digests. `--check` refuses a gate file older than 24 hours or missing
any digest; `--require-reservation` also demands the inert
`1.0.0-reserved.N` version on every `@eidnara` name.

Refresh a receipt's destination hashes with `sha256sum <file>`; the checker
recomputes them and refuses stale values.

## Rust

The workspace uses Rust 2024, Cargo resolver 3, and MSRV 1.89 declared once in
`[workspace.package]`. Every member sets `publish = false`. Cargo commands run
once the first member arrives in U2.

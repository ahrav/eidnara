# U1 review notes

Per-file review evidence for the U1 receipt. Every file in this wave is either
authored in the destination (`source: null`) or copied verbatim from a pinned
source commit.

## Authored files

Design review: each authored file implements a numbered step of U1 in
`docs/plans/2026-09-01-0633-refactor-eidnara-repository-migration-plan.md`
(commons repository). The mapping:

| File | Plan step |
|---|---|
| `Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `package.json`, `tsconfig.json`, `.gitignore`, `LICENSE`, `NOTICE`, `README.md` | U1 files list; R1, R17 |
| `.github/workflows/ci.yml` | U1 files list; repository gates table |
| `migration/registry.json` | steps 3, 4 (KTD3, KTD4, KTD13) |
| `migration/upstream-readiness.json`, `migration/owners.json` | step 5 (KTD16, KTD17) |
| `migration/waves/U1/waivers.json` | step 6 waiver schema |
| `migration/waves/U1/crate-name-collision-check.json` | step 2 |
| `scripts/eidnara-migration/check.ts`, `check.test.ts` | steps 6, 7, 8, 10 (KTD1, KTD11, KTD12) |
| `scripts/eidnara-migration/registry-audit.ts`, `registry-audit.test.ts` | step 11 |
| `scripts/eidnara-migration/generate-property-index.ts`, `generate-property-index.test.ts` | step 12 (KTD14) |
| `docs/runbooks/architecture-review.md` | step 9 |
| `docs/properties/README.md`, `fixtures/vocabulary/README.md` | U1 files list |

Negative tests: `scripts/eidnara-migration/check.test.ts` (56 tests),
`registry-audit.test.ts` (6 tests), `generate-property-index.test.ts` (4 tests)
cover the U1 test scenarios AE1, AE2, AE6, AE7, AE13, AE14, AE19, AE20, AE21,
AE25, AE26, AE30, AE32 and the unlabeled scenarios in the plan's U1 list.
Control-record JSON files are exercised by the checker itself:
`bun run check:repo`.

## Copied files

`docs/properties/METHOD.md` is a verbatim copy of
`magic-context@b5273dcb2a76fb0ffe9800b7c54bbd8d1ad98825:docs/properties/METHOD.md`.
The plan pins this revision as the method for every catalog. Its prose was read
in full during U1 review and left unchanged; a `/doc-rigor` edit would change
the pinned method and is out of scope for this wave. Any wording change to
METHOD goes through a separate reviewed pin change.

## Generated files

- `bun.lock`: `bun install` output.
- `release/registry-gate.json`: `bun run eidnara:registry-audit` output; raw npm
  responses are hashed, not stored.

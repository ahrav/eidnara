# U1 review notes

These notes provide per-file review evidence for the U1 receipt. Every file in
this wave is either authored in the destination (`source: null`) or copied
verbatim from a pinned source commit.

## Authored files

Each authored file implements a numbered U1 step from the repository plan
`2026-09-01-0633-refactor-eidnara-repository-migration-plan` in the `primitives`
source. The mapping is:

| File | Plan step |
| --- | --- |
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
| `docs/properties/README.md` | U1 files list |

The negative tests in `scripts/eidnara-migration/check.test.ts` (56 tests),
`registry-audit.test.ts` (6 tests), and `generate-property-index.test.ts` (4
tests) cover U1 scenarios AE1, AE2, AE6, AE7, AE13, AE14, AE19, AE20, AE21,
AE25, AE26, AE30, and AE32, plus the unlabeled scenarios in the plan's U1
list. The checker exercises the control-record JSON files with
`bun run check:repo`.

## Copied files

`docs/properties/METHOD.md` is adapted from the method document at
`host@b5273dcb2a76fb0ffe9800b7c54bbd8d1ad98825`: the four field-format lines
that used an em dash as the separator between an enumerated field head and its
note use a spaced hyphen instead, and the formatting rule names that
convention. No other prose changed; the receipt records the file as `adapted`
for that reason. Editing the method further would change the pinned method and
is out of scope for this wave.

## Generated files

- `bun.lock`: `bun install` output.
- `release/registry-gate.json`: `bun run eidnara:registry-audit` output; raw npm
  responses are hashed, not stored.

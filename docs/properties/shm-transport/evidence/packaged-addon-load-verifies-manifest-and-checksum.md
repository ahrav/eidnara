# packaged-addon-load-verifies-manifest-and-checksum

## Discovery trigger

`docs/shm-transport.md:96-98` documents a manifest-and-checksum verification on
the platform-package load path and a clean-install gate that this tree does not
implement. No catalog record owned the verification, and the native CI step
builds a local `shm_native.node` first, so the verified path is never the tested
path.

## Evidence trail

- `packages/shm-native/index.ts:191-229` is `packageAddonPath(platform)`. It
  resolves the package directory, throws `NativeStartupError("missing_addon")`
  when the directory is absent (`:197`), reads and parses
  `payload-manifest.json` or throws `missing_manifest` (`:200-209`), checks
  `manifest.package.name` and `.target` against the platform or throws
  `wrong_platform_payload` (`:210-215`), finds the payload entry and requires a
  64-hex-digit `sha256` or throws `missing_checksum` (`:216-219`), throws
  `missing_addon` when the payload file is absent (`:221-222`), hashes the
  payload with `createHash("sha256")` and throws `checksum_mismatch` on a
  difference (`:224-226`), and returns the path (`:228`).
- `index.ts:231-262` is the load: `requireAddon` computes
  `new URL("./shm_native.node", import.meta.url)` and takes that file when it
  exists (`:236-238`), otherwise `packageAddonPath(platform)` (`:239`); it then
  `createRequire`s the path (`:245`) and refuses a non-release build
  (`debug_build`, `:249-251`) or a wrong-target binary (`wrong_platform_binary`,
  `:252-254`).
- `index.ts:35-45` is the `NativeStartupError` reason union, which names every
  refusal above.
- `.github/workflows/ci.yml:170-181`: the native step runs `build:native`, then
  typecheck, the package tests, and the Bun capability test. `build:native`
  places `shm_native.node` beside `index.ts`, so every in-tree run takes the
  local path.
- `packages/shm-native/tests/` contains no reference to `packageAddonPath`,
  `payload-manifest`, `missing_manifest`, `wrong_platform_payload`, or
  `checksum_mismatch`.

## Failure scenario

A release ships a platform package with a missing or stale manifest, a checksum
entry for another build, or a payload altered after the manifest was written. In
the tree these are refused before any code loads. Nothing observes that the
refusals still fire, so a regression in any check, or a reordering that loads
before hashing, passes CI and every catalog gate.

## Timing windows and dependencies

None; a single synchronous load. The dependency is the absence of a local
`shm_native.node`, which decides which path runs at all.

## What a test must construct

A staged package directory under a temporary prefix with no local
`shm_native.node` beside `index.ts`; then, per fault shape, one mutation and an
assertion on the exact `NativeStartupError` reason plus the absence of a loaded
addon; and one unaltered package that loads and probes available.

## Investigation log

### Q: Does any in-tree run take the package path?

- Sources examined: `ci.yml:170-181`, `packages/shm-native/package.json`
  scripts, `packages/shm-native/tests/`.
- Findings: `build:native` always precedes the tests and produces the local
  file; no test stages a package.
- Missing evidence: none.
- Conclusion: `not yet` exercised; the record is a requirement with an
  implemented mechanism and no witness.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 23, `index.ts:196-208` now `index.ts:231-262`: `requireAddon` also translates a failed load of a present, checksummed payload into `addon_load_failed` (`index.ts:244-248`).
  - line 29, `index.ts:23-28` now `index.ts:35-45`: The union carries a tenth reason at HEAD, `addon_load_failed` (`index.ts:44`), which the refusal list above does not name.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

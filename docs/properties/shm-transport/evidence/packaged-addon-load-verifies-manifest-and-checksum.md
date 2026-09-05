# packaged-addon-load-verifies-manifest-and-checksum

## Discovery trigger

`docs/shm-transport.md:97-98` documents a manifest-and-checksum verification on
the platform-package load path and a clean-install gate that this tree does not
implement. No catalog record owned the verification, and the native CI step
builds a local `shm_native.node` first, so the verified path is never the tested
path.

## Evidence trail

- `packages/shm-native/index.ts:203-241` is `packageAddonPath(platform)`. It
  resolves the package directory, throws `NativeStartupError("missing_addon")`
  when the directory is absent (`:209`), reads and parses
  `payload-manifest.json` or throws `missing_manifest` (`:212-221`), checks
  `manifest.package.name` and `.target` against the platform or throws
  `wrong_platform_payload` (`:222-227`), finds the payload entry and requires a
  64-hex-digit `sha256` or throws `missing_checksum` (`:228-231`), throws
  `missing_addon` when the payload file is absent (`:233-234`), hashes the
  payload with `createHash("sha256")` and throws `checksum_mismatch` on a
  difference (`:236-238`), and returns the path (`:240`).
- `index.ts:243-274` is the load: `requireAddon` computes
  `new URL("./shm_native.node", import.meta.url)` and takes that file when it
  exists (`:248-250`), otherwise `packageAddonPath(platform)` (`:251`); it then
  `createRequire`s the path (`:257`), maps a loader failure to `addon_load_failed`
  (`:258-260`), and refuses a non-release build (`debug_build`, `:261-263`) or a
  wrong-target binary (`wrong_platform_binary`, `:264-266`); both post-load checks
  call exports of the already executed module (`:261`, `:264`). The first failure
  is memoized in `loadError` (`:270-271`) and rethrown by every later call
  (`:245`). The parsed manifest is used without a shape check (`:213-216`,
  `:223`, `:228`), so a `null` or malformed manifest raises a raw `TypeError`
  rather than a `NativeStartupError`.
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
  - line 23, `index.ts:208-220` now `index.ts:243-274`: `requireAddon` also translates a failed load of a present, checksummed payload into `addon_load_failed` (`index.ts:256-260`).
  - line 29, `index.ts:23-28` now `index.ts:35-45`: The union carries a tenth reason at HEAD, `addon_load_failed` (`index.ts:44`), which the refusal list above does not name.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.

### Q: What did the fresh-context evaluation of this record find?

- Sources examined: `packages/shm-native/index.ts:203-274`,
  `packages/shm-native/package.json`, `release/registry-gate.json`,
  `docs/shm-transport.md:95-99`.
- Findings: nine bare citations in the record were stale after two base merges
  and are corrected; the reason taxonomy is not total over malformed manifest
  content; the manifest is unsigned and co-located with the payload, so the check
  detects corruption and substitution rather than tampering; the failure is
  memoized for the process; the platform package the path loads is unpublished
  and absent from the tree.
- Missing evidence: a manifest producer and a staged-package test.
- Conclusion: Guarantee, Check, Fault/timing angle, and Impact restated; three
  open questions added.

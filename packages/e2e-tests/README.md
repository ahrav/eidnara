# @eidnara/e2e-tests

End-to-end tests for the Eidnara adapters against real harness processes.
The package is private and never published.

## What runs

- **Rust mode only.** The OpenCode harness starts a real `opencode serve`
  with the built plugin bundle (`packages/opencode-plugin/dist/index.js`,
  loaded by `file://` URL) against the daemon's `direct_host_fixture`
  (`crates/daemon`, built with `--features direct-host-fixture`). The runner
  writes user-tier consent for `transform_mode: "rust"` plus the fixture's
  `subc.connection_file`, so the plugin routes every transform through the
  daemon. There is no TypeScript transform mode and no plugin database; the
  harness reads nothing but OpenCode's own session store and the daemon's
  `session.status` route.
- **Pi load smoke.** `pi-smoke` starts a Pi RPC process with the built Pi
  extension (`packages/pi-plugin/dist/index.js`), completes one mock turn,
  checks the tools are registered, and asserts that the isolated data
  directory contains no storage file afterwards. It makes no stored-data
  assertions.
- **Mock provider.** `src/mock-provider/server.ts` serves the Anthropic
  Messages API shape (streaming and error bodies) so no test needs a real
  model.

## Retained suite

`mode-manifest.json` lists every test file under `tests/`; each Rust-mode
entry is `tier: "rust-only"` and the Pi entry is `tier: "pi-smoke"`, all with
`contract_refs: ["U5-PORT"]`, and
`validate-mode-manifest` fails when a test file lacks an entry or an entry
lacks a file. The retained set is:

```
cache-invariants            rust-fm-oc-2                 rust-park-self-heal
cache-stability             rust-fm-oc-3                 rust-removal-self-heal
incident-pool-green         rust-fm-oc-5                 rust-smoke
rust-ctx-reduce-roundtrip   rust-fold-under-pressure     rust-steady-state-byte-identity
rust-duplicate-tool-use-id  rust-historian-producer      rust-tail-mutation-readopt
rust-multi-frame-delta-perf thinking-block-safety        pi-smoke
```

Seventeen Rust-mode tests plus `pi-smoke`.

## Prerequisites and skipping

Every Rust-mode test is wrapped in `describe.skipIf(!rustPrereqs.ok)`.
`detectRustModePrereqs()` (`src/rust-runner/hermetic-host.ts`) requires
Linux, `cargo`, the workspace's `direct_host_fixture` example, and a
shared-memory channel the current runtime can start. The plugin reaches the
daemon only through that channel, and OpenCode embeds the same Bun release
the test runner uses, so the probe predicts the plugin. Bun 1.3.14 lacks
`worker_threads.markAsUntransferable`, which the channel's capability probe
needs; on that runtime the suite skips and prints
`shared-memory channel unavailable on this runtime: runtime_mechanism_unavailable`.

`pi-smoke` is wrapped in `describe.skipIf(!piPrereqs.ok)`. `detectPiPrereqs()`
(`src/pi-runner/spawn.ts`) requires `@earendil-works/pi-coding-agent`
(installed as a dev dependency of `packages/pi-plugin`, resolved from that
package's `node_modules` or the root `node_modules/.bun`), `node` on `PATH`
at or above the floor `packages/pi-plugin/package.json` declares in
`engines.node` (Pi's CLI runs under Node and loads the extension into it),
and the built Pi extension. With
`EIDNARA_E2E_REQUIRE_PI=1` an unmet prerequisite fails the file instead of
skipping it; the `gates` job sets it because it provides all three, so a skip
there would mean a resolution or layout regression.

## Commands

```bash
bun run test                      # unit tests for the harness, scripts, and incident pool
bun run test:rust                 # the retained Rust-mode suite (skips when prerequisites are unmet)
bun run test:incidents:rust       # the incident pool over the Rust harness
bun run validate-mode-manifest    # every test file has an entry and every entry a file
bun run validate:incident-history # catalog, adjudications, and source inventory agree
bun run mutation:rust-fm          # apply each FM-OC mutation, run its test, revert
```

`test` and `test:rust` build `packages/opencode-plugin/dist/index.js` when
it is absent. `test:rust` also builds `packages/shm-native/index.js` (the
Node entry the extension imports) and `packages/pi-plugin/dist/index.js`
when they are absent; run `bun test tests/pi-smoke.test.ts` directly only
after `bun run ensure:pi-dist`.

Environment:

- `EIDNARA_E2E_MODE=rust` is the only accepted mode; `run-test-selection.ts`
  exits 2 for any other value.
- `EIDNARA_E2E_REQUIRE_PI=1` makes `pi-smoke` fail rather than skip when a Pi
  prerequisite is missing.
- `EIDNARA_E2E_DIRECT_HOST_FIXTURE_BIN` overrides the fixture binary path
  (default `target/debug/examples/direct_host_fixture`).
- `EIDNARA_RUST_E2E_FOLD=1` and `EIDNARA_RUST_E2E_DUPLICATE_IDS=1` enable the
  two tests that drive the daemon past its pressure thresholds.

## Incident pool

`src/incident-pool/` runs registered incident cases in isolated child
processes over the Rust harness. Each case child appends one JSON envelope
to the file named by `EIDNARA_INCIDENT_ENVELOPE_PATH` inside its case
workspace; the runner reads it after exit and classifies a missing,
duplicate, oversized, or malformed envelope. `incidents/catalog.json`,
`incidents/adjudications.jsonl`, and `incidents/source-inventory.json` carry
the Rust variants that the source-linked regression scenarios back;
`validate:incident-history` checks them against each other.

## Mutation drills

`mutations/*.json` record text mutations of
`packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts` and of
`crates/daemon/testdata/differential-golden.json`, with the test that must
turn red under each. The `mutation:*` scripts apply a mutation, run its test,
record the outcome, and revert. A drill refuses to record evidence from a
suite that skipped, so the records stay `null` with an `adequacy_finding`
until the runtime can start the shared-memory channel.

## CI

The `gates` job installs OpenCode 1.18.22 and Pi 0.80.2 on Node 24.18.0,
builds the fixture, and runs `validate-mode-manifest` and `test:rust` with
`EIDNARA_E2E_REQUIRE_PI=1`.

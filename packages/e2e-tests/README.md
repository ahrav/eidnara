# @eidnara/e2e-tests

End-to-end tests for the Eidnara adapters against real harness processes.
The package is private and never published.

## What runs

- **Rust mode only.** The OpenCode harness starts a real `opencode serve`
  with the built plugin bundle (`packages/opencode-plugin/dist/index.js`,
  loaded by `file://` URL) against the daemon's `direct_host_fixture`
  (`crates/daemon`, built with `--features direct-host-fixture`). The runner
  writes the fixture's `host.connection_file` into the user tier, and the
  plugin routes every transform through the daemon. There is no other
  transform mode and no plugin database; the
  harness reads nothing but OpenCode's own session store and the daemon's
  `session.status` route.
- **Pi load smoke.** `pi-smoke` starts a Pi RPC process with the built Pi
  extension (`packages/pi-plugin/dist/index.js`), completes one mock turn,
  checks the tools are registered, and asserts that the isolated data
  directory contains no storage file afterwards. It makes no stored-data
  assertions.
- **Mock provider.** `src/mock-provider/server.ts` serves the Anthropic
  Messages API shape (streaming and error bodies) so no test needs a real
  model. `new MockProvider({ forward })` instead forwards every request to one
  selected HTTPS Messages endpoint under frozen call, output, timeout, and
  spend limits (`src/mock-provider/forward.ts`); scripting is unavailable in
  that mode. `RustTestHarness.create({ forward })` runs OpenCode on
  `forward.model` at `forward.contextLimit`.
- **Bedrock-only callers.** `bedrock-only-opencode` and `bedrock-only-pi` run
  a deployment whose only model credential is the `amazon-bedrock` row and
  whose environments carry no `ANTHROPIC_*` variable. Memory capture, the
  context researcher, the Memory Classifier, the History Summarizer, and
  Wrapup each complete against `src/bedrock-peer/`, a loopback Bedrock
  runtime that answers `converse-stream` over HTTP/1.1 and cleartext HTTP/2
  only for requests whose SigV4 signature verifies under that row. The
  fixture runs ModelExecution through the real OpenCode and Pi backends
  (`--harness-runtime`): both release closures are materialized from the
  pinned binaries, the Pi closure gains one provider extension that points
  `amazon-bedrock` at the peer, and OpenCode's inline config names the peer
  as that provider's `baseURL`. The live sessions reach the peer through
  `opencode.json` and Pi's `models.json`.

## Retained suite

`mode-manifest.json` lists every test file under `tests/`; each Rust-mode
entry is `tier: "rust-only"`, `compression-fidelity-pi` is `tier: "pi-rust"`,
and `pi-smoke` is `tier: "pi-smoke"`. Each
entry names the contracts it covers in `contract_refs`: the port suites carry
`["U5-PORT"]`, and `compression-fidelity-qualification` carries its
property-catalog records `cf-fixture-script-qualification` and
`cf-delivery-credit-requires-published-folded-capture`.
`validate-mode-manifest` fails when a test file lacks an entry or an entry
lacks a file. A file whose every test is `it.skip` must carry a
`quarantined` reason in its entry; the validator refuses a fully skipped
file without one, refuses a marked file that has live tests again, and
prints the quarantines it validated so a green `test:rust` never hides
them. The retained set is:

```
compression-fidelity-delivery   compression-fidelity-memory   compression-fidelity-pi
compression-fidelity-qualification   compression-fidelity-forwarding
cache-invariants            rust-fm-oc-2                 rust-park-self-heal
cache-stability             rust-fm-oc-3                 rust-removal-self-heal
incident-pool-green         rust-fm-oc-5                 rust-smoke
rust-eidnara-reduce-roundtrip   rust-fold-under-pressure     rust-steady-state-byte-identity
rust-duplicate-tool-use-id  rust-history_summarizer-producer      rust-tail-mutation-readopt
rust-multi-frame-delta      rust-stale-preference        thinking-block-safety
pi-smoke
```

Twenty-one Rust-mode tests plus `compression-fidelity-pi` and `pi-smoke`.
`rust-stale-preference` runs only
under `EIDNARA_EVAL_S0_BUDGET_MS`, like the S0 campaign.

## Compression fidelity delivery

The direct-host fixture resolves compression fidelity scenarios through the
digest-checked corpus (`crates/daemon/testdata/compression-fidelity.json`).
Three control commands drive it:

- `script-cases` takes `entries`, up to eight corpus scenario IDs or
  `filler`, replacing any earlier queue. An unknown ID, an empty queue, or a
  ninth entry is refused with a typed error code. While a queue is armed,
  every admitted summarizer request consumes its front entry: a scenario
  entry binds its source's approved example to the ordinals the request
  presents, and a filler entry answers with compact fixture-authored
  segments. A request that does not present the source's messages fails as
  `fixture_script_mismatch`, and a request after the queue is empty fails as
  `fixture_script_exhausted`; neither returns default text. A scheduled
  `typed-failure` consumes no entry.
- `filler:N` answers one request with at most `N` compact rows, `echo`
  answers with the default segments whose bodies repeat the presented text,
  `echo:N` repeats it in at most `N` rows, and a scenario ID with
  `@p1-only` serves its approved example without the P2 and P3 bodies. A
  row count outside 1 to 64 is refused as `bad_row_count`.
- `script-status` reports the queue and the lifetime bound, filled,
  mismatched, and exhausted counts. Its `bindings` list the current
  selection's delivered answers: scenario, ordinal range, and answer digest.
  The backend counters keep their existing meaning.
- `script-source` returns a scenario source's native records and leak
  probes. `src/compression-fidelity/delivery.ts` seeds the records into
  OpenCode's own `opencode.db`; the TypeScript side never reads native text
  from the corpus file.

`compression-fidelity-delivery` runs the campaign in
`src/compression-fidelity/campaign.ts`: m1, warm, cold m0, natural decay to
P2 through P5 checked against `decay-oracle.ts`, guard pressure for C1.S5 and
C3.S5, a parser-fallback row, and the capability pins in `capabilities.ts`.
Each case runs against its own harness as a concurrent test, and `test:rust`
passes `--max-concurrency 6`, the limit Bun applies to concurrent tests.
`compression-fidelity-pi` serves C1 through the Pi `context` handler at P1 in
m1 and m0. `compression-fidelity-memory` credits C3's memory example only
when a verified copy reaches `<project-memory>`, recovers it through
`eidnara_search`, and serves C4's omitted row as a hint only while
auto-search is on. Its fixture controls are `memory-seed`, which commits a
verified project memory into an existing decision's scope,
`memory-admission`, which records one admission event on a decision, and
`user-hint-outcome`, which returns the newest auto-search decision and its
ranking trace. The pass line's `admission`, `invocation_bytes`,
`invocation_charged`, and `history_budget` fields feed both.

`compression-fidelity-forwarding` runs one OpenCode turn through the
forwarding mock to an in-process provider double: the double calls `read`, the
tool-result turn is forwarded and captured, the final answer reaches the
session, and the canary credential appears only on outbound requests.

`compression-fidelity-qualification` drives one scripted case to accepted
publication and judges the next session-correlated provider request: the
pass line must show an applied recipe served from the transform, the history
must serve the reviewed P1 body, and outside the `<session-history>` wrapper
the request must carry none of the source's leak probes.
`RustTestHarness.tagCaptures` files main requests under their session and
case identity, and `resetMock()`, which `runScriptedToolCall` uses, retains
them before the mock starts a fresh run. With
`EIDNARA_FIDELITY_OBSERVATIONS_DIR` set, each judgment is written there as an
owner-only `opencode-delivery.<case>.<scenario>.<stage>.json` record. CI sets
`EIDNARA_E2E_REQUIRE_FIDELITY=1`, which turns a missing Rust prerequisite
into a failure of the qualification file.

## Prerequisites and skipping

Every Rust-mode test is wrapped in `describe.skipIf(!rustPrereqs.ok)`.
`detectRustModePrereqs()` (`src/rust-runner/hermetic-host.ts`) requires
Linux, `cargo`, the workspace's `direct_host_fixture` example, and a
shared-memory channel the current runtime can start. The plugin reaches the
daemon only through that channel, and OpenCode embeds the same Bun release
the test runner uses, so the probe predicts the plugin. The probe gates exact
external-buffer bounds, detachment, and cleanup hooks; transfer prevention is
reported (`transferPreventionMechanism`), not gated, because Bun 1.3.x has no
working `worker_threads.markAsUntransferable` and Node refuses to transfer
external buffers on its own. A runtime that fails a gated mechanism skips the
suite and prints `shared-memory channel unavailable on this runtime: <reason>`.

`pi-smoke` is wrapped in `describe.skipIf(!piPrereqs.ok)`. `detectPiPrereqs()`
(`src/pi-runner/spawn.ts`) requires `@earendil-works/pi-coding-agent`
(installed as a dev dependency of `packages/pi-plugin`, resolved from that
package's `node_modules` or the root `node_modules/.bun`), `node` on `PATH`
at or above the floor `packages/pi-plugin/package.json` declares in
`engines.node` (Pi's CLI runs under Node and loads the extension into it),
and the built Pi extension. With
`EIDNARA_E2E_REQUIRE_PI=1` an unmet prerequisite fails the file instead of
skipping it; the `native-addon` job sets it because it provides all three, so a skip
there would mean a resolution or layout regression.

The Bedrock-only files also need the pinned OpenCode (`opencode` on `PATH`
resolving to the `opencode.exe` the OpenCode release closure pins), `node`
on `PATH` matching the Pi closure's interpreter pin, and `npm` with registry
access: the first run installs `@earendil-works/pi-coding-agent` at the
pinned version under the system temp directory, and its shrinkwrap
reproduces the release closure's files. A different binary skips both files,
and `EIDNARA_E2E_REQUIRE_PI=1` turns that skip into a failure.

## Commands

```bash
bun run test                      # unit tests for the harness, scripts, and incident pool
bun run test:rust                 # the retained Rust-mode suite (skips when prerequisites are unmet)
bun run test:incidents:rust       # the incident pool over the Rust harness
bun run validate-mode-manifest    # every test file has an entry and every entry a file
bun run validate:incident-history # catalog, adjudications, and source inventory agree
bun run mutation:rust-fm          # apply each FM-OC mutation, run its test, revert
bun run eval:compression-fidelity --baseline <dir> --candidate <dir> --reviews <dir> --out <dir>
                                  # assemble fidelity evidence into a private manifest and report
```

`test` builds `packages/opencode-plugin/dist/index.js` when it is absent.
`test:rust` rebuilds `packages/opencode-plugin/dist/index.js` and
`packages/pi-plugin/dist/index.js` on every run, so edited plugin sources
are never exercised through a stale bundle, and builds
`packages/shm-native/index.js` (the Node entry the extension imports) when
it is absent; run `bun test tests/pi-smoke.test.ts` directly only after
`bun run ensure:pi-dist`.

Environment:

- `EIDNARA_E2E_MODE=rust` is the only accepted mode; `run-test-selection.ts`
  exits 2 for any other value.
- `EIDNARA_E2E_REQUIRE_PI=1` makes `pi-smoke` fail rather than skip when a Pi
  prerequisite is missing.
- `EIDNARA_E2E_DIRECT_HOST_FIXTURE_BIN` and `EIDNARA_E2E_EVAL_RUNNER_BIN`
  override the daemon example binaries (defaults
  `target/debug/examples/direct_host_fixture` and
  `target/debug/examples/eval_runner`, built on demand with the
  `direct-host-fixture` and `eval-runner` features). A set variable must name
  an executable file; a stale path fails the prerequisite check and the build
  step rather than falling back to a build. `scripts/check-rust-prerequisites.ts
  --build` builds whichever examples are not yet resolved.
- `EIDNARA_RUST_E2E_FOLD=1` and `EIDNARA_RUST_E2E_DUPLICATE_IDS=1` enable the
  two tests that drive the daemon past its pressure thresholds.
- `EIDNARA_E2E_PAYLOAD_DIR` names a built payload package, such as
  `packages/host-linux-x64-gnu` after `bun run payload:dev`, and runs
  `tests/payload-fused-search.test.ts`. The suite starts that package's
  `eidnara-host` over the OpenCode harness's data root and points OpenCode and
  the Pi RPC process at it. It installs the fixture admission pair that the
  `search_admission_records` example writes for the payload's embedding bundle
  (`EIDNARA_E2E_SEARCH_ADMISSION_RECORDS_BIN` overrides that binary). Both
  `eidnara_search` tools must then rank a memory created before admission and
  one created after it through the fused route's dense lane. With the variable
  set, a missing payload file, OpenCode, or Pi fails the suite.

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
`packages/opencode-plugin/src/hooks/context/transform-session-client.ts` and of
`crates/daemon/testdata/differential-golden.json`, with the test that must
turn red under each. The `mutation:*` scripts apply a mutation, run its test,
record the outcome, and revert. A drill refuses to record evidence from a
suite that skipped, so the records stay `null` with an `adequacy_finding`
until the runtime can start the shared-memory channel.

## CI

The `native-addon` job installs OpenCode 1.18.22 and Pi 0.80.2 on Node 24.18.0,
builds the daemon examples, exports their paths through the variables above,
and runs `validate-mode-manifest` and `test:rust` with
`EIDNARA_E2E_REQUIRE_PI=1` after its payload smoke, with
`EIDNARA_E2E_PAYLOAD_DIR` naming the payload that smoke built.

## Eidnara on/off evaluation

`bun run eval:ab` runs one seeded project world through Pi and OpenCode with
and without Eidnara and records every turn and model call. `bun run
eval:ab:report <out>` prints accuracy by fact kind, a paired on/off test,
turn latency, model calls and tokens per caller, and memory.

- The world (`src/ab-eval/world.ts`) is a synthetic service repository plus
  coding sessions. Fact turns plant decisions, corrections, constraints,
  incident IDs, cross-session facts, and error codes that exist only in test
  output; probe turns ask about each fact once. Grading is a whole-word token
  match, so no judge runs.
- The Bedrock gateway (`src/ab-eval/gateway.ts`) answers filler and fact turns
  with scripted assistant steps whose tool calls run in the harness, and
  forwards probes, native compaction, and every Eidnara model call to Bedrock
  with the caller's AWS credentials. A scripted turn spends no model time, so
  its wall time is harness and plugin overhead.
- Arms (`src/ab-eval/arms.ts`): `pi-off`, `pi-on`, `pi-onraw`, `oc-off`,
  `oc-on`. The `on` arms strip the temperature from every Eidnara model call
  at the gateway, since Opus 5.5 on Bedrock refuses the parameter; `pi-onraw`
  forwards those calls with the temperature the plugin set. On every arm the
  gateway removes the harness's `thinking` request field from each forwarded
  call. Eidnara arms run the release `direct_host_fixture` with the pinned
  harness closures. An unknown arm name fails the run before any arm starts.
- When passwordless `sudo` can create mount and PID namespaces, each harness
  runs in its own mount and PID namespace. Empty mounts cover `/tmp`, the home directory, and the run
  directory, and the arm's own directory is mounted back; the arm's `/proc`
  lists only its own processes. The harness runs as the invoking user with
  `no_new_privs` set, so `sudo` inside the arm stays unprivileged. The run
  directory must sit outside the repository and the harness install
  directories. `--sandbox off` runs without isolation.
- Each run writes to a fresh directory: `--out` must be missing or empty, and
  the default is a timestamped directory under `<tmp>/ab-eval/runs/` that
  `runs/latest` links to. The command exits nonzero when any arm fails, and
  the other arms' results stay in the directory.
- A turn that reaches its timeout is aborted, and the next turn starts once
  the harness has stopped it. The gateway retries throttled Bedrock calls
  until the turn's deadline.
- `--stall-at <session:turn,...>` stops the daemon for `--stall-ms` before
  those prompts, to exercise a pass that misses its deadline.
- The gateway answers a request whose estimated input exceeds the arms'
  200k window with `prompt is too long`, as a model with that window does;
  `--enforce-window off` forwards it to Bedrock.

```sh
PATH=<dir with opencode 1.18.22>:$PATH bun run eval:ab --tier m --seed 21 \
  --arms pi-off,pi-on,oc-off,oc-on --out /tmp/ab-eval/runs/m1 --pace-ms 2000
bun run eval:ab:report /tmp/ab-eval/runs/m1
```

Tiers: `xs` (2 x 24 turns), `c` (1 x 170), `s` (3 x 110), `m` (12 x 260),
`l` (40 x 600). Run reports stay outside the repository.

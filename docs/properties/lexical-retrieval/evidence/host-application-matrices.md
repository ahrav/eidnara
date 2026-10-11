# host-application-matrices

## Discovery trigger

#391 AC8: independently verify both supported OpenCode and Pi matrices
through the supplied exact, lexical, f32, fusion, packing, and application
path; captured applied context matches independent expectations, with one
lexical contribution per occurrence and no denied payload; registration, RPC
completion, attempted requests, counters, or advertised strings cannot
substitute for real application capability; skipped supported cells block
acceptance. The RP2.3 property bundle is unavailable here, so the record is
reconstructed from the ticket's criterion. PR #967 lists AC8 as external: it
needs host owners' real OpenCode and Pi application capability and the
production f32 oracle.

## Evidence trail

The record's `Existing check` is none. The code below is the path a matrix
would exercise and the host tests that stop short of it.

Daemon side:

- `admit_lexical` (`crates/daemon/src/query_route.rs:1242`) turns each
  `Contribution` into one `LaneHit` for `Lane::Lexical`, so the lexical lane
  offers at most one hit per occurrence to fusion. `lexical_incomplete`
  (`:1586`) and `lexical_bound_reason` (`:1599`) map `ScanBound`,
  `CommonTerms`, `RankBudget`, and `AcceptedBound` to the wire lane status.
- `crates/daemon/tests/query_route_handler.rs:247`
  `the_running_daemon_serves_the_route_from_its_converged_family` asserts
  `lanes.lexical.status == "complete"` (`:259`) on a fused answer whose
  `entries` array is empty, so no lexical entry crosses the wire in that test.

Host wire and application:

- `ROUTE_LANES` (`packages/opencode-plugin/src/shared/kernel-client/wire.ts:484`)
  names `exact`, `lexical`, `dense`. `RouteEntry` (`:495`) carries
  `occurrence_id`, `position`, the contributing `lanes` (`:500`), and a
  `canonical` reference. The fused payload carries one `LaneStatus` per lane
  (`:509`, `:515`).
- `wire.test.ts:426` parses a lexical status of `incomplete` with reason
  `common_terms` and `also: ["rank_budget"]`; it exercises status parsing,
  not an entry with a lexical lane.
- `searchThroughRoute`
  (`packages/opencode-plugin/src/tools/eidnara-search/route-search.ts:74`)
  is the application step both hosts share: it sends `retrieval.query`,
  folds entries by `canonical.decision_object_id` while collecting their
  lanes (`:101`), and reads the selected decisions through
  `readObjectRowsChunked` on the `explicit_search` surface (`:131`). Payload
  text therefore loads after the route answers, through the daemon's read
  surface, never from the lexical lane. `lanesOf`
  (`tools/eidnara-search/render.ts:29`) renders the collected lanes as
  `lanes=...`.
- Pi's tool (`packages/pi-plugin/src/tools/eidnara-search.ts:3`) imports
  `executeEidnaraSearch` from the OpenCode package, which calls
  `searchThroughRoute` (`execute.ts:183`). One application path serves both
  hosts.
- `packages/pi-plugin/src/tools/eidnara-search.test.ts:113` asserts
  `match=fused lanes=dense` from a scripted route reply; the reply names the
  lexical lane only as `{ status: "complete" }` (`:94`).

End-to-end host matrix that exists:

- `packages/e2e-tests/tests/payload-fused-search.test.ts:98`
  `fused search through a built payload` starts a built payload host
  (`PayloadHost`, `src/rust-runner/payload-host.ts:47`) under the OpenCode
  harness (`RustTestHarness`, `src/rust-harness.ts:229`) and a Pi harness,
  creates one memory before and one after admission in each host, and
  asserts each ranks through `expectDenseOnly` (`:47`), whose header regex
  requires `lanes=dense` (`:51`). The query and memory word sets are
  disjoint by construction (`:21`), so the lexical lane contributes nothing.
- The admission records come from
  `crates/daemon/examples/search_admission_records.rs`, whose module doc
  (`:1`, `:7`) calls them test-only fixture evidence with no qualification
  measurement.
- CI runs the suite with `EIDNARA_E2E_PAYLOAD_DIR` set
  (`.github/workflows/ci.yml:1029`); `mode-manifest.json:105` registers it
  in the `pi-rust` tier with the dense-lane rationale.

## Failure scenario

Lexical retrieval ships with daemon-level evidence only. A host that drops
the lexical contribution during folding, renders two rows for one
occurrence, or hydrates a decision the route did not authorize would pass
every current test, because no test sends a query whose words match a
memory through the lexical lane in either host. The catalog's Impact line:
lexical retrieval ships without host evidence.

## Timing windows and dependencies

A dropped application: the host receives a fused entry with `lexical` in its
lanes and the rendered or captured context omits it. Dependencies: a real
host application capability for OpenCode and for Pi, the production f32
oracle for the dense lane of the same matrix, and the RP2.9 corpus and
protocol. The existing payload suite depends on a built payload package,
installed OpenCode, and installed Pi, and skips when the payload directory
is unset.

## What a test must construct

- A payload host with admission installed, as `payload-fused-search` does.
- A memory whose text shares at least one word with the query, so the
  lexical lane ranks it; a control with disjoint words, so the dense lane
  alone ranks it, as the existing `BEFORE` and `AFTER` pairs do.
- One cell per supported host: OpenCode through `RustTestHarness` and Pi
  through `PiTestHarness`, each asserting a header with `lexical` among its
  lanes and exactly one row per occurrence.
- A denied decision in the same projection, asserting its payload is absent
  from the captured applied context.
- A dropped-application control that fails independently.
- The production f32 oracle and the RP2.9 corpus, which the repository does
  not hold.

## Investigation log

### Q: Who supplies the real OpenCode and Pi application capability and matrices?

- Sources examined: #391 "Host owners supply real supported OpenCode/Pi
  capabilities and matrices"; the PR #967 audit of AC8; the catalog record.
- Findings: the repository holds the shared application path and a dense-only
  host matrix, and no lexical host cell. The ticket assigns the capability
  and matrices to host owners and bars simulated capability.
- Missing evidence: the owners' capability, the production f32 oracle, and
  the RP2.9 corpus.
- Conclusion: needs human input.

### Q: Does any existing host test reach a lexical contribution?

- Sources examined: `payload-fused-search.test.ts:21`, `:47`, `:51`;
  `eidnara-search.test.ts:94`, `:113`; `wire.test.ts:426`;
  `query_route_handler.rs:259`.
- Findings: the payload suite asserts `lanes=dense` exclusively and builds
  disjoint word sets so lexical cannot rank; the Pi unit test scripts a
  dense-only entry; the wire test parses lexical status only; the daemon
  handler test returns an empty entry list.
- Missing evidence: a host test whose query shares a word with a memory.
- Conclusion: resolved with answer - no; every lexical check stops at the
  daemon or at status parsing, which is why the record is `not yet`.

### Q: Is "no denied payload" structurally enforced on the host path?

- Sources examined: `route-search.ts:101` through `:139`;
  `retrieval-reads-no-payload` for the daemon side.
- Findings: the host reads payloads only for decisions the route named, via
  `readObjectRowsChunked` on `explicit_search`, after the ranking returns.
  Whether that read refuses a denied decision is the read surface's
  contract, which this record does not examine.
- Missing evidence: a host test with a denied decision in the ranked set.
- Conclusion: unresolved, needs the host matrix cell with a denied decision.

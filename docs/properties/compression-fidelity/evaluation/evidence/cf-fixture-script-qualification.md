# cf-fixture-script-qualification

## Discovery trigger

U3 in the [settled contract](https://github.com/ahrav/eidnara/issues/707),
"Milestone boundaries and dependencies" (historical plan lines 217-227),
requires initial ungated qualification inside U3, after U1/U2,
before downstream fixture-backed claims. Case-ID selection, alias binding,
bounded script consumption, and explicit mismatch/exhaustion failure are proposed
obligations.

Inspected HEAD: `12873a25f814b8ee1aca47033e2ef6f1e6f10d3d`, 2026-10-08
(first inspected at `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`, 2026-09-19,
before the case script existed).
[Catalog provenance](../catalog.md#scope-and-evidence-boundary) records plan
revision drift. Reachability is test-only: the control socket belongs to the
feature-gated daemon example, not the production host wire. Exercise: partial;
the case script and one ungated qualification run exist, and an identity-bound
receipt consumed by later delivery rows does not.

## Evidence trail

- [Fixture target](../../../../../crates/daemon/Cargo.toml#L12-L15) requires
  `direct-host-fixture`. The [control enum](../../../../../crates/daemon/examples/direct_host_fixture.rs#L660-L685)
  accepts success, block, release, failure, outage, counters, shutdown, and the
  three script commands `script-cases`, `script-status`, and `script-source`.
  [Dispatch](../../../../../crates/daemon/examples/direct_host_fixture.rs#L853-L865)
  routes them to `CaseScript::select`, `CaseScript::status`, and
  `source_records`, returning each `ScriptError` code as a typed rejection.
- [Case script](../../../../../crates/daemon/examples/direct_host_fixture/case_script.rs#L33)
  bounds a selection at `MAX_QUEUE = 8`.
  [`select`](../../../../../crates/daemon/examples/direct_host_fixture/case_script.rs#L673-L703)
  rejects an empty or over-bound queue and an unknown scenario before any state
  changes, then arms a new generation.
  [`answer`](../../../../../crates/daemon/examples/direct_host_fixture/case_script.rs#L720-L755)
  consumes the front entry per admitted summarizer request, binds the scenario
  to the presented records, and returns `Exhausted` or `Mismatch` as a failure
  while counting it.
  [`delivered`](../../../../../crates/daemon/examples/direct_host_fixture/case_script.rs#L759-L770)
  records a binding only after the sink accepts the emitted text.
- [Backend execution](../../../../../crates/daemon/examples/direct_host_fixture.rs#L405-L435)
  parses the presented records from the request prompt and asks the armed
  script for an answer on a success or block behavior.
  [`summary_text` and `emit_summary`](../../../../../crates/daemon/examples/direct_host_fixture.rs#L233-L269)
  turn a refused answer into a typed failure and hand the receipt to
  `delivered` only when the sink status is `Accepted`; `fixture-success` is the
  fallback for a prompt with no presented records.
- [Control tests](../../../../../packages/e2e-tests/src/rust-runner/hermetic-host.test.ts#L121-L365)
  cover readiness, reply validation, permissions, redaction, counters, and
  cleanup. The [script test](../../../../../packages/e2e-tests/src/rust-runner/hermetic-host.test.ts#L367-L555)
  constructs a valid alias binding, unknown IDs, an over-bound and an oversized
  selection, a mismatched request, and queue exhaustion, and asserts the typed
  failures. Status: `unaudited`.
- [Qualification test](../../../../../packages/e2e-tests/tests/compression-fidelity-qualification.test.ts#L53-L186)
  arms `[C1.S1, filler, filler, filler]`, waits for the binding in
  `script-status`, waits for accepted nonempty publication, and judges the
  later correlated provider capture with `judgeDelivery`, emitting the
  `cf-fixture-script-qualification` marker only on a served verdict. Status:
  `unaudited`.
- [Producer route test](../../../../../packages/e2e-tests/tests/rust-history_summarizer-producer.test.ts#L82-L90)
  asserts started/completed counters. It never asserts a published history body.
- [Fold test](../../../../../packages/e2e-tests/tests/rust-fold-under-pressure.test.ts#L15-L42)
  gates the actual fold scenario and otherwise checks that the flag is false.
  [The flag](../../../../../packages/e2e-tests/src/rust-scenario-support.ts#L12-L18)
  is `EIDNARA_E2E_FOLD=1`; the live scenario checks nonempty history at lines 82-86.
- [Current CI](../../../../../.github/workflows/ci.yml#L977-L1005) builds the
  fixture, sets `EIDNARA_E2E_REQUIRE_FIDELITY=1`, and runs `test:rust`, which
  includes the qualification test through `mode-manifest.json`. The fold flag
  stays unset there. No specific CI run is claimed here.

## Failure scenario

A test selects a case but an unrelated producer request consumes its output,
or an exhausted script returns `fixture-success`. Backend completion increases,
yet no intended summary is accepted. A later capture may contain raw history or
unrelated content while the report marks U3 complete.

The competing explanation is that the current backend route test already proves
publication. Its assertions and the request-ignoring backend distinguish route
completion from the required publication claim. No executable compression script
or observed fidelity failure is asserted by this record.

## Timing windows and dependencies

Selection precedes backend execution; validation and publication follow it;
the provider capture observes a later boundary. Request and script correlation
must survive these steps. Control readiness and counter changes cannot collapse
them into one success state. Stale prebuilt binaries also require the corpus
identity check from `cf-corpus-byte-identity`.

Missing prerequisites, gated execution, invalid XML, or discarded coverage leave
qualification incomplete. U4 can evaluate reachable rows, but missing required
fold scenarios still block full acceptance. This record does not change the
production protocol or prescribe a new producer API.

This record owns initial qualification, not every later invocation. The
[delivery-credit property](../../delivery/catalog.md#cf-delivery-credit-requires-published-folded-capture),
[recovery-credit property](../../recovery/catalog.md#cf-unavailable-evidence-no-credit),
and [real-batch property](cf-reviewed-semantic-batch-reached.md) consume it but
still require their own captured observations. Qualification does not reorder
U1-U5 or turn later pass-through/missing captures into successful compression.

## What a test must construct

1. After U1/U2, qualify valid alias-bound XML through accepted nonempty
   publication and correlated provider capture in the ungated U3 lane.
2. Then exercise bounded case-ID selection through the separate control socket,
   matching actual request aliases and observing every script consumption.
3. Supply unknown IDs, alias mismatch, extra requests after exhaustion, and a
   queue over its declared bound. Require explicit failure, not sentinel success.
4. Construct missing prerequisites without marking qualification successful.
   Record `cf-eval-publication-observed` only from the positive control's state.

## Investigation log

### Q: Can existing ungated fixture execution publish a valid script?
- Sources examined: Backend execution, control enum, case script, the script
  test, the qualification test, and the CI lane at the anchors above.
- Findings: At the first inspected HEAD, routing and counters existed and
  case-ID scripting did not. At the current HEAD the case script binds a
  queued scenario to the presented records, the fixture emits the scripted
  text, and the qualification test observes accepted nonempty publication and
  a correlated provider capture without the fold flag.
- Missing evidence: An identity-bound receipt that later delivery rows
  consume; each row re-judges its own capture.
- Conclusion: Resolved for initial qualification by execution of the
  qualification test. The receipt handoff to delivery rows stays open.

### Q: What script queue bound is required?
- Sources examined: Plan line 219 and `MAX_QUEUE` at the anchor above.
- Findings: The plan requires a bounded queue but specifies no queue length.
  The fixture declares `MAX_QUEUE = 8` and `select` rejects a longer queue.
- Missing evidence: A reviewer's confirmation that 8 is the intended U3 bound.
- Conclusion: Needs human input on the number. The fixture's 8 is the bound
  the checks exercise.

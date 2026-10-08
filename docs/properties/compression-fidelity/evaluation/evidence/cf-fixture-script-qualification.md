# cf-fixture-script-qualification

## Discovery trigger

U3 in the [settled contract](https://github.com/ahrav/eidnara/issues/707),
"Milestone boundaries and dependencies" (historical plan lines 217-227),
requires initial ungated qualification inside U3, after U1/U2,
before downstream fixture-backed claims. Case-ID selection, alias binding,
bounded script consumption, and explicit mismatch/exhaustion failure are proposed
obligations.

Inspected HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`, 2026-09-19.
[Catalog provenance](../catalog.md#scope-and-evidence-boundary) records plan
revision drift. Reachability is test-only: the control socket belongs to the
feature-gated daemon example, not the production host wire. Exercise: not yet.

## Evidence trail

- [Fixture target](../../../../../crates/daemon/Cargo.toml#L12-L15) requires
  `direct-host-fixture`. The [control enum](../../../../../crates/daemon/examples/direct_host_fixture.rs#L604-L634)
  accepts success, block, release, failure, counters, and shutdown, not case IDs.
- [Backend execution](../../../../../crates/daemon/examples/direct_host_fixture.rs#L383-L499)
  ignores `_request`, resets the next behavior to success, emits
  `fixture-success`, and increments completion. None of these actions binds
  aliases or demonstrates accepted nonempty history.
- [Counter snapshots](../../../../../crates/daemon/examples/direct_host_fixture.rs#L99-L110)
  load fields individually. [Blocked release](../../../../../crates/daemon/examples/direct_host_fixture.rs#L155-L185)
  waits for a per-call resumption acknowledgment, which is not a summary receipt.
- [Framing and dispatch](../../../../../crates/daemon/examples/direct_host_fixture.rs#L673-L832)
  bound frames and reject malformed/unknown commands with static diagnostics.
  They do not bound a corpus-script queue because that queue is absent.
- [Control tests](../../../../../packages/e2e-tests/src/rust-runner/hermetic-host.test.ts#L105-L223)
  cover readiness and reply validation. The prerequisite-gated test at lines
  196-320 covers permissions, redaction, counters, and cleanup. Status: `unaudited`.
- [Producer route test](../../../../../packages/e2e-tests/tests/rust-history_summarizer-producer.test.ts#L82-L90)
  asserts started/completed counters. It never asserts a published history body.
- [Fold test](../../../../../packages/e2e-tests/tests/rust-fold-under-pressure.test.ts#L15-L42)
  gates the actual fold scenario and otherwise checks that the flag is false.
  [The flag](../../../../../packages/e2e-tests/src/rust-scenario-support.ts#L12-L18)
  is `EIDNARA_E2E_FOLD=1`; the live scenario checks nonempty history at lines 82-86.
- [Current CI](../../../../../.github/workflows/ci.yml#L977-L1004) builds the
  fixture and invokes E2E selection. It does not enable that fold flag or prove
  the planned U3 qualification. No CI run is claimed here.

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
- Sources examined: Backend execution, current control enum, producer route
  assertions, and gated fold test at the anchors above.
- Findings: Routing and counters exist; case-ID scripting and qualification do
  not. The backend's literal success output is not the planned reviewed XML.
- Missing evidence: A valid alias-bound script through real validation and
  nonempty publication, followed by final provider capture without the fold flag.
- Conclusion: Unresolved, needs implementation and execution. Broader runtime
  work may be a dependency; no success is inferred from source inspection.

### Q: What script queue bound is required?
- Sources examined: Plan line 219 and fixture frame cap at lines 38-40.
- Findings: The plan requires a bounded queue but specifies no queue length.
  The 64 KiB frame cap is not a queue-capacity answer.
- Missing evidence: The declared U3 script bound for the reviewed cases.
- Conclusion: Needs human input. This catalog adds no numeric bound.

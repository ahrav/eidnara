# Recovery portfolio evaluation

This file records the independent review agent's findings and their final
disposition. It uses the final evidence, including the checked C6 refutation,
not an earlier unresolved allegation. Source revision:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`, inspected 2026-09-19.
See the [catalog index](../README.md#scope-and-provenance) for supplied scope,
older-catalog limitations, and the explicit `_lenses` placement deviation.

## Portfolio disposition

The [catalog](catalog.md) contains five active records, three unexercised and
two partial after U2, and five
matching evidence files. Four are `always` safety records; one is a `sometimes`
reachability record. Four surfaces are `test-only`; registered memory recovery
is `default-production`. Test-only native/replay predicates do not make the
underlying storage APIs test-only. All [existing checks](existing-checks.md)
remain unaudited.

The original-revision C6 construction is supported by inspected code. Its
combined execution is still missing. This is neither a passing C6 result nor
evidence that the consumer can retrieve native bytes.

The native-reopen evidence file has 155 lines, exceeding the method's suggested
60–120 lines to retain the verified descriptor/evidence distinction, historical
selector, reopen ordering, and investigation log. No evidence is removed to
meet the suggested length.

## Independent findings and dispositions

### Harness fit

- **Refinement applied: use existing witnesses and real tool routing.**
  [cf-native-reopen-bytes](evidence/cf-native-reopen-bytes.md) extends the daemon
  native-source witness.
  [cf-visible-memory-recovery](evidence/cf-visible-memory-recovery.md) uses the
  registered OpenCode tool through `executeEidnaraSearch` to `kernel.read`, with
  visible-derived arguments and delivered supporting text within fixed limits.
  The same-named daemon history/note facade is not that tool. Internal reads,
  private oracle IDs, and pre-pack hits cannot establish consumer recovery.
- **Refinement applied: share qualification, not credit.** Recovery consumes
  evaluation's [initial fixture qualification](../evaluation/evidence/cf-fixture-script-qualification.md)
  and delivery's actual invocation observations. Initial qualification remains
  inside U3 after U1/U2. It cannot substitute for a particular recovery call and
  its result reaching the consumer.

### Coverage balance

- **Refinement applied: one canonical disposition owner.**
  [cf-unavailable-evidence-no-credit](evidence/cf-unavailable-evidence-no-credit.md)
  owns exactly one visible/discoverable/unavailable disposition per material
  obligation, follow-up, and scenario. Delivery consumes it rather than creating
  another oracle. Unavailable evidence receives no useful-preservation or
  recovery credit. Explicitly permitted abstention can satisfy consumer safety,
  but cannot clear loss of the only relevant constraint or turn P4 omission
  into preservation.
- **Refinement applied: distinguish admission from recoverability.** The
  [memory-recovery witness](evidence/cf-visible-memory-recovery.md) shares the
  source/admission/invocation fixture with delivery's memory record. Delivery
  checks admitted visible content; recovery adds visible argument provenance,
  registered execution, and result delivery. These are distinct assertions over
  shared evidence, not duplicate fixtures.
- **Refinement applied: positive bytes and independent substitutions.**
  [cf-exact-source-binding](evidence/cf-exact-source-binding.md) checks occurrence,
  revision, representation, span, and bytes. Equal text from another occurrence,
  equal-length revisions, normalized transcripts, and fuller summaries remain
  negative controls. A positive read requires present text; `None` is not
  `Some("")`. Report every [declared situation](fault-map.md#coverage-checks-to-add)
  separately, including the registered-call and Pi location checks.

### Implementability and the C6 refutation

**Suggestion rejected: replace the original with its successor because the
original cannot be read.** The final
[native evidence](evidence/cf-native-reopen-bytes.md#investigation-log) refutes
that premise. Descriptor succession invalidates the observation descriptor,
not its distinct `evidence_meta`. The existing historical selector accepts the
saved original commit sequence; the guarded artifact reader checks retained
evidence metadata, digest, and purge state.

The supported construction retains the original native binding and descriptor
commit sequence, publishes a same-length message successor, closes and reopens
storage, selects the original through `live_source_descriptors` at that saved
sequence, and reads its retained evidence handle through `read_artifact`.
Reopen advances the epoch, so no old source hold or cursor crosses it. Current
live inventory, a successor, a distinct occurrence, or a direct CAS-file read
cannot replace this witness. Tool `result_revision` is itself an identity
field; that control is distinct from same-lineage message succession.

Purge, retirement, corruption, and refusal can still prevent a read. They do
not justify changing the positive oracle or bypassing guards. The construction
is code-supported but unexecuted; the remaining gap is execution, not a missing
design permission or a need for a source-access API.

### Wildcard and bias

- **Bias surfaced: privileged bytes are not agent access.**
  [cf-shipped-recovery-capabilities](evidence/cf-shipped-recovery-capabilities.md)
  keeps consumer exact expansion unsupported. Pi history folding shipped after
  the inspected revision; the record's open question asks how reports treat it.
  Memory search can establish a decision or rationale, not exact transcript
  recovery. A changed registry/schema/Pi gate invalidates the pinned expectation
  for review; the catalog does not require that future capabilities stay absent.
- **Bias surfaced: bounded witnesses have limited claims.** Clean reopen is
  neither process-crash nor power-loss evidence. A fired situation does not
  prove semantic success or unbounded liveness. Six synthetic cases do not
  establish reliability, and humans still judge whether delivered memory
  supports the independent obligation.

## Open prerequisites and test-strategy handoff

Route all five records and the [fault map](fault-map.md) to
`/testing:test-strategy`:

| Records | Reusable seam and required evidence |
| --- | --- |
| Native reopen and exact binding | U2 daemon native-source witness, historical selection, guarded reader, complete annotations, and positive/substitution/refusal controls. |
| Unavailable evidence | One disposition predicate consuming U3 complete-invocation and tool observations, with preservation and consumer safety kept separate. |
| Visible memory recovery | U3/U4 registered OpenCode loop using the shared admission fixture, visible cues, fixed call/output limits, and captured delivered results. |
| Shipped capabilities | Existing registry/schema, search-route, and Pi entrypoint checks bound to the evaluated revision and configuration. |

The combined original-revision C6 run, ungated fold/tool capture, fixed limits,
and human-approved obligations/abstention cases remain open. No new retention
policy, exact-expansion API, admission bypass, second memory store, or Pi
transform follows. Existing-test adequacy belongs to
`/testing:invariant-test-review`; guard strength belongs to
`/low-level-systems:defensive-assertions-and-invariant-guards`.

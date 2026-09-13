# Decode fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`.
HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
The [catalog scope](catalog.md#scope-and-provenance) records accepted plan and
external leads. No tests or benchmarks run. Availability below means that an
existing seam can construct input, not that the proposed assertion exists.
The completed independent pass by `ses_f6756093fffeVjNp36S3E8pKrM`, supplied
and dispositioned on 2026-09-13, leaves every claim unexercised.

## Fault classes and availability

| Class | Construction and existing seam | Availability |
| --- | --- | --- |
| Recognized duplicate field | Literal bytes at daemon lib.rs:19762-19964 and dispatch_body; repeat CK role or block kind with a valid final value. | Existing local seam; new nested witnesses missing. |
| Retained-map or unknown-field duplicate | Literal tool-input map or ignored field; compare separately from recognized struct duplicates. | Existing parser seam; explicit controls proposed. |
| Null/default/type/variant | Missing, null, false, wrong scalar, unknown enum tag, positional/wrong envelope shape. | Existing corpus partly covers; explicit profile null preserves the existing wrapper boundary and does not authorize its deferred refactor. |
| Malformed ignored subtree | Invalid UTF-8, lone surrogate, depth/range excess, trailing JSON, raw-value-token position. | Compatibility walk and corpus exist; discarded-CK token case missing. |
| Extension placement | Same sentinel at disposable envelope and inside a kept Value. | Existing public wire types; full placement matrix missing. |
| Raw-page alteration | Change unknown CK field with stale digest, then recompute valid digest. | Page handler and generated-corpus fixture exist; targeted vector missing. |
| Page boundary form | Any null/partial page envelope, nonarray field, continuation marker/chunk, scalar-only edit. | Probe, assembler, page handler, and TS generator exist. |
| Non-transform route | Echo/status/unknown/facade body beside transform discriminator. | Existing dispatch and body-entry seams. |
| Ownership transition | Drop byte Vec, request, projection, or cache owner while another Arc remains. | Existing sharing/cache seams; combined decoded-buffer-drop witness missing. |
| Copy-on-write/public mutation | Hold another owner, mutate one field or call accessor without edit. | Public APIs and Arc::make_mut exist; complete prospective matrix missing. |
| Envelope cache regression | Inspect final types and serde graph for hidden envelope materialization. | Source inspection available; final replacement absent. |

Pool starvation, charge thresholds, and measured allocation payoff stay with
A1/A3 and the accounting agent. No crash, network partition, election, or
wall-clock timeout is required to construct this slice's vulnerable states.

## Per-property mapping

| Property | Required enabling state | Exact observation | Missing evidence |
| --- | --- | --- | --- |
| [envelope-decode-has-no-retained-tree](evidence/envelope-decode-has-no-retained-tree.md) | Final wire types and both decode entries, mixed payload variants. | Derived serde with no full envelope Value/cache or replay serializer. | Replacement source and structural witness. |
| [typed-failure-preserves-tree-outcome](evidence/typed-failure-preserves-tree-outcome.md) | Literal corpus with nonbinding admission; baseline and final artifacts. | Route, accept/reject, code, known fields, and payload Values match authorized expectations. | Nested corpus and raw-token baseline/final runs; any raw-token acceptance change requires stop-and-report, not an exception. |
| [unknown-envelope-fields-do-not-erase-payload-values](evidence/unknown-envelope-fields-do-not-erase-payload-values.md) | Kept/discarded placement pairs and explicit Option-null controls. | Unknown typed-envelope keys absent; each kept field equals independent typed expectation. | Full field-placement matrix; integration owner must reconcile TE08 while this spec preserves R3. |
| [page-and-other-routes-retain-tree-semantics](evidence/page-and-other-routes-retain-tree-semantics.md) | Bound route, valid page set, stale/updated raw digest, non-transform controls. | Hash raw arrays before normalization; preserve assembly/dispatch results. | Unknown-CK digest vector and final generated-corpus run. |
| [decoded-snapshots-own-and-share-prefixes](evidence/decoded-snapshots-own-and-share-prefixes.md) | Nonempty cached prefix, suffix, separate owners, both cache-source choices. | Mobility constraints, intact values after owner drops, and correct pointer sharing. | Final byte-buffer-drop/cache-eviction sequence. |
| [typed-mutation-is-visible-and-copy-isolated](evidence/typed-mutation-is-visible-and-copy-isolated.md) | Decoded/constructed twins, two siblings, populated public fields, multiple owners. | Typed expected serialization/equality; no stale replay or peer mutation. | Full public-field/no-op-access matrix. |
| [nested-duplicate-fallback-is-exercised](evidence/nested-duplicate-fallback-is-exercised.md) | Recognized CK/block duplicates with valid final values and literal unpaged route. | Gate accepts, typed decode is Invalid, tree conversion succeeds, handler reports Tree. | Both fixed markers on final derived structs. |

## Coverage checks to add

The active reachability record requires these constant, globally scoped names:

- `typed-wire-decode-message-duplicate-fallback`
- `typed-wire-decode-block-duplicate-fallback`

Each marker combines the independent conditions in the mapping above. Merely
observing Tree is insufficient because page fields, escaped discriminators,
and gate rejection also select it. Neither marker asserts a violated safety
condition. Both can fire on a correct replacement.

Check `sometimes` independently for each marker. Keep separate campaign
witnessed bits and require `message_seen && block_seen` at completion. Report
each marker's result even in an aggregate summary. Neither a nonzero total
hit count nor `message_seen || block_seen` satisfies the record; one marker
cannot mask an unfired other marker.

Additional proposed situation counters can distinguish payload placement,
raw-page validation before normalization, ready-snapshot fallback after cache
eviction, and a copy-on-write edit while another reader survives. Test strategy
must decide whether exhaustive named cases already establish those conditions
before adding instrumentation. No dynamic marker names are proposed.

An unfired required marker has two possible explanations: the generator did
not construct its preconditions, or the state is no longer reachable. Verify
literal bytes and gate observations first. This finite-campaign failure is not
proof against an unbounded liveness claim.

## Cross-work and compatibility disposition

The raw-value-token case remains a source-backed parity hypothesis, not a
confirmed defect. A discriminating baseline/final run is required. Any observed
acceptance change stops implementation for a report; no compatibility
exception is authorized by the review or the user amendment.

Worktree-only TE08 requires exact captured ingress including unknown CK
fields. Accepted R3 remains authoritative for this specification. Its owner
records the integration owner's reconciliation before integration; this map
does not revise the other agent's catalog or reinterpret R3 as lossless replay.

## Leverage ranking by cheapest valid oracle

1. Inspect wire definitions and old replay assertions. This immediately
   separates the prospective contract from implemented HEAD behavior.
2. Extend the existing literal corpus and module-local body-entry comparison.
   It observes lane and decode failures without adding a public wire field.
3. Build independent typed payload/mutation expectations. Public constructors
   and serde entrypoints already expose the required state.
4. Extend Arc-prefix tests with byte-buffer drop and ready-snapshot fallback.
   Values and pointer identity are separate observations.
5. Add raw-page digest vectors, then use the existing generated TypeScript and
   real-host test for the cross-language boundary. Its ignored status and
   generated-input prerequisite must remain explicit.

These are seam recommendations, not settled test-form choices or reported
execution. `/testing:test-strategy` owns those choices. All existing checks
remain unaudited until their named reviewers inspect the oracles.

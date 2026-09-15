# response-retention-isolation

## Discovery trigger

A retained response A never pins B's block; retained binary responses consume a separate per-connection quota until native release; stream items are private copies. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `packages/opencode-plugin/src/shared/host-client/connection.ts:1057`
- `packages/opencode-plugin/src/shared/host-client/connection.ts:1088`

Witness status: not yet - retained binary unary quotas and stream private copies belong to #550 (native/TypeScript) and #552 (Rust client); the transport side is proved by `released-block-reuse-preserves-held-bytes`.

## Failure scenario

Retention that pinned unrelated storage would reintroduce the FIFO coupling at the client layer.

## Timing windows and dependencies

Retention across close and reconnect.

## What a test must construct

A retained after its connection closes while B cycles.

Situation markers that must fire independently of the safety check:

- `client.retained_response_across_close`

Check semantics: `always` - holding A leaves `descriptors_outstanding` and B's class free count unaffected by A; retained-quota refusals recover independently.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: not yet at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: #550 and #552.
- Conclusion: unresolved, needs the named handoff.

# response-retention-isolation

## Discovery trigger

A retained response A never pins B's block; retained binary responses consume a separate per-connection quota until native release; stream items are private copies. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/host-runtime/src/ring_transport.rs:1611`
- `crates/host-runtime/src/client.rs:477`
- `packages/opencode-plugin/src/shared/host-client/connection.ts:1136`
- `packages/opencode-plugin/src/shared/host-client/connection.ts:1194`

Witness status: yes - Rust client: `RingClientEndpoint::try_recv_with` (`crates/host-runtime/src/ring_transport.rs:1611`) copies each body to private bytes and releases the lease before the frame leaves the bridge, and `crates/host-runtime/tests/client.rs:498` holds response A and every B response, more than the 4 KiB class has blocks, and keeps A past close; the stream budget's refusal is covered by `exhausted_retention_cancels_only_the_saturating_stream` in crates/host-runtime/src/client.rs and the unpolled-unary budget, its refusal of a second maximum response, and its charge release by `crates/host-runtime/src/client.rs:7356`. TypeScript client: `retainBinary` (`packages/opencode-plugin/src/shared/host-client/connection.ts:1136`) charges a caller-held binary unary lease to separate per-connection byte and count quotas from delivery until the caller releases it, including after the connection closes, and `binary unary leases are charged to their own byte and count quotas until the caller releases them` in packages/opencode-plugin/src/shared/host-client/connection.test.ts refuses on each quota independently with the lease released unread, refunds exactly once, and keeps the aggregate budget untouched; stream items stay private copies (`ownedStreamCopy`). The transport side is proved by `released-block-reuse-preserves-held-bytes`.

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
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.

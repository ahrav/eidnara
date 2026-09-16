# route-query-embedding-completes-before-blocking-scan

## Discovery trigger

The RP2.7 specification requires query embedding in async context before the
bounded blocking scan, under the one budget derived at entry, and the U3c
ticket asks for a witness that no synchronous embedding runs on the blocking
scan thread (parent Q6).

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u3b-query-route` at
`41fb5258`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/query_route.rs` `handle_retrieval_query`: when
  `limits.dense` is set, the embedder runs through `UnitRunner::run_step`, the
  handler awaits it, checks `SharedBudget::is_exhausted`, and only then calls
  `run_unit` for the scan; the scan closure receives `Embedded`.
- `QueryEmbedder for LocalEmbeddingsComponent` embeds in process with
  `embed_blocking`; `HandlerCore::query_embedder` returns the lifecycle
  owner's lane, or a test override.
- `crates/daemon/tests/query_route_handler.rs`
  `the_query_is_embedded_by_the_lane_before_the_scan_and_the_lane_degrades_typed`;
  `crates/daemon/tests/query_route_dense.rs`
  `dense_positions_follow_the_producer_and_the_fused_order_follows_the_oracle`.

## Failure scenario

The scan closure embeds while holding the projection connection, or the scan
is submitted after the budget lapsed during embedding.

## Timing windows and dependencies

The deadline lapses during the embedding step.

## What a test must construct

- A converged family so the handler reaches the embedding step.
- A scripted embedder that sleeps past the remaining duration.
- Stored unit vectors under the fixture generation for the position oracle.

## Investigation log

### Q: Why embed through the lane in process rather than the host embedding route?

- Sources examined: `LocalEmbeddingsComponent` on the lifecycle owner; the
  host embedding route's job protocol.
- Findings: the family's identity is derived from the same lane, so an
  in-process vector is in the stored rows' space; routing would add a
  cross-process deadline conversion the specification asks to avoid when not
  needed.
- Missing evidence: none.
- Conclusion: resolved with answer - in process; recorded as the Q6 decision.

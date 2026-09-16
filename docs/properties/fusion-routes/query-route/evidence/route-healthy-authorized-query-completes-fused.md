# route-healthy-authorized-query-completes-fused

## Discovery trigger

The RP2.7 U3b acceptance criteria require a nonempty fused ranking whose
order equals the U2 oracle over the lanes' rankings, so an always-refusing
implementation fails.

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u2-weighted-rrf` at
`6dea07f455d536eec50556116c882c8dc6a00d98`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/tests/query_route.rs` `Fixture::oracle` builds the exact
  and lexical rankings directly from `exact::page` and `lexical::retrieve`,
  sums `weight / (k + position)` by hand with unequal weights and `k = 7`,
  and orders by score then identifier bytes; it never calls `fuse`.
  `a_healthy_query_completes_fused_in_the_oracles_order` compares the route's
  entry order to it.
- `crates/daemon/tests/query_route_handler.rs`
  `the_running_daemon_serves_the_route_from_its_converged_family` drives
  the handler against a family the daemon converged.

## Failure scenario

The route refuses or returns an empty answer for a healthy query.

## Timing windows and dependencies

None.

## What a test must construct

- A kernel with materialized claim descriptors and a projection built from
  its snapshot.
- For the handler path, lifecycle records so the daemon converges a family.

## Investigation log

### Q: Why is the handler-level answer empty?

- Sources examined: the lifecycle owner's freshness check in
  `crates/daemon/src/search_lifecycle_owner.rs`; runs of the handler test
  that committed decisions after convergence.
- Findings: decisions committed after convergence left the family past the
  freshness limit in this harness, and the pin was refused as
  `lane_unavailable`; committing before convergence left an embedding job
  pending and the family never reached Current within the test bound.
- Missing evidence: a harness that catches the family up.
- Conclusion: unresolved - the nonempty comparison is made at the `execute`
  level; needs human input on the catch-up harness.

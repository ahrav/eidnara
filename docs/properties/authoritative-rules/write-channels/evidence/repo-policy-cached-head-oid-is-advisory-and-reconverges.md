# repo-policy-cached-head-oid-is-advisory-and-reconverges

## Discovery trigger

KTD2 trigger: "the m0 compose path calls `reconcile` before the canonical-
memory read and returns immediately when the cached HEAD oid for the binding
is unchanged"; U5 step 1. The brief asks for the derived state (cached HEAD
oid per binding), its staleness contract, concurrent passes for the same
project, and the forbidden outcome "a stale rule from an older HEAD still live
after reconciliation" (F6). Skill step 7 (govern derived state).

## Evidence trail

Verified at HEAD:

- `crates/kernel/src/envelope.rs:1310-1330`: Kernel commits serialize on one
  writer connection with `IMMEDIATE` transactions; two reconcile passes cannot
  interleave inside a commit, but their commits can land in either order.
- `crates/kernel/src/admission.rs:1424-1430`: "this store admits one writer".
- `crates/kernel/src/applicability/checkout.rs:699-715`: the existing
  HEAD-before-and-after pattern for detecting a moved HEAD within one scan.
- `envelope.rs:1279-1309`: a same-key same-digest retry replays; the key the
  plan uses is `<path>:<oid>`, so two passes at the same oid collapse to one
  write and two passes at different oids both write.

Plan-only: the cache, its location (daemon memory per binding digest), when
it is set, and whether overlapping passes for one project exist on the m0
compose path.

Derived-state ledger (proposed):

- Derivative: cached HEAD oid per binding digest. Purpose: skip work.
- Base authority: git HEAD for "is there work"; Kernel rows for "what is
  served". The cache wins nothing.
- Derivation: oid of the last pass whose file loop completed.
- Refresh: compare on every m0 compose; any inequality triggers reconcile.
- Visible freshness: best effort; stale across HEAD moves (benign) and
  across overlapping passes (the window below).
- Allowed consumer: `reconcile` early return. Prohibited: any Kernel write
  decision, any serving verdict, any claim about row state.
- Mismatch detection: none in the plan; a cheap oracle exists (root
  lineage's live observation evidence provenance revision).
- Rebuild: process restart clears it; first pass reconciles (replays when
  nothing changed).
- Failure meaning: absent means reconcile; wrong means F6 until HEAD moves.

## Failure scenario

Legal witness: pass P1 at HEAD X reconciles and sets cache X. Pass P2 at HEAD
X reads cache X and returns without any Kernel call. HEAD moves to Y; pass P3
reads cache X != Y, reconciles, sets cache Y.

Smallest violating trace (two overlapping passes, HEAD moves between their
oid reads):

```
A: resolve HEAD = X; read tree(X); compute plan from live rows (rev N)
B: resolve HEAD = Y; read tree(Y); compute plan from live rows (rev N)
B: envelope(F, key F:Y) commits -> lineage at N+1, provenance Y
B: cache := Y
A: envelope(F, key F:X) attempts supersede of rev N rows -> NotFound
   (invalidated by B); envelope poisoned; nothing committed    [good]
A: cache := X                                                 [stale, heals]
```

Variant that does not heal:

```
A: resolve X; B: resolve Y
A: envelope(F, F:X) commits -> rev N+1 at X
B: envelope(F, F:Y): plan computed inside the closure from live rows
   (now rev N+1 at X) -> commits rev N+2 at Y               [good]
A: cache := X
B: cache := Y                                                 [consistent]
```

versus, with plans computed outside the closure:

```
A: resolve X; B: resolve Y; both plan from rev N
B: envelope(F, F:Y) commits rev N+1 at Y
A: envelope(F, F:X): predecessors rev N gone -> NotFound -> poisoned [good]
```

and the true falsifier, requiring the plan inside the closure and the cache
set from the pass's local oid:

```
A: resolve X; B: resolve Y
B: envelope(F, F:Y) commits rev N+1 at Y;   B: cache := Y
A: envelope(F, F:X) plans inside closure from rev N+1 (Y) rows and commits
   rev N+2 at X (an older HEAD)              <- Kernel now at X
A: cache := X   -> next pass at Y sees X != Y and heals        [heals]
```

or with cache writes reordered: `A: cache := X` then `B: cache := Y` after A's
commit landed last: Kernel at X, cache Y, HEAD Y. No pass reconciles until
HEAD moves. F6 holds until then.

## Timing windows and dependencies

Two transform passes for one bound project whose oid reads straddle a commit
to the repository, with Kernel commit order and cache write order inverted.
Whether the m0 compose path can run two passes for one project concurrently
is unverified (OQ-N2-8).

## What a test must construct

- Two reconcile invocations for one binding driven with a controllable gix
  oid (fixture repository advanced between their reads) and a barrier that
  orders their Kernel commits opposite to their cache writes.
- Assert after both complete: the set of `provenance.revision` over live
  `repository_policy` decisions has size one; it equals the cache; a third
  pass at HEAD performs no writes when the cache equals HEAD and the Kernel
  revision equals HEAD.
- Oracle at every early return: the live root observation's evidence
  provenance revision equals the cached oid.
- Unit check that the cache is set only after the whole file loop completes.

## Investigation log

### Q: Can the cache be derived from Kernel state instead of the local oid?
- Sources examined: plan U5 step 1; `admission.rs:2726-2795` (observation
  rows carry evidence; evidence carries provenance).
- Findings: after the per-file commits, the module can read the root
  lineage's live observation and its artifact provenance and set the cache
  from that; this closes the inverted-order window. Not in the plan.
- Missing evidence: design decision.
- Conclusion: needs human input (OQ-N2-7 in open-questions.md).

### Q: Do overlapping passes for one project occur?
- Sources examined: plan KTD2 (trigger on the m0 compose path).
- Findings: not examined in this pass; `transform.rs` concurrency is N4/S4
  territory.
- Missing evidence: the compose path's per-project serialization, if any.
- Conclusion: unresolved, resolvable at implementation.

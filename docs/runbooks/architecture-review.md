# Architecture review runbook

A human reviewer applies this runbook; nothing enforces it mechanically.
It governs the pre-port and post-integration architecture reviews
for product-source waves U2, U3, U4, U5, and U7. U1 and U8 carry no
product source, so they skip this review.

## Skill and invocation

- Skill: `/software-architecture:improve-codebase-architecture`, at the
  revision installed on the reviewing machine. Naming the skill revision in
  the wave note lets later readers identify the rubric that produced the
  report.
- Invocation: run the skill once before porting the wave scope against the
  source checkout at the pinned commit. Run it again after integration against
  the destination checkout. Each run produces an HTML report in the OS temp
  directory. Copy only the candidate table and the decisions into the wave
  note, `migration/waves/<wave>.md`.
- Scope manifest: a JSON file passed to the skill. It lists the modules,
  interfaces, implementations, seams, and adapters the wave touches. It also
  records the `git log --since` window used to measure recent change pressure.

```json
{
  "modules": ["crates/lease", "crates/storage"],
  "interfaces": ["lease::Lease", "storage::Store"],
  "adapters": ["storage::sqlite", "storage::postgres"],
  "change_pressure_window": "2026-06-01..2026-09-01"
}
```

## Candidate strength

The report classifies each candidate. Before recording the result, the owner
re-derives its strength with this rubric. The report's label is advisory.

| Strength | Deletion test | Interface metric |
| --- | --- | --- |
| Strong | passes | interface smaller than implementation by the report's own metric |
| Worth exploring | passes one of the two | |
| Speculative | neither | |

Deletion test: imagine deleting the module. If its complexity reappears in one
place, such as in its callers or a sibling, the module concentrates complexity
and passes the test. If the complexity spreads to several callers unchanged, the
module is a pass-through and fails the test.

Interface metric: the report's count of public surface compared with the
module's implementation size. The owner records both numbers.

## Decisions

- `accepted`: the change lands inside the owning wave. Record the final
  verdict, implementation evidence, affected property records, and specialist
  routes. Rerun `/doc-rigor`, update the affected records under
  `docs/properties/<part>/`, and rerun proofs and tests for touched files. Then
  repeat the post-integration review.
- `rejected`: record named call sites showing that complexity moves rather than
  concentrates, or that the claimed seam has one adapter. One adapter is a
  hypothetical seam and cannot justify an abstraction.
- `recorded`: use only for Worth exploring and Speculative candidates, or for
  Strong candidates first raised by a change inside the review loop
  (loop-created). A recorded loop-created Strong candidate carries a tracking
  issue.

An original-scope Strong candidate left `unresolved` or `recorded` blocks the
wave.

## Routing

- Route keep, move, split, or merge verdicts through
  `/design-review:cohesion-coupling-and-modularity` before implementation.
- Route domain, concurrency, performance, unsafe, language, test, or persistence
  concerns through `/ask-skills` when that skill is installed. Otherwise, name
  the specialist skill directly in the wave note.

## Loop bound

Run at most two post-integration iterations per wave. A third iteration never
runs. When the second iteration still leaves an unresolved original-scope
Strong candidate, or a wave accumulates a third unresolved original-scope
Strong candidate, the wave stops and records an escalation entry in the wave
note instead. The entry names a scope decision (mechanism left scope,
subsystem dropped, or deferred with a tracking issue), and that decision
resolves the candidate for the wave.

## Record

Each wave note, `migration/waves/<wave>.md`, records the pre-port and
post-integration runs. For each candidate it carries the title, the strength,
the decision, and a one-line rationale. Interface and implementation sizes,
deletion-test detail, specialist routes, and the skill revision may be added
when they help a reader; they are not required. Nothing validates the note;
it is written for human readers.

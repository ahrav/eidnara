# Architecture review runbook

This runbook governs the pre-port and post-integration architecture reviews
for product-source waves U2, U3, U4, U5, and U7. U1 and U8 carry no
product source, so they skip this review.

## Skill and invocation

- Skill: `/software-architecture:improve-codebase-architecture`, at the
  revision installed on the reviewing machine. Name the skill revision in the
  wave note so later readers can identify the rubric that produced the report.
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
  routes. Rerun `/doc-rigor`, property impact, proofs, and tests for touched
  files. Then repeat the post-integration review.
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

Run at most two post-integration iterations per wave. The third unresolved
original-scope Strong candidate in one wave, or a third iteration, requires an
escalation entry in the wave note. The entry names a scope decision
(mechanism left scope, subsystem dropped, or deferred with a tracking issue).

## Record

Each wave note, `migration/waves/<wave>.md`, records the pre-port and
post-integration runs: the source commit or destination commit analyzed, each
candidate with its title, strength, origin, decision, modules, interface and
implementation sizes, deletion-test rationale, specialist routes, and final
verdict. Nothing validates the note; it is written for human readers.

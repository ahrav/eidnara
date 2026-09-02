# Architecture review runbook

This runbook governs the pre-port and post-integration architecture reviews
for product-source waves U2, U3, U4, U5, and U7. U1 and U8 record
`architecture_impact: not-applicable` because they contain control records and
validators, not product source.

## Skill and invocation

- Skill: `/software-architecture:improve-codebase-architecture`, at the
  revision installed on the reviewing machine. Record the skill file's SHA-256
  in `architecture-impact.json` under `reports[].skill_sha256`. This lets later
  readers identify the rubric that produced the report.
- Invocation: run the skill once before porting the wave scope against the
  source checkout at the pinned commit. Run it again after integration against
  the destination checkout. Each run produces an HTML report in the OS temp
  directory. Copy only the report SHA-256 (`report_hash`), scope digest, and
  candidate table into the repository.
- Scope manifest: a JSON file passed to the skill. It lists the modules,
  interfaces, implementations, seams, and adapters the wave touches. It also
  records the `git log --since` window used to measure recent change pressure.
  Its SHA-256 is `analyzed.scope_hash`.

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
  (`origin: loop-created`). A recorded loop-created Strong candidate carries a
  bead id.

An original-scope Strong candidate left `unresolved` or `recorded` blocks the
wave.

## Routing

- Route keep, move, split, or merge verdicts through
  `/design-review:cohesion-coupling-and-modularity` before implementation.
- Route domain, concurrency, performance, unsafe, language, test, or persistence
  concerns through `/ask-skills` when that skill is installed. Otherwise, name
  the specialist skill directly in `specialist_routes`.

## Loop bound

Run at most two post-integration iterations per wave. The third unresolved
original-scope Strong candidate in one wave, or a third iteration, requires an
`escalation` record in `architecture-impact.json`. The record must include a
scope decision (`mechanism-left-scope`, `subsystem-dropped`, or
`deferred-with-bead`) and a bead id. The checker refuses a third
post-integration report.

## Record shape

`migration/waves/<wave>/architecture-impact.json`:

```json
{
  "schema_version": 1,
  "wave": "U2",
  "reports": [
    {
      "phase": "pre-port",
      "iteration": 0,
      "analyzed": { "repo": "primitives", "commit": "<sha>", "scope_hash": "<sha256>" },
      "report_hash": "<sha256>",
      "skill_sha256": "<sha256>",
      "candidates": []
    },
    {
      "phase": "post-integration",
      "iteration": 1,
      "analyzed": { "repo": "eidnara", "commit": "<sha>", "scope_hash": "<sha256>" },
      "report_hash": "<sha256>",
      "skill_sha256": "<sha256>",
      "candidates": [
        {
          "title": "...",
          "strength": "Strong",
          "origin": "original-scope",
          "decision": "accepted",
          "modules": ["crates/lease"],
          "interface": "...",
          "implementation": "...",
          "deletion_test": { "concentrates_complexity": true, "rationale": "..." },
          "benefits": { "locality": true, "leverage": false, "testability": true },
          "claims_flexibility": false,
          "adapters": [],
          "specialist_routes": ["cohesion-coupling-and-modularity"],
          "final_verdict": "...",
          "implementation_evidence": "...",
          "property_impact": "migration/waves/U2/property-impact.json",
          "affected_properties": ["..."]
        }
      ]
    }
  ]
}
```

Run this command to validate the record:

```sh
bun run eidnara:check architecture-impact migration/waves/<wave>/architecture-impact.json
```

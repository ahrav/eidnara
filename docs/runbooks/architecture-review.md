# Architecture review runbook

This runbook governs the pre-port and post-integration architecture reviews
that every product-source wave (U2, U3, U4, U5, U7) runs. U1 and U8 record
`architecture_impact: not-applicable` because their files are control records
and validators, not product source.

## Skill and invocation

- Skill: `/software-architecture:improve-codebase-architecture`, at the
  revision installed on the reviewing machine. Record the skill file's SHA-256
  in `architecture-impact.json` under `reports[].skill_sha256` so a later reader
  can tell which rubric produced the report.
- Invocation: run the skill once before porting the wave scope against the
  source checkout at the pinned commit, and once after integration against the
  destination checkout. Each run produces an HTML report in the OS temp
  directory. Copy nothing from the report into the repository except its
  SHA-256 (`report_hash`), the scope digest, and the candidate table.
- Scope manifest: a JSON file passed to the skill listing the modules,
  interfaces, implementations, seams, and adapters the wave touches, plus the
  `git log --since` window used to measure recent change pressure. Its SHA-256
  is `analyzed.scope_hash`.

```json
{
  "modules": ["crates/lease", "crates/storage"],
  "interfaces": ["lease::Lease", "storage::Store"],
  "adapters": ["storage::sqlite", "storage::postgres"],
  "change_pressure_window": "2026-06-01..2026-09-01"
}
```

## Candidate strength

The report classifies each candidate. The owner re-derives the strength with
this rubric before recording it; the report's own label is advisory.

| Strength | Deletion test | Interface metric |
|---|---|---|
| Strong | passes | interface smaller than implementation by the report's own metric |
| Worth exploring | passes one of the two | |
| Speculative | neither | |

Deletion test: imagine deleting the module. If the complexity it holds
reappears in one place (its callers now carry it, or a sibling absorbs it), the
module concentrates complexity and the test passes. If the complexity spreads
to several callers unchanged, the module is a pass-through and the test fails.

Interface metric: the report's count of public surface versus implementation
size for the module. The owner records both numbers.

## Decisions

- `accepted`: the change lands inside the owning wave. Record the final
  verdict, the implementation evidence, the affected property records, and the
  specialist routes taken. Rerun `/doc-rigor`, property impact, proofs, and
  tests for the touched files, then repeat the post-integration review.
- `rejected`: record named call sites showing that the complexity moves rather
  than concentrates, or that the claimed seam has one adapter. One adapter is a
  hypothetical seam and cannot justify an abstraction.
- `recorded`: only for Worth exploring and Speculative candidates, and for
  Strong candidates first raised by a change made inside the review loop
  (`origin: loop-created`). A recorded loop-created Strong candidate carries a
  bead id.

An original-scope Strong candidate left `unresolved` or `recorded` blocks the
wave.

## Routing

- Keep, move, split, or merge verdicts go through
  `/design-review:cohesion-coupling-and-modularity` before implementation.
- Domain, concurrency, performance, unsafe, language, test, or persistence
  concerns go through `/ask-skills` when that skill is installed, otherwise the
  owner names the specialist skill directly in `specialist_routes`.

## Loop bound

At most two post-integration iterations per wave. The third unresolved
original-scope Strong candidate in one wave, or a third iteration, requires an
`escalation` record in `architecture-impact.json` with a scope decision
(`mechanism-left-scope`, `subsystem-dropped`, or `deferred-with-bead`) and a
bead id. The checker refuses a third post-integration report.

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
      "analyzed": { "repo": "commons", "commit": "<sha>", "scope_hash": "<sha256>" },
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

Validate with `bun run eidnara:check architecture-impact migration/waves/<wave>/architecture-impact.json`.

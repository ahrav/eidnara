# Daemon property catalogs

Records for `crates/daemon`, carried from the host repository at `eb6da6109`
as six area catalogs. Each area keeps the source part's `catalog.md`,
`existing-checks.md`, `fault-map.md`, `portfolio-evaluation.md`, and
`evidence/`; the `_lenses/` directories are the source's working material and
stay for traceability. `_lenses/scope-map-and-risk-ranking.md` at this level is
the source's module-wide scope map that fixed the six area boundaries.

| Area | Source part | Subject |
| --- | --- | --- |
| `history_summarizer/` | `part-4a-history_summarizer` | the history_summarizer publish path and its validation gate |
| `transform/` | `part-4b-transform` | the transform pass engine and its cache-state transition |
| `handlers/` | `part-4c-handlers` | durable operation handlers and staging coordinators |
| `facade/` | `part-4d-facade` | the facade surface, note evaluation, and response assembly |
| `rendering/` | `part-4e-rendering` | rendered output, tags, and nudge overlay |
| `decisions/` | `part-4f-decisions` | decision units, configuration, and harness codecs |
| `interpretation-layer/` | the generalize-source-context specification ([#1105](https://github.com/ahrav/eidnara/issues/1105)), Part A and Part B S2, authored at `a0e3f2993` and verified at `41d889ea4` | tool-name, argument-key, and native-field interpretation, the literal classification, and the corpus loss conditions |

The first six areas carry host-repository catalogs; `interpretation-layer/`
is authored in this repository and states its own provenance in its README.
Each carried `catalog.md` opens with a provenance section that states what the port
changed under it: renamed crates, modules, tables, and identifiers; the
mechanisms the port deleted, whose records carry `Status: invalidated`; and the
rule that line citations are the source catalog's coordinates, marked where the
file or line is absent here and otherwise unverified until a campaign
re-verifies them. The record contract is [`../METHOD.md`](../METHOD.md).

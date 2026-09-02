# Property catalogs

Each subdirectory holds one property catalog: concrete, evidence-backed
statements of what a subsystem must always hold, what must eventually happen,
and which rare situations a test campaign has to reach. `METHOD.md` is the
record contract; it is copied from `magic-context@b5273dcb2a76fb0ffe9800b7c54bbd8d1ad98825`
and is the pinned method for every catalog here.

## Layout

```
docs/properties/
  METHOD.md              record contract (pinned)
  <part>/
    catalog.md           authored records, one `### <slug>` block each
    index.json           generated from catalog.md; never edited by hand
    evidence/<slug>.md   per-record evidence
    existing-checks.md   check inventory
    fault-map.md         fault-to-property map
    relationships.md     shared-mechanism relationships
    portfolio-evaluation.md
```

`catalog.md` is the authored source. `index.json` is generated:

```
bun scripts/eidnara-migration/generate-property-index.ts docs/properties/<part>
bun scripts/eidnara-migration/generate-property-index.ts docs/properties/<part> --check
bun run eidnara:check property-catalog docs/properties/<part>/index.json
```

`--check` fails when `index.json` drifts from `catalog.md`. The `property-catalog`
checker then enforces METHOD's vocabulary on the generated index: `Type`,
`Reachability`, `Status`, `Exercised`, `Check` semantics, and `Confidence` must
use the enumerated values. Records that deviate in the source catalogs fail
here and are reconciled by the wave that migrates them.

## Coverage authority

The on-disk `<part>/` directories are the coverage authority. A status table in
a README is advisory. Every record carries a `provenance` of `<repo>@<sha>`
when it enters through a wave's `property-impact.json`; carried-forward records
copy their source status verbatim, and core records carry current
discriminating evidence.

## Parts

| Part | Source | Wave |
|---|---|---|
| `shared-primitives` | `commons/docs/property-catalogs/cortexkit-lease/` plus discovery for cache stability, storage types, non-lease storage | U2 |
| `host-runtime`, `shm-transport`, `tokenizer` | `magic-context/docs/properties/part-1-*`, `part-2a` through `part-2f` | U3 |
| `semantic-kernel`, `daemon` | `part-3-store-core`, `part-4a` through `part-4f` | U4 |
| `authority-transition`, `lkg`, `retrieval`, `dreamer`, `embeddings`, `git-ingestion` | `part-5a-storage`, `part-5c-transform-ts`, discovery | U5 |
| `cli`, `historian-ts` | `part-5d-cli`, `part-5b-historian-ts` | U7 |

No part directory exists before its wave.

# Property catalogs

Each subdirectory holds one property catalog. Catalogs state what a subsystem
must always hold, what must eventually happen, and which rare situations a test
campaign must reach. `METHOD.md` defines the record contract. It is copied from
`magic-context@b5273dcb2a76fb0ffe9800b7c54bbd8d1ad98825` and pinned for every
catalog here.

## Layout

```text
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

`catalog.md` is the authored source. The generator creates `index.json`:

```sh
bun scripts/eidnara-migration/generate-property-index.ts docs/properties/<part>
bun scripts/eidnara-migration/generate-property-index.ts docs/properties/<part> --check
bun run eidnara:check property-catalog docs/properties/<part>/index.json
```

`--check` fails when `index.json` differs from `catalog.md`. The
`property-catalog` checker then enforces METHOD's vocabulary on the generated
index. `Type`, `Reachability`, `Status`, `Exercised`, `Check` semantics, and
`Confidence` must use the enumerated values. Records that deviate in source
catalogs fail here. The wave that migrates them must reconcile those records.

## Coverage authority

The on-disk `<part>/` directories are the coverage authority. A status table in
a README is advisory. Every record that enters through a wave's
`property-impact.json` carries a `provenance` value in the form `<repo>@<sha>`.
Carried-forward records copy their source status verbatim. Core records carry
current discriminating evidence.

## Parts

| Part | Source | Wave |
| --- | --- | --- |
| `shared-primitives` | `commons/docs/property-catalogs/cortexkit-lease/` plus discovery for cache stability, storage types, non-lease storage | U2 |
| `host-runtime`, `shm-transport`, `tokenizer` | `magic-context/docs/properties/part-1-*`, `part-2a` through `part-2f` | U3 |
| `semantic-kernel`, `daemon` | `part-3-store-core`, `part-4a` through `part-4f` | U4 |
| `authority-transition`, `lkg`, `retrieval`, `dreamer`, `embeddings`, `git-ingestion` | `part-5a-storage`, `part-5c-transform-ts`, discovery | U5 |
| `cli`, `historian-ts` | `part-5d-cli`, `part-5b-historian-ts` | U7 |

Part directories do not exist before their assigned waves.

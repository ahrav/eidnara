# Property catalogs

Each subdirectory holds one property catalog. Catalogs state what a subsystem
must always hold, what must eventually happen, and which rare situations a test
campaign must reach. `METHOD.md` defines the record contract. It is copied from
the host source repository at `b5273dcb2a76fb0ffe9800b7c54bbd8d1ad98825` and
pinned for every catalog here.

## Layout

```text
docs/properties/
  METHOD.md              record contract (pinned)
  <part>/
    catalog.md           authored records, one `### <slug>` block each
    evidence/<slug>.md   per-record evidence
    existing-checks.md   check inventory
    fault-map.md         fault-to-property map
    relationships.md     shared-mechanism relationships
    portfolio-evaluation.md
```

`catalog.md` is the authored source. `Type`, `Reachability`, `Status`,
`Exercised`, `Check` semantics, and `Confidence` use METHOD's enumerated
values. Nothing generates or validates the catalog; the wave that carries a
record reconciles it with METHOD by hand.

## Coverage authority

The on-disk `<part>/` directories are the coverage authority. A status table in
a README is advisory. A record that enters through a wave names its source in
plain words: the source repository and the commit it was read at, for example
"the shared-crate source repository at `89abb40`". Carried-forward records copy
their source status verbatim. Core records carry current discriminating
evidence.

## Parts

| Part | Source | Wave |
| --- | --- | --- |
| `shared-primitives` | the lease property catalog in the shared-crate source repository at `89abb40` plus discovery for cache stability, storage types, non-lease storage | U2 |
| `host-runtime`, `shm-transport`, `tokenizer` | the host source repository's catalogs `part-1-*`, `part-2a` through `part-2f` | U3 |
| `semantic-kernel`, `daemon` | `part-3-store-core`, `part-4a` through `part-4f` | U4 |
| `authority-transition`, `lkg`, `retrieval`, `dreamer`, `embeddings`, `git-ingestion` | `part-5a-storage`, `part-5c-transform-ts`, discovery | U5 |
| `cli`, `historian-ts` | `part-5d-cli`, `part-5b-historian-ts` | U7 |

Part directories do not exist before their assigned waves.

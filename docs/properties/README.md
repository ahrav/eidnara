# Property catalogs

Each subdirectory holds one property catalog. Catalogs state what a subsystem
must always hold, what must eventually happen, and which rare situations a test
campaign must reach. `METHOD.md` defines the record contract. It is copied from
the host source repository at `b5273dcb2a76fb0ffe9800b7c54bbd8d1ad98825` and
kept by review; nothing checks it against the source.

## Retired performance artifacts

Benchmark runners, timing reports, and capacity research models are no longer
kept in this repository. References to those artifacts in older catalog records
are historical, not runnable checks or current performance evidence. Correctness
and resource-bound regression tests remain; their fixtures live beside the tests.

## Layout

```text
docs/properties/
  METHOD.md              record contract (copied from the source, kept by review)
  <part>/
    catalog.md           authored records, one `### <slug>` block each
    evidence/<slug>.md   per-record evidence
    relationships.md     shared-mechanism relationships (optional)
    [<area>/]            optional grouping level for the three files below
      existing-checks.md check inventory
      fault-map.md       fault-to-property map
      portfolio-evaluation.md
```

`existing-checks.md`, `fault-map.md`, and `portfolio-evaluation.md` sit at the
part root or under an optional `<area>/` level.

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
| `memory-store`, `daemon` | `part-3-store-core` (as `memory-store/`), `part-4a` through `part-4f` (as `daemon/<area>/`), and the `part-4-module` scope lens, all in the host source repository at `eb6da6109` | U4 |
| `authority-transition`, `lkg`, `retrieval`, `memory_classifier`, `embeddings`, `git-ingestion` | `part-5a-storage`, `part-5c-transform-ts`, discovery | U5 |
| `cli`, `history_summarizer-ts` | `part-5d-cli`, `part-5b-history_summarizer-ts` | U7 |
| `search-projection` | the RP2.1 specification ([#347](https://github.com/ahrav/eidnara/issues/347)) and its companion catalogs for embedding, export and recovery, and projection coverage, authored against Eidnara `913234433` | RP2.1 P1 |
| `hot-path-optimization` | the hot-path latency specification ([#350](https://github.com/ahrav/eidnara/issues/350)), whose parent supplement was published under the superseded [#351](https://github.com/ahrav/eidnara/issues/351); the parent supplement and its `latency-audit/` area are authored against Eidnara `913234433` | HP1 M0 |
| `opencode-plugin/transform-edit-responses` | the Transform Edit Responses specification ([#525](https://github.com/ahrav/eidnara/issues/525)); the client execution records TE17 to TE24, TE26, TE27, and TE30 are authored against the #533 change | TE U3 |
| `canonical-positive-claim-projection` | the RP2.4 specification ([#408](https://github.com/ahrav/eidnara/issues/408)) and the slugs its implementation tickets assign; the specification's catalog bundle is unavailable, so records are reconstructed from the specification and verified against the kernel code that implements them | RP2.4 |
| `fusion-routes` | the RP2.7 specification ([#630](https://github.com/ahrav/eidnara/issues/630)) and the slugs its local companion bundle proposed; each implementation ticket lands the records whose checks it makes executable, starting with `fusion/` identity records from [#638](https://github.com/ahrav/eidnara/issues/638) | RP2.7 U1 |
| `selection-packing` | the RP2.8 specification ([#629](https://github.com/ahrav/eidnara/issues/629)); each implementation ticket lands the records whose checks it makes executable, starting with `identity/` from [#631](https://github.com/ahrav/eidnara/issues/631); `marker-ledger.md` tracks the precondition markers the packing campaign names | RP2.8 U1 |
| `evidence-backed-memory-review` | the evidence-backed memory review specification ([#709](https://github.com/ahrav/eidnara/issues/709)); the parent's original portfolio is unavailable, so the owner accepted the specification's supplemental slugs as replacement obligations; each residual ticket lands the records whose checks it makes executable, starting with canonical resolution from [#725](https://github.com/ahrav/eidnara/issues/725) | Review U6 |

Part directories do not exist before their assigned waves.

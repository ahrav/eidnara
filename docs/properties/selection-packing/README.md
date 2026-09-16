# RP2.8 selection-packing property catalogs

Companion catalogs for the RP2.8 specification
([#629](https://github.com/ahrav/eidnara/issues/629)): deduplication, bounded
selection, and packing. Each implementation ticket lands the records whose
checks it makes executable, so the on-disk part is the coverage authority for
what is exercised and the specification remains the authority for what is still
proposed.

## Parts

| Part | Owns | Landed by |
| --- | --- | --- |
| `identity/` | Per-occurrence attribution on shared payloads, the grouping key, and the non-grouping classes. | RP2.8 U1 ([#631](https://github.com/ahrav/eidnara/issues/631)). |
| `required/` | The required phase: failure classes, the integer budget, and the trace that witnesses no optional event or retrieval call precedes completion. | RP2.8 U2 ([#632](https://github.com/ahrav/eidnara/issues/632)). |
| `grouping/` | Grouping as a pure function of the selected set, the per-identity coverage partition, the optional skip-and-continue scan, and the optional-phase bounds. | RP2.8 U3 ([#633](https://github.com/ahrav/eidnara/issues/633)). |
| `accounting/` | The accounting profile and its revision, the revision-keyed cost cache, rendered-delta charging with the group wrapper charged once, and the rendered-bytes and estimated-tokens bounds. | RP2.8 U4a ([#634](https://github.com/ahrav/eidnara/issues/634)). |

The serialization and application parts enter this directory
with the tickets that land their checks. Each part follows `../METHOD.md`.

## Precondition-marker ledger

`marker-ledger.md` lists every precondition marker the packing campaign names,
which record each marker serves, and whether a check has fired it. A ticket
that adds a marker adds a row; a ticket that fires one records the check that
fired it. RP2.8 U5 closes the ledger per harness. The RP2.8 Q8 owner freezes
each per-harness marker set before the harness tickets implement against it;
until then the harness column reads `retrieval` or `daemon` for markers
observed at those crate boundaries.

# Daemon interpretation layer

Property catalog for the code that reads harness records and tool calls and
decides what a tool name, an argument key, or a native field means: selection,
chunk text, wire extraction, tool outcome, injection, the summarizer chunk
formatter, the OpenCode and Pi codecs, and the `tool_vocabulary` module that
holds their tool-name and argument-key tables.

## Source

The generalize-source-context specification
([#1105](https://github.com/ahrav/eidnara/issues/1105)) carries the records,
Part A (existing behavior) and Part B S2 (the vocabulary module and its gate).
U5 ([#1110](https://github.com/ahrav/eidnara/issues/1110)) lands them; the
provenance and verification commit are stated in
[`catalog.md`](catalog.md#scope). The ten #1104 records this work touches are
[linked, not revised](catalog.md#1104-records-this-work-touches).

## Layout

| File | Contents |
| --- | --- |
| `catalog.md` | Scope, reachability classes, index, the 22 records, relationship map, the linked #1104 records |
| `literals.md` | The literal classification table: every field, tool, and argument-key literal of the layer with its class and location, the excluded literals, and the gate-set equality |
| `loss-conditions.md` | The closed loss-condition enumeration mirrored from `crates/daemon/src/source_corpus.rs`, the non-unit reasons, and the fail-closed dispositions of the document reader and the capture pre-screen at HEAD |
| `existing-checks.md` | Claim-bearing checks in the tree, all `unaudited` |
| `fault-map.md` | Fault classes, the records each constructs, coverage checks to add, and a ranking by cheapest valid oracle |
| `portfolio-evaluation.md` | The Part A evaluation's dispositions, the owner biases, and what re-verification changed |
| `evidence/<slug>.md` | One evidence file per record |

## Later units

U6 ([#1111](https://github.com/ahrav/eidnara/issues/1111)) replaced the
behavior il-readmission-block-ends-batch-unnamed-at-head describes, and the
record is invalidated; U7
([#1112](https://github.com/ahrav/eidnara/issues/1112)) invalidates
il-repository-id-equals-project-id-at-head, and U8
([#1113](https://github.com/ahrav/eidnara/issues/1113)) revises
il-doc-read-nofollow-final-component-flat-list. Each of those units updates
the record it changes.

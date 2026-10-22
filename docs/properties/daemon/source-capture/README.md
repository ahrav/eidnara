# Source capture

Property catalog for the source-capture specification
([#1066](https://github.com/ahrav/eidnara/issues/1066)): the consent-gated
capture of native message text, tool outputs, tool errors, and configured
repository documents as Kernel source descriptors, with their retention,
expiry, re-run, deletion, outcome-record, and summarizer-digest rules. The
boundary is described in [`docs/source-capture.md`](../../../source-capture.md).

## Source

The specification published its catalog as two issue comments, verified at
baseline `08de9051a`: Part A (20 records of existing behavior) and Part B (22
proposed obligations), with the existing-check inventory, the fault map, the
coverage markers, and a fresh-context portfolio evaluation. This directory
carries those 42 records in METHOD form, reconciled with the checks units U1
to U10 landed, plus three gap records: the resource bounds the
specification's evaluation queued, and the R1 reach and single-store stop
condition this catalog's evaluation queued. Evidence that the comments held inline lives in `evidence/<slug>.md`.

## Layout

| File | Contents |
| --- | --- |
| `catalog.md` | Scope, reachability classes, index, the 45 records, relationship map |
| `evidence/<slug>.md` | One evidence file per record |
| `existing-checks.md` | Claim-bearing checks, all `unaudited`, plus areas with no check |
| `fault-map.md` | Fault classes, coverage witnesses, leverage ranking |
| `portfolio-evaluation.md` | The fresh-context evaluation of this portfolio and how each finding was dispositioned |
| `spec-integration.md` | Requirement to unit to record to test crosswalk, and the stop conditions |
| `_lenses/` | The authoring groups' working records, kept for traceability |

## Landed by

| Unit | Ticket | Records whose check it added |
| --- | --- | --- |
| U1 | #1080 | `sc-consent-gate-admits-capture`, `sc-unit-cap-refuses-whole-never-prefix`, `sc-capture-resource-bounds` |
| U2 | #1082 | `sc-consented-window-reaches-publisher`, `sc-capture-state-has-one-durable-store`, `sc-response-independent-of-publication`, `sc-one-outstanding-batch-refuse-newest`, `sc-terminal-disposition-never-retried`, `sc-pending-unit-publishes-next-ready-pass`, `sc-capture-creates-no-decision`, `sc-capture-logs-carry-no-text`; invalidates `sc-harness-classes-have-no-production-publisher` |
| U3 | #1083 | `sc-adapter-identity-native-or-refused` (the Pi row case) |
| U4 | #1084 | `sc-harness-capture-deadline-replay-never-renews`, `sc-expiry-descriptor-first-cited-survives`, `sc-expiry-drains-within-page-bound` |
| U5 | #1085 | `sc-outcome-observed-bounded-occurrence-bound`, `sc-v1-receipt-replays-legacy` |
| U6 | #1088 | `sc-digest-bounded-deterministic-char-boundary`, `sc-digest-citations-never-reach-artifact`, `sc-tool-call-line-carries-no-outcome` (restated) |
| U7 | #1086 | `sc-rerun-exactly-one-live-generation` |
| U8 | #1087 | `sc-delete-retires-before-memory-store-delete`, `sc-delete-retry-recovers-crash-between-stores`, `sc-deleted-session-not-resurrected-by-queued-capture`; invalidates `sc-session-delete-leaves-kernel-descriptors` |
| U9 | #1081 | `sc-repodoc-one-live-revision-above-historical-head` (the Kernel guard), `sc-descriptor-revision-exceeds-live-predecessor-only` |
| U10 | #1089 | `sc-repodoc-raw-bytes-separate-from-prompt-hash`, `sc-repodoc-only-verified-missing-retires`, `sc-repodoc-one-live-revision-above-historical-head` (capture side) |

The record contract is [`../../METHOD.md`](../../METHOD.md).

# System lens: existing test strategy

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External lead: latency-audit B1 and W5 check inventories.

## Observations

- `crates/daemon/src/served_json.rs:171-253` pins scalar encoding and decoded
  key order against the value-round-trip reference.
- `crates/daemon/src/transform.rs:13714-13938` pins served bytes, shell
  provenance, equality indexing, positional precedence, and signed zeros.
- `transform.rs:14467-14490` projects `wire-golden.json` and compares the
  resulting flat records to `ingress-projection-golden.json`.
- Static parsing of that HEAD fixture finds seven messages and sixteen blocks:
  three text, one reasoning, one redacted reasoning, five calls, five results,
  and one opaque. All ten tool flags are explicitly false. Media is absent.
  Output variants are text, json, execution_denied, error_text, error_content.

## Candidate and gaps

The fixture is not a complete plugin-emission partition. Freeze absent/true
tool flags and missing media/content variants before implementation. Preserve
explicit-false and nonplugin fixtures as separately classified cases.
Existing catalogs' historical exercise statements are not inherited here.

## Narrow nonapplicability

Allocation benchmarks do not prove identity. Every discovered check remains
unaudited, and no tests or benchmarks run in this discovery pass.

# Canonical positive-claim projection

Property catalog for the RP2.4 specification
([#408](https://github.com/ahrav/eidnara/issues/408)): canonical claim facts,
trusted causal classification, materialization into the shared search
projection, and validated delivery through the harnesses.

## Source

The specification names a 35-property catalog bundle that was authored
alongside it and kept outside the repository. That bundle is not available in
any checked-out tree, so the records here are reconstructed from the stable
slugs the specification and its implementation tickets list, from the
specification's constraints and acceptance criteria, and from the code and
tests that implement them. Each record names the code it was verified against
at authoring time. A record whose slug the bundle described differently must be
reconciled by review against the bundle when it becomes available; nothing here
claims to reproduce the bundle's text.

## Layout

| File | Contents |
| --- | --- |
| `catalog.md` | The records that have an implementation surface in the tree, one `### <slug>` block each, plus the index of every slug the specification assigns |
| `evidence/<slug>.md` | One evidence file per authored record |
| `existing-checks.md` | Claim-bearing checks in the tree, all `unaudited` |
| `fault-map.md` | Fault classes, required faults per property, and coverage checks to add |
| `spec-integration.md` | Slug to specification section to implementation ticket to code crosswalk |

Records enter this catalog with the implementation ticket that gives them a
code surface. Slugs assigned to a later ticket appear in the index with no
record until that ticket lands.

`spec-integration.md` is an extra file beyond the METHOD table: it carries the
slug crosswalk the specification requires. `_lenses/` is absent because the
records were reconstructed from the specification and the code rather than
discovered through lens passes; `portfolio-evaluation.md` records the
independent review that stood in for the fresh-context evaluation. Evidence
files run shorter than METHOD's 60 to 120 line target because each record's
evidence is one module and one test file.

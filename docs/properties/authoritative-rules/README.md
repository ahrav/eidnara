# Authoritative rules property catalog

This part holds the property catalog for the specification "Authoritative
rules reach coding agents automatically": two authority channels (user
directives through a user-invoked harness command; repository policy from
committed `AGENTS.md` files at HEAD) that write Kernel decisions through the
unchanged admission policy, a daemon-side scope context, two-phase packing
with a reserved rules share and disclosed omission, and cross-harness parity.

Inspected source revision: `08de9051a735f136ff63a9dca49eaa11493b3006`
(the settled plan's baseline commit). Date: 2026-10-21. Every `file:line`
reference was verified at that revision by the pass that wrote it; the
fresh-context evaluation spot-checked about seventy of them and the two it
found off are corrected. Records for implemented behavior are claims under
test, not proof. Records for proposed behavior describe obligations the
milestones U1 to U6 must make checkable; none is exercised at HEAD.

The published specification issue reference is recorded here after
publication: [https://github.com/ahrav/eidnara/issues/1067](https://github.com/ahrav/eidnara/issues/1067).

## Parts

| Part | Surfaces | Records | Origin | Companions |
| --- | --- | --- | --- | --- |
| `kernel-authority-and-scope/` | S1 Kernel admission and authority mechanics; S2 scope eligibility | 12 (plus 5 reused) | implemented | `existing-checks.md`, `fault-map.md`, `handoff.md`, `portfolio-evaluation.md`, `evidence/`, `_lenses/` |
| `injection-and-harness/` | S3 packing primitives; S4 m0 injection path; S5 harness surfaces | 12 (plus 8 reused) | implemented | same set |
| `write-channels/` | N1 directive channel and explicit-user subject guard; N2 repository policy ingestion and reconciliation | 14 | proposed | `certificate.md`, `fault-map.md`, `open-questions.md`, `evidence/` |
| `read-side/` | N3 injection scope context; N4 two-phase packing, disclosure, diagnostics; N5 harness directive command and parity | 15 | proposed | same set |

`portfolio-evaluation.md` at this level is the fresh-context evaluation of the
two implemented parts with its disposition; the per-part files point to it.
`spec-integration.md` is the crosswalk from slug to specification section,
requirement, and milestone. The proposed parts carry no portfolio result; an
output certificate records scope, authority, rules, commands, derived state,
and falsifiers for each.

Record origin is stated under each slug heading: `Origin: implemented
behavior` or `Origin: proposed design obligation (not implemented at HEAD)`.
Reachability labels describe HEAD for implemented records and the built path
for proposed ones; six kernel records are `test-only` today because the
writers U3 and U5 add do not exist yet, and they are relabeled in one pass
when those milestones land.

## Scope boundary

Discovery of implemented behavior is bounded to the mechanisms the settled
plan relies on or changes. The evaluation records this as a bias (B2) and the
one gap it produced (the Kernel read caps that run before packing) is an open
question in the specification. Items the lens passes found outside that
boundary stay in `_lenses/` for traceability.

## Reading order

1. `spec-integration.md` for the slug-to-requirement map.
2. The part `catalog.md` for the records; `evidence/<slug>.md` for the trail,
   failure scenario, and what a test must construct.
3. `fault-map.md` for the faults and enabling states a campaign needs.
4. `portfolio-evaluation.md` for the independent evaluation and its
   disposition; `write-channels/open-questions.md` and
   `read-side/open-questions.md` for the design questions the passes could
   not settle, which the specification's Open Questions section carries with
   owners and resolution points.

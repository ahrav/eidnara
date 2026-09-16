# RP2.7 fusion-routes property catalogs

Companion catalogs for the RP2.7 specification
([#630](https://github.com/ahrav/eidnara/issues/630)): one fusion boundary and
daemon routes. The specification's local bundle holds 39 proposed records
across fusion, query-route, and application surfaces. This directory carries a
record only once an implementation ticket lands its executable check, so the
on-disk part is the coverage authority for what is exercised and the
specification remains the authority for what is still proposed.

## Parts

| Part | Owns | Landed by |
| --- | --- | --- |
| `fusion/` | Identity, lane consolidation, digests, parent groups, and the pure fusion arithmetic over declared lane rankings. | RP2.7.U1 ([#638](https://github.com/ahrav/eidnara/issues/638)); RP2.7.U2 adds the arithmetic records. |
| `query-route/` | The request budget, tracked blocking work, interruptible projection reads, and the `retrieval.query` route's authorization, bounds, cancellation, revalidation, terminals, and disable. | RP2.7.U3a ([#639](https://github.com/ahrav/eidnara/issues/639)); RP2.7.U3b ([#641](https://github.com/ahrav/eidnara/issues/641)); RP2.7.U3c ([#642](https://github.com/ahrav/eidnara/issues/642)) adds the dense-lane records. |

| `application/` | The preparation and receipt lifecycle behind `retrieval.prepare`, `retrieval.apply`, and `retrieval.confirm`, and later the capability gates. | RP2.7.U4 ([#643](https://github.com/ahrav/eidnara/issues/643)); U5 adds the capability records. |

The capability-gate records enter this directory with the ticket that lands
their checks. Each part follows `../METHOD.md`. Records that a later ticket lands keep their
specification slugs so the specification's acceptance rows can be traced to
one executable check.

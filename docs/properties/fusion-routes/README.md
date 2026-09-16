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
| `query-route/` | The request budget, tracked blocking work, interruptible projection reads, and later the route's authorization, degradation, and terminals. | RP2.7.U3a ([#639](https://github.com/ahrav/eidnara/issues/639)); U3b and U3c add the route records. |

The application part enters this directory with the tickets that land its
checks. Each part follows `../METHOD.md`. Records that a later ticket lands keep their
specification slugs so the specification's acceptance rows can be traced to
one executable check.

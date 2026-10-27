# Fault map: dispute evidence on kernel.commit

System: the PR A branch tip of #1160 (base `6159fd224`, code commit
`6c4d445e3`, review-round code commits `511cd6d73` and `f68e895ca`,
evidence-walk commit `f94f3163b`; line numbers verified at `f94f3163b`). Every active record is `always` or
`always-or-unreached`; each needs its enabling state constructed or it
passes vacuously. Fault
class names F1 to F17 refer to
[`../disposition-path/fault-map.md`](../disposition-path/fault-map.md);
D1 to D14 are added here. Coverage markers assert preconditions, never the
violation.

## Fault classes and availability

| Class | What it is | Available | How |
|---|---|---|---|
| D1 dispute without evidence | `mark_disputed` or `contradict` with neither field, or an empty id | yes, route | raw body; `dispute(.., json!({}))` (`crates/daemon/tests/kernel_routes.rs:5580`) |
| D2 evidence on another event | a reference or id on `mark_stale`, `explicit_reject`, `quarantine` | yes, route | raw body |
| D3 retired evidence id | `evidence_meta.invalidated_commit_seq` set | yes, store API | `retire_evidence` on an uncited row (`kernel_routes.rs:5820-5825`) |
| D4 uncited or unscoped-only evidence | live row with no scoped live citer | yes, store API | `ingest_artifact`; an unscoped `insert_decision` citer (`kernel_routes.rs:5830-5838`) |
| D5 foreign evidence | live row cited only by another bound project | yes, route | `bind_project("project-b")` (`kernel_routes.rs:5843`) |
| D6 malformed reference | oid case, length, path traversal, protected component, NUL, length, detection | yes, route | raw body |
| D7 reserved bytes in a valid reference | `;`, `=`, `%`, `@`, spaces, non-ASCII | yes, route | raw body |
| D8 raised reporter assertion | `asserted_taint_class` above the derivation | yes, route | raw body |
| D9 same session, other evidence | two disputes of one subject under two identities | yes, command and route | `memoryMarkOperationId` with two `args` |
| D10 citer retired after preview | the only project citer retired between a preview and its commit | yes, route | `retire_decision` between `preview` and `commit` (`kernel_routes.rs:6416`) |
| D11 mixed citers | one evidence id cited by the project and by a foreign scope that sorts first or whose citers were written first | yes, store API and route | `insert_scope` and a foreign `insert_decision` (`kernel_routes.rs:6285`) |
| D12 citer in the same envelope | an `insert_decision` citing the evidence ahead of the dispute in one envelope | yes, route | `citing_decision` before `dispute` in one commit (`kernel_routes.rs:6516`) |
| D13 forged reason field | a disposition body carrying `reason` | yes, route | raw body (`kernel_routes.rs:6252`) |
| D14 approved relaxation | a dispute naming a valid approval against a more restrictive prior | yes, route | `seed_approval` and `approval_object_id` (`kernel_routes.rs:6459`) |

Not available: a second writer blocked on the writer lock while a dispute
holds it (needed to construct a retirement queued behind the dispute), and
a route-level reply drop (F12). A queued retirement of the cited evidence
row itself cannot occur: the row is cited by a live project row, and
`retire_evidence` refuses cited evidence with `Conflict`
(`crates/kernel/src/slice/write.rs:585-608`). The reachable queued case
is a citer retirement. No asserted-edge citer can be built: no Rust code
under `crates/` writes `asserted_edges`.

## Per-property required faults and enabling state

| Record | Required faults | Enabling state | Exercised |
|---|---|---|---|
| `dispute-requires-evidence-from-plugin` | D1, D2, D13 | raw bodies; command parser | partial - command arm |
| `cited-evidence-live-at-dispute-commit` | D3, D10 | retired row; live cited row; citer retired after preview | yes (queued retirement not constructed: retiring cited evidence is refused, and no lock-holding harness exists for a queued citer retirement) |
| `cited-evidence-visible-to-bound-project` | D4, D5, D10, D11, D12 | two projects; unscoped citer; uncited row; mixed citers; same-envelope citer | yes (asserted-edge citer not constructed: no writer exists) |
| `evidence-ref-well-formed-and-redaction-stable` | D6, D7 | malformed and reserved-byte references; both fields on one dispute | yes |
| `reporter-class-derived-never-asserted-up` | D8, F1 | raised and lowered assertions; subject classes unlike the reporter's | partial - command arm |
| `dispute-with-evidence-never-raises-standing` | F2, F3, F15, D14 | priors in every disposition and maturity; evidence on and off; a valid approval | yes |
| `dispute-identity-includes-evidence-key` | D9, F10, F11 | two evidence keys; replay; reuse | yes (F12 not constructed) |
| `dispute-preview-writes-nothing` | D1 to D6, D10 | preview then commit at one tip; a tip that moves between them | yes |
| `structured-reason-projects-typed-reporter-data` | D7, D13, F15 | reserved bytes; a forged reason; a cascade | partial - PR A form |
| `use-outcome-link-names-own-session-prior-serve` | (PR B) | a serve before a dispute in one session | not yet |

## Coverage checks to add

Constant-name `sometimes` markers. None asserts a violation. None of the
seven exists in code at `f68e895ca`: `git grep` finds no marker name
outside `docs/`. The third column names the test that reaches each
marker's precondition today, which a marker added at the named code
point would observe.

| Marker | Fires when | Precondition reached by |
|---|---|---|
| `dispute_refused_without_evidence` | `DisputeReason::validated` sees a dispute with neither field | `kernel_routes.rs:5971` |
| `dispute_cites_retired_evidence` | `evidence_cited_in` reads a retired `evidence_meta` row inside a dispute | `kernel_routes.rs:5801` |
| `dispute_cites_foreign_only_evidence` | `evidence_cited_in` offers at least one scope and the filter accepts none | `kernel_routes.rs:5801` |
| `dispute_reference_holds_reserved_bytes` | `encode` escapes at least one byte of a stored path | `kernel_routes.rs:5880` |
| `dispute_reporter_lowered` | the resolved taint ranks below the derived taint | `kernel_routes.rs:6025` |
| `dispute_cites_live_evidence_with_queued_retirement` | a retirement of the cited row's only project citer waits on the writer while the dispute holds it | none; needs a lock-holding harness. A retirement of the cited evidence row itself is refused while cited, so the citer is the only retirement that can queue |
| `dispute_evidence_cited_by_asserted_edge_only` | the only scoped citer is an asserted edge | none; no writer of `asserted_edges` exists |

The owned markers named in
[`../disposition-path/fault-map.md`](../disposition-path/fault-map.md)
(`admission_write_with_evidence_id`, `disposition_subject_above_candidate`,
and the rest) carry their constructing tests there.

## Ranking by cheapest valid oracle

1. The route tests in section "kernel.commit disputes carrying evidence"
   and the review-round tests appended after it already construct every D
   class; adding the seven markers above is the remaining work, and two
   of them have no reaching test.
2. One kernel test that inserts an asserted edge citing evidence and reads
   `evidence_cited_in`: closes the asserted-edge branch. It needs a test
   writer for `asserted_edges`, since none exists.
3. A lock-holding harness in `crates/kernel/tests` that retires the
   evidence's only citer from a second writer while a dispute transaction
   is open: the only way to observe the queued-retirement order.
4. One TypeScript assertion that both flag orders give one identity.

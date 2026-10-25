# Open questions for the P5 proposed obligations

Extracted from `certificate.md` (the "Unknowns" hand-off list and the
"Conflicts between the plan and HEAD 3a3b916b7" section). Owners are left as
"specification owner" where the certificate names none. Each item stays open
until the specification resolves it on its own evidence; the dependency
catalog does not resolve any of them (see bias B3 in
`../dependencies/portfolio-evaluation.md`).

## Unknowns OQ-A to OQ-E

| Id | Question | Owner |
| --- | --- | --- |
| OQ-A | Definition of the case-fold function F applied to both sides before comparison, and how it relates to the FTS5 diacritic handling (certificate R-A3). | specification owner |
| OQ-B | One eligibility predicate shared by the pass, the sweep, and the reads: which dispositions are candidates and which are swept; `Stale` and `Quarantined` need a human decision (certificate conflict 6). | specification owner |
| OQ-C | Descriptor text source (H3): carried out of the publication job past the capture commit gate, or read back from the Kernel evidence bytes (certificate conflict 4). | specification owner |
| OQ-D | The candidate-set bound N, and whether a budgeted Kernel id read is needed given the unbounded `visible_domain_decisions_as_of_in_scope` (plan OQ5-1; certificate conflict 2). | specification owner |
| OQ-E | Whether the TS plugin hides the `candidates` action from the tool schema when the daemon flag is off (certificate conflict 3). | specification owner |

Plan open questions OQ5-2 and OQ5-3 remain open in the plan and are carried
by the certificate unchanged.

## Plan-versus-HEAD conflicts

| Id | Conflict | Owner |
| --- | --- | --- |
| C1 | Summary source: `ClaimFacts` carries ids, admission facts, served standing, occurrences, and causality (`claim_facts.rs:146-156`, `:90-97`) and no payload text; summary text is `DecisionRow.payload.summary` (`slice/read.rs:11-22`) through `decisions_for_objects_as_of_within_budget` (`:236-241`) after `decision_payload_sizes_as_of` (`:330`) or `decision_page_within_budget` (`:126-133`). The plan's `TooManyClaims` bound does not bound the summary read. | specification owner |
| C2 | Unbounded id read: `visible_domain_decisions_as_of_in_scope` has no budget and no row limit (`admission.rs:3087-3098`); "bounded work per pass regardless of project size" needs a caller-side heap plus either a budgeted Kernel variant or acceptance that the SQL walk scales with the project (OQ5-1, OQ-D). | specification owner |
| C3 | "Flag off: no behavior change anywhere" fails at the storage layer: U3 is a memory-store baseline change and the digest rotation closes the MemoryReviewer activation gate until renewed (`docs/memory-reviewer-operations.md:34-43`); the TS tool schema gains `candidates` regardless of the daemon flag unless the plugin hides it (OQ-E). Proposed wording: "no runtime behavior change". | specification owner |
| C4 | Trigger location: the `Published` list and the unit text exist only inside `publish_locked` under the capture commit gate (`source_capture.rs:789-800`, `:826-840`); "on publish settlement" needs new plumbing to carry (evidence id, text or handle) past the gate, or a Kernel read-back; U2's file list omits it. | specification owner |
| C5 | Line drift: the plan cites `lib.rs:9145-9158` for `AfterResponse`; at HEAD it is `:9157-9176`. The plan cites `crates/daemon/Cargo.toml:41` for the retrieval dependency; context records `:45`. | specification owner |
| C6 | Eligibility predicate: the plan says "live" for candidates and "superseded, contradicted, or retired" for the sweep; `Disposition` also has `Stale`, `Disputed`, `Rejected`, `Quarantined` (`admission.rs:97-105`). One shared predicate is required (OQ-B). | specification owner |
| C7 | Dispositions do not retire a decision object; retirement is `invalidated_commit_seq` on the registry row (`schema.rs:93`). "Retired" and "superseded" are distinct Kernel facts and the sweep must check both. | specification owner |

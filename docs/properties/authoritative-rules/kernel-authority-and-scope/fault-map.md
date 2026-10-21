# Fault map: Kernel admission and scope eligibility (S1, S2)

HEAD `08de9051a`. Every record is `always`; a non-vacuous run needs the
enabling state below to occur, witnessed by precondition markers that fire
on a correct implementation.

## Fault classes and availability

| Class | Description | Available today | How constructed |
| --- | --- | --- | --- |
| F1 wrong-kind subject | `AcceptedAdr` or `ConfigObserved` against a non-decision object, a decision of another kind, or a missing id | yes (store API) | `insert_domain` or `insert_decision` with kind `memory`, then `record_admission` |
| F2 off-lineage trigger | observation on another `source_id`, another `source_revision`, wrong `observation_kind` | yes (store API) | `insert_observation` with a skewed lineage; `stage_with_observation` helper in `kernel_admission.rs:1240` |
| F3 dead trigger | observation invalidated, or its backing evidence invalidated, between write and admission | yes (store API plus direct SQL in tests) | `an_invalidated_observation_cannot_authorize` pattern |
| F4 sensitivity tightening | trigger, cited evidence, or registry row at `Sensitive` or `Secret` | yes (ingest with asserted class; re-ingest replay) | `ingest_evidence(.., Sensitive)` helper (`kernel_admission.rs:92`) |
| F5 authority withdrawal | `revoke_approval`, `retire_decision`, `supersede_decision`, restrictive `Disposition` on an approval with dependents | yes (store API; route for retire, supersede, disposition) | `seed_approval` then a dependent admitted with `approval_object_id` |
| F6 cascade overflow | more than 4,096 transitive dependents, or a dependent on another `policy_revision` | partial - policy revision via direct SQL (`revocation_survives_a_dependent_on_a_superseded_policy_revision`); the 4,096 cap needs a generated graph | bulk seeding in a test; no harness exists |
| F7 malformed succession | spec with changed domain, source kind, source id; revision not advanced; object id equal to predecessor; barred predecessor or survivor | yes (store API and route) | one field varied per case |
| F8 class forgery at the route | `source_kind` outside the four kinds; asserted class above derived | yes (route request body) | `a_plugin_route_cannot_declare_a_class_above_the_derived_one` pattern |
| F9 model-mediated change to an explicit-user subject | route `retire_decision`, `supersede_decision`, `disposition` on a decision admitted at `explicit_user` | yes (route after `seed_approval`) | `seed_approval` then a route commit |
| F10 scope variation | stored scopes with extra terms (`entity`, `branch`, `environment`), set terms, redacted values, no project term, no scope | yes (store API `insert_scope`; `ScopeTermSpec`) | a test-written `ScopeSpec`; no production writer |
| F11 context variation | `ScopeMatchContext` with and without values per dimension | yes (pure value) | `ScopeMatchContext::new().with_value(..)` |
| F12 policy text drift | any edit inside a `policy-digest` section | yes (source edit) | the pin test fails on the edited source |

No crash, partition, or clock fault applies within one envelope: every
mechanism here runs in one SQLite transaction (assumption: the envelope
commit path serializes writers; the N2 pass verifies the lock and cites it
in `repo-policy-cached-head-oid-is-advisory-and-reconverges`). A crash
mid-cascade rolls the whole transaction back; the kernel's envelope
atomicity catalogs own that.

## Per-record required faults and enabling state

| Record | Required faults | Enabling state | Coverage check to add (precondition only) |
| --- | --- | --- | --- |
| `explicit-user-accepted-adr-self-approves-to-approved` | none (positive path) | a live `adr_accepted` decision at `Normal`; `AcceptedAdr` at explicit-user classes | `sometimes(accepted_adr_recorded_on_live_accepted_decision)`: an `AcceptedAdr` request whose subject satisfied `APPROVAL_OBJECT_PREDICATE` was evaluated |
| `accepted-adr-requires-live-accepted-decision-subject` | F1 | the three negative subjects | `sometimes(accepted_adr_requested_against_non_accepted_subject)`: an `AcceptedAdr` request reached `prepare_with_facts` with `subject_is_accepted_decision == false` |
| `config-observed-promotes-only-on-exact-lineage-trigger` | F2, F3, F4 | a `config_present` observation on the subject lineage, plus one of each negative control | `sometimes(config_observed_with_live_on_lineage_trigger)` and `sometimes(config_observed_with_trigger_that_failed_validation)`: both branches of `validate_trigger` taken |
| `sensitive-decision-is-hidden-on-automatic-surfaces-and-grants-no-authority` | F4 | a self-approved `adr_accepted` decision whose served class is `Sensitive`; an `AutoInject` read; a dependent citing it | `sometimes(auto_inject_read_saw_an_approved_sensitive_row)`: `surface_visibility` evaluated with `Approved`/`Automatic` and `Sensitive` on `AutoInject` |
| `authority-loss-cascades-to-latest-dependents` | F5; F6 for the deferred branch | an approval with at least one elevated dependent, and a dependent approval with its own dependent for transitivity | `sometimes(authority_withdrawn_with_nonempty_dependents)`: `demote_authority_dependents` entered with `load_approval_dependents` non-empty; `sometimes(cascade_deferred_a_dependent)` for F6 |
| `decision-succession-keeps-lineage-advances-revision-and-forbids-identity-reuse` | F7 | a live predecessor; specs varying one field; a `Rejected` predecessor (currently untested) | `sometimes(supersede_attempted_with_rejected_predecessor)` and `sometimes(supersede_attempted_with_predecessor_object_id)` |
| `explicit-reject-moves-to-rejected-review-only-and-bars-succession` | none for the transition; F7 for the follow-up supersede; F5 when the subject is an approval | a live decision; an `explicit_reject` disposition; a follow-up supersede and relaxation | `sometimes(explicit_reject_admitted_on_live_decision)`; `sometimes(relaxation_requested_from_rejected_without_approval)` |
| `plugin-commit-classes-are-closed-at-model-inference` | F8 | requests with the four kinds, `user_directive`, `repository_policy`, unknown; asserted higher classes | `sometimes(route_request_named_unknown_source_kind)`; `sometimes(route_request_asserted_class_above_derived)` |
| `policy-digest-pins-revision-one` | F12 (mutation) | a kernel test build | none beyond the test itself; a mutation-testing run over the marked sections is the coverage evidence |
| `scope-filter-excludes-a-term-on-an-unresolved-dimension` | F10, F11 | stored scopes with extra terms; contexts lacking the dimension | `sometimes(scope_filter_judged_a_scope_with_a_non_project_term)`: `names_project` saw a canonical scope with `present` bits beyond `Project` |
| `sql-project-prefilter-is-a-superset-of-the-daemon-verdict` | F10 | scopes with foreign, absent, redacted, and range `project` terms in one store | `sometimes(prefilter_kept_a_row_the_filter_then_dropped)`: a row passed SQL and failed `ScopeFilter::matches`; `sometimes(prefilter_dropped_a_row)` |
| `served-rows-exclude-objects-invalidated-at-the-snapshot` | none (snapshot boundary) | one Visible decision retired and one superseded in a single envelope; reads pinned before and at the commit | `sometimes(read_pinned_before_invalidating_commit)`: a served read whose snapshot precedes a later invalidation of one of its rows; `sometimes(read_pinned_at_invalidating_commit)` |

Markers are named constants; none is the negation of its record's
invariant. `sometimes(prefilter_kept_a_row_the_filter_then_dropped)` fires
on a correct system (a scope with no project term is kept by SQL and
refused by the filter), so it is a precondition, not the violation.

F9 belongs to no record here: it is the enabling state for the N1 pass's
explicit-user subject guard, and at HEAD the route accepts it (lead L4 in
`catalog.md`).

## Ranking by cheapest valid oracle

1. `policy-digest-pins-revision-one`: already executable; zero new work. The
   oracle is the existing test; a mutation run over the marked sections adds
   situation evidence.
2. `plugin-commit-classes-are-closed-at-model-inference`: pure functions plus
   one route request; add the `user_directive` and `repository_policy`
   negative cases to the existing exhaustive unit test.
3. `scope-filter-excludes-a-term-on-an-unresolved-dimension`: pure
   `names_project` over constructed `ScopeTermSpec` lists; extend
   `a_scope_names_a_project_only_through_an_exact_project_term` with `entity`
   exact and set terms. A property test over `context_strategy`
   (`kernel_scope_algebra.rs:586`) covers F11.
4. `accepted-adr-requires-live-accepted-decision-subject` and
   `explicit-user-accepted-adr-self-approves-to-approved`: one store test
   with the `seed_approval` sequence and a `read_visible` on `AutoInject`;
   add the missing-object and other-kind subjects.
5. `config-observed-promotes-only-on-exact-lineage-trigger`: one store test
   on a decision-object subject (the existing tests use staged candidates);
   the negative controls already exist as patterns.
6. `explicit-reject-moves-to-rejected-review-only-and-bars-succession`: one
   route test adding `supersede_decision` after `explicit_reject`.
7. `decision-succession-keeps-lineage-advances-revision-and-forbids-identity-reuse`:
   add the `Rejected` predecessor and the same-object-id cases to
   `kernel_slice.rs`.
8. `sensitive-decision-is-hidden-on-automatic-surfaces-and-grants-no-authority`:
   one store test ingesting a `Sensitive` artifact as the decision's cited
   evidence, then `read_visible` on `AutoInject` and a dependent admission.
9. `authority-loss-cascades-to-latest-dependents`: the four entry points
   exist as tests except supersession of an approval with dependents; add
   that case. The F6 cap needs a generated graph of 4,097 dependents, which
   is the only expensive construction in this map.
10. `sql-project-prefilter-is-a-superset-of-the-daemon-verdict`: needs a
    test that runs `visible_as_of_in_scope` and `read_visible` over one
    store seeded with every scope shape and compares id sets; moderate cost
    because the scope shapes must be written through `insert_scope`.
11. `served-rows-exclude-objects-invalidated-at-the-snapshot`: one store,
    one envelope, two pinned reads; the cheapest oracle is the row set of
    `read_visible` compared across the two snapshots.

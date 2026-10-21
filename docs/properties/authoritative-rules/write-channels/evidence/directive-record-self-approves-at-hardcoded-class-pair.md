# directive-record-self-approves-at-hardcoded-class-pair

## Discovery trigger

Plan requirement R1 and KTD1: a user-invoked command records a directive as a
Kernel decision with `decision_kind = adr_accepted`, `source_kind =
user_directive`, classes `(explicit_user, user_explicit)`, event
`AcceptedAdr`, so the policy self-approves it and it is Visible on AutoInject.
Also R4 (policy unchanged) and KTD6 (render by `source_kind`). Acceptance
example AE1.

## Evidence trail

Verified at HEAD `08de9051a`:

- `crates/kernel/src/admission.rs:4221-4234` `admission_ceiling`: for
  `AcceptedAdr`, when `subject_is_accepted_decision` and the automatic ceiling
  is at least Verified, the ceiling is Approved. The pair
  `(ExplicitUser, UserExplicit)` has automatic ceiling Verified
  (`admission.rs:4197-4206`), so the event reaches Approved without an
  approval object.
- `admission.rs:264-385` `evaluate_admission`: `authority_holds` falls back to
  `subject_is_accepted_decision` when no approval is cited (`:315-318`);
  `requires_approval` is false for `AcceptedAdr` at or below the admission
  ceiling (`:333-339`).
- `admission.rs:2436-2458` `subject_is_accepted_decision` queries a live
  `adr_accepted` decision through `APPROVAL_OBJECT_PREDICATE`
  (`admission.rs:710-714`). The decision must therefore exist and be live in
  the same transaction before `record_admission` runs.
- `admission.rs:843-856` `approval_row_fields` requires
  `sensitivity_class='normal'`, `visibility='automatic'`, `disposition=
  'active'`, classes `explicit_user/user_explicit`; `visibility_row`
  (`admission.rs:4243-4255`) yields `Automatic` only for Active rows at or
  above Verified; `surface_visibility` (`admission.rs:4116-4138`) hides
  Sensitive on AutoInject.
- `admission.rs:1566-1580`: an `AcceptedAdr` request does not inherit a
  stored approval, so a self-approving record never depends on an ancestor.
- `crates/daemon/tests/kernel_routes.rs:4355-4386` `seed_approval` performs
  insert then `AcceptedAdr` at exactly these classes through the store API;
  the plan cites it as `:4356-4384`, which is within the function.
- `crates/daemon/src/kernel_routes/commit.rs:271-312` `resolve_classes`
  refuses an asserted class above the derived one; the directive route is
  proposed to refuse any asserted class field outright.

Plan-only (unverifiable at HEAD): the route name, body shape, the lineage
format `directive:<uuid>`, the handler constant.

## Failure scenario

Legal witness: user runs the command with text T; the daemon commits one
envelope: `insert_decision(adr_accepted, user_directive, directive:u, rev 1,
Normal, scope project:<digest>, summary T)` then `record_admission(
AcceptedAdr, (ExplicitUser, UserExplicit), subject = the new object)`. The
admission row reads Approved/Active/automatic/normal; a `kernel.read` on
AutoInject serves it.

Smallest violating traces:

1. Order inverted: `record_admission` before `insert_decision` in the
   envelope. `subject_is_accepted_decision` is false, `load_subject_facts`
   returns `NotFound` for the unregistered id; the commit fails. If the
   handler swallowed the error and inserted the decision anyway, a live
   `adr_accepted` row without an admission row would exist: Hidden, and a
   later `AcceptedAdr` from any route would self-approve it.
2. Sensitivity Sensitive on the decision spec: the admission row stays
   Approved but `approval_row_fields` and `surface_visibility` hide it; the
   directive is recorded but never injected and never grants authority.
3. A body with `asserted_source_class: "explicit_user"` accepted by a handler
   that forwards it: the class pair stops being a route constant and becomes
   caller data.

## Timing windows and dependencies

Within one envelope; the same-transaction prior cache (`admission.rs:2184-
2200`) makes the just-inserted decision visible to the admission. No window
across commits exists if both writes share the closure. Depends on the
project scope row existing (`ensure_scope`, `commit.rs:493-520`).

## What a test must construct

- A bound project and a `directive.record` request with text T, an
  `operation_key`, and no asserted class fields.
- Assertions: one new live `decisions` row with `decision_kind='adr_accepted'`,
  `source_kind='user_directive'`, `source_revision=1`,
  `sensitivity_class='normal'`; one `admission_decisions` row for it with
  `event_kind='accepted_adr'`, `source_class='explicit_user'`,
  `taint_class='user_explicit'`, `effective_maturity='approved'`,
  `visibility='automatic'`, `commit_seq` equal to the object's
  `created_commit_seq`; a `kernel.read` on `auto_inject` includes it.
- Negative: the same body with `asserted_taint_class` yields `invalid_params`
  and no new rows.
- Pin: `crates/kernel/src/admission.rs:5734` `policy_revision_matches_policy_content`
  still passes (no table change).

## Investigation log

### Q: Does the plan's "Approved" claim hold with no approval object?
- Sources examined: `admission.rs:264-385, 4221-4234, 2436-2458`.
- Findings: yes, through `admission_ceiling` and `authority_holds` fallback.
- Missing evidence: none for the Kernel half.
- Conclusion: resolved with answer: verified.

### Q: Does the route need `deny_unknown_fields` or an explicit refusal?
- Sources examined: `commit.rs:960-1000` (`kernel_request` decode).
- Findings: the commit body uses serde structs; `IntentRequest` has
  `deny_unknown_fields` (`project.rs:140-147`).
- Missing evidence: the directive body type does not exist.
- Conclusion: unresolved, needs the U3 body definition (resolvable at
  implementation).

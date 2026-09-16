# RP2.7 application properties

## Scope and provenance

System: `/local/home/ahrav/scratch/eidnara`.
Base: the `rp27/u3c-dense-lane` branch head at `f318c4a4`, itself on
`main` at `8e0491225a7292ef077c675d44b94f94a24041d3` through the RP2.7 stack.
Method: `../../METHOD.md` and `property-discovery-and-catalog`.

Source: the RP2.7 specification
([#630](https://github.com/ahrav/eidnara/issues/630)) and its local companion
bundle, whose application catalog proposed these slugs as unexercised
`test-only` obligations. The RP2.7.U4 ticket
([#643](https://github.com/ahrav/eidnara/issues/643)) lands the preparation
and receipt lifecycle records; the RP2.7.U5 ticket
([#644](https://github.com/ahrav/eidnara/issues/644)) lands the capability
gate records.

This part owns `crates/daemon/src/edit_receipts.rs`: the bounded in-memory
receipt store behind `retrieval.prepare`, `retrieval.apply`, and
`retrieval.confirm`, whose wire shape is Section 7.8 of
`docs/host-wire-protocol.md` (context-application protocol 2).

Parent decisions recorded here. Q7: a deliberate second application of one
selection is a legal intent, so the tuple (daemon incarnation, context
revision, action, selection digest, accounting profile) is a length-delimited
fingerprint and the identity minted at preparation is the key. Q8: the
incarnation signal is a random value minted whenever a receipt limit set is
installed over no store, so a restart or an uninstall retires it, and every
preparation identity carries it as its prefix; a key of another incarnation is
`unknown` and forwards nothing until a confirm with the exact applied identity
reclassifies it; a duplicate while an apply is in flight answers `in_flight`
with the recorded forwarded identity and forwards nothing. Q9: retention has a
count bound and a time bound enforced separately, the time bound must be at
least `RETENTION_FLOOR`, twice the wire's 30 s request deadline, and an evicted
or expired key is refused as `receipt_unavailable`; read-back is a confirm
whose `applied_identity` equals the forwarded identity the daemon recorded, and
for a key of another incarnation, where no record survives, a confirm whose
`preparation_id` has the minted shape and whose lowercase well-formed
forwarded and applied identities agree, recorded so the key does not fall back
to `unknown`; a read-back the project cannot record is `receipt_unavailable`
and records nothing. Receipts are keyed by the route's bound project, so a key
is honored only on a route of the project that prepared it, and the count
bound is per project: it is enforced when a key is minted or a read-back is
recorded, the oldest settled receipts make room in one pass, and a project of
in-flight or unknown receipts refuses the new preparation with
`preparation_failure`/`receipt_capacity`, so a read never evicts, one project
never evicts another's receipts, and an edit that may already be applied is
never dropped. `max_keys` has a fixed ceiling, `MAX_KEYS_CEILING`, refused at
installation. A limits change keeps the receipts.

Parent decisions for the gate recorded here. RP2.8 Q7: the capability answer
is not wire-visible except as the `capability_unsupported` and
`capability_undeclared` terminals on a gated `retrieval.prepare`,
`retrieval.apply`, or `retrieval.confirm`; Append is not a gated class. Q10: suppression is
whole-message, so a survivor confirmed only for a span is not a confirmed
survivor and the recipe carrier for empty replacement is unchanged
(`replace` with `edit_bytes` zero); cross-step reuse is a declared class
(`reuse`) with the replacement capacity bound and no harness declares it; an
accounting profile is part of the preparation fingerprint, so a changed
profile is a new intent rather than a mutation of an existing one. Q11: the Pi
set is empty until Pi registers a context hook and a revision token; a
suppression decided before the plugin's response is not accepted, because the
adapter must supply the surviving set on the prepare itself; chained-extension
visibility belongs to the harness adapter that assembles the invocation
(RP2.8.U5).

## Reachability and observation contract

Every record here is `test-only`: the routes answer `disabled` until
`Handler::set_edit_receipt_limits` installs an approved set, and no production
caller installs one yet. Observation points: `ReceiptStore::{prepare, apply,
confirm}` and the three handlers in `crates/daemon/src/edit_receipts.rs`. The
witnesses drive a `KernelDaemon` through `dispatch_value_for_test` in
`crates/daemon/tests/edit_receipts.rs`, with an independent edit log of every
`forwarded` answer as the effect oracle, and a second daemon as the restart.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [apply-selection-preparation-application-are-distinct-states](#apply-selection-preparation-application-are-distinct-states) | safety | test-only | always | active | high |
| [apply-idempotency-key-binds-tuple-and-dedups-by-digest](#apply-idempotency-key-binds-tuple-and-dedups-by-digest) | safety | test-only | always | active | high |
| [apply-stale-preparation-is-rejected-before-edit](#apply-stale-preparation-is-rejected-before-edit) | safety | test-only | always | active | high |
| [apply-unknown-outcome-never-replays-blindly](#apply-unknown-outcome-never-replays-blindly) | safety | test-only | always | active | high |
| [apply-daemon-receipt-does-not-mark-harness-edit-applied](#apply-daemon-receipt-does-not-mark-harness-edit-applied) | safety | test-only | always | active | high |
| [apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay](#apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay) | safety | test-only | always | active | medium |
| [apply-receipt-belongs-to-the-project-that-prepared-it](#apply-receipt-belongs-to-the-project-that-prepared-it) | safety | test-only | always | active | high |
| [apply-outcomes-are-distinct-and-empty-replacement-is-applied-replacement](#apply-outcomes-are-distinct-and-empty-replacement-is-applied-replacement) | safety | test-only | always | active | high |
| [apply-append-allowance-and-replacement-capacity-are-bound-before-preparation](#apply-append-allowance-and-replacement-capacity-are-bound-before-preparation) | safety | test-only | always | active | high |
| [apply-healthy-prepared-application-terminates-with-known-outcome](#apply-healthy-prepared-application-terminates-with-known-outcome) | liveness | test-only | always | active | high |
| [apply-context-capabilities-default-closed-per-harness](#apply-context-capabilities-default-closed-per-harness) | safety | test-only | always | active | high |
| [apply-consumer-capability-strings-never-authorize-edits](#apply-consumer-capability-strings-never-authorize-edits) | safety | test-only | always | active | high |
| [apply-suppression-requires-confirmed-surviving-span](#apply-suppression-requires-confirmed-surviving-span) | safety | test-only | always | active | high |
| [apply-adapter-validates-entire-assembled-invocation](#apply-adapter-validates-entire-assembled-invocation) | safety | test-only | always | active | low |
| [apply-enabled-outcomes-are-proven-on-real-harness-paths](#apply-enabled-outcomes-are-proven-on-real-harness-paths) | safety | test-only | always | active | low |

## Records

### apply-selection-preparation-application-are-distinct-states

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `same_key_and_digest_replays_the_known_outcome_with_one_effect`; `crates/daemon/tests/edit_receipts.rs` `a_lost_acknowledgment_is_sticky_unknown_and_a_fenced_confirm_is_a_conflict`.
Guarantee: A selection is not a preparation, a preparation is not a forwarded edit, and a forwarded edit is not an applied one: the receipt moves prepared, in flight, complete or unknown, each observable on the wire, and no step is implied by an earlier one.
Check: `always` - `prepare` answers `prepared`; the first `apply` answers `forwarded`; a duplicate answers `in_flight`; `confirm` with the applied identity answers `complete`; a confirm of a preparation that was never forwarded is `conflict`. `always` because every request reads the receipt's state.
Fault/timing angle: None.
Required faults and enabling state: A `KernelDaemon` with a receipt limit set installed.
Confidence: high - [evidence](evidence/apply-selection-preparation-application-are-distinct-states.md).
Each state is a distinct `kind`/`state` pair on the wire.
Existing check: `crates/daemon/tests/kernel_routes.rs` idempotent `kernel.commit` replay tests, status unaudited.
Impact: A caller could treat a ranking or a preparation as an edit that happened.
Open questions: None.

### apply-idempotency-key-binds-tuple-and-dedups-by-digest

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `same_key_and_digest_replays_the_known_outcome_with_one_effect`; `crates/daemon/src/edit_receipts.rs` `a_changed_digest_against_an_unknown_receipt_is_a_conflict`.
Guarantee: The preparation identity is minted per preparation and its fingerprint is a length-delimited derivation over daemon incarnation, context revision, action, selection digest, and accounting profile; a retry under one identity and one digest returns the recorded state with exactly one forwarded effect, and a retry with another digest is a typed conflict.
Check: `always` - two prepares over one tuple yield two identities and one fingerprint; the second `apply` of one identity answers `in_flight` and the independent edit log holds one effect; an apply with a changed span after the forward is `conflict`, whether the receipt is in flight, complete, or unknown; after the acknowledgment every replay answers `complete` with the same outcome and a second acknowledgment with another outcome is `conflict`. `always` because every apply of an owned key compares the digest.
Fault/timing angle: A duplicate arriving while the apply is in flight.
Required faults and enabling state: Two prepares over one tuple; a changed span after a forward.
Confidence: high - [evidence](evidence/apply-idempotency-key-binds-tuple-and-dedups-by-digest.md).
Parent Q7: a second application of one selection is a legal intent, so the tuple is the fingerprint and the minted identity is the key.
Existing check: `crates/daemon/tests/kernel_routes.rs` idempotent `kernel.commit` replay tests, status unaudited.
Impact: Two intents over one tuple would collide, or a retry would forward a second edit.
Open questions: None.

### apply-stale-preparation-is-rejected-before-edit

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `a_changed_context_between_prepare_and_apply_is_stale_and_forwards_nothing`; `crates/daemon/tests/edit_receipts.rs` `outcomes_are_distinct_and_capacity_is_bound_before_preparation` for the misspelled span key.
Guarantee: An apply whose current context revision, representation, selected spans, or span set differs from the prepared one is refused as `stale_preparation` before anything is forwarded, and the preparation stays usable under its own context; a `spans` item with an unknown field is `invalid_params` rather than a whole-buffer span.
Check: `always` - a changed revision, a changed representation, a changed span end, and a dropped span each answer `stale_preparation` with no effect logged; the same preparation then forwards under its original context; a span item spelled `spn` is `invalid_params`. `always` because the digest is recomputed on every apply and every span item is parsed strictly.
Fault/timing angle: Context changes, including compaction, between prepare and apply.
Required faults and enabling state: A prepared receipt and a differing context body.
Confidence: high - [evidence](evidence/apply-stale-preparation-is-rejected-before-edit.md).
The comparison is the RP2.7.U1 preparation digest, so the stale set is exactly the digest's input set.
Existing check: `crates/daemon/tests/kernel_routes.rs` idempotent `kernel.commit` replay tests, status unaudited.
Impact: An edit prepared against one context would be applied to another.
Open questions: None.

### apply-unknown-outcome-never-replays-blindly

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `a_restart_leaves_forwarded_and_unforwarded_keys_unknown_until_read_back`, `uninstalling_the_limit_set_drops_the_receipts_like_a_restart_so_a_read_back_still_lands`, and `a_lost_acknowledgment_is_sticky_unknown_and_a_fenced_confirm_is_a_conflict`; `crates/daemon/src/edit_receipts.rs` `a_foreign_key_completes_only_on_a_well_formed_matching_read_back_and_stays_complete` and `a_read_back_the_store_cannot_record_is_refused_rather_than_answered_complete`.
Guarantee: After a daemon restart, or after the limit set is uninstalled and reinstalled, every key of the prior incarnation is `unknown`, whether it had been forwarded or only prepared; after a lost acknowledgment the key is `unknown`; `unknown` is sticky, forwards nothing on retry, and is reclassified only by a confirm whose applied identity equals the forwarded identity.
Check: `always` - a forwarded key and a prepared key from a shut-down daemon answer `unknown` on a fresh daemon and again on retry with an empty edit log; a confirm without an applied identity, with another identity, with an uppercase or otherwise malformed one, or for a key without the minted `<incarnation>-<identity>` shape leaves `unknown` and records nothing; a confirm with the exact identity answers `complete`, later applies read `complete`, and another outcome, a missing applied identity, or another applied identity is `conflict`; a read-back over a project whose every receipt is in flight is `receipt_unavailable`, records nothing, and the key stays `unknown`; a forwarded key answers `unknown` after the limit set is uninstalled and reinstalled on one daemon, and its read-back answers `complete` with one effect logged; on one daemon a confirm without an applied identity turns an in-flight receipt `unknown`, the retry forwards nothing, and a read-back naming another forward or another applied identity is `conflict` because the forwarded identity stays recorded through `unknown`. `always` because the incarnation prefix and the state are read on every request.
Fault/timing angle: Daemon restart after forward, restart after prepare, lost acknowledgment.
Required faults and enabling state: A second `KernelDaemon`; a confirm with `applied_identity: null`.
Confidence: high - [evidence](evidence/apply-unknown-outcome-never-replays-blindly.md).
Parent Q8: the incarnation signal is the random value `ReceiptStore` receives when a limit set is installed over no store, and every preparation identity carries it as its prefix; a key of another incarnation is `unknown`.
Existing check: `crates/daemon/tests/kernel_routes.rs` idempotent `kernel.commit` replay tests, status unaudited.
Impact: A retry after a crash would append or replace twice.
Open questions: None.

### apply-daemon-receipt-does-not-mark-harness-edit-applied

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `a_lost_acknowledgment_is_sticky_unknown_and_a_fenced_confirm_is_a_conflict`; `crates/daemon/tests/edit_receipts.rs` `a_restart_leaves_forwarded_and_unforwarded_keys_unknown_until_read_back`.
Guarantee: The daemon records the attempt and the forwarded identity before answering `forwarded`, and marks the receipt complete only on a confirm carrying an applied identity equal to that forwarded identity; a receipt alone, a confirm without an applied identity, or a confirm naming another forward never completes it.
Check: `always` - a confirm for a never-forwarded preparation is `conflict`; a confirm with a forwarded identity other than the recorded one is `conflict`; a confirm without an applied identity is `unknown`; only the exact identity completes. `always` because the acknowledgment is the only path to `complete`.
Fault/timing angle: A stale apply acknowledging after a newer forward.
Required faults and enabling state: A forwarded receipt and confirms with wrong or absent identities.
Confidence: high - [evidence](evidence/apply-daemon-receipt-does-not-mark-harness-edit-applied.md).
The forwarded identity is derived from the preparation identity and digest, so an acknowledgment cannot be forged from the tuple alone.
Existing check: `crates/daemon/tests/kernel_routes.rs` idempotent `kernel.commit` replay tests, status unaudited.
Impact: A daemon-side receipt could claim an edit the harness never applied.
Open questions: None.

### apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `the_count_bound_evicts_the_oldest_settled_key_and_never_an_in_flight_one` and `the_route_is_disabled_until_an_approved_limit_set_is_installed`; `crates/daemon/src/edit_receipts.rs` `a_key_expires_by_its_creation_time_not_by_its_last_use`, `a_full_store_of_in_flight_receipts_refuses_a_new_preparation`, `an_unknown_receipt_is_never_the_victim_because_its_edit_may_be_applied`, `a_refused_request_still_expires_receipts_past_retention`, and `max_keys_is_capped_and_a_narrower_limit_evicts_the_oldest_settled_receipts_at_the_next_mint`.
Guarantee: Receipts are bounded per project by a key count enforced when a key is minted or a read-back is recorded, and by a retention measured from creation, not last use, enforced on every access; a key past either bound is dropped and any later apply or confirm for it is refused as `receipt_unavailable`; an in-flight or unknown receipt is never evicted for count, because its edit may already be applied, so a project of in-flight or unknown receipts refuses the new preparation; a narrower limit evicts the excess oldest settled receipts in one pass at the next mint; a retention shorter than `RETENTION_FLOOR` or a `max_keys` above `MAX_KEYS_CEILING` is refused at installation; no limit set disables the routes.
Check: `always` - with `max_keys = 2` a third prepare evicts the first, whose apply and confirm are `receipt_unavailable`, the survivors still forward, a fourth prepare over two in-flight receipts is `preparation_failure`/`receipt_capacity`, and a wider limit set keeps the receipts; with synthetic instants a key used every two seconds still expires ten seconds after creation; eight receipts narrowed to three keep the in-flight one, the youngest settled one, and the new mint; with `max_keys = 1` a prepare over an unknown receipt is `receipt_capacity` and the receipt still completes on its read-back; an over-capacity prepare and a malformed apply at the retention bound leave the store empty; a retention one millisecond below the floor is refused and the routes stay `disabled`; `MAX_KEYS_CEILING + 1` is refused and `MAX_KEYS_CEILING` accepted. `always` because expiry runs on every access and the count bound on every mint.
Fault/timing angle: Time passing; count overflow.
Required faults and enabling state: Narrow limits installed on a live daemon.
Confidence: medium - [evidence](evidence/apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay.md).
Parent Q9: retention is bounded below by `RETENTION_FLOOR`, twice the wire's fixed 30 s request deadline (`docs/host-wire-protocol.md` Section 11), not the operator's `deadline_ceiling` for `retrieval.query`; the eviction disposition is refusal, not a tombstone, because a dropped key cannot be told from one never minted.
Existing check: `crates/daemon/tests/kernel_routes.rs` idempotent `kernel.commit` replay tests, status unaudited.
Impact: An unbounded store would grow with every preparation; an evicted key that replayed would forward a second edit.
Open questions: None.

### apply-receipt-belongs-to-the-project-that-prepared-it

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `a_receipt_belongs_to_the_project_that_prepared_it`.
Guarantee: A receipt is keyed by the route's bound project: a preparation identity of this incarnation is `receipt_unavailable` on a route bound to another project for apply and confirm, one project's mints never evict another project's receipts, and one project of in-flight receipts never makes another project's prepare fail `receipt_capacity`.
Check: `always` - with `max_keys = 1` and two routes bound to two project roots on one daemon, the key prepared on the first is `receipt_unavailable` on the second for apply and confirm and still forwards on the first; a prepare on the second answers `prepared` while the first project's only receipt is in flight; the first project's receipt then still answers `in_flight`; the edit log holds one effect. `always` because every request looks the key up under the bound project's scope id.
Fault/timing angle: None.
Required faults and enabling state: A second route bound to another project root on one `KernelDaemon`.
Confidence: high - [evidence](evidence/apply-receipt-belongs-to-the-project-that-prepared-it.md).
The scope id is `ProjectBinding::scope_id`, the same per-project key prefix the kernel routes' durable idempotency receipts use.
Existing check: `crates/daemon/tests/kernel_routes.rs` `replayed_intents_return_one_receipt_and_projects_never_collide`, status unaudited.
Impact: A caller bound to one project could consume, reclassify, or evict another project's receipts on the same daemon.
Open questions: None.

### apply-outcomes-are-distinct-and-empty-replacement-is-applied-replacement

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `outcomes_are_distinct_and_capacity_is_bound_before_preparation`.
Guarantee: `keep`, `append`, `applied_replacement`, and `preparation_failure` are the only outcomes a confirm may carry and each is distinct on the wire; `unknown` is a receipt state and is refused as an outcome; an empty replacement is prepared, forwarded with zero bytes, and confirmed as `applied_replacement`.
Check: `always` - four receipts confirmed with the four literals read back through `apply` as exactly those four distinct literals; `applied` and `unknown` are `invalid_params`; a replace with `edit_bytes = 0` forwards and completes as `applied_replacement`. `always` because the outcome enum is closed.
Fault/timing angle: None.
Required faults and enabling state: A live daemon with limits installed.
Confidence: high - [evidence](evidence/apply-outcomes-are-distinct-and-empty-replacement-is-applied-replacement.md).
`Outcome` derives its wire spelling from a closed enum; `Unknown` is not a member.
Existing check: `crates/daemon/tests/kernel_routes.rs` idempotent `kernel.commit` replay tests, status unaudited.
Impact: A harness could report an outcome the daemon cannot classify, or an empty replacement could pass as `keep`.
Open questions: None.

### apply-append-allowance-and-replacement-capacity-are-bound-before-preparation

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `outcomes_are_distinct_and_capacity_is_bound_before_preparation`.
Guarantee: The append allowance and the replacement capacity are checked against `edit_bytes` before any identity is minted or receipt stored; an over-capacity preparation answers `preparation_failure` with the bound it exceeded.
Check: `always` - one byte over the append allowance and one byte over the replacement capacity each answer `preparation_failure` with reasons `append_allowance` and `replacement_capacity`, and no key exists for them. `always` because the check precedes minting.
Fault/timing angle: None.
Required faults and enabling state: Limits with small allowances.
Confidence: high - [evidence](evidence/apply-append-allowance-and-replacement-capacity-are-bound-before-preparation.md).
The accounting arithmetic is RP2.8; this route binds the two totals it is given.
Existing check: `crates/daemon/tests/kernel_routes.rs` idempotent `kernel.commit` replay tests, status unaudited.
Impact: An oversized edit would be prepared and forwarded before the harness could refuse it.
Open questions: None.

### apply-healthy-prepared-application-terminates-with-known-outcome

Type: liveness
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `same_key_and_digest_replays_the_known_outcome_with_one_effect`; `crates/daemon/tests/edit_receipts.rs` `outcomes_are_distinct_and_capacity_is_bound_before_preparation`.
Guarantee: A prepare, apply, and confirm under one unchanged context and one acknowledgment ends in a `complete` receipt whose outcome every later apply and confirm returns.
Check: `always` - the healthy sequence answers `prepared`, `forwarded`, `complete`; replays of apply and confirm answer the same `complete` outcome. `always` because every healthy request must terminate.
Fault/timing angle: None.
Required faults and enabling state: A live daemon with limits installed.
Confidence: high - [evidence](evidence/apply-healthy-prepared-application-terminates-with-known-outcome.md).
An always-refusing store would pass every safety record and fail this one.
Existing check: `crates/daemon/tests/kernel_routes.rs` idempotent `kernel.commit` replay tests, status unaudited.
Impact: A route that never completes would leave every edit `in_flight`.
Open questions: None.

### apply-context-capabilities-default-closed-per-harness

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/context_capabilities.rs`
`a_backend_that_overrides_nothing_declares_no_class_and_the_harness_tables_are_recorded`,
`without_a_declaration_every_gated_class_is_denied_as_unreadable_and_append_still_works`,
`an_unavailable_backend_is_an_unreadable_declaration_not_a_closed_one`,
`the_real_harness_tables_allow_exactly_the_recorded_classes`, and
`the_declaration_is_latched_at_bind_and_reread_by_a_new_bind`.
Guarantee: `LlmExecutionBackend::context_capabilities` defaults to the empty
set; the OpenCode declaration allows exactly suppression and replacement and
the Pi declaration is empty; the declaration is read once at route bind,
held for the route epoch, and re-read by a new bind; a class the latched
declaration does not allow is `capability_unsupported`, a declaration that
could not be read is `capability_undeclared` with its reason, and a backend
whose `unavailable_reason` for the harness is set is unreadable with that
reason rather than closed; both fail closed
before any capacity check or minted identity, and the receipt's class is gated
again on apply and confirm so a route whose declaration is closed cannot drive
a receipt another route prepared; append is never gated.
Check: `always` - a backend that overrides nothing denies every class for
both harnesses; a backend whose `unavailable_reason` is `descriptor_absent`
latches `Unreadable("descriptor_absent")` for both harnesses; a daemon
without a source denies every class as
`no_declaration` and still prepares an append; under the recorded tables an
`opencode` route prepares `replace` and `suppress` and is denied `reuse`
while a `pi` route is denied all three and still prepares an append; an
oversized replace on an undeclared route is denied by the gate, not the
capacity; a source whose answer changes after bind leaves the bound route
denied while a second route bound afterwards on the same daemon prepares, and
the first route can neither apply nor confirm the second's preparation while
the second applies and confirms it. `always` because every
gated request reads the latched declaration.
Fault/timing angle: The backend answer changes during a route epoch.
Required faults and enabling state: A mutable capability source; the recorded
harness tables; a daemon with no source.
Confidence: high - [evidence](evidence/apply-context-capabilities-default-closed-per-harness.md).
The trait's other defaulted method, `unavailable_reason`, defaults open; the
new method's documentation says why this one defaults closed.
Existing check: `crates/host-runtime/tests/model_execution_protocol.rs`
`unavailable_reason` override tests, status unaudited.
Impact: A harness adapter that cannot construct or account for an edit could
be offered one.
Open questions: None.

### apply-consumer-capability-strings-never-authorize-edits

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/context_capabilities.rs`
`the_real_harness_tables_allow_exactly_the_recorded_classes`.
Guarantee: The consumer capability strings a route carries at bind are not
read by any authorization path; a consumer advertising `replacement` and
`suppression` is denied when the host declaration disallows the class, and no
probe, response field, or third channel exposes or widens the declaration.
Check: `always` - a `pi` route bound with consumer strings advertising
`replacement` and `suppression` is denied both; `git grep
consumer_capabilities crates/daemon/src` finds only the bind identity's field,
never a read. `always` because the gate reads the latched declaration alone.
Fault/timing angle: None.
Required faults and enabling state: A route bound with advertising consumer
strings under a closed declaration.
Confidence: high - [evidence](evidence/apply-consumer-capability-strings-never-authorize-edits.md).
Existing check: `crates/host-runtime/src/handler.rs` documents the identity
fields as unverified claims, status unaudited.
Impact: A plugin could claim a class and receive an edit the host cannot
account for.
Open questions: None.

### apply-suppression-requires-confirmed-surviving-span

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `crates/daemon/tests/context_capabilities.rs`
`suppression_needs_whole_message_survivor_proof_for_every_selected_occurrence`.
Guarantee: A suppression is prepared only when the adapter supplies a
surviving set that confirms every selected occurrence whole; an empty set,
a selected occurrence absent from it, or a survivor confirmed only for a
span each fail as `preparation_failure` with a distinct reason and mint
nothing.
Check: `always` - no survivors is `no_survivor_proof`; one of two selected
occurrences confirmed is `unconfirmed_survivor`, as is a survivor whose buffer
length disagrees with the context's span for the same occurrence; a survivor
with a partial span is `span_granularity`; a survivor that is not an
occurrence identifier is `malformed_survivor`, as is a survivors set carrying
two entries for one occurrence, in either order; a selected occurrence absent
from the context's own spans is `selection_not_in_spans`; both confirmed
whole, with a null span or a span covering the context's buffer, prepares.
`always` because the check runs on every suppression.
Fault/timing angle: Partial visibility or a replaced slot between the plugin's
observation and the prepare.
Required faults and enabling state: A harness declaring suppression; survivor
sets of each shape.
Confidence: high - [evidence](evidence/apply-suppression-requires-confirmed-surviving-span.md).
Parent Q10 is recorded as whole-message granularity; the span-level case is
refused rather than admitted.
Existing check: None found.
Impact: A span the harness no longer shows could be suppressed as though it
were visible.
Open questions: None.

### apply-adapter-validates-entire-assembled-invocation

Type: safety
Reachability: test-only
Status: active
Exercised: partial - the daemon's boundary is the capacity check in
`crates/daemon/tests/edit_receipts.rs`
`outcomes_are_distinct_and_capacity_is_bound_before_preparation`; the
whole-invocation validation with accounting headroom is RP2.8.U5's adapter
and has no witness here.
Guarantee: The daemon binds append allowance and replacement capacity before
preparation and the harness adapter validates the entire assembled invocation
with accounting headroom before it applies; a payload that fits alone but not
in the invocation is refused by the adapter, never applied.
Check: `always` - the daemon half is the capacity witness; the adapter half is
unimplemented. `always` because every application crosses both.
Fault/timing angle: None.
Required faults and enabling state: The RP2.8.U5 adapter.
Confidence: low - [evidence](evidence/apply-adapter-validates-entire-assembled-invocation.md).
Existing check: None found.
Impact: An edit that fits its own bound could overflow the invocation.
Open questions:

- The adapter-side validation lands with RP2.8.U5. (needs human input)

### apply-enabled-outcomes-are-proven-on-real-harness-paths

Type: safety
Reachability: test-only
Status: active
Exercised: partial - `crates/daemon/tests/context_capabilities.rs`
`the_real_harness_tables_allow_exactly_the_recorded_classes` drives the
recorded OpenCode and Pi declarations through bound routes; the enabled
outcomes with plugin-supplied applied identity on a running harness wait for
RP2.8.U5.
Guarantee: On the OpenCode path each enabled class has a witnessed outcome
carrying plugin-supplied applied identity; on the Pi path every gated class is
denied and pure packing still works; a plugin build without the transform
hook never produces an applied outcome.
Check: `always` - the Pi denials and the pure-packing append are witnessed
through a bound `pi` route; the OpenCode allowed set is witnessed at the gate;
the applied-identity witnesses are not yet possible without the harness-side
apply. `always` because every enablement claim needs its witness.
Fault/timing angle: None.
Required faults and enabling state: A running harness with the RP2.8.U5 apply.
Confidence: low - [evidence](evidence/apply-enabled-outcomes-are-proven-on-real-harness-paths.md).
Existing check: `crates/daemon/tests/model_execution_roundtrip.rs` real harness
subprocess runs, status unaudited.
Impact: A class could be declared enabled without a harness ever proving it.
Open questions:

- The applied-identity witnesses land with RP2.8.U5. (needs human input)

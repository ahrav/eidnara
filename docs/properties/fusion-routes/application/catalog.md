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
revision, action, selection digest, accounting profile identity and revision)
is a length-delimited fingerprint and the identity minted at preparation is
the key. Q5: the daemon owns the accounting profile; it binds its current
profile into the preparation digest and the fingerprint, answers it on the
prepared reply, and an apply must echo it, refusing `profile_mismatch` before
the digest comparison and `profile_unavailable` when the daemon holds none.
Q7 (capability visibility): the answer stays off the wire and `append` is
never gated; the plugin treats a capability terminal as a denial for the rest
of the route epoch. Q8: the
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
installation. A limits change keeps the receipts under the new bounds.

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
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `a_changed_context_between_prepare_and_apply_is_stale_and_forwards_nothing`; `crates/daemon/tests/edit_receipts.rs` `outcomes_are_distinct_and_capacity_is_bound_before_preparation` for the misspelled span key; `crates/daemon/tests/edit_receipts.rs` `the_daemon_binds_its_profile_at_prepare_and_an_apply_must_echo_it_exactly` for the accounting profile.
Guarantee: An apply whose current context revision, representation, selected spans, or span set differs from the prepared one is refused as `stale_preparation` before anything is forwarded, and the preparation stays usable under its own context; an apply whose echoed accounting profile is not the daemon's is refused as `profile_mismatch`, one against a daemon holding no profile as `profile_unavailable`, and a malformed profile is `invalid_params`; a `spans` item with an unknown field is `invalid_params` rather than a whole-buffer span.
Check: `always` - a changed revision, a changed representation, a changed span end, and a dropped span each answer `stale_preparation` with no effect logged; the same preparation then forwards under its original context; the prepared answer carries the exact tokenizer's identity and revision, an echo with another revision or another identity answers `profile_mismatch`, a one-member object or a bare string is `invalid_params`, and after the profile is withdrawn both a new prepare and an apply of a pending receipt answer `profile_unavailable`, all with no effect logged; a span item spelled `spn` or one that omits `span` is `invalid_params`, and so is an unknown top-level field such as `selections` on apply and prepare, although the context is flattened into the body. `always` because the digest, which covers the profile, is recomputed on every apply and every span item is parsed strictly.
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
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `a_restart_leaves_forwarded_and_unforwarded_keys_unknown_until_read_back`, `uninstalling_the_limit_set_drops_the_receipts_like_a_restart_so_a_read_back_still_lands`, `a_lost_acknowledgment_is_sticky_unknown_and_a_fenced_confirm_is_a_conflict`, and `every_application_outcome_leaves_the_kernel_tip_and_write_counters_unchanged`, which reads `kernel::write_observer` before and after disable, failure, stale refusal, lost acknowledgment, decline, and uninstall and then commits one packing-caused control write; `crates/daemon/src/edit_receipts.rs` `a_foreign_key_completes_only_on_a_well_formed_matching_read_back_and_stays_complete` and `a_read_back_the_store_cannot_record_is_refused_rather_than_answered_complete`.
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
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `a_lost_acknowledgment_is_sticky_unknown_and_a_fenced_confirm_is_a_conflict`; `crates/daemon/tests/edit_receipts.rs` `a_restart_leaves_forwarded_and_unforwarded_keys_unknown_until_read_back`; `packages/opencode-plugin/src/hooks/context/context-application.test.ts` `never reports applied when the acknowledgment is lost, whatever the daemon answers`, `never reports applied from a daemon receipt alone`, `reports unknown, not an error, when the confirm cannot reach the daemon after publication`, and `reports unknown, never refused, when the daemon answers the confirm with a terminal after publication`.
Guarantee: The daemon records the attempt and the forwarded identity before answering `forwarded`, and marks the receipt complete only on a confirm carrying an applied identity equal to that forwarded identity; a receipt alone, a confirm without an applied identity, or a confirm naming another forward never completes it. On the plugin side, `refused` is answered only before publication; once `publish` has run, a confirm the daemon refuses (`receipt_unavailable`, `conflict`, `disabled`, or a capability terminal) or cannot receive is `unknown` carrying the preparation, forwarded, and applied identities and the daemon's terminal, because the host may hold the edit whatever the daemon says.
Check: `always` - a confirm for a never-forwarded preparation is `conflict`; a confirm with a forwarded identity other than the recorded one is `conflict`; a confirm without an applied identity is `unknown`; only the exact identity completes; the plugin publishes exactly once and answers `unknown` with the identities and the terminal for each of `receipt_unavailable`, `conflict`, and `disabled` on confirm. `always` because the acknowledgment is the only path to `complete` and the plugin's result kinds are decided after publication by whether the daemon confirmed the applied identity, never by the refusal shape.
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
Check: `always` - four receipts confirmed with the four literals read back through `apply` as exactly those four distinct literals; `applied` and `unknown` are `invalid_params`, and so is a confirm that omits `applied_identity`, which leaves the receipt in flight rather than turning it `unknown`; a replace with `edit_bytes = 0` forwards and completes as `applied_replacement`. `always` because the outcome enum is closed.
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
`the_production_source_reads_each_harness_once_and_every_bind_is_a_lookup`,
`the_real_harness_tables_allow_exactly_the_recorded_classes`, and
`the_declaration_is_latched_at_bind_and_reread_by_a_new_bind`.
Guarantee: `LlmExecutionBackend::context_capabilities` defaults to the empty
set; the OpenCode declaration allows exactly suppression and replacement and
the Pi declaration is empty; the production source reads each harness's
availability and declaration from the backend once at startup and a bind is
a lookup; the declaration is latched once at route bind,
held for the route epoch, and re-read from the source by a new bind; a class the latched
declaration does not allow is `capability_unsupported`, a declaration that
could not be read is `capability_undeclared` with its reason, and a backend
whose `unavailable_reason` for the harness is set is unreadable with that
reason rather than closed; both fail closed
before any capacity check or minted identity, and the receipt's class is gated
again on apply and confirm so a route whose declaration is closed cannot drive
a receipt another route prepared; append is never gated.
Check: `always` - a backend that overrides nothing denies every class for
both harnesses; a backend whose `unavailable_reason` is `descriptor_absent`
latches `Unreadable("descriptor_absent")` for both harnesses; the production
source over a counting backend answers three rounds of binds without a
further backend read; a daemon
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
Reachability: default-production - the OpenCode plugin validates every
candidate surface before `replaceHostArrayContents` in
`packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts`.
Status: active
Exercised: yes - the daemon's boundary is the capacity check in
`crates/daemon/tests/edit_receipts.rs`
`outcomes_are_distinct_and_capacity_is_bound_before_preparation`; the
adapter's whole-invocation validation is
`packages/opencode-plugin/src/hooks/context/invocation-budget.test.ts`; on Pi,
`packages/pi-plugin/src/context-application-pi.test.ts` charges the whole
system prompt under the `pi-heuristic` profile inside `editSystemPrompt`, so a
candidate the usable window refuses is `keep` with the prompt unchanged; and,
through the OpenCode transform's publication step,
`packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts`
`publishes when the whole invocation fits the context limit with headroom`,
`declines the pass when the whole invocation exceeds the context limit`,
`publishes a candidate over the context limit when it is no larger than the
incoming surface`, and `gates nothing for a model models.dev cannot name,
although its usage sample inverts to the 128k default`.
Guarantee: The daemon binds append allowance and replacement capacity before
preparation, and each plugin charges its whole assembled surface under its own
heuristic profile, the estimator's heuristic ratio plus headroom: the OpenCode
plugin every entry of the candidate message-entry surface by its canonical
length, the Pi plugin the whole system prompt by its UTF-8 byte length; each declines
publication when the charge exceeds the limit and the candidate is larger than
the incoming surface, the OpenCode plugin against the model's reported context
limit and the Pi plugin against Pi's usable window (the window less its output
reserve); a payload that fits alone but not in the invocation is
refused by the adapter, never applied; a candidate no larger than the incoming
surface is never refused for the window's own size; a limit the host has not
reported gates nothing, and the usage sample's percentage is not a report,
because the producers compute it against the 128k default for a model
models.dev cannot name; and the local estimate is labeled heuristic under the
estimator generation, never exact.
Check: `always` - the charge equals the ceiling of the summed lengths over the
heuristic ratio, then times one plus the headroom, and equals `estimateTokens`
under the forced heuristic for the same length; the transform reads the bound
from `resolveTrustedContextLimit` alone, admits at the limit, refuses one below
it with the host array unchanged, and publishes a shrinking candidate under a
one-token limit; a growing candidate charged over 128k tokens publishes for an
unknown model whose usage sample inverts to the default; the profile carries
the heuristic authority and the estimator generation as its revision. `always`
because every application crosses both bounds.
Fault/timing angle: None.
Required faults and enabling state: A models.dev limit one below the charged
total while the usage sample inverts to the opposite verdict; a model
models.dev cannot name with a usage sample computed against the default; an
estimator swap that moves the generation.
Confidence: high - [evidence](evidence/apply-adapter-validates-entire-assembled-invocation.md).
Existing check: None found.
Impact: An edit that fits its own bound could overflow the invocation.
Open questions: None.

### apply-enabled-outcomes-are-proven-on-real-harness-paths

Type: safety
Reachability: test-only
Status: active
Exercised: partial - `crates/daemon/tests/context_capabilities.rs`
`the_real_harness_tables_allow_exactly_the_recorded_classes` drives the
recorded OpenCode and Pi declarations through bound routes;
`packages/opencode-plugin/src/hooks/context/context-application.test.ts`
drives the plugin's prepare, apply, and confirm client against a scripted
daemon and witnesses the applied identity, the lost acknowledgment under both
daemon answers, the receipt-alone negative control, the confirm that cannot
reach the daemon after publication, and the capability fallback latched per
route; the client has no production caller because no daemon route yet
produces a packed body, and the run against a real OpenCode server is not
performed. On Pi, `crates/daemon/tests/context_capabilities.rs`
`pi_pure_packing_yields_one_outcome_set_whatever_the_consumer_advertises_and_writes_nothing`
drives every gated class, an over-allowance preparation, a foreign profile
echo, and the append lifecycle through two `pi` binds, and
`packages/pi-plugin/src/context-application-pi.test.ts` drives the same client
with the system-prompt slot; neither plugin has a production caller for the
client because no daemon route yet produces a packed body, and the run through
the Pi runner is not performed.
Guarantee: On the OpenCode path each enabled class has a witnessed outcome
carrying plugin-supplied applied identity; on the Pi path every gated class is
denied, pure packing appends one owned block to the system prompt, a denied
class is never simulated, two binds differing only in advertised consumer
strings yield one outcome set, and a lost acknowledgment is `unknown`; a
plugin build without the transform hook never produces an applied outcome.
Check: `always` - the Pi denials and the pure-packing append are witnessed
through a bound `pi` route with the daemon's profile echoed at apply, the
outcome set is `[capability_unsupported x3, preparation_failure
append_allowance, profile_mismatch, append complete, lost unknown]` under both
consumer-string sets with the kernel tip and projection counter unchanged; the plugin's `replace` intent on Pi falls back to `append` once,
latches the denial, and a second run keeps the existing block rather than
writing a second; a block whose id or body reproduces the open delimiter is
never written and confirms as `keep` with the prompt unchanged, so the owned
block stays the last open delimiter before the trailing close
(`context-application-pi.test.ts` `never writes a block whose body or id
reproduces the open delimiter, so the owned block stays locatable`); a prepared
answer whose `preparation_id` is outside the minted
`<incarnation>-<identity>` shape is `malformed_prepare_answer` before any
adapter edit (`context-application.test.ts` `treats a preparation id outside
the minted shape as a malformed answer and never applies it`); the OpenCode
allowed set is witnessed at the gate; the
OpenCode applied-identity witnesses come from the scripted daemon. `always`
because every enablement claim needs its witness.
Fault/timing angle: None.
Required faults and enabling state: A running OpenCode server driving the
plugin's `ContextApplication` client against the daemon; the Pi runner with a
built plugin for the Pi arm.
Confidence: medium - [evidence](evidence/apply-enabled-outcomes-are-proven-on-real-harness-paths.md).
Existing check: `crates/daemon/tests/model_execution_roundtrip.rs` real harness
subprocess runs, status unaudited.
Impact: A class could be declared enabled without a harness ever proving it.
Open questions:

- The end-to-end run against a real OpenCode server is outstanding; the
  scripted-daemon witnesses stand in for it. (needs human input)

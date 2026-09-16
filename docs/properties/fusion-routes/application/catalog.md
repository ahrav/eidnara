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
and receipt lifecycle records. The capability-gate records enter with
RP2.7.U5.

This part owns `crates/daemon/src/edit_receipts.rs`: the bounded in-memory
receipt store behind `retrieval.prepare`, `retrieval.apply`, and
`retrieval.confirm`, whose wire shape is Section 7.8 of
`docs/host-wire-protocol.md` (context-application protocol 1).

Parent decisions recorded here. Q7: a deliberate second application of one
selection is a legal intent, so the tuple (daemon incarnation, context
revision, action, selection digest, accounting profile) is a length-delimited
fingerprint and the identity minted at preparation is the key. Q8: the
incarnation signal is a random value each `HandlerCore` mints and every
preparation identity carries as its prefix; a key of another incarnation is
`unknown` and forwards nothing until a confirm with the exact applied identity
reclassifies it; a duplicate while an apply is in flight answers `in_flight`
with the recorded forwarded identity and forwards nothing. Q9: retention has a
count bound and a time bound enforced separately, the time bound must be at
least `retention_floor`, the longest supported retry path, and an evicted or
expired key is refused as `receipt_unavailable`; read-back is a confirm whose
`applied_identity` equals the forwarded identity the daemon recorded, and for
a key of another incarnation, where no record survives, a confirm whose
well-formed forwarded and applied identities agree, recorded so the key does
not fall back to `unknown`. The count bound is enforced when a key is minted:
the oldest settled receipt makes room and a store of in-flight receipts
refuses the new preparation with `preparation_failure`/`receipt_capacity`, so
a read never evicts and an edit that may already be applied is never dropped.
A limits change keeps the receipts.

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
| [apply-outcomes-are-distinct-and-empty-replacement-is-applied-replacement](#apply-outcomes-are-distinct-and-empty-replacement-is-applied-replacement) | safety | test-only | always | active | high |
| [apply-append-allowance-and-replacement-capacity-are-bound-before-preparation](#apply-append-allowance-and-replacement-capacity-are-bound-before-preparation) | safety | test-only | always | active | high |
| [apply-healthy-prepared-application-terminates-with-known-outcome](#apply-healthy-prepared-application-terminates-with-known-outcome) | liveness | test-only | always | active | high |

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
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `same_key_and_digest_replays_the_known_outcome_with_one_effect`.
Guarantee: The preparation identity is minted per preparation and its fingerprint is a length-delimited derivation over daemon incarnation, context revision, action, selection digest, and accounting profile; a retry under one identity and one digest returns the recorded state with exactly one forwarded effect, and a retry with another digest is a typed conflict.
Check: `always` - two prepares over one tuple yield two identities and one fingerprint; the second `apply` of one identity answers `in_flight` and the independent edit log holds one effect; an apply with a changed span after the forward is `conflict`; after the acknowledgment every replay answers `complete` with the same outcome and a second acknowledgment with another outcome is `conflict`. `always` because every apply compares the digest.
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
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `a_changed_context_between_prepare_and_apply_is_stale_and_forwards_nothing`.
Guarantee: An apply whose current context revision, representation, selected spans, or span set differs from the prepared one is refused as `stale_preparation` before anything is forwarded, and the preparation stays usable under its own context.
Check: `always` - a changed revision, a changed representation, a changed span end, and a dropped span each answer `stale_preparation` with no effect logged; the same preparation then forwards under its original context. `always` because the digest is recomputed on every apply.
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
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `a_restart_leaves_forwarded_and_unforwarded_keys_unknown_until_read_back` and `a_lost_acknowledgment_is_sticky_unknown_and_a_fenced_confirm_is_a_conflict`; `crates/daemon/src/edit_receipts.rs` `a_foreign_key_completes_only_on_a_well_formed_matching_read_back_and_stays_complete`.
Guarantee: After a daemon restart every key of the prior incarnation is `unknown`, whether it had been forwarded or only prepared; after a lost acknowledgment the key is `unknown`; `unknown` is sticky, forwards nothing on retry, and is reclassified only by a confirm whose applied identity equals the forwarded identity.
Check: `always` - a forwarded key and a prepared key from a shut-down daemon answer `unknown` on a fresh daemon and again on retry with an empty edit log; a confirm without an applied identity, with another identity, or with a malformed one leaves `unknown`; a confirm with the exact identity answers `complete`, later applies read `complete`, and another outcome is `conflict`; on one daemon a confirm without an applied identity turns an in-flight receipt `unknown`, the retry forwards nothing, and a read-back naming another forward or another applied identity is `conflict` because the forwarded identity stays recorded through `unknown`. `always` because the incarnation prefix and the state are read on every request.
Fault/timing angle: Daemon restart after forward, restart after prepare, lost acknowledgment.
Required faults and enabling state: A second `KernelDaemon`; a confirm with `applied_identity: null`.
Confidence: high - [evidence](evidence/apply-unknown-outcome-never-replays-blindly.md).
Parent Q8: the incarnation signal is the random `edit_incarnation` each `HandlerCore` mints and every preparation identity carries as its prefix; a key of another incarnation is `unknown`.
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
Exercised: yes - `crates/daemon/tests/edit_receipts.rs` `the_count_bound_evicts_the_oldest_settled_key_and_never_an_in_flight_one` and `the_route_is_disabled_until_an_approved_limit_set_is_installed`; `crates/daemon/src/edit_receipts.rs` `a_key_expires_by_its_creation_time_not_by_its_last_use` and `a_full_store_of_in_flight_receipts_refuses_a_new_preparation`.
Guarantee: Receipts are bounded by a key count enforced when a key is minted and by a retention measured from creation, not last use, enforced on every access; a key past either bound is dropped and any later apply or confirm for it is refused as `receipt_unavailable`; an in-flight receipt is never evicted for count, so a full store of in-flight receipts refuses the new preparation; a retention shorter than `RETENTION_FLOOR` is refused at installation; no limit set disables the routes.
Check: `always` - with `max_keys = 2` a third prepare evicts the first, whose apply and confirm are `receipt_unavailable`, the survivors still forward, a fourth prepare over two in-flight receipts is `preparation_failure`/`receipt_capacity`, and a wider limit set keeps the receipts; with synthetic instants a key used every two seconds still expires ten seconds after creation; a retention one millisecond below the floor is refused and the routes stay `disabled`. `always` because expiry runs on every access and the count bound on every mint.
Fault/timing angle: Time passing; count overflow.
Required faults and enabling state: Narrow limits installed on a live daemon.
Confidence: medium - [evidence](evidence/apply-receipt-retention-is-bounded-and-eviction-cannot-authorize-replay.md).
Parent Q9: retention is bounded below by `RETENTION_FLOOR`, one route deadline ceiling plus one client retry; the eviction disposition is refusal, not a tombstone, because a dropped key cannot be told from one never minted.
Existing check: `crates/daemon/tests/kernel_routes.rs` idempotent `kernel.commit` replay tests, status unaudited.
Impact: An unbounded store would grow with every preparation; an evicted key that replayed would forward a second edit.
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

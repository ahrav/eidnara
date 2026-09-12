# Ingress decode and admission surface

This lens records the admission and decode obligations that any change to the
per-turn `transform` ingress path must preserve. Anchors are checked in
`/local/home/ahrav/scratch/eidnara` at
`913234433ae36a80a6e22c6aac14c7f9aab74386` on 2026-09-10. It is analysis
only; no test ran and nothing outside this file changed. Lifetime-of-charge
obligations stay with [E2][e2]; this lens adds the admission order, the charge
magnitude, the discriminator read, and the typed-decode equivalence.

[`Handler::handle`][handle] (the audit calls it `Module::handle`; the trait is
[`CompositeComponent`][composite]) runs four steps before any route work: the
[byte cap][bytecap], the [footprint scan][footprint], a non-awaiting
[`try_reserve_resident`][reserve] on the host [scratch pool][pools], and
`serde_json::from_slice::<Value>` with parse failure mapped to `Value::Null`.
The parse charge is held until `handle` returns, so it covers the `Value`
tree, the typed request, and response encoding in [`settle_prepared`][settle].
Rejection codes are `invalid_params` for both cap overflow and a footprint
above [`resident_capacity`][capacity] (the helper is named
[`request_too_large_error`][toolarge] but emits `invalid_params`; the string
`request_too_large` exists only in the [direct-host fixture control
channel][fixture]), and `queue_full` for a transient scratch shortfall
([`resident_capacity_error`][queuefull]). The [wire contract][wire63] names
`invalid_params` for the 1 MiB facade and 32 MiB transform limits; the
resident-slice semantics it spells out ([§7.5.1][wire751], [§8.3][wire83]) are
Synapse and host statements, not a context-module contract. Only bodies over
1 MiB run the [`RequestMethodProbe`][probe], whose [`ProbeString`][probestr]
skips structured or long discriminators and whose [class set][class] is
`transform`, `state_sync`, and `kernel.artifact.ingest.page`.

Routing then reads the raw `Value`:
[`dispatch_value_with_inbound_bytes`][dispatch] takes `method` as a string,
falls back to `kind`, and treats a non-string as absent; a `{name, arguments}`
body without either goes to the facade; anything
else gets [`unrecognized_request_shape`][unrecognized] (a malformed body reports
as `non-object JSON (null)`). [`handle_transform_dispatch`][tdispatch] splits
on [`has_transform_page_fields`][pagefields], which is key presence, so a
`null` page field selects the page lane. The unpaged lane reads
[`request_observed_at_ms`][observed] from the raw `Value`, then runs
[`from_value::<TransformRequest>`][fromvalue] with failures as `bad_request`.
[`TransformRequestWire`][wirestruct] is not `deny_unknown_fields`, has no
`method` field, and does not validate `kind`; `session_id` and `render_config`
are the only required fields. Each [`WireMessage`][wiremsg] and
[`WireBlock`][wireblock] decodes through a `Value` and retains it, which is the
basis of [`RETAINED_STRING_COPIES`][copies]. The paged lane reaches the same
typed decode through [assembled pages][pageapply] without a second footprint
reservation. Tests enter at [`dispatch_value`][testentry] with a built `Value`,
so nothing exercises the admission chain end to end except one small
[direct-host transform][directhost]. The production plugin sends both
[`method` and `kind`][plugin] and [pages bodies over 512 KiB][paging], so an
unpaged transform over 1 MiB has no known production sender.

## Candidate properties

### admission-refusal-is-effect-free

Type: safety
Check: `always` - for every request that `handle` refuses before
`dispatch_value_with_inbound_bytes`, assert that the terminal code is
`invalid_params` or `queue_full`, that no [`TransformDispatchTicket`][ticket]
was created, that `transform_route_channels`, the prompt-surface freeze, page
staging, and the store are unchanged, and that the parse charge (if taken) and
the ingress charge are released when the handler future ends. `always` because
the condition must hold on every refusal, and the refusal set is defined by
code position, not by an observed defect.
Guarantee: An admission refusal produces one terminal error and no
dispatch-side effect.
Fault/timing angle: A fused decode-and-charge design that starts typed decode
before the admission decision can reach the post-parse side effects at
[`transform_route_channels`][routechan] and
[`freeze_prompt_surface_selection`][freeze], which run before
[`ticket.accept()`][accept], on a body that the current code refuses.
Required faults and enabling state: A body over the applicable byte cap, a
body whose footprint exceeds `resident_capacity()` (about 176 MiB at the fixed
[`SCRATCH_RESERVED_BYTES`][scratchconst]), or a scratch pool drained by
concurrent parses so `try_charge` fails transiently.
Reachability: test-only - the production plugin caps unpaged bodies at
512 KiB, whose footprint is about 1.6 MiB against a 176 MiB pool. A transient
`queue_full` under concurrent Synapse and context load is plausible but not
verified here.
Existing check: none found at the handler level. Pure-function tests cover the
cap and the bound ([byte cap test][t-cap], [footprint tests][t-fp]);
[prepared_output.rs][t-prep] shows error outcomes settle without an output
reservation.
Open questions:
- Is a transient `queue_full` from the shared scratch pool expected in default
  production, and does the context module owe a `retry_after_ms` on it the way
  Synapse does? Today [`RequestOutcome::error`][outcome] sends none. (needs
  human input)

### parse-charge-bounds-decoded-residency

Type: safety
Check: `always` - for every admitted request, assert
`charge >= nodes * size_of::<Value>() * VALUE_NODE_SLACK + string_bytes *
RETAINED_STRING_COPIES + VALUE_ENVELOPE_BYTES` where `nodes` and
`string_bytes` come from the scan, and independently assert that the typed
decode retains at most `RETAINED_STRING_COPIES` owned copies of each string
block (the `WireMessage` original, the `WireBlock` original, and the
`BlockKind` text). `always` because the bound is evaluated on every decode and
a single under-reservation escapes the resident envelope.
Guarantee: The resident charge taken before decode is an upper bound on the
bytes the decode retains for the request's lifetime.
Fault/timing angle: An optimization that charges during a typed parse instead
of before it, or that changes how many copies `WireMessage` and `WireBlock`
keep, can shrink the charge while the retained bytes stay the same, or the
reverse; [`try_reserve_resident`][reserve] documents that reserving after
allocation lets the allocation escape the envelope.
Required faults and enabling state: A body with a large text block (the
existing [copy test][t-fp] uses 1 MiB) decoded through the full typed path,
with a copy count taken from the resulting `TransformRequest`.
Reachability: default-production - every unpaged transform takes this path.
Existing check: [footprint tests][t-fp] assert the arithmetic of the bound and
the three-copy multiplier; none found that counts copies retained by the typed
decode, and none found that compares the charge with a measured decode.
Open questions:
- [E2][e2] owns the lifetime of the charge; this record owns its magnitude.
  Should the two be one record in synthesis? (needs human input)
- The paged lane reaches the typed decode from staged pages with no
  footprint reservation for the assembled whole; is the page staging budget
  the intended cover? (needs human input)

### route-discriminator-read-is-independent-of-typed-decode

Type: safety
Check: `always` - over a fixed corpus of discriminator shapes, assert
`route(body) == route_reference(body)` where the reference is the current
`Value` read: `method` as a string, else `kind` as a string, non-string
treated as absent; `{name, arguments}` without either selects the facade;
any of the six [`TRANSFORM_PAGE_FIELDS`][pageconst] present by key (including
`null`) selects the page lane; and the typed request ignores `method` and
does not validate `kind`. `always` because routing is evaluated on every
request and any divergence misroutes silently.
Guarantee: The lane a body reaches depends only on the raw discriminator and
page-field key reads, never on the typed decode.
Fault/timing angle: None in time; the hazard is a direct typed parse that
derives the lane from `TransformRequest.kind`, treats a `null` page field as
absent, or reads `kind` before `method`.
Required faults and enabling state: Bodies such as
`{"method":0,"kind":"transform"}`, `{"method":"transform","kind":"x"}`,
`{"kind":"transform","transform_page_id":null}`, and
`{"name":"x","arguments":{}}`.
Reachability: default-production - the plugin sends both discriminators
([plugin][plugin]); the `kind`-only form is sent by
[direct_host.rs][directhost].
Existing check:
[`dispatch_routes_each_envelope_class_to_a_distinct_arm`][t-dispatch]
covers `kind` routing, the facade envelope error, and the unrecognized shape
for object and non-object bodies; [route-shape unreachability tests][t-shape]
cover rejected aliases; none found for a `null` page field or a non-string
`method` at the dispatch layer (the [byte cap test][t-cap] covers the probe's
reading only).
Open questions: None.

### typed-decode-outcome-is-entry-path-independent

Type: safety
Check: `always` - as a differential oracle, assert that
`decode_unpaged(body)` and `decode_via_value(body)` agree on accept versus
reject, on the error code, and on the resulting `TransformRequest`, for a
corpus that includes duplicate keys among known top-level fields (current
behaviour is last-wins because [`Map::insert`][mapinsert] replaces and the
derived struct never sees the duplicate, whereas a direct derived decode
returns `duplicate field` per [serde_derive][derivedup]), unknown fields
(ignored), `null` on `Option` fields (`None`), `null` on defaulted non-`Option`
fields (`bad_request`), missing `session_id` or `render_config`
(`bad_request`), and malformed JSON (currently `unrecognized_request_shape`
via `Value::Null`). `always` because each shape must decode the same way on
every request; the corpus makes the check finite.
Guarantee: A transform body decodes to the same outcome whether it enters as a
raw slice on the unpaged lane, as a `Value` assembled from pages, or through
any replacement decode.
Fault/timing angle: None in time; the hazard is a code change that swaps
`from_slice::<Value>` plus `from_value` for `from_slice::<TransformRequest>`
on one lane only, so the two lanes diverge on duplicates and malformed input.
Required faults and enabling state: The shape corpus above, fed through
[`dispatch_value_for_test`][testentry] and through the page lane.
Reachability: default-production - the unpaged lane carries every body up to
512 KiB and the page lane carries the rest ([paging][paging]).
Existing check: [`transform_request_parses_full_flat_wire_envelope`][t-envelope]
decodes one full envelope; [transform_meta_bound.rs][t-meta] decodes a
`kind`-only body; none found for duplicate keys, `null` handling, or malformed
JSON on either lane.
Open questions:
- Is last-wins on duplicate top-level keys a contract or an accident of the
  `Value` round trip? Synapse rejects duplicates ([§7.5.1][wire751]); the
  context module has no written rule. (needs human input)
- Must malformed JSON keep reporting `unrecognized_request_shape` with
  `non-object JSON (null)`, or may it become `bad_request`? (needs human input)
- Numeric edge cases (integers above `u64::MAX`, exponents, `-0`) pass through
  `Value` number normalization today; equivalence with a direct typed decode
  for the `u64`, `usize`, and `f64` fields is unresolved, needs a differential
  run.

### oversize-probe-never-widens-beyond-dispatch-class

Type: safety
Check: `always` - for every body over `MAX_FACADE_FRAME_BYTES`, assert
`probe_admits(body)` implies `dispatch_route(body)` is one of `transform`,
`state_sync`, `kernel.artifact.ingest.page`; assert the transform ceiling is
inclusive at `MAX_TRANSFORM_FRAME_BYTES`; and assert the probe allocates no
memory proportional to the body before the resident reservation. Conservative
refusals (the probe errors on duplicate discriminator keys and refuses) are
permitted. `always` because a widened cap on a non-transform route is a
capacity bypass on every occurrence.
Guarantee: The over-1 MiB probe admits a body past the facade cap only when
dispatch would route it to a transform-class handler.
Fault/timing angle: The probe runs before the parse charge, so its scratch is
unaccounted; a probe that builds a structured `method` would hold
body-proportional memory outside the resident envelope while other requests
are admitted.
Required faults and enabling state: Bodies over 1 MiB with a string, numeric,
array, object, or 2 MiB `method`; a `kind` beside a non-transform `method`;
duplicate `method` keys; a body of exactly 32 MiB and of 32 MiB plus one.
Reachability: test-only for the transform route - the plugin pages at 512 KiB
and no other transform sender was found in `packages/`. The same probe path
is default-production for `kernel.artifact.ingest.page` if a client sends a
page over 1 MiB; the daemon allows 16 MiB decoded per page ([ingest][ingest])
but no client page size was verified here.
Existing check: [`request_byte_cap_widens_for_transform_class_only`][t-cap]
covers the class set, `kind` beside `method`, non-string fallback,
unparseable refusal, structured and long `method`, and a body above the
transform ceiling; none found for exactly 32 MiB, for duplicate discriminator
keys, or for probe memory.
Open questions:
- A body over 1 MiB with duplicate `method` keys is refused by the probe but
  would be admitted under 1 MiB (last-wins). Is the asymmetry acceptable as
  conservative refusal? (needs human input)

### parse-reservation-is-fail-fast-and-scratch-scoped

Type: safety
Check: `always` - assert that the handler's parse charge is taken with a
non-awaiting `try_charge` on the host scratch pool, never on the ingress pool,
and that the refusal code is `invalid_params` exactly when
`footprint > resident_capacity()` and `queue_full` exactly when the footprint
fits the ceiling but the pool is short. `always` because the pool identity and
the code mapping are fixed per request and a blocking reservation would
change admission ordering for every peer.
Guarantee: The handler's parse reservation never waits and never draws on the
pool that admits other connections' frames.
Fault/timing angle: The scratch pool is one host-wide [`ByteBudget`][pools]
shared by the context, Synapse, and Broca components; concurrent large parses
race for it, and a reservation that awaited would hold the pending and task
permits while parked.
Required faults and enabling state: Two or more concurrent bodies whose
footprints sum above `SCRATCH_RESERVED_BYTES`; one body whose footprint
exceeds it alone (a 32 MiB scalar-dense body bounds far above the ceiling).
Reachability: default-production - every request takes the reservation; the
refusal branches need the constructed states above.
Existing check: [`ByteBudget` tests][t-budget] cover permanent versus
transient `None`, all-or-none acquisition, and the `u32` conversion refusal;
[pool split test][t-pools] covers the three non-overlapping pools; none found
that exercises the handler's `queue_full` or footprint `invalid_params`
branch.
Open questions:
- [`SCRATCH_RESERVED_BYTES`][scratchconst] is documented as sized for Synapse
  budgets; the context transform footprint (up to about 96 MiB of string
  copies for a 32 MiB body) shares the slice without appearing in the sizing
  or in `validate_serving_limits`. Is that intended? (needs human input)

## Existing checks

| Check | Source condition | Status |
| --- | --- | --- |
| [`request_byte_cap_widens_for_transform_class_only`][t-cap] | `enforce_request_byte_cap` class set, `kind` beside `method`, non-string fallback, unparseable refusal, structured and 2 MiB `method`, body above the 32 MiB ceiling | unaudited |
| [`value_footprint_counts_nodes_outside_strings_only`][t-fp] | separators inside strings and escaped quotes do not count as nodes | unaudited |
| [`value_footprint_charges_every_retained_copy_of_string_bytes`][t-fp] | bound is at least three copies of a 1 MiB text block | unaudited |
| [`scalar_dense_bodies_bound_far_above_their_wire_size`][t-fp] | node cost dominates for `[1,1,...]`; string bytes are not charged as nodes | unaudited |
| [`dispatch_routes_each_envelope_class_to_a_distinct_arm`][t-dispatch] | `kind` routing, `facade_envelope_not_supported`, `unrecognized_request_shape` for object and non-object | unaudited |
| [`management_drop_alias_routes_are_rejected`][t-shape] | retired aliases return `unrecognized_request_shape` | unaudited |
| [`indexing_embedding_git_and_mural_are_unreachable_from_every_route_shape`][t-shape2] | internal names are not routable by `method`, `kind`, or facade | unaudited |
| [`transform_request_parses_full_flat_wire_envelope`][t-envelope] | `from_value::<TransformRequest>` accepts one full envelope | unaudited |
| [`first_hard_pass_meta_respects_the_store_durable_text_bound`][t-meta] | `from_value::<TransformRequest>` accepts a `kind`-only body (feature `bench-internals`) | unaudited |
| [`readiness_permissions_catalog_and_real_unary_transform`][directhost] | one small `kind`-only transform admitted through the real host | unaudited |
| [`typed_errors_and_stream_markers_have_no_prepared_body`][t-prep] | error outcomes settle without an output reservation | unaudited |
| [`capacity_separates_permanent_from_transient_exhaustion`][t-budget] | `try_charge` above capacity is permanent and consumes nothing | unaudited |
| [`try_charge_is_exact_and_all_or_none`][t-budget] | over-capacity acquisition leaves the budget unchanged; `u32` overflow refuses | unaudited |
| [`the_resident_cap_splits_into_three_non_overlapping_pools`][t-pools] | ingress, egress, and scratch pools sum to the floor | unaudited |

None found:

- A handler-level test that drives `Handler::handle` with an over-cap or
  over-footprint body and asserts `invalid_params`, or drains the scratch pool
  and asserts `queue_full`. `RequestCtx` is transport-private, so unit and
  integration tests enter at [`dispatch_value`][testentry].
- A differential test of duplicate keys, `null` fields, or malformed JSON
  across the unpaged and paged lanes.
- A test that a `null` page field selects the page lane.
- A test at exactly `MAX_TRANSFORM_FRAME_BYTES`.
- A test that counts string copies retained by the typed decode.
- The `request_too_large` assertion in [direct_host.rs][fixture] tests the
  fixture's line-based control channel, not the handler.

## Contract-versus-code disagreements

None found for the context module, because the wire contract leaves routed
bodies opaque and states only the `invalid_params` cap codes
([§6.3][wire63]). Two adjacent observations, not disagreements:

- The audit names a `request_too_large` code; the handler helper
  [`request_too_large_error`][toolarge] emits `invalid_params`, which matches
  the contract.
- [§7.5.1][wire751] says every Synapse `queue_full` carries a retry hint and
  that a permanently unservable parse is `schema_violation`; the context
  module emits `queue_full` without a hint and `invalid_params` for the
  permanent case. The contract scopes those rules to Synapse.

## Anchors

[e2]: ../../catalog.md#request-work-accounting-covers-retained-resources
[handle]: ../../../../../crates/daemon/src/lib.rs#L11805-L11827
[composite]: ../../../../../crates/host-runtime/src/composite.rs#L40-L58
[bytecap]: ../../../../../crates/daemon/src/lib.rs#L15472-L15488
[footprint]: ../../../../../crates/daemon/src/lib.rs#L15427-L15455
[copies]: ../../../../../crates/daemon/src/lib.rs#L15408-L15417
[toolarge]: ../../../../../crates/daemon/src/lib.rs#L15458-L15463
[queuefull]: ../../../../../crates/daemon/src/lib.rs#L15465-L15470
[probe]: ../../../../../crates/daemon/src/lib.rs#L15311-L15323
[probestr]: ../../../../../crates/daemon/src/lib.rs#L15330-L15393
[class]: ../../../../../crates/daemon/src/lib.rs#L15395-L15406
[dispatch]: ../../../../../crates/daemon/src/lib.rs#L12558-L12652
[unrecognized]: ../../../../../crates/daemon/src/lib.rs#L12674-L12698
[pagefields]: ../../../../../crates/daemon/src/lib.rs#L12654-L12658
[pageconst]: ../../../../../crates/daemon/src/lib.rs#L745-L752
[tdispatch]: ../../../../../crates/daemon/src/lib.rs#L7887-L7903
[observed]: ../../../../../crates/daemon/src/lib.rs#L7926-L7932
[fromvalue]: ../../../../../crates/daemon/src/lib.rs#L7933-L7941
[freeze]: ../../../../../crates/daemon/src/lib.rs#L8036-L8037
[routechan]: ../../../../../crates/daemon/src/lib.rs#L8045-L8048
[accept]: ../../../../../crates/daemon/src/lib.rs#L8066
[ticket]: ../../../../../crates/daemon/src/lib.rs#L574-L631
[pageapply]: ../../../../../crates/daemon/src/lib.rs#L9425-L9433
[settle]: ../../../../../crates/daemon/src/lib.rs#L12072-L12087
[testentry]: ../../../../../crates/daemon/src/lib.rs#L12484-L12499
[wirestruct]: ../../../../../crates/daemon/src/transform.rs#L801-L972
[wiremsg]: ../../../../../crates/memory-store/src/lib.rs#L126-L143
[wireblock]: ../../../../../crates/memory-store/src/lib.rs#L250-L264
[reserve]: ../../../../../crates/host-runtime/src/handler.rs#L474-L484
[capacity]: ../../../../../crates/host-runtime/src/handler.rs#L486-L491
[outcome]: ../../../../../crates/host-runtime/src/handler.rs#L230-L235
[pools]: ../../../../../crates/host-runtime/src/runtime.rs#L814-L822
[scratchconst]: ../../../../../crates/host-runtime/src/config.rs#L21-L31
[ingest]: ../../../../../crates/daemon/src/kernel_routes/ingest.rs#L50-L55
[wire63]: ../../../../host-wire-protocol.md#L308
[wire751]: ../../../../host-wire-protocol.md#L440
[wire83]: ../../../../host-wire-protocol.md#L750
[plugin]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L759-L761
[paging]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L635-L640
[mapinsert]: https://docs.rs/serde_json/1.0.151/src/serde_json/map.rs.html#127-129
[derivedup]: https://docs.rs/serde_derive/1.0.229/src/serde_derive/de/struct_.rs.html#269
[t-cap]: ../../../../../crates/daemon/src/lib.rs#L18567-L18625
[t-fp]: ../../../../../crates/daemon/src/lib.rs#L18627-L18691
[t-dispatch]: ../../../../../crates/daemon/src/lib.rs#L26568-L26623
[t-shape]: ../../../../../crates/daemon/src/lib.rs#L32419-L32436
[t-shape2]: ../../../../../crates/daemon/src/lib.rs#L32439-L32479
[t-envelope]: ../../../../../crates/daemon/src/transform.rs#L16162-L16188
[t-meta]: ../../../../../crates/daemon/tests/transform_meta_bound.rs#L21-L96
[directhost]: ../../../../../crates/daemon/tests/direct_host.rs#L48-L128
[fixture]: ../../../../../crates/daemon/tests/direct_host.rs#L285-L290
[t-prep]: ../../../../../crates/daemon/tests/prepared_output.rs#L103-L115
[t-budget]: ../../../../../crates/host-runtime/src/wire.rs#L825-L865
[t-pools]: ../../../../../crates/host-runtime/src/config.rs#L480-L503

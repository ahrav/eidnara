# Transform Edit Responses: the recipe wire switch (#538 U3-U5)

This file records the client and daemon behavior that the two-source recipe
switch adds on `feat/transform-recipe-wire`, stacked on
`feat/transform-recipe-builder` (`ed538110`). The eleven #533 records in
[`catalog.md`](catalog.md) keep their revision-bound line references; where this
change renames or replaces one of their named witnesses, the replacement is
named here. Unchanged numeric references and the following run receipts describe
the pre-review tree at `1f5531cc`, not the later review fixes.

Run receipts recorded for that tree, from the repository root with Node 24.18.0 first on
PATH: `bun run check:repo` exits 0 (`packages/opencode-plugin` 3511 pass, 0
fail); `cargo +1.98 test -p daemon --all-features --locked --no-fail-fast
--tests --examples` reports 1552 passed, 0 failed with the four tests skipped
that also fail on the predecessor under full-suite load
(`dreamer_run_task_bounds_*`,
`publication_search_deadline_preserves_admission_without_recharging`,
`full_constructor_observation_covers_receipts_hashing_and_arc_conversion`);
`cargo +1.98 clippy --workspace --all-targets --all-features --locked -- -D
warnings` and `cargo +1.98 fmt --all -- --check` pass.

## What crosses the wire

- Request: `base_revision` is mandatory; the daemon refuses its absence with
  `transform_base_revision_missing` (`crates/daemon/src/lib.rs:8331-8336`).
  `previous_output_revision` names the output the client applied.
- Response: `base_revision`, `output_revision`, `operations`, and
  `previous_output_revision` only when a `previous` keep was used. `messages`
  and `native_messages` are daemon-internal (`#[serde(skip)]`); the native
  suffix field and its finalization are deleted. An `ok` response without a
  recipe is refused at the seam (`transform_recipe_omitted`,
  `lib.rs:15310-15315`).
- Inserts are written from retained values at write time against lengths
  measured when the attach encoded them (`dispatch.rs:46`, `:73`;
  `lib.rs` native branch of `respond_transform`); kept messages contribute no
  literal bytes.

## Records touched

### TE20 `publication-is-current-and-atomic`

The candidate builder is `applyTransformRecipe`
(`rust-mode-transform.ts:657`): it parses and validates the whole recipe,
reserves value and retained-length slots per output entry plus one temporary
length slot per inserted entry, applies against the captured input and retained
previous output, and only then
hands the array to the existing boundary, ownership, source, and container
checks. Witnesses: `rust-mode-transform.test.ts:2301` "publishes at the exact
candidate charge and leaves the host array intact" and "declines one byte
short of the candidate charge and leaves the host array intact" (the decline
log names `recipe output array`); `:1742` "rejects a recipe that keeps from a
previous output the client did not apply" (`missing_previous_base`,
`wrong_base_revision`, and `malformed` are each refused before any write).
The open question in the #533 record about a malformed final operation is
answered by the shared fixture cases in
`crates/daemon/tests/fixtures/transform-edit-recipe-v1.json` and the
generated single-fault mutations in `crates/daemon/tests/edit_recipe_generated.rs`.

### TE21 `previous-base-is-applied-and-live`

Status: active, exercised. In `rust-mode-transform.ts`, each retained `applied`
output carries a descriptor-safe `capture`. The client validates that capture
before advertising `previous_output_revision` and again after the response
arrives, before accepting a recipe that names the previous output. Mutation
before advertisement drops the previous source; mutation while a response is
pending declines publication and NACKs deliveries. The current-input capture
alone cannot validate objects retained from an earlier host array.

The synchronous publication block stores the applied output. Invalidation,
clear, budget eviction, and session-count eviction drop it. The daemon offers
its retained output only when the request names that revision. The CK branch of
`respond_transform` compares served canonical bytes. It offers no CK input
candidates: typed decoding discards unknown envelope fields and normalizes
defaults, so typed equality cannot prove equality with raw client input. Native
input keeps remain enabled because native values retain their JSON representation.

Witnesses in `rust-mode-transform.test.ts`:
- "rejects mutated retained output before request"
- "rejects mutated retained output pending response"
- "keeps from the applied previous output and the submitted input, then acks
  its note deliveries"

The mutation tests send serialized fake requests and fresh second-pass input
objects. They verify retained-object mutation cannot change the published
array and distinguish successful input-only recovery from rejected delivery.
Daemon witnesses in `lib.rs` cover missing and matching advertised revisions,
normalized previous output across unknown-field and typed-payload edits
(`wire_recipe_keeps_only_normalized_previous_output`), and normalized passthrough
against raw client bases (`wire_passthrough_recipe_matches_typed_output_not_raw_input`).
`recipe_matching_compares_served_payload_fields` checks that unknown envelope
fields disappear while provider-extra changes remain significant.

### TE22 `delivery-disposition-follows-publication`

The missing-native compatibility retry is deleted. Renamed witnesses:
`rust-mode-transform.test.ts:1606` "nacks every delivery of an ok response that
carries no recipe and does not retry" (one body, both IDs NACKed, no ACK,
`failureCount` 1) replaces "nacks discarded delivery IDs and acks only IDs from
the applied retry response"; `:1637` "nacks initial and retry delivery IDs when
the full retry still cannot be applied" now drives the retry through
`need_full_sync` and a wrong `base_revision`; `:1703` "fails a delta pass whose
response carries no recipe and sends the next pass in full" replaces "retries
with full arrays when a delta response omits native content".

### TE25 optional-output budget

`AppliedOutputBudget` in `rust-mode-transform.ts` charges each session's
applied output at its canonical bytes plus eight bytes per retained length
and the descriptor-safe snapshot estimate. It evicts the least recently
retained sessions once the 64 MiB
`OPTIONAL_OUTPUT_BUDGET_BYTES` (`:130`) is exceeded, and refuses a single
retention larger than the budget. Refusal or eviction drops only the
`previous` source; the pass that produced the output has already published.
The 64-session `wireCaches` bound still applies, and a count eviction releases
the victim's charge (`storeWireCache`). Witness:
`rust-mode-transform.test.ts:1782` "evicts the least recently retained applied
output once the optional budget is exceeded". The end-to-end eviction path
(budget or count) followed by a full-input recipe is covered on the daemon
side by `lib.rs:25199`
`handler_native_delta_cache_eviction_self_heals_full_then_delta`
(`previous_output_revision` absent after `native_attachments.remove`, present
again on the following pass).

### TE30 `inbound-baseline-independent-of-output-base`

The delta-versus-full control now exists on the daemon: `lib.rs:25520`
`handler_tail_delta_cross_frontier_tool_arc_matches_full_control` and
`lib.rs:26307`
`handler_delta_normalization_matches_full_when_reserved_todo_starts_at_frontier`
apply a tail-delta pass and a full-request control through the test client
and compare the reconstructed native arrays byte for byte; the delta pass
carries `previous_output_revision` and previous-source keeps. The client keeps
the acknowledged input as its delta baseline (`computeWireDelta` over raw
snapshots), and `measureInputLengths` (`rust-mode-transform.ts:366`) reuses the
acknowledged prefix's lengths, so the input base and the applied output stay
separate owners.

## Daemon-side witnesses

- `lib.rs:23157` `transform_response_seam_turns_missing_recipe_into_a_typed_refusal`.
- `lib.rs:22659` `cached_transform_response_writer_is_byte_identical_to_value_round_trip`
  (a one-message CK passthrough inserts its typed served value).
- `edit_recipe.rs:1192` `revision_allocator_names_each_pass_once_and_refuses_exhaustion`.
- `lib.rs:26412` `native_attachment_reuses_transform_tag_baseline_and_preserves_bytes`
  replays the served array against the native attachment.
- `crates/daemon/tests/direct_host.rs:50` and `:133` drive a real fixture host
  over the wire and reconstruct the served array with
  `tests/support/applied.rs`; `:133` proves the persisted-state replay after a
  fixture restart reconstructs the same frozen m0.
- `crates/daemon/tests/transform_canonical_memory.rs` reads project memory
  through the reconstructed array.
- `crates/daemon/tests/host_adapter.rs` pins that `respond_transform` writes
  through `PreparedOutput::transform_recipe` and never through `serde_json::to_vec`.

## Cross-language round trip

The shared fixture file and `edit_recipe_generated.rs` are the executable
agreement between the Rust and TypeScript appliers. The separate
`verify-serialized-transform-pages.ts` command generates requests with
`base_revision` and verifies all 19 paging/admission cases against a real
host. Its Rust test reconstructs both paged and unpaged responses through
`support::applied::applied_messages` before comparing them.

A live daemon-to-plugin
round trip is not exercised by CI on this tree: the e2e Rust-mode harness gates
on the shared-memory channel probe, which reports
`runtime_mechanism_unavailable` under Bun 1.3.14 (`markAsUntransferable` is not
implemented), so `packages/e2e-tests` skips those scenarios here and on the
same Bun in CI. The daemon integration tests above cover the daemon side of the
wire; the plugin unit tests cover the client side against recipe fakes.

## Known gap

Canonical lengths for non-integer numeric content can differ by a few bytes
between the two appliers (`1.0` versus `1`); see
[`docs/transform-edit-recipe.md`](../../../transform-edit-recipe.md). The
64 MiB reconstruction cap is checked with each side's own measurement.

# frozen-admission-outcomes-and-boundaries-stay-stable

System: daemon admission and refusal classification.
HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
[Source register](../source-register.md) defines P and B. No new tests run.

## Discovery trigger

P:L18 makes changed original A1-A3 admission outcomes a stop condition. P:L70
preserves refusal witnesses, while KTD4 at P:L88 accepts a wider string-charge
ceiling. P:L176 permits retuning an old witness; that conflict remains open.
The independent review corrects discovery's overbroad freeze of all boundaries.

## Evidence trail

1. `docs/host-wire-protocol.md:304-314` separates the 64 MiB transport limit
   from application `invalid_params` at facade/transform caps.
2. `crates/daemon/src/lib.rs:16144-16161` admits bodies at the byte cap and
   refuses larger ones, using route classification for the wider cap.
3. `:16084-16093` checks a byte-derived footprint floor and precharges escape
   scratch. `:16099-16136` maps permanent refusal to `invalid_params` and
   transient refusal to `queue_full` with distinct messages.
4. `crates/daemon/src/metered_decode.rs:246-303` tries bounded batches down to
   the exact shortfall. Needed bytes above capacity are permanent; insufficient
   free capacity at an otherwise fitting step is transient.
5. `crates/daemon/src/lib.rs:20188-20229` compares both lanes at
   `footprint - 1` and `footprint`, computed during that same test run.
6. `:20414-20470` holds a real charge and tests transient versus permanent
   classification. `:20533-20580` checks route/store effects around refusal.
7. `:19569-19645` covers route-sensitive byte caps and the inclusive 32 MiB
   boundary. These assertions exist; their adequacy is unaudited.
8. `crates/daemon/src/metered_decode.rs:419-435` excludes string bytes from
   `footprint_floor`; its node charge stays fixed under KTD4.
9. A1 at `docs/properties/hot-path-optimization/latency-audit/catalog.md:142-156`
   notes that the managed client drops later terminals. One-terminal evidence
   needs observation before that filtering, not the settled client result.

Reachability is default-production: the handler at `:12153-12170` runs these
gates. Pressure and exact threshold cases require constructed inputs.

## Failure scenario

The implementation lowers string charge, then regenerates the witness using
its new footprint. Both versions reject their own witness, but the original
body is now accepted at the original capacity. The suite appears stable
because it has changed the question.

Competing explanation: a new string-heavy input becomes admissible under the
accepted wider ceiling. That is allowed outside the original frozen cases.
Keep those cases distinct from additive ceiling witnesses; only the former
require their original byte/capacity/outcome receipts to remain identical.

## Timing windows and dependencies

Preserve the sequence of other owners' charge acquisitions and releases.
A body eventually larger than capacity can encounter transient shortfall
before its size is known. Do not replace the deciding-gate classification
with a second parse solely to force a preferred code.

Partition the contract explicitly:

- Length caps, node-only footprint_floor, deciding-gate error mapping, one
  terminal per settled refusal, and no dispatch effect stay unchanged.
- Original A1-A3 bytes, capacities, pressure schedules, and outcomes stay
  frozen. Any changed original case triggers stop and owner escalation.
- The live string-charge ceiling may expand under KTD4. Record its new
  neighbours additively; never use them to replace an original witness.

This is not blanket invariance of the admitted set for every possible input.

## What a test must construct

- Byte-identical original A1-A3 bodies, fixed numeric capacity, and baseline
  outcome receipts captured before the optimization.
- Fixed byte-cap/node-floor neighbours and separate additive string-ceiling cases.
- Plain and escaped text plus dense ignored/native nodes.
- A held pool with independently known occupied bytes and a bounded retry
  after releasing that owner.
- State observers for ticket acceptance, route binding, prompt freeze, page
  staging, and store rows. Existing route/store checks cover only part.
- Raw or producer-side terminal counts for each settled refusal identity.
- The same body forced through both legal dispatch lanes where applicable.

## Investigation log

### Q: Is witness retuning compatible with the requested freeze?

- Sources examined: P:L18, P:L51, P:L70, P:L88, P:L164, P:L176;
  `crates/daemon/src/lib.rs:20188-20199`.
- Findings: The plan contains both preservation and retuning instructions.
  Dynamic-capacity tests cannot decide historical absolute stability.
- Missing evidence: Owner disposition if an original case changes under KTD4.
- Conclusion: needs human input for P:L176's retuning instruction. The wider
  ceiling is accepted; original witnesses remain frozen and cannot be replaced.

### Q: Is a durable frozen admission manifest available?

- Sources examined: A1-A3 catalog leads and the scoped tests above.
- Findings: Tests contain bodies and assertions but several numeric capacities
  derive from candidate code. No final before/candidate replay runs exist here.
- Missing evidence: Immutable fixture bytes, resource schedules, and receipts.
- Conclusion: unresolved, needs an implementation-owned baseline artifact.

### Q: Did discovery freeze more than the accepted plan requires?

- Sources examined: P:L88; original R3; independent finding 1.
- Findings: Requiring unchanged decisions at every string-charge boundary
  would contradict the accepted wider ceiling.
- Missing evidence: None for this scope correction; candidate results remain absent.
- Conclusion: resolved. Preserve original cases and fixed rules, add new ceiling
  witnesses, and keep the retuning conflict open. The slug is unchanged.

## Typed-wire U1 execution, 2026-09-13

`frozen_corpus_footprints_replay_with_only_string_charge_changes` in
`crates/daemon/src/lib.rs` holds the 46 A2 bodies with their footprints and
terminals recorded at three string copies before the change: the terminal
through both lanes with an unbounded pool, at the frozen footprint, and one byte
under it. Replay on the current tree: every unbounded terminal is unchanged;
both lanes agree at every capacity; every frozen terminal is unchanged except a
frozen too-large terminal on a body with string bytes, which is now the body's
unbounded terminal. For every such body the frozen and current footprints differ
by exactly two times the visited string bytes, and for the 30 bodies whose
`Value` tree keeps every key that count equals the tree's string total. The
raw-value-token-under-discriminator body stays refused at its frozen footprint
because the byte-derived node floor, not the string charge, refuses it; the
page-field bodies keep their own `invalid_params` terminals. Length caps and the
node floor are not touched by the coefficient. A1's ring witnesses in
`crates/daemon/tests/direct_host.rs` are node-dense and unaffected. New ceiling
witnesses are additive (`text_heavy_admission_ceiling_witnesses`).

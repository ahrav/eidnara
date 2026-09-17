# Serialization fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`. Base: `016c7127`.

| Fault or state | Available seam | Marker | Records |
| --- | --- | --- | --- |
| Serialized-bytes limit one below the closed render | `finalize` with a limit derived from the full render, then reduced by one. | `packing.serialization.bound_at_render_minus_one` | packing-adjustment-removes-last-admitted-within-the-cap, packing-body-serialized-once-through-the-guard |
| Accounting bound one below the closed render | `prepare_required` and `prepare_optional` under one `AccountingBounds` derived from the full render, then reduced by one; the refusal is the optional phase's, before `finalize`. | `packing.serialization.accounting_refused_before_serialization` | packing-adjustment-removes-last-admitted-within-the-cap, packing-limits-fail-closed-at-manifest-parse |
| Pass cap spent with a bound still exceeded | A cap of zero, or a limit below the required render with a cap larger than the admitted count. | `packing.serialization.cap_exhausted` | packing-adjustment-removes-last-admitted-within-the-cap |
| Evaluation budget exhausted before serialization | `EvalBudget::cancel` before `finalize`. | `packing.serialization.budget_exhausted` | packing-adjustment-removes-last-admitted-within-the-cap |
| Evaluation budget ends while the fitting body is written | `guard_calls::on_write` in `crates/daemon/src/dispatch.rs` cancelling the budget from inside `write_to`. | `packing.serialization.budget_ended_during_write` | packing-adjustment-removes-last-admitted-within-the-cap |
| Guard measured or written more than once, or written after a refusal | The `guard_calls` per-thread counters in `crates/daemon/src/dispatch.rs`, reset before `finalize`. | `packing.serialization.guard_calls_counted` | packing-body-serialized-once-through-the-guard |
| Body past the transport maximum | A transform recipe one byte past `MAX_WIRE_BODY_BYTES` in the guard's own tests; no packing fixture renders 64 MiB. | `packing.serialization.transport_maximum_exceeded` | packing-body-serialized-once-through-the-guard |
| Packing group present without approval, partial, unknown, non-numeric, version-mismatched, zero, or past the transport maximum | Manifest fixtures built from `PACKING_LIMITS` with one malformation each. | `packing.serialization.malformed_packing_group` | packing-limits-fail-closed-at-manifest-parse |
| Cost cache cleared, warm, or rotated at a token edge; concurrent threads; fresh process | `cost_cache::clear`, `cost_cache::rotate`, an estimated-tokens bound one below the full render, `std::thread::scope`, and the test binary re-executed with `--exact`. | `packing.serialization.cache_state_varied` | packing-output-byte-identical-across-cache-states |

Every seam is a pure function, a constructed manifest, or a test-support
cache control; no fault injection framework is needed for this part. Marker
status lives in `../marker-ledger.md`.

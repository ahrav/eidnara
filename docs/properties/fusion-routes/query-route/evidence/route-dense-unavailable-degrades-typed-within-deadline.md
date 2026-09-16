# route-dense-unavailable-degrades-typed-within-deadline

## Discovery trigger

The RP2.7 specification requires that a saturated or unavailable dense lane
returns an explicitly typed exact plus lexical result within the original
deadline, not `deadline` and not dense completion, and that validation
failure, cancellation, and corruption are never relabeled as degraded success
(parent Q5).

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u3b-query-route` at
`41fb5258`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/query_route.rs`: `EmbedFailure::Unavailable` becomes
  `DenseLane::Unavailable(reason)` and the lane status `unavailable`;
  `EmbedFailure::Faulted` is `lane_unavailable`/`embedding_failed`;
  `DenseRefusal::Corruption` is `lane_unavailable`/`dense_corruption`;
  `DenseRefusal::QueryShape` degrades the lane; `DenseRefusal::Budget` is the
  budget's verdict.
- `crates/daemon/src/query_route.rs` `embed_refusal`: the lane's typed
  `DenseUnavailable` is the classifier. `LaneUnavailable { state }` names the
  state, `LaneBusy` is `busy`, `IdentityChanged` is `lane_changed`, input
  refusals are `input`, and an `Execution`, `Artifact`, or `Invariant`
  inference failure is `Faulted`. The earlier mapping read `embed_blocking`'s
  `InferenceError::Artifact` as `busy`, which is also what that call returns
  for a lane that turned failing or disabled under it and for an artifact the
  backend declared unusable.
- `crates/daemon/src/query_route.rs` `has_prose` and the handler's embedding
  gate: a request without text outside its selector mentions is not embedded
  and the dense lane is `undeclared`, mirroring the lexical lane's `Direct`
  handling.
- `crates/daemon/tests/query_route_dense.rs`
  `an_unavailable_embedding_lane_degrades_to_a_nonempty_exact_and_lexical_answer`
  `producer_corruption_is_typed_while_a_foreign_query_shape_degrades_the_lane`,
  `a_request_without_prose_leaves_a_ready_dense_lane_undeclared_and_runs_no_producer`,
  and `a_coverage_shortfall_and_a_row_bound_leave_the_dense_lane_incomplete`.
- `crates/daemon/tests/query_route_handler.rs`
  `a_selector_only_request_is_never_embedded_and_leaves_the_dense_lane_undeclared`
  and
  `an_artifact_fault_during_inference_is_embedding_failed_and_the_lane_is_disabled_after`.

## Failure scenario

A busy lane is reported as the caller's deadline, or a corrupt vector is
ranked and served as a degraded success. An inference that declares its
artifact unusable is reported as `busy`, so the disabled lane counts as
saturation. A selector-only lookup is embedded and scanned, so a ranking over
selector syntax is fused into a direct answer.

## Timing windows and dependencies

None.

## What a test must construct

- A projection with stored vectors; `DenseLane::Unavailable` per reason.
- A zero-norm stored vector encoded at the generation's dimension.
- A daemon engine scripted to fail one inference with
  `InferenceError::Artifact`.
- A selector-only request against a ready lane, and through the handler a
  counting embedder.

## Investigation log

### Q: Should a busy lane retry inside the deadline?

- Sources examined: `embed_admitted`, whose `run_inference` takes the `cpu`
  permit with `try_acquire` and refuses `LaneBusy` without waiting.
- Findings: a retry would spend the caller's budget on the lane's queue; the
  degraded answer is available at once.
- Missing evidence: RP2.9 calibration.
- Conclusion: resolved with answer for this build - degrade at once; retry is
  an open calibration question.

### Q: Which refusals does `embed_blocking` fold into `InferenceError::Artifact`?

- Sources examined: `crates/host-runtime/src/local_embeddings/mod.rs`
  `embed_blocking`, `run_inference`, `settle_inference`;
  `crates/daemon/src/embedding_dispatch.rs` refusal classification.
- Findings: `Artifact` carries a starting, disabled, failing, or shut-down
  lane, a held `cpu` permit, a lane that turned failing after the permit was
  taken, and a backend artifact fault that disables the lane. The batch
  dispatcher reads `DenseUnavailable::Inference(Artifact)` as a permanent
  stop, not a retry.
- Conclusion: resolved - the query embedder uses
  `preflight_embedding_for_lane` and `embed_admitted`, whose
  `DenseUnavailable` separates these cases.

### Q: Does the dense lane run for a request the lexical lane treats as prose-free?

- Sources examined: `crates/retrieval/src/exact/selector.rs`
  `Intent::lexical_segments`; `crates/daemon/src/query_route.rs`
  `lexical_lane`, `execute`, and the handler's embedding step.
- Findings: `lexical_lane` returns `undeclared` for `Intent::Direct`; the
  dense lane ran for every ready lane and the handler embedded before
  classification.
- Conclusion: resolved - the handler classifies before embedding and `execute`
  gates the dense lane on `has_prose`.

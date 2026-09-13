# served-canonicalization-campaign-reaches-risk-classes

## Discovery trigger

The safety candidates need reusable situation coverage. A green byte or
allocation check is weak evidence when it never constructs escaped identity,
late disorder, empty metadata, or an incomplete serialization prefix.

## Evidence trail

- [Module tests][tests] already accept controlled generic Serialize sources
  and contain nested/scalar/key cases. Their status is unaudited.
- [Recording and finalization][encode] expose the successful private test
  observation seam without requiring another production entry point.
- [Default HarnessMeta][meta] omits its default members; typed message data
  includes meta, providing a production `{}` witness.
- [Typed block parent][block] and [tagged child][kind] establish the production
  ordered-parent/disordered-child case. The exact ordered WireMessage root
  remains a private generic witness under sorted Value iteration.

Numbered anchors were verified with `git show HEAD:<path>` at
`2e4433e6b511ae74944df8a9669c428e73915d29`. Plan baseline `4980f8af` and
historical runs do not mark these campaign situations exercised.

## Failure scenario

A generator can construct only unordered unescaped roots. Such a campaign
never tests the intended canonical saving or the escape-aware identity path.
Another campaign may execute sorting lines without a second disordered object,
so short-circuiting never affects its bytes. Both can pass without useful
coverage of their risk classes.

## Timing windows and dependencies

Accumulate situations over multiple calls. No single input must be both
successful and failing or both identity and disordered. The final assertion
requires the conjunction of all declared markers, not any one marker.
Set markers from independent setup/emission preconditions, not S1–S5 outcomes.
Missing markers indicate a construction or reachability gap, not liveness.
Report every missing marker separately, including missing members of composite
rows; an opaque campaign pass/fail result is insufficient for follow-up.

## What a test must construct

Names below are proposed constant globally unique markers, not existing code.
Every row is required. Compound rows accumulate each listed member before
their marker becomes true; they are not satisfied by one representative.

| Constant marker | Independent precondition and reachability |
| --- | --- |
| `canonical_output_fast_path_empty_and_singleton` | Both empty and singleton objects are emitted; include default HarnessMeta `{}` through the production facade. |
| `canonical_output_fast_path_six_permutations` | All six distinct three-key emission orders are presented, including identity and nonidentity. Generic exhaustive probe is test-only. |
| `canonical_output_fast_path_key_classes` | Emitted keys include each of empty, prefix `a`/`a b`, quote, backslash, control, and Unicode classes. These keys fit production Value-backed extras. |
| `canonical_output_fast_path_escaped_identity` | An object has at least two keys, at least one requires escaping, and decoded emission order is nondecreasing. |
| `canonical_output_fast_path_escaped_inversion` | An escaped-key object's emitted decoded sequence has an inversion before sorting. |
| `canonical_output_fast_path_stable_duplicates` | Private generic sources emit equal decoded keys with distinct value tags, both grouped and displaced by another key. No Value conversion. |
| `canonical_output_fast_path_late_disorder` | An early object and later siblings/descendants each have emitted decoded inversions; ordered and empty objects are interleaved. |
| `canonical_output_fast_path_ordered_parent_child_disorder` | A real typed block has an ordered parent and disordered tagged child. This production witness requires child traversal, but its typed message root is already disordered. |
| `canonical_output_fast_path_ordered_root_child_disorder` | A private generic source emits an ordered root whose only disorder is nested. This test-only witness isolates a false-negative root-only aggregate; the production block-parent witness does not. |
| `canonical_output_fast_path_identity_ranges` | Successful real recording includes nested empty/singleton objects, arrays of objects, scalar gaps, and punctuation-bearing strings with unchanged tables available. |
| `canonical_output_fast_path_numeric_forms` | Each of `-0.0`, `1.0`, large/small exponents, signed minimum, and unsigned maximum is emitted in nested payloads. |
| `canonical_output_fast_path_success_classes` | Both independently canonical and independently disordered successful sources are presented to the counted serializer checks. |
| `canonical_output_fast_path_error_before_write` | The private source raises its planned error before any output write. |
| `canonical_output_fast_path_error_open_nested` | The private source emits a nonempty prefix with an open nested object, then raises its planned error. |

Use `sometimes` for the accumulated situation requirement. Safety outcomes
such as wrong changed flags, unwanted B allocations, or corrupted bytes must
never be coverage predicates. Keep observations private and tied to real
encode; do not add production counters or duplicate serialization setup.

## Investigation log

### Q: Why is the record test-only if many classes occur in production?

- Sources examined: the facade, generic tests, and typed serializers.
- Findings: the joint required set includes deliberately failing sources and
  duplicate-emission probes that production WireMessage does not expose.
- Missing evidence: campaign accumulation and execution at this revision.
- Conclusion: resolved with answer. Label the joint record test-only and each
  production witness explicitly; do not infer production reachability wholesale.

### Q: Who owns accumulation and follow-up for unfired markers?

- Sources examined: METHOD's situation-coverage rules and the lane candidates.
- Findings: every listed precondition can fire on a correct implementation.
- Missing evidence: implemented campaign checks and evidence of all markers.
- Conclusion: unresolved, needs `/testing:test-strategy`; existing fixture
  adequacy goes to `/testing:invariant-test-review`.

[tests]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L170-L252
[encode]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L121-L142
[meta]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L73-L123
[block]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L243-L248
[kind]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/memory-store/src/lib.rs#L326-L339

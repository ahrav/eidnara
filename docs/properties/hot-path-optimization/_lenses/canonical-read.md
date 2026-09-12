# Canonical-read surface

This summarizes separate prior read-only surface discovery supplied in the
task, not a new test run or portfolio evaluation. Anchors are rechecked in
`/local/home/ahrav/scratch/eidnara` at
`913234433ae36a80a6e22c6aac14c7f9aab74386` on 2026-09-10. The
[external-evidence scope](../catalog.md#scope-and-provenance) is pending final
confirmation; historical citations and exercise are not carried forward.

[The async transform handler][pass] pins one canonical read before its
synchronous transform closure. Emergency reruns at [8264][retry] reuse it.
[The kernel query][sql] has an inner admission join and no memory domain/kind
predicate. It materializes a full Vec before caller selection. Unadmitted
objects therefore are not part of its candidate result.

[The route reader][read] applies domain/kind and scope filtering before newest
selection and payload-prefix truncation. Serving order is commit sequence
descending, then object ID ascending, not the SQL object's lexical order.
[Canonical conversion][convert] then removes Labeled and nonpositive categories,
trims to the memory budget, and computes the snapshot revision.

K1-K4 preserve those boundaries. K3 requires row pressure with small payloads;
K4 requires byte pressure with at most 8192 candidates. Both are required.
The exact reference commit is fixed in [the catalog][reference], independent of
how tests package it. Pushdown must retain uncertain scope handling
and sensitivity folding. Moving positive-category filtering before caps changes
the baseline. Skipped corrupt-row error observability remains unresolved.
[Existing indexes][indexes] are evidence of schema shape, not an EQP result or
a performance claim. No query-plan experiment runs here.

[pass]: ../../../../crates/daemon/src/lib.rs#L8175-L8252
[retry]: ../../../../crates/daemon/src/lib.rs#L8316-L8418
[sql]: ../../../../crates/kernel/src/admission.rs#L3132-L3298
[read]: ../../../../crates/daemon/src/kernel_routes/read.rs#L159-L248
[convert]: ../../../../crates/daemon/src/canonical_memory.rs#L184-L212
[indexes]: ../../../../crates/kernel/src/schema.rs#L93
[reference]: ../catalog.md#fixed-reference-identity

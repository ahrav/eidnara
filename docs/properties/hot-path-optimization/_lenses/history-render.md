# History-render surface

This summarizes separate prior read-only surface discovery supplied in the
task, not a new test run or portfolio evaluation. Anchors are rechecked in
`/local/home/ahrav/scratch/eidnara` at
`913234433ae36a80a6e22c6aac14c7f9aab74386` on 2026-09-10. The
[external-evidence scope](../catalog.md#scope-and-provenance) is pending final
confirmation; historical citations and exercise are not carried forward.

[The inner renderer][inner] joins nonempty compartment bodies with two newlines
and demotes oldest input positions until its positive body budget fits.
Nonpositive direct-API budgets disable that guard. [The outer retry][outer]
measures the wrapped history slice and can still exceed 105% after three
rerenders. It is not a whole-prompt budget guarantee.

H1-H4 preserve final bytes and those distinct boundaries. H3 witnesses inner
multi-demotion pressure; H4 witnesses outer retry pressure independently.
The baseline renderer/tokenizer and JSON/SHA fixtures have the exact source
commit recorded above; oracle packaging cannot change acceptance semantics.
Internal estimates and batched demotions are allowed. Final counting includes seams,
Unicode, escaping, and tier-5 removal. Equal-tier body bytes can repeat, so a
demotion need not miss [the token cache][cache]. No global BPE monotonicity or
byte-sum law follows from fixture-local comparisons.

[The goldens][goldens] use JSON bodies and SHA-256, not insta. The tight test's
final-fit counter is not an independent multi-demotion witness. The tokenizer
is a [normal daemon dependency][dependency], despite the older catalog's
blanket test-only label. [Ordinary SOFT][soft] can replace m1 and other rendered
units while preserving existing m0; pressure refold may rematerialize m0.
[Pure Defer/SoftPlus][defer] replays the retained prefix unchanged.

[inner]: ../../../../crates/daemon/src/decay_render.rs#L289-L338
[outer]: ../../../../crates/daemon/src/m0_compose.rs#L178-L215
[cache]: ../../../../crates/daemon/src/token_cache.rs#L165-L180
[goldens]: ../../../../crates/daemon/src/decay_render.rs#L614-L800
[dependency]: ../../../../crates/daemon/Cargo.toml#L21-L32
[soft]: ../../../../crates/daemon/src/transform.rs#L4455-L4507
[defer]: ../../../../crates/cache-stability/src/lib.rs#L221-L287

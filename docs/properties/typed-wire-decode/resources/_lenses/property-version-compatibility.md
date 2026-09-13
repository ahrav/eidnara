# Property lens: version compatibility

HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
Scope and P notation: [source register](../source-register.md).

P:L164 prohibits wire-visible changes, while P:L88 changes heap coefficients.
Freeze refusal results at the same numeric capacity and input bytes. A test
that derives its capacity from each version's footprint cannot compare the
old and new admitted sets (`crates/daemon/src/lib.rs:20188-20199`).

The local `main` is not the plan's baseline. Use the pinned B identity from
the source register, then record the actual candidate artifact. W1 is
invalidated at the cited HEAD; versioned source prose must not supply a
fictional active gate. Candidates: frozen boundary preservation and narrowly
scoped before/after evidence.

# `failed-acquire-preserves-prior-epoch`

- **Discovery:** lifecycle and I/O-failure passes.
- **Primary evidence:** exclusive acquisition unlocks on persistence error (`FileLeaseStore::acquire_above`); `persist_epoch` never truncates and writes canonical digits from most to least significant (`crates/lease/src/lib.rs:636-675,830-838`).
- **Discriminating fact:** the supported invariant is monotonicity, not byte equality: a canonical positive-prefix overwrite leaves the old value, the new value, or a higher decimal splice, so the cited test proves that no parseable aftermath is lower than the prior epoch, not that the prior bytes survive. Short input is padded with non-decimal markers before conversion, so interruption after progress leaves malformed state unless the canonical write completes.
- **Existing evidence:** `interrupted_persist_never_leaves_a_lower_parseable_epoch` injects short ordered writes through production `persist_epoch` for empty, variable-width, and canonical-width prior states, parses through production `read_epoch`, and proves any parseable aftermath is not lower; the asserted count of parseable aftermaths keeps the canonical digit-over-digit splice covered rather than skipped (`crates/lease/src/lib.rs:1138-1519`).
- **Failure scenario:** real `File` error behavior is not exercised; non-returning process termination and power loss require separate evidence.
- **Timing window:** after any positive write progress in `persist_epoch` and before the update completes.
- **Instrumentation:** missing exact filesystem/device error through `File`; no production failpoint was added.
- **Open-question log:** identify a deterministic real-file fault mechanism if stronger evidence is required.

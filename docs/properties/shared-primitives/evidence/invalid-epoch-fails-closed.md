# `invalid-epoch-fails-closed`

- **Discovery:** data-integrity and protocol-format passes.
- **Primary evidence:** `read_epoch` accepts only 1-20 ASCII digits in `u64` range (`crates/lease/src/lib.rs:593-622`).
- **Discriminating fact:** ordinary and shared acquisition reject empty state, and every acquisition mode rejects nonempty, non-decimal, oversized, or out-of-range state with `InvalidData`; `open_lease_file` publishes only initialized canonical zero.
- **Existing evidence:** `invalid_epoch_states_fail_closed` exercises ordinary and floor-based acquisition for parser-invalid states and preserves bytes (`crates/lease/src/lib.rs:840-893`). `maximum_epoch_is_readable_but_exhausted` separates valid shared reads from exclusive exhaustion (`crates/lease/src/lib.rs:923-947`). Floor-based empty-state recovery is separately pinned by `exclusive_epoch_exceeds_resource_floor` at `crates/lease/src/lib.rs:720-739`.
- **Failure scenario:** a lease file in any format other than 1-20 ASCII decimal digits fails closed rather than silently issuing epoch 1.
- **Instrumentation:** a corruption-specific public error remains absent; callers see `LeaseError::Io`.
- **Open-question log:** none for the current decimal format.

## U2 audit

- **Classification:** `core`. The accepted byte grammar (1-20 ASCII digits, `u64` range) is a fencing-bytes contract.
- **New evidence:** `invalid_epoch_states_fail_closed` (`crates/lease/src/lib.rs:840-893`) gains six bodies: `+1`, `-1`, `U+0020 1`, `1 U+0020`, `0x1f`, `1_0`, where `U+0020` stands for one literal space byte, so the third body is a space followed by the digit one and the fourth is the digit one followed by a space. `str::parse::<u64>` accepts `+1`, and a trimming parser accepts the padded forms, so these cases separate the byte grammar from a lenient rewrite.
- **Discrimination:** a mutant that replaces the digit check with `from_utf8().trim().parse::<u64>()` fails on the new bodies.
- **Verdict:** pass; every body is asserted with exact `LeaseError::Io(InvalidData)`, the path in the message, and unchanged file bytes, through every acquisition mode that accepts the body class.

# `acquisition-does-not-follow-symlink`

- **Discovery:** targeted security refinement after portfolio evaluation.
- **Primary evidence:** `lease_open_options` applies Unix `O_NOFOLLOW`; Windows applies `FILE_FLAG_OPEN_REPARSE_POINT`, and `protect_open_file` rejects reparse-point metadata.
- **Existing evidence:** `acquisition_refuses_symlink_and_leaves_target_untouched` exercises exclusive and shared acquisition against an existing Unix symlink target. Windows compilation is checked separately.
- **Failure scenario:** Windows runtime behavior, other non-Unix targets, and dangling Unix symlink coverage remain untested.
- **Timing window:** symlink exists before open; no race is required.
- **Instrumentation:** existing-target content and mode snapshots exist; dangling-target and Windows runtime coverage do not.
- **Open-question log:** Windows deployment support is not declared, though lock-specific code compiles for Windows.

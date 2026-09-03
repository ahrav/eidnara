# `lease-file-creation-is-never-permissive`

- **Discovery:** targeted security refinement after portfolio evaluation.
- **Primary evidence:** after an existing-path open returns `NotFound`, `open_lease_file` creates a same-directory `NamedTempFile`, initializes it, and publishes the same open inode with `persist_noclobber` (`crates/lease/src/lib.rs:238-275`). `tempfile`'s private-file creation contract is part of this property.
- **Existing evidence:** `an_acquired_lease_file_is_owner_only` checks post-acquisition mode; no concurrent creation observer exists.
- **Failure scenario:** unsupported non-Unix permission semantics are outside this Unix-qualified property.
- **Timing window:** the final path is absent until the initialized private inode is published; descriptor-relative hardening still normalizes pre-existing files.
- **Instrumentation:** concurrent mode observer and open-success witness.
- **Open-question log:** none for Unix creation mode.

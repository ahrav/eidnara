# `unix-lease-file-is-owner-only`

- **Discovery:** security and bug-history passes.
- **Primary evidence:** `protect_open_file` performs descriptor-relative lease hardening; both acquisition modes use `open_lease_file`; commit `49bcaa2` records measured permissive deployment files.
- **Existing evidence:** `an_acquired_lease_file_is_owner_only` checks exclusive acquisition over a pre-existing `0644` file. Public `protect_file` has separate path-based behavior.
- **Failure scenario:** a restored permissive file remains permissive on the untested shared path. Creation-time exposure is cataloged separately.
- **Timing window:** descriptor open through descriptor-relative hardening; later path replacement remains a lock-domain risk.
- **Instrumentation:** compare opened/locked inode identity with the inode whose mode is checked; shared-path outcome is also missing.
- **Open-question log:** owner-only mode is Unix-only; no Windows ACL contract is documented.

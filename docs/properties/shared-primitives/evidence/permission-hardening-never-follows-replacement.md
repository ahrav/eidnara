# `permission-hardening-never-follows-replacement`

- **Discovery:** security wildcard pass.
- **Primary evidence:** `protect_open_file` applies metadata checks and `set_permissions` to the same opened `File`; `lease_open_options` uses Unix `O_NOFOLLOW`. Public `protect_file` uses path-based `symlink_metadata` and `set_permissions` without opening caller-owned files.
- **Existing evidence:** public-helper and acquisition symlink tests assert target content and mode. No deterministic path-replacement race is injected.
- **Failure scenario:** lease-path replacement after open can create a second lock domain but cannot redirect descriptor-relative lease hardening. Public `protect_file` can race with replacement between metadata and chmod.
- **Timing window:** lease open to lock remains relevant to inode replacement; public hardening has a separate metadata-to-chmod path race.
- **Instrumentation:** inode identity capture is still missing for the separate lock-domain replacement property.
- **Open-question log:** directory permissions, replacement actors, and process privilege in deployment are unknown.

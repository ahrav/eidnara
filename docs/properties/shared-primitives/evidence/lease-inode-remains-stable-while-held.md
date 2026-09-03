# `lease-inode-remains-stable-while-held`

- **Discovery:** concurrency, recovery, and resource passes.
- **Primary evidence:** `open_lease_file` returns an open inode and both acquisition methods retain its descriptor-bound lock; no later path/inode revalidation exists.
- **Documented lead:** `lease-store-density.md:22-24` says files are never unlinked to avoid the unlink-inode race.
- **Failure scenario:** holder locks inode A; external unlink/replacement creates inode B at the same path; another acquirer locks B successfully.
- **Timing window:** replacement after first open and before holder drop.
- **Existing evidence:** none; test cleanup occurs only after handle drop.
- **Instrumentation:** missing descriptor/path `(device,inode)` comparison.
- **Open-question log:** external consumer lease roots and cleanup actors were not supplied. Human deployment inventory is required.

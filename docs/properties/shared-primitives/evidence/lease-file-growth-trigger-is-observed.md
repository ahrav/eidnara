# `lease-file-growth-trigger-is-observed`

- **Discovery:** resource, product-context, and liveness passes.
- **Primary evidence:** `lease-store-density.md:3-51` reports 20,484 files, about 2.9 MiB/day physical growth, a 1 GiB trigger, and named watchers.
- **Implementation evidence:** `FileLeaseStore::lease_path` derives one deterministic file per distinct key, and production code never unlinks final lease paths.
- **Existing evidence:** no watcher, alarm delivery, or owner acknowledgement is inspectable in this repository.
- **Failure scenario:** ephemeral identities cross the configured threshold, but watcher heartbeat, signal delivery, or owner acknowledgement is absent.
- **Timing window:** months of production accumulation; a configurable small threshold makes campaign convergence constructible.
- **Instrumentation:** external watcher claimed but unavailable; inode monitoring not mentioned.
- **Open-question log:** watcher status and inode owner require human confirmation.

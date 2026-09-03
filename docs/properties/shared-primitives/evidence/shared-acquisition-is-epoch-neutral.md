# `shared-acquisition-is-epoch-neutral`

- **Discovery:** concurrency and protocol passes.
- **Primary evidence:** `open_lease_file` initializes canonical zero before no-clobber publication, and `FileLeaseStore::acquire_shared` uses only `try_lock_shared` plus `read_epoch` after open.
- **Existing evidence:** `concurrent_shared_first_acquisitions_coexist` synchronizes eight fresh-key readers and holds every successful guard until all results are observed.
- **Existing evidence:** `shared_first_initializes_canonical_zero` covers first creation; `shared_acquisition_does_not_bump_the_write_epoch` observes equal parsed epochs across shared acquisitions; `shared_holders_coexist_but_block_exclusive` holds two shared handles concurrently.
- **Failure scenario:** a refactor calls `bump_epoch_above` or writes reader metadata, consuming or racing the writer fence.
- **Enabling state:** prior nonzero writer epoch and simultaneous shared holders.
- **Instrumentation:** partial; no byte-level before/after observation.
- **Open-question log:** descriptor-relative hardening can mutate Unix mode on the shared path. Epoch-neutrality excludes writer-epoch changes, not file metadata changes or first-file initialization to zero.

# `shared-epoch-never-authorizes-write`

- **Discovery:** protocol-contract and interface wildcard passes.
- **Primary evidence:** `HeldFileLease::epoch` serves both modes; `FileLeaseStore::acquire` and `FileLeaseStore::acquire_shared` both return that concrete guard, while the shared-method docs restrict its epoch to observation.
- **Existing evidence:** `shared_acquisition_does_not_bump_the_write_epoch` confirms a shared handle returns the current writer epoch. No production in-repo caller uses `acquire_shared`.
- **Failure scenario:** consumer loses acquisition-mode provenance and passes shared `epoch()` into a write fence that accepts equal epochs.
- **Timing window:** no fault; misuse at consumer boundary.
- **Instrumentation:** missing guard-mode tag and write-site provenance assertion.
- **Open-question log:** external consumers named in the density doc were not supplied. Whether shared mode is used remains `(needs human input)`.

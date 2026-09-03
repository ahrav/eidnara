# `epoch-input-size-is-bounded`

- **Discovery:** resource-boundary pass.
- **Primary evidence:** `read_epoch` uses `Read::take(21)`, preallocates 21 bytes, and rejects lengths above the 20-byte decimal maximum (`crates/lease/src/lib.rs:756-785`).
- **Existing evidence:** `invalid_epoch_states_fail_closed` exercises a 21-byte file through ordinary and floor-based acquisition (`crates/lease/src/lib.rs:1003-1056`).
- **Failure scenario:** oversized restored or hostile files fail without proportional allocation.
- **Timing window:** none; file contents alone enable it.
- **Instrumentation:** a counting `Read + Seek` wrapper around the in-memory source gives the bytes-read total that the bound is asserted on; syscall-level read sizes against the real lease file are not observed or asserted.
- **Open-question log:** a future format must revise the 20-byte limit deliberately.

## U2 audit

- **Classification:** `core`. The 20-byte decimal format and the 21-byte read bound are the fencing bytes the record names.
- **New evidence:** `epoch_read_is_bounded_regardless_of_file_size` (`crates/lease/src/lib.rs:1859-1911`) wraps a 1 MiB cursor in a `CountingReader`, calls `read_epoch`, and asserts `InvalidData` with at most 21 bytes read through that wrapper; it then seeds a real 1 MiB lease file and drives exclusive, shared, and floor-based acquisition through it, asserting the error kind and unchanged bytes. The file-backed half asserts the outcome only; it does not count the bytes the `File` reads.
- **Discrimination:** replacing `take(EPOCH_WIDTH + 1)` with an unbounded read fails the byte-count assertion.
- **Verdict:** pass; the check `always(bytes_read_for_epoch <= 21)` is asserted directly on the counting wrapper rather than inferred from the rejection.

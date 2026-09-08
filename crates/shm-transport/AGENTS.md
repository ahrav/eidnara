# Shared-memory transport

Treat unsafe code in this crate as a verification boundary. After changing unsafe code, run the Miri and Valgrind commands from `.github/workflows/ci.yml`; ordinary Cargo tests are not enough.

`fuzz/` is a separate Cargo workspace with its own lockfile. Use `--manifest-path crates/shm-transport/fuzz/Cargo.toml` for its commands and `--locked` for commands that resolve dependencies.

Tests marked ignored as child roles are launched by parent tests. Do not add them to the default test run.

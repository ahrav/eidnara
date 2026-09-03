# `data-root-resolves-under-the-managed-directory`

- **Discovery:** U3, when the managed directory leaf was renamed and the resolver was split for Rust 2024.
- **Primary evidence:** `default_data_root(xdg_data_home, home)` in `crates/host-runtime/src/instance.rs` is the only place the two variables are interpreted; `data_dir_path` passes `std::env::var_os` values into it. `default_root_follows_xdg_then_home` drives every branch with explicit values and `explicit_override_resolves_canonical_layout` pins `<override>/eidnara/run`.
- **Existing evidence:** the two tests named above; `MANAGED_DIR_NAME` and `RUNTIME_DIR_NAME` are the only path segments and every managed path derives from them.
- **Failure scenario:** a relative `XDG_DATA_HOME` joined to the working directory.
- **Timing window:** none.
- **Instrumentation:** none.
- **Audit verdict (U3):** pass. The test spells the expected paths as literals (`/xdg-root/eidnara/run`, `/home-root/.local/share/eidnara/run`) rather than deriving them from the constants; relative, empty, and absent values are each covered; the predecessor test mutated process environment under a mutex, which the split removes.
- **Open-question log:** none.

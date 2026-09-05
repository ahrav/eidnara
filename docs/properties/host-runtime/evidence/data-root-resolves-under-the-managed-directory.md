# data-root-resolves-under-the-managed-directory

## Discovery trigger

At U3 the managed directory leaf was renamed to `eidnara` and the resolver
was split so the two environment values become arguments instead of being
read inside the function. The split was forced by Rust 2024, which makes
`std::env::set_var` unsafe and so made the predecessor's environment-mutating
test unwritable without `unsafe`. The property matters because every path the
host publishes, locks, or stores under derives from this one root: a root
that depends on the working directory scatters state by launch directory,
and a root under an attacker-chosen relative path lets the attacker pick
where the host writes.

## Evidence trail

All references are at `572315a`.

Entry point. `data_dir_path` (`instance.rs:130-143`) takes an optional
override. A relative override is refused as `Insecure` (`:133-136`) because
the setup socket path derives from it and
`ConnectionInfo::validate` rejects relative socket paths
(`connection_file.rs:70`). An absolute override is returned as is (`:137`).
With no override, `data_dir_path` reads `XDG_DATA_HOME` and `HOME` with
`std::env::var_os` (`:139-140`) and passes them into `default_data_root`
inside a `DataRootEnv` struct (`:147-150`) whose named fields prevent the
two values being swapped. A grep for `var_os` and `env::var` across
`crates/host-runtime/src` finds no other reader of these two variables; the
Broca backends set `HOME` in child environments (`broca/opencode.rs:179`,
`broca/pi.rs:322`) but do not read it.

Resolver. `default_data_root` (`:155-167`) defines `absolute`, which
converts a value to a `PathBuf` and keeps it only if `is_absolute()`
(`:156-159`). It returns an absolute `XDG_DATA_HOME` (`:160-161`), else
`$HOME/.local/share` for an absolute `HOME` (`:162-163`), else
`Err(InstanceError::NoDataDir)` (`:164`). An empty string is not absolute,
so empty and relative values fall through identically. `NoDataDir`'s
display text at `:78-81` tells the operator to set one of the variables or
an override.

Layout. `MANAGED_DIR_NAME` is `eidnara` (`:170`) and `RUNTIME_DIR_NAME` is
`run` (`:174`). `managed_dir_path` joins the first to the data root
(`:178-180`); `runtime_dir_path` joins the second to that (`:183-185`). The
only other consumer of `data_dir_path` is `coordination_dir_path`
(`lifecycle.rs:55`), which joins `.eidnara-coordination` beside the managed
subtree. The production caller is `InstanceGuard::acquire`
(`instance.rs:231-245`), which resolves `runtime_dir_path` at `:245` with
`config.data_dir.as_deref()` from `runtime.rs:565-568`; `HostConfig::data_dir`
defaults to `None` (`config.rs:214`, `:230`).

Existing checks, verified, both unit tests in `instance.rs` run under
`cargo test --workspace --all-targets` (`.github/workflows/ci.yml:118`):

- `explicit_override_resolves_canonical_layout` (`:860-864`) resolves
  `runtime_dir_path` over a temp root and asserts it equals
  `<root>/eidnara/run` built from string literals (`:863`).
- `default_root_follows_xdg_then_home` (`:867-904`) constructs
  `DataRootEnv` directly (`:868-876`) and asserts against literal
  `PathBuf`s: `/xdg-root/eidnara/run` when both are absolute (`:878-881`);
  `/home-root/.local/share/eidnara/run` for `XDG_DATA_HOME` of
  `relative/xdg`, `./xdg`, and `""` (`:884-890`) and for an absent
  `XDG_DATA_HOME` (`:892-895`); `NoDataDir` for an absent `XDG_DATA_HOME`
  with a relative `HOME` (`:898-901`) and for both absent (`:903`). The
  `resolve` closure appends `MANAGED_DIR_NAME` and `RUNTIME_DIR_NAME` from
  the constants (`:875`), so the expected side is literal and the actual
  side goes through the constants, which is the direction that detects a
  renamed constant.

## Failure scenario

1. A launcher exports `XDG_DATA_HOME=data` and starts the host from
   `/srv/a`, then a second launcher does the same from `/srv/b`.
2. A resolver that joined the relative value to the working directory would
   open `/srv/a/data/eidnara/run` and `/srv/b/data/eidnara/run`. Neither
   host sees the other's lifetime lock, so both run.
3. As written, `absolute` drops the relative value and both hosts fall back
   to `$HOME/.local/share`, sharing one root and one lock.

The second impact in the record is the attacker-chosen directory: a process
that can set a relative `HOME` or `XDG_DATA_HOME` for the host could
otherwise steer its runtime directory under any path reachable from the
working directory.

## Timing windows and dependencies

None. The resolver is a pure function of its two arguments. The one
environmental dependency is that `data_dir_path` reads the process
environment at call time; a host that changed its own environment between
calls could see two roots. No production code calls `set_var`, and the test
no longer needs to.

## What a test must construct

The record's check is fully constructed. The only combinations the table
does not include are a relative `XDG_DATA_HOME` with a relative or absent
`HOME`, and an empty `HOME`; by the code at `:160-164` each falls to
`NoDataDir`, but no assertion says so. An `is_absolute` value that is not a
canonical path, such as `/xdg-root/../other`, is accepted without
normalization; the record does not claim normalization, and no test asserts
either behaviour.

## Investigation log

### Q: Does the test still touch the process environment?

- Sources examined: `instance.rs:130-143`, `:867-904`; a grep for
  `set_var` in `crates/host-runtime`.
- Findings: the test builds `DataRootEnv` values inline and never calls
  `std::env::set_var` or `var_os`. The only `set_var` calls in the crate
  are in the Broca fixture child at `tests/broca_subprocess.rs:277-279`,
  which runs in a separate process. `data_dir_path` is the only reader, and
  it is not under test here; it is exercised indirectly by every
  `InstanceGuard::acquire` call with `None`, which the unit tests avoid by
  always passing `Some(root)`.
- Missing evidence: a test of `data_dir_path(None)` itself. Such a test
  would need the process environment and is the case the split removed.
- Conclusion: resolved. The resolver's branches are covered without
  environment mutation; the two-line glue at `:139-140` is covered only by
  reading.

### Q: Does the record's signature match the code?

- Sources examined: `instance.rs:147-150`, `:155`; the catalog record's
  `Check` line.
- Findings: the record writes `default_data_root(xdg, home)`. The function
  takes one `DataRootEnv` argument with fields `xdg_data_home` and `home`
  (`:147-150`, `:155`). The behaviour is as the record describes; the
  calling shape differs.
- Missing evidence: none.
- Conclusion: resolved with a note. The record's shorthand is accurate about
  the two inputs and their precedence; the actual parameter is a named
  struct, which is the mechanism that prevents swapping them.

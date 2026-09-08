# Host runtime

`tests/broca_subprocess.rs` uses `harness = false`. Add each new case to the test table in `main`; `#[test]` does not register it.

Keep external-runtime and child-role tests ignored. Run external-runtime tests only with their documented environment variables; parent tests launch child roles themselves.

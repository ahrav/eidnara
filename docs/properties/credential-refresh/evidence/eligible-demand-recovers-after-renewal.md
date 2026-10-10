# eligible-demand-recovers-after-renewal

- `crates/daemon/src/source_recovery_tests.rs:68`, `:93`, `:120`, `:151` fold
  again on the normal, emergency, wrapup, and reattach paths after a typed
  source failure and its cooldown.
- `crates/host-runtime/tests/model_execution_subprocess.rs:4266`
  `a_day_of_rotations_and_an_external_login_reuses_one_adapter_and_owner`
  runs 48 Bedrock runs over a virtual day through one owner and one adapter,
  with 24 rotations and one external login, and asserts the exact row
  sequence.

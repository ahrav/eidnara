# bounded-history-outage-recovery

- Gates: `gate()` and `QualificationReport::build` in
  `crates/eval-core/src/qualification.rs`; a report qualifies only with no host
  shortfall, no cgroup constraint, every witness run passed at its source
  commit, and every gate empty.
- Lineage oracle: `audit_lineage` at `qualification.rs:910`, with failing
  controls in the same file's tests.
- Outage judge: `the_outage_judge_fires_on_each_failure` at `:1296`.
- Warm acquisition: `warm_acquisition_p99_stays_within_one_millisecond` at
  `crates/host-runtime/tests/model_execution_supervisor.rs:1813`, ignored and
  release only.
- No qualifying run exists: this host has 128 CPUs and runs under a cgroup.

# dec-a-commit-cluster-trigger-config-is-inert-in-this-crate

## Discovery trigger

The source catalog described configurable commit-cluster triggering, while the
daemon supplied fixed values. That config-wiring observation remains at
`74044960ee91641dec95c8552f15282844a18b13`. It is scoped to the daemon reader,
not a claim about every TypeScript or external consumer. Rust paths below are
relative to `crates/daemon/src/`.

## Evidence trail

The daemon defines `DEFAULT_COMMIT_CLUSTER_TRIGGER_ENABLED` as `true` and
`DEFAULT_MIN_COMMIT_CLUSTERS` as `3` (`lib.rs:642-643`). Its production trigger
context uses those constants (`lib.rs:4985-5007`). The boundary's commit-cluster
arm requires the enable flag, sufficient clusters, and sufficient tokens
(`boundary.rs:814-819`).

`DaemonConfig` has no commit-cluster fields (`config.rs:80-109`), and the
consumed-key table has no corresponding pointer (`config.rs:588-618`). Supplying
an unknown `commit_cluster_trigger` block therefore does not change the context
or produce a key-specific unsupported-setting warning through the classified
merge (`config.rs:710-755`).

The source-catalog defaults were also `true` and `3`. A default-valued fixture
cannot distinguish config wiring from constants. The original configuration
document is absent here; its configurability claim is retained as a historical
contract under test, not as a newly verified user-facing document.

## Failure scenario

A user supplies `commit_cluster_trigger.enabled: false` or a nondefault
`min_clusters`. The daemon still evaluates the commit-cluster arm using `true`
and `3`. A behavioral check must isolate that arm from pressure and tail-size
triggers; otherwise an unchanged fire result does not prove a wiring defect.

## Timing windows and dependencies

No timing fault is required. Reachability is `default-production` for the
context construction in `prepare_historian_fire`. A meaningful config test needs
a nondefault supplied value plus either observation of the constructed context
or a workload whose outcome distinguishes the requested value from the constants.

## What a test must construct

Use an explicit nondefault user value and assert that it reaches the trigger
context or produces a key-specific unsupported-setting diagnostic. For a
behavioral oracle, provide enough commit clusters and tokens for one setting but
not the other, while keeping the other trigger conditions false.

Existing checks remain `unaudited`: the default-constant assertion at
`boundary.rs:2011-2015` pins the default, not configurability. Direct contexts in
tests supply `min_commit_clusters: 2` with enabled and disabled flags
(`lib.rs:16662-16679`, `lib.rs:16928-16946`); they do not load these controls
from config.

## Investigation log

### Q: Does the classified reader consume the commit-cluster controls?

- Sources examined: `config.rs:80-109`, `config.rs:588-618`,
  `config.rs:710-755`, and `lib.rs:4985-5007`.
- Findings: the reader has no matching fields or keys; the context receives
  constants. Default-valued comparisons cannot distinguish these paths.
- Missing evidence: a config-to-trigger check with a discriminating nondefault
  input; confirmation of the intended daemon-facing configuration contract.
- Conclusion: resolved with answer for implementation behavior. The retained
  configurability obligation needs human input rather than an inferred policy
  change.

### Historical investigation

The [pre-refresh evidence](https://github.com/ahrav/eidnara/blob/74044960ee91641dec95c8552f15282844a18b13/docs/properties/daemon/decisions/evidence/dec-a-commit-cluster-trigger-config-is-inert-in-this-crate.md)
preserves the configuration quotation and cross-component leads. Its source
coordinates and product-wide conclusions are not current confidence evidence.

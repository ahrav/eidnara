# dec-a-execute-threshold-lower-bound-is-documented-20-and-enforced-1

## Discovery trigger

The source catalog quoted a `20-90` execute-threshold range and found a Rust
floor of one. The Rust behavior remains at
`74044960ee91641dec95c8552f15282844a18b13`. The source-catalog
`CONFIGURATION.md` is absent here; its quoted range is historical contract
evidence, not a verified current document.

All Rust references below name `crates/daemon/src/config.rs` at that revision.

## Evidence trail

The default is `65.0` (`config.rs:20-21`, `config.rs:115`), and the ceiling is
`90.0` (`config.rs:26-27`). The final merge clamps the effective threshold to
`[1.0, 90.0]` (`config.rs:750-752`), with no range warning.

`number_at` accepts a finite numeric leaf (`config.rs:963-968`), and `apply_key`
assigns it (`config.rs:807-811`). Thus a user value of `5` stays five; `0.5`
becomes one. Neither is raised to twenty.

The project class is raise-only (`config.rs:682-686`). A project value below
the effective user value is rejected by the candidate comparison
(`config.rs:734-744`). A project-only value of five therefore cannot reproduce
the user-tier lower-bound behavior against the default of sixty-five.

## Failure scenario

If the source-catalog `20-90` obligation remains the intended contract, a user
value below twenty is accepted without the required clamp, rejection, or warning.
This is a config-resolution claim. It does not imply that every low-threshold
pass executes regardless of scheduler gates or workload.

## Timing windows and dependencies

There is no timing requirement for the merge check. Use only the user tier so a
project-policy warning cannot satisfy the range oracle. File-cache reuse is not
needed to construct this case.

## What a test must construct

Resolve standard user values such as `5` and `0.5`, with no project tier. Assert
that the effective threshold is at least twenty or the warning vector identifies
`/execute_threshold_percentage`. The existing implementation does neither.

`project_threshold_may_only_raise` (`config.rs:1297-1302`) asserts `70` raised by
project `91` produces `90`. `default_threshold_matches_typescript_schema`
(`config.rs:1305-1308`) pins sixty-five. Both have status `unaudited`; neither
exercises the lower bound.

## Investigation log

### Q: Is the lower-bound case reachable through the project tier alone?

- Sources examined: `config.rs:682-686`, `config.rs:710-755`, and
  `config.rs:807-811`.
- Findings: a project cannot lower the default. An explicitly low user value
  reaches the clamp without project rejection.
- Missing evidence: none for that path.
- Conclusion: resolved with answer. Use a user-only fixture.

### Q: Which lower bound is the intended contract?

- Sources examined: the final Rust clamp and the source-catalog quotation.
- Findings: the implementation floor is one; the historical quoted floor is
  twenty. The original configuration document is not present at HEAD.
- Missing evidence: confirmation that the historical range is the intended
  daemon contract, rather than a constraint on a separate frontend.
- Conclusion: needs human input. This documentation refresh changes no policy.

### Historical investigation

The [pre-refresh evidence](https://github.com/ahrav/eidnara/blob/74044960ee91641dec95c8552f15282844a18b13/docs/properties/daemon/decisions/evidence/dec-a-execute-threshold-lower-bound-is-documented-20-and-enforced-1.md)
contains the original range quotation and downstream investigation. Its source
coordinates and unconditional execution predictions are not current evidence.

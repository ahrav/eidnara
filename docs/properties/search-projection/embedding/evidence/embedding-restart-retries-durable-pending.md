# embedding-restart-retries-durable-pending

## Discovery trigger

P1 lines 109 and 149 require durable pending work to survive dispatch and
process restart. The intended recovery must make bounded progress once the
lane and store are healthy; keeping a row forever is not sufficient.
Sources, date, and SHA: [source register](../catalog.md#source-register).
Reachability is test-only because no production RP2.1 restart driver exists.

## Evidence trail

- `crates/host-runtime/src/synapse/jobs.rs:323-342` creates a fresh incarnation
  nonce and empty maps on construction.
- `jobs.rs:349-356` rejects job IDs from another incarnation.
- `jobs.rs:597-606` reports Restarted for unknown or expired/evicted jobs too;
  the outcome cannot identify which loss caused it.
- `jobs.rs:407-458` permits identical retryable failures to be replaced after
  a successful fresh admission, while other retained jobs are reused.
- `crates/host-runtime/tests/synapse_jobs.rs:297-334` preserves local work across
  route loss. That does not construct loss of the host process.
- `crates/host-runtime/src/synapse/jobs.rs:1123` checks retained retry behavior,
  not reconstruction from a durable store.
- `docs/host-wire-protocol.md:502` describes a TypeScript recovery ledger.
  P1 moves product recovery to durable search pending; the wording needs review.
- `crates/kernel/tests/cas_fault_injection.rs:924-989`, `:1046-1094` supplies
  the reusable, unaudited barrier/kill/reopen pattern. Product pending/vector
  hooks and the recovery oracle remain missing.

## Failure scenario

A descriptor or result is lost and the host restarts. The driver polls only
the old job ID, receives Restarted, and never rescans durable Pending(K).
Alternatively it retries indefinitely with a permanent invalid identity or
resets its deadline every pass, hiding lack of bounded recovery.
Counting exactly one inference over all lifetimes would reject valid recovery:
pure computation may repeat after local retention or incarnation loss.
The meaningful effect is one valid durable completion per current K.

## Timing windows and dependencies

Crash after pending commit, after admission, after inference, and after vector
commit but before completion. The durable-completion record owns the safety
of each residue; this record owns recovery from recoverable residues.
Freeze a finite target set and provide an approved fault-free service window.
Continued model failure, unbounded competing queries, or repeated identity
changes do not satisfy the progress premise and must be reported as such.
RP2.9 sets attempt and recovery bounds; a generous arbitrary timeout is invalid.
The check applies per admitted recovery episode with independently witnessed
premises. It remains RP2.9-blocked. A missing required episode cannot pass the
[shared acceptance check](../../projection-coverage/catalog.md#projection-acceptance-situations-witnessed).
Projection catch-up and authorized recovery have their own
[convergence record](../../export-recovery/catalog.md#rp21-catchup-and-authorized-recovery-converge).

## What a test must construct

Persist K and retain its identity outside the killed process. Restart with a
fresh JobTable and unchanged valid model; verify old IDs are not reused as
authority. Let the scanner discover durable work and obtain a new descriptor.
Stop faults and require Complete(K) plus V(K) within approved attempts/window.
Include retryable execution failure, result expiry, and lost descriptor cases.
Include a changed-identity control that becomes obsolete without completing
newer work. Keep attempts, accepted jobs, and committed product effects separate.
`rp21_embedding_restart_has_stable_pending_and_service` records a fresh process,
durable current pending work, and available service, not successful recovery.
No in-memory reset or orderly route close substitutes for actual termination.
Adapt the existing kernel CAS process-crash pattern rather than add a broad
crash framework. Its developer-filesystem scope at
`crates/kernel/tests/cas_fault_injection.rs:1-6` does not prove power-loss behavior.

## Investigation log

### Q: Does module_restarted mean the previous computation never happened?

- Sources examined: `jobs.rs:597-606`; P1 state transitions; wire line 502.
- Findings: It merges unknown, expired, evicted, and foreign-incarnation states.
  The computation or vector commit may already have happened.
- Missing evidence: None for the ambiguity of that existing outcome.
- Conclusion: Resolved. Reconcile durable K; do not infer absence of effects.

### Q: What bounds recovery and which failures stop automatic retry?

- Sources examined: P1 lines 115-121 and U3; P2 limit-approval contract.
- Findings: Numeric limits are explicitly unset. Local retryable-failure rules
  do not define the durable scanner's attempt policy or operator transition.
- Missing evidence: Approved retry attempts, service opportunity, recovery
  interval, and permanent-failure/operator semantics.
- Conclusion: Needs human input. The finite recovery check is blocked on L.

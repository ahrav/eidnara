# cc-read-distinguishes-disabled-unavailable-and-empty

Verified at the #1168 branch tip (base `cda655423`).

## Discovery trigger

An agent that reads an empty page as "no contradiction" when the feature is
off or the store is down draws a false conclusion, so every terminal must
differ from an empty page.

## Evidence trail

- `crates/daemon/src/contradiction_candidates/read.rs:225` answers
  `disabled` for a false frozen flag before the store is reached; `:229`
  answers `store_unavailable` for a daemon with no Memory Store.
- `crates/daemon/src/contradiction_candidates/read.rs:247` answers
  `disabled` for a root whose memories authority is not MODULE through
  `module_authority` (`:182-190`), which answers `store_unavailable` for a
  failed lookup (`:188`; the same helper judges the not-live path at `:264`),
  and a failed page read answers it too (`:283-286`).
- Kernel states come from `kernel_request` and `state_only` and carry no
  `items` key.
- The authority lookup and the page read wait for the store connection
  through `module_authority_for_route_within` and
  `candidates_for_decision_under_route_within`, which judges the route's
  MODULE authority in the connection that reads the page, under the read's
  deadline, so a held connection answers `store_unavailable` at the bound.
  The statements inside the transaction are indexed seeks over at most 65
  rows (`crates/memory-store/baseline.sql:1294`; `PageQuery::read` holds the
  keyset statements and the `limit + 1` bound,
  `crates/memory-store/src/contradiction_candidates.rs:872`), below the 1000
  VM-step interval at which `with_conn_interruptible` polls a stop
  (`crates/storage/src/lib.rs:191`), so the connection wait is the one wait
  the deadline ends
  (`crates/daemon/src/candidate_pass_tests.rs:2500`, asserted at `:2533-2537`).
- `docs/host-wire-protocol.md:861` documents the three terminals, the kernel
  states, and the empty page.

## Failure scenario

A Kernel that is starting answers `{items: [], next: null}`; the agent
concludes the memory has no candidates.

## Timing windows and dependencies

The flag is frozen at route bind, so a configuration change after the bind
does not change the route's answer.

## What a test must construct

A flag-off route with the table renamed away and a flag-on control; a root
without MODULE authority; a daemon with no Memory Store; a Kernel not yet
open and one whose open failed; an eligible decision with zero rows. All in
`crates/daemon/src/candidate_pass_tests.rs:1849` and `:2562`.

## Investigation log

### Q: Does a failed Kernel open answer differently from a starting one?

- Sources examined: `crates/daemon/src/candidate_pass_tests.rs:2577`, `:2594`.
- Findings: a Kernel not yet open answers `unavailable`/`store_starting`;
  opening at a file path answers `unavailable`/`store_unavailable`.
- Missing evidence: none.
- Conclusion: resolved with answer.

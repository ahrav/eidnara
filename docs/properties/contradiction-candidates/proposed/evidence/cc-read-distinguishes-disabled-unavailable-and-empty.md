# cc-read-distinguishes-disabled-unavailable-and-empty

Verified at the #1168 branch tip (base `cda655423`).

## Discovery trigger

An agent that reads an empty page as "no contradiction" when the feature is
off or the store is down draws a false conclusion, so every terminal must
differ from an empty page.

## Evidence trail

- `crates/daemon/src/contradiction_candidates/read.rs:148` answers
  `disabled` for a false frozen flag before the store is reached; `:135`
  answers `store_unavailable` for a daemon with no Memory Store.
- `crates/daemon/src/contradiction_candidates/read.rs:160` answers
  `disabled` for a root whose memories authority is not MODULE, and a failed
  authority lookup or page read answers `store_unavailable`.
- Kernel states come from `kernel_request` and `state_only` and carry no
  `items` key.
- `docs/host-wire-protocol.md:827` documents the three terminals, the kernel
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
`crates/daemon/src/candidate_pass_tests.rs:1242` and `:1575`.

## Investigation log

### Q: Does a failed Kernel open answer differently from a starting one?

- Sources examined: `crates/daemon/src/candidate_pass_tests.rs:1590`, `:1607`.
- Findings: a Kernel not yet open answers `unavailable`/`store_starting`;
  opening at a file path answers `unavailable`/`store_unavailable`.
- Missing evidence: none.
- Conclusion: resolved with answer.

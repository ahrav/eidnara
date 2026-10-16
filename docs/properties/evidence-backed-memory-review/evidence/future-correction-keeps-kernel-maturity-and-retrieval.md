# future-correction-keeps-kernel-maturity-and-retrieval

## Discovery trigger

#597 acceptance: create admits `ModelInference` with `Other`, and the actual
disposition, visibility, and maturity are verified even when the outcome word
is `deny`; revise creates a fresh successor that preserves domain and source
identity, increases the source revision, inherits no trusted maturity, and
never folds into a survivor; explicit retrieval can see the successor while
automatic retrieval respects the candidate ceiling.

## Evidence trail

`crates/kernel/src/review_acceptance.rs` - `revise` derives a fresh object id
from the settlement key, refuses one already in the registry (`fresh`), and
writes the successor through `Envelope::supersede_decision`; `admit` cites the
support through decision events and records the admission, whose
`AdmissionDecision` becomes the settlement's `AdmittedState`.

`crates/kernel/src/admission.rs` - `visibility_row` maps an active candidate to
`ExplicitLabeled`; `Automatic` needs `Verified` or above.

## Failure scenario

The successor reuses a live object id and folds the predecessor into it, or is
admitted with the predecessor's explicit-user maturity and surfaces
automatically.

## Timing windows and dependencies

None.

## What a test must construct

An explicit-user decision revised through an accepted proposal; a staged
subject created through one. Read each new decision's registry row, its stored
admission, and its surface eligibility for explicit search and auto-inject.

## Investigation log

### Q: Does the successor's admission read the predecessor's history?

- Sources examined: `crates/kernel/src/admission.rs` (`load_prior_decision`).
- Findings: the revise test asserts `candidate` for the successor of an
  explicit-user decision, so no prior maturity reaches it.
- Missing evidence: none.
- Conclusion: resolved with answer.

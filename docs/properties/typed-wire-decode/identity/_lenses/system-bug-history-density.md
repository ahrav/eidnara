# System lens: bug history and density

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: issues 350, 426, 435, 436, 438, 441, and 524.

## Observations

The issues were read, including returned comments, as planning context. Issue
426 requests canonical served bytes and digest-indexed lookup. Issues 350 and
435 preserve frozen behavior; issue 524 concerns owned transport input.
An issue's title, state, or description does not establish an identity defect.

`git diff --name-only e451a2b4 HEAD` over the plan's core identity sources,
plugin emitter, and normative protocol returns no paths. The core baseline
source is therefore available without switching to stale local `main`.

`crates/daemon/src/transform.rs:13797-13938` contains detailed receipt
witnesses, including signed zeros and synthetic distinguishable receipts.
`crates/memory-store/src/lib.rs:16096-16112` pins old unknown-field replay.
These are claim-bearing checks, not runtime incident evidence.

## Narrow nonapplicability and missing evidence

No additional incident, external repository, production dump, or reproduction
was supplied. No root cause is promoted from issue wording. History inspection
here establishes baseline availability, not a statistical churn ranking.
Candidate: re-audit the changed contract behind dense legacy tests.

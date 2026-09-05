# Portfolio evaluation: Discovered at U3 (host-runtime)

Run by an independent evaluator with fresh context that had not seen the
discovery reasoning, against the 16 records under `## Discovered at U3` in
[catalog.md](../catalog.md) (`:9875-10120`), their 16 evidence files, and the
just-written [existing-checks.md](existing-checks.md) and
[fault-map.md](fault-map.md). The charter was to expose systematic gaps rather
than to agree. The verdict is **REFUTED as finished**, and this file records
that rather than softening it.

The shape of the findings is specific to what this record set is. The earlier
parts were written against code with almost no executing coverage, and their
evaluations found checks that could not decide. This set is the opposite
starting position: 237 claim-bearing checks, every one of them running in CI
twice, and the inventory is right that this is a stronger position than any
earlier part had. **The failure mode that comes with that position is
different, and all three of the gaps below are instances of it: the records
were written outward from the checks that exist, so the properties they state
are the ones the suite already names, and the suite's own blind spots became
the catalog's.** 109 of the 237 checks map to no record
(`existing-checks.md:460`), and the highest-impact unmapped cluster is six
tests asserting that a Broca cancellation cannot be overwritten by a later
completion, which is the single most consequential invariant in the Broca area
and is not a record.

The second shape is narrower and mechanical. **Six of the sixteen records say
`Open questions: None.` while their own evidence file ends an investigation-log
entry with `unresolved` or `needs human input`.** METHOD.md's rule 2 makes
those conclusions valid, and the schema puts them on the record. Three of the
six carry a reachability conclusion that contradicts the record's own
`Reachability:` clause. The information was not missing and the inference was
not missing; the conclusion simply did not travel from `../evidence/` back into
`../catalog.md`.

Four lenses: harness fit, coverage balance, implementability, and a wildcard
pass questioning the framing.

## Provenance

Branch `u3/16-catalog-host-runtime`, `HEAD` = `572315a` ("Address host-runtime
catalog review findings"), confirmed with `git log -1`, which is what the three
artifacts state. The working tree is clean apart from seven modified evidence
files and the untracked `discovered-at-u3/` directory. Every `file:line` below
was printed individually at that commit.

This evaluation created only this file. No record, source file, test, or
sibling artifact was edited, and no formatter was run. Dispositions below are
proposed, not applied.

Mechanical state, checked for this pass: 16 records, 16 entries in
`../index.json`, 16 files in `../evidence/`, slugs equal across all three,
every evidence link resolves. `../index.json`'s `source_sha256` matches
`../catalog.md`'s current digest. Evidence files run 131 to 158 lines, below
the rest of the part's median of 167, so METHOD.md's 60-to-120 target is
exceeded by the whole part and not by this set in particular.

Two facts underpinning several findings, recorded as facts rather than
impressions. `rg 'OpenCodeBackend::new|PiBackend::new' crates/` returns zero
hits anywhere in the workspace, tests included. `rg 'profile\.|\[profile'
--glob '*.toml' .` returns zero hits and there is no `.cargo/config.toml`.

## Disposition summary

| Category | Count | Proposed status |
| --- | --- | --- |
| refinement | 8 | edits to record fields and to the two support artifacts |
| gap | 3 | queued for a follow-up pass, not mined |
| bias | 3 | require human judgment; one of them decides 16 labels |

Record count stays 16 under every refinement. No refinement adds, removes,
splits, or renames a record. Six records gain an open question, four gain a
corrected `Reachability:` note or `Exercised:` line, and two gain a corrected
`Check:` line.

## Distribution, this set against the other 133 records in the part

Computed from `../index.json`, which carries all 149 records.

| Field | Discovered at U3 (16) | Rest of the part (133) |
| --- | --- | --- |
| Type | 15 safety, 1 liveness, 0 reachability | 106 safety, 8 liveness, 19 reachability |
| Semantics | 16 `always` | 108 `always`, 10 `sometimes`, 6 `always-or-unreached`, 6 `reachable`, 3 `unreachable` |
| Reachability class | 16 `default-production` | 112 `default-production`, 13 `test-only`, 8 `explicit-config-only` |
| Exercised | 8 `yes`, 8 `partial`, 0 `not yet` | 7 `yes`, 60 `partial`, 66 `not yet` |
| Confidence | 7 high, 9 medium, 0 low | 127 high, 6 medium, 0 low |
| Existing check named | 16 of 16 | 81 of 133 |
| Records with an open question | 2 of 16 | 59 of 133 |

Three of those rows are the evaluation in compressed form.

**Semantics is monotone.** This is the first set in the part with zero
non-`always` records. The fault map notices and states it plainly
(`fault-map.md:176-179`), then treats it as a convenience: no record carries a
marker, so the forbidden `always(!X)`/`sometimes(X)` pairing cannot arise. That
is true and it is not the interesting consequence. See gap G3.

**`Exercised` and `Confidence` move in opposite directions.** The `yes` rate
goes from 5 percent to 50 percent while the high-confidence rate falls from 95
percent to 44 percent. Part of the first move is real: these checks execute,
and the earlier inventories' "runs in no CI job" statements do not hold at this
`HEAD`, which this inventory verifies correctly at `ci.yml:118`, `:122`, `:126`
and `:14`. But `Exercised:` is a claim about constructed coverage and
`Confidence:` is a claim about verified facts, and a set that is
simultaneously more covered and less verified than every predecessor deserves
an explanation the artifacts do not give. Refinements R4 and R7 each move one
record from `yes` toward `partial`, and bias B3 decides whether six more
should move.

**Records with an open question drop from 44 percent to 12 percent, and the
evidence files disagree.** Seven of the sixteen evidence files reach an
`unresolved` or `needs human input` conclusion. Two records carry an open
question. See refinement R1.

## Independent evaluation, four lenses

**Harness fit.** The fixture inventory is accurate and the fault map's family
grouping is the right frame: `ScriptedBackend`, the self-re-executing
`harness = false` binary, and `DeterministicEngine` cover the three
subsystems, and the map's claim that the fixtures for all nine fault-needing
records already exist holds up. Two harness findings survive that. First, the
one timing-critical oracle in the set runs on a real clock at 2.5 times the
bound it is meant to pin, in the one test file that uses no paused clock (R6).
Second, `hang_ignore_term` is the fixture that reaches the reaping record's
escalation path, and it is also the input on which the record's `always` is
false (R3), which is the same "the fixture exists and cannot measure the
thing" split the runtime-config evaluation had to draw for `F7`.

**Coverage balance.** Areas are unevenly served by line count and much more
unevenly by property kind. Broca gets 5 records for 4,960 lines and Synapse 4
for 4,968, which is defensible; `harness_closure.rs` gets 1 for 1,146, which
is bias B2. The real imbalance is by kind. Every one of the 16 records states
a validation, identity, placement, or bound property, and not one states a
concurrent-lifecycle property, in two subsystems whose source comments are
almost entirely about concurrent lifecycle. Gaps G1 and G2 are the two largest
holes that leaves.

**Implementability.** Strong, and stronger than the ranking claims. Every
record names a fixture that exists, and the ranking by cheapest valid oracle
is correctly ordered except that its two hedges are both resolvable from the
checkout and both resolve in the cheap direction (R8). The one record that
cannot be made non-vacuous by fixture work is the route-open record, because
it has no production evaluation point to instrument (R4), and the one blocked
clause is `CREDENTIAL_ROW_CAP_BYTES`, which the inventory correctly reduces to
a code fact.

**Wildcard.** Two framing questions the other three lenses do not reach. The
first is that the record set inherits the test suite's shape, which is visible
in the distribution table's last two rows and is the common cause of all three
gaps. The second is that `default-production` is asserted 16 times in a
checkout where the production consumer does not exist, and where three of the
records' own evidence files say so; the fault map raises this and changes
nothing (`fault-map.md:60-61`). That is refinement R2 for the parts that are
factual and bias B1 for the part that is judgment.

## Refinements

Ordered most systematic first. R2 and R3 interact: both rest on the same Broca
entry-point census, and R3's correction is the reason R2's labels cannot be
repaired by a note alone.

### R1. Six records say `Open questions: None.` while their evidence file does not

Six of the sixteen evidence files end an investigation-log entry with
`unresolved` or `needs human input`, and their records carry no open question.
METHOD.md:14-15 makes both conclusions valid outcomes, METHOD.md:52 says to
append `(needs human input)` to the record's list, and METHOD.md:55 permits
`Open questions: None.` only when there are none.

| Record | Evidence conclusion | Substance |
| --- | --- | --- |
| [host-proof-construction-matches-the-committed-vectors](../catalog.md#host-proof-construction-matches-the-committed-vectors) | [`:136`](../evidence/host-proof-construction-matches-the-committed-vectors.md) unresolved | `docs/host-wire-protocol.md` section 5.2's example proofs match neither the committed vectors nor a recomputation under the documented daemon version. A contract-versus-code disagreement, which METHOD.md:16-19 says to report with both sides cited |
| [coordination-locks-live-beside-the-managed-subtree](../catalog.md#coordination-locks-live-beside-the-managed-subtree) | [`:148`](../evidence/coordination-locks-live-beside-the-managed-subtree.md) needs human input | the cutover probe must digest the current coordination directory separately from the predecessor's, and this tree does not record the predecessor name |
| [broca-identical-resends-converge-on-one-run](../catalog.md#broca-identical-resends-converge-on-one-run) | [`:145`](../evidence/broca-identical-resends-converge-on-one-run.md) needs human input | the guarantee states no bound, and the code bounds convergence to the 15-minute terminal retention and the 256-session cap. Either the guarantee gains "within the retention window" or the design intends a dedup the process-local index cannot provide |
| [broca-children-are-reaped-as-a-process-group](../catalog.md#broca-children-are-reaped-as-a-process-group) | [`:154`](../evidence/broca-children-are-reaped-as-a-process-group.md) unresolved | the shutdown test discards the unresolved count and tolerates four seconds of grandchild-probe lag, so it cannot refute a late kill |
| [broca-child-environment-carries-only-the-provider-row](../catalog.md#broca-child-environment-carries-only-the-provider-row) | [`:145`](../evidence/broca-child-environment-carries-only-the-provider-row.md) needs human input | in this checkout the aggregate cap and the credential verifier are reached only from tests, and the wiring that decides the production path is out of tree |
| [synapse-inference-runs-through-a-sealed-runtime-image](../catalog.md#synapse-inference-runs-through-a-sealed-runtime-image) | [`:131`](../evidence/synapse-inference-runs-through-a-sealed-runtime-image.md) unresolved | whether `ort` 2.0.0-rc.13's `init_from` can fall back to `ORT_DYLIB_PATH` or a default search path when the given path fails was not read |

The proposed disposition is to lift each conclusion onto its record verbatim,
not to resolve any of them. Three carry a further consequence handled below:
the third feeds gap G1's neighbouring question, and the fourth is the evidence
half of refinement R2.

### R2. Three `Reachability:` notes are contradicted by the crate's own constructors

These are factual corrections, separable from the blanket-label judgment in
bias B1. Each note asserts a production behaviour that a caller in this tree
refutes.

**[credential-fingerprint-derives-from-the-product-domain](../catalog.md#credential-fingerprint-derives-from-the-product-domain)
says "every provider credential row is fingerprinted before a harness
spawns".** The fingerprint check runs only under
`if let Some(verifier) = &self.credential_verifier` (`broca/mod.rs:223-235`).
`BrocaComponent::new` sets `credential_verifier: None` (`mod.rs:73-80`, the
field at `:80`). The verifier is installed only by `new_with_credentials`
(`mod.rs:82`), whose sole caller in the workspace is
`tests/broca_protocol.rs:443`. So a component built through the crate's default
constructor fingerprints nothing, and every other test and both examples use
that constructor. The derivation function is still exercised, at
`subprocess.rs:174` through `provider_row`, which production paths do call
(`opencode.rs:116`, `pi.rs:215`); the *check* is what has no production
producer here.

**[broca-child-environment-carries-only-the-provider-row](../catalog.md#broca-child-environment-carries-only-the-provider-row)
says "every harness child receives the snapshot environment".** `EnvSnapshot`'s
only public constructor is `capture_from` (`subprocess.rs:97`), and its callers
are `tests/broca_subprocess.rs:2827`, `:2836`, `:2841`, `:2871`, `:2885`,
`tests/broca_protocol.rs:436`, and the inline test at `subprocess.rs:1662`.
Nothing in `src/` builds one. Every consumer takes it as an argument
(`OpenCodeBackend::new` `opencode.rs:39`, `PiBackend::new` `pi.rs:60`,
`new_with_credentials` `mod.rs:82`), and the first two have zero callers
anywhere in the workspace. The record's own evidence file states this and
concludes `needs human input`.

**[coordination-locks-live-beside-the-managed-subtree](../catalog.md#coordination-locks-live-beside-the-managed-subtree)
says "every incarnation takes the lifetime and transaction locks", and its
`Check:` names `LifecycleTransactionLock::acquire_exclusive`.** Production
takes the lifetime lock: `runtime.rs:565` calls `InstanceGuard::acquire`, which
calls `LifetimeLock::acquire` at `instance.rs:244`, which opens `lifetime.lock`
at `lifecycle.rs:182`. Nothing in production takes the transaction lock.
`acquire_exclusive` (`lifecycle.rs:456`) has callers only inside
`lifecycle.rs`'s `#[cfg(test)]` module (`:1135` onward). `acquire_shared`
(`:471`) has one non-test caller, `probe_lifecycle` (`:805`, using it at
`:873`), which is `pub` and re-exported at `lib.rs:73` and which nothing in
this tree calls. What production does reach is the lock *file*:
`open_coordination_lock_create` materializes both names in a loop
(`lifecycle.rs:78`), so a lifetime acquisition creates `transaction.lock` too.
The honest split is that the placement half is production through file
creation, and the `acquire_exclusive` oracle the `Check:` names is test-only.

### R3. Two Broca records assert an `always` the code refutes on a slow child

Both omit the same disjunct, which makes this systematic rather than isolated.

**[broca-children-are-reaped-as-a-process-group](../catalog.md#broca-children-are-reaped-as-a-process-group)**
asserts "no process of a reaped group survives the terminal".
`terminate_group` (`subprocess.rs:670`) has two exits that return an error
precisely because that could not be established: `:692-695` when the leader is
not reapable within the grace, and `:697-700` when members could not be
confirmed stopped. The grace is `termination_grace`, 5 seconds by default
(`:232`), and it is applied four times in sequence on that path (`:679`,
`:684`, `:689`, `:691`), so the reaping ceiling is four graces and not one. An
oracle written to the record's `Check:` fails on a correct build as soon as a
child outlives the grace, which is what `hang_ignore_term` exists to produce
(`tests/broca_subprocess.rs:2553`).

**[broca-permits-and-charges-return-to-baseline](../catalog.md#broca-permits-and-charges-return-to-baseline)**
asserts "after shutdown the state is empty". That clause is true and the code
says why: local state is released even when teardown is unproven
(`supervisor.rs:614`, implemented at `:635-639`). What the record omits is the
return value. `Supervisor::shutdown` returns the count of runs whose teardown
was never proven (`:611`, `:630-633`), and
`shutdown_counts_runs_with_unproven_teardown` asserts it equals 1
(`tests/broca_supervisor.rs:770`). The doc comments at `:615-618` are explicit
that a drained supervisor does not prove harness process trees stopped and
that the component must not report a clean shutdown while provider work may
still run. A baseline check that reads only the metrics passes in exactly the
state the code calls unclean.

The correct shape for both is a disjunction: either the group is confirmed
stopped, or the terminal carries `work_unresolved` (`supervisor.rs:966`) and
the control operation returns `teardown_unconfirmed` (`:557-566`). Six tests
already assert the second branch, and no record names it, which is gap G1.

### R4. The route-open record has no production evaluation point

[canonical-route-open-declares-its-exact-body-length](../catalog.md#canonical-route-open-declares-its-exact-body-length)
has both conjuncts inside the test. `canonical.len() == 167` measures a string
literal written in `tests/protocol_vectors.rs:197-200`; `raw_client::header`
and `raw_client::decode_header` are a hand-written codec at
`tests/support/raw_client.rs:271` and `:283`. The host's decoder,
`wire::decode_header` (`wire.rs:311`), never sees the committed bytes. The
inventory records this correctly as an observation about the check's shape
(`existing-checks.md:186-194`), and the fault map arrives at the same place
from the other side: its coverage-check table proposes 32 markers and not one
of them is for this record, because there is no production function in the
check to place one in.

The consequence the artifacts do not draw is that the record's `Impact:`, "The
first request on every connection is misframed", cannot be produced by any
defect this check detects. No possible host implementation fails it. That
makes `Exercised: yes` and `Confidence: high` overstatements for the record as
written, and it puts the record's real subject at "a documentation vector is
internally consistent", which is a smaller and true claim. The host decoder's
totality and rejection completeness are already covered elsewhere in this
part, by Group G at `../catalog.md:3988`, so the honest disposition narrows
this record rather than expanding it.

### R5. The one liveness record has no bound, and no code constant can supply one

[synapse-degrades-to-disabled-and-keeps-the-context-routable](../catalog.md#synapse-degrades-to-disabled-and-keeps-the-context-routable)
is `Type: liveness` with `Check: always` and no bound of any kind.
METHOD.md:88-95 requires a bounded fault-free window stated in the units the
code bounds and forbids an unbounded "eventually". The only bound in play is
the harness timeout: `BUDGET` is 5 seconds at `tests/support/synapse.rs:22`,
used at `tests/synapse_roundtrip.rs:112`. The fault map identifies this and
asks the question (`fault-map.md:342-345`); the record says
`Open questions: None.`

The reason no code unit is available is already recorded in this part. Part
2e's Group D is titled "admission bounds and the deadline that does not exist"
(`../catalog.md:7808`), so a context request has no configured deadline to
bound it. Two dispositions are defensible and neither invents a number. The
record becomes `safety` over its two observable facts, that `activate` returns
`Ok` with the lane disabled and that the Synapse bind is refused with
`artifact_invalid` (`tests/synapse_roundtrip.rs:92-93`), with routability
demoted to a `sometimes` witness; or the record stays `liveness` and carries
the unresolved bound question. What it cannot do is stay `liveness` with
`Open questions: None.`

### R6. The admission record's `Check:` drops expiry, and its expiry test runs at 2.5 times the bound

[synapse-admission-boundaries-are-exact](../catalog.md#synapse-admission-boundaries-are-exact)'s
`Guarantee:` ends "and reports expired jobs as `module_restarted`". Its
`Check:` covers the count boundary, the byte boundary, live-work eviction, and
charge return, and says nothing about expiry. So the record's one word,
"exact", is not applied to its one timing clause.

The code's condition is exact:
`now.duration_since(at) >= self.limits.retention` (`jobs.rs:624`). The one
host-level test drives it with 100 ms retention
(`tests/synapse_jobs.rs:242`) and a 250 ms real sleep (`:266`), so it passes
on an implementation using `>` instead of `>=`, on `2 * retention`, and on any
constant below 250 ms. `tests/synapse_jobs.rs` uses `start_paused` at zero
sites, against 6 in `synapse_protocol.rs` and 5 in `broca_supervisor.rs`, so
the deterministic-clock fixture the boundary needs exists in two sibling files
and is unused here. The inline companion at `jobs.rs:880` uses
`retention: Duration::ZERO`, which also misses the boundary from the other
side.

### R7. The proof record's `Exercised: yes` rests on a shared literal, not a call

The record's `Exercised:` line reads "the crate-internal vector test and the
independent `raw_client` oracle both pass on the regenerated vectors". Both
passing on regenerated vectors is the non-discriminating case, and the record
already knows it: its `Fault/timing angle:` says only an external oracle
detects a transcript change both sides apply. The inventory establishes the
mechanism (`existing-checks.md:123-133`): `auth.rs:641` pins `compute_proof`
to two hex literals, `tests/protocol_vectors.rs:33` pins `raw_client::proof`
to two decimal arrays, the two agree byte for byte, and
`proof_folds_every_input` (`:75`) perturbs the oracle alone. The one place the
two functions meet live is a handshake with no perturbation (`:221`). The
record's `Check:` states an equality that no test evaluates, so `Exercised:`
should be `partial` with the missing half named, and the record should keep
`Confidence: high`, which is about the mechanism reading and is correct.

### R8. Three open items in the support artifacts are resolvable from the checkout

None changes a record; all three move work down the cost ranking or close a
sampling limit.

The fault map's ranking item 2 says that whether an integration binary can
reach `compute_proof` "is the one thing to verify first"
(`fault-map.md:278`). It can: `pub fn compute_proof` at `auth.rs:119`, under
`pub mod auth` at `lib.rs:7`. The 15-line fallback the item describes is
unnecessary.

Ranking item 6 says `wire.rs`'s decoder "may not be reachable from an
integration binary, which decides whether the test lives inline"
(`fault-map.md:309`). It is reachable: `pub mod wire` at `lib.rs:36` and
`pub fn decode_header` at `wire.rs:311`. Both hedges resolve in the cheap
direction, which moves items 2 and 6 up the ranking.

The inventory records that whether the release profile enables
`debug-assertions` "was not read" and that it decides whether the four
`debug_assert!` sites exist in production (`existing-checks.md:495`, repeated
as a sampling limit at `:620`). No `[profile]` section exists in any TOML in
the repository and there is no `.cargo/config.toml`, so Cargo's default
`debug-assertions = false` applies to release. The four sites do not exist in
a release build, including `supervisor.rs:641`, which
`existing-checks.md:288-290` names as "the one production guard on this
invariant" for the permits record. The correct statement is a debug-only
guard, and the permits record's only production enforcement is the returned
count that R3 says the check omits.

## Gaps queued for a follow-up pass

Recorded, not mined. All three verified for this evaluation.

| # | Gap | Evidence |
| --- | --- | --- |
| G1 | **Broca terminal exclusivity and unproven teardown have six dedicated tests, an explicit code contract, and no record.** `supervisor.rs:91` states first-terminal-append-wins so completion cannot overwrite cancellation; `finish` implements it with the `state.terminal_appended \|\| state.purged` early return at `:938-945`; `:113` states that cancel and delete report failure if descendants may still execute a billable request; `:966` sets `work_unresolved` on `FailedUnresolved`; `:557-566` turns it into the `teardown_unconfirmed` error. Six tests assert exactly this, all in `tests/broca_supervisor.rs`: `:544 cancel_covers_queued_and_running_runs_and_stays_idempotent`, `:604 completion_cannot_overwrite_a_committed_cancellation`, `:685 unproven_teardown_fails_cancel_and_delete`, `:730 cancellation_winning_the_terminal_still_reports_unproven_teardown`, `:770 shutdown_counts_runs_with_unproven_teardown`, `:799 terminal_cap_never_evicts_a_run_awaiting_teardown`. `existing-checks.md:452` lists them under "no record. Status, replay, cancel, delete, and teardown-proof contracts". The impact is the same one the dedup record exists for, two billed model calls and two divergent transcripts for one prompt, reached through a different window: a cancel racing a completion rather than a resend racing a send. Fixture cost is zero, because `gated_ignoring_cancel` (`tests/support/broca.rs:117`) and `ScriptedBackend::with_behavior` already build both shapes. This is also the missing half of refinement R3: the two records that assert an unconditional `always` would be repaired by the disjunct this record would state. |
| G2 | **The Synapse activation-drop path has two tests, an explicit code comment, and no record.** `synapse/mod.rs:1023` says "Dropping the activation future does not stop the blocking task", immediately above the `spawn_blocking` at `:1024` that loads the bundle and the ONNX Runtime image. Two tests assert the consequences: `tests/synapse_bundle.rs:757 a_dropped_activate_keeps_shutdown_waiting_for_the_blocking_load` and `:818 an_abandoned_activation_holds_the_instance_lock_until_the_blocking_load_stops`. `existing-checks.md:453` lists both under "no record", correctly calling them activation-drop contracts. The second is the higher-impact one: the instance lock is the daemon's single-incarnation fence (`instance.rs:244`, `lifecycle.rs:182`), an abandoned activation holds it for the duration of a blocking model load, and `runtime.rs:562-580` retries `InstanceGuard::acquire` a bounded number of times before returning `AlreadyRunning`. So a dropped activation can make a successor host refuse to start, which is a startup-availability property with the widest blast radius in the Synapse area and is stated nowhere. Neither test needs an ONNX Runtime library, so both run in CI today. |
| G3 | **The set has zero `reachability`-type records and zero non-`always` semantics, and at least four properties in scope fit the other four semantics exactly.** The fault map states the fact and treats it as a convenience (`fault-map.md:176-179`). The consequences it does not draw: (a) three `Exercised: partial` clauses are situation coverage by METHOD.md:74-75 and none is stated as `sometimes`, namely a resend after a terminal was retained then evicted, a backend that never exits, and a fault during inference itself. The fault map proposes markers for all three (`u3_broca_terminal_evicted_from_retained_set`, `u3_broca_group_signaled_sigkill_after_grace`, and the `F15` row), which is the right mechanism attached to no record. (b) One production `unreachable!` exists in scope, `synapse/mod.rs:327` `"ready lanes embed"`, which is METHOD.md's `unreachable` case by definition and has no record. It sits between two separate acquisitions of the same lock, `ready_lane()` at `:313-318` and `status()` at `:300-311`, so a `Starting` to `Ready` transition between them reaches it and panics. `embed_blocking` (`:322`) has only test callers here, all ORT-gated, so a record on it would be the set's first honest `test-only` label, which is what makes it worth writing. (c) The reaping record's first conjunct is a bounded liveness claim wearing a safety label: `terminate_group` bounds it at four times `termination_grace` (`subprocess.rs:679`, `:684`, `:689`, `:691`, the constant at `:232`), which is a bound in the units the code bounds and is exactly what METHOD.md:94-95 asks for. Fifteen safety records and one unbounded liveness record is not a distribution artifact of the subject matter; it is what writing records outward from an `always`-shaped assertion suite produces. |

## Biases requiring human judgment

1. **Whether all 16 records may keep `default-production` when the production
   consumer is not in this checkout.** The facts pull both ways and both sides
   are verified. *For the label:* `host-runtime` is a library whose consumer is
   the `daemon` crate, scheduled for U4 (`docs/properties/README.md:52`), and
   the records' clauses describe the host as deployed. *Against:*
   METHOD.md:20-23 requires the class verified per record at authoring time
   with its evidence, and names a blanket preamble claim as an error that has
   already cost one revision. The evidence available here is four callers, all
   examples or a bench: `examples/synapse_host.rs:137`,
   `examples/perf_host.rs:100`, `examples/synapse_perf.rs:385`,
   `benches/ipc_budget.rs:111`. No `daemon` crate is a workspace member
   (`Cargo.toml:3-11`). The other 133 records in this part use 13 `test-only`
   and 8 `explicit-config-only` labels, so the vocabulary is in active use and
   this set declines all of it. The fault map says outright that the label
   "cannot be checked against a production caller in this checkout"
   (`fault-map.md:60-61`) and then relabels nothing. *Two sub-questions the
   answer must also settle:* whether `SynapseComponent::new(None)`
   (`synapse/mod.rs:219`, which `examples/synapse_host.rs:116-123` builds when
   the bundle argument is `-`) makes the sealed-image and validation records
   `explicit-config-only`; and what refinement R2's three cases become, since
   their entry points are test-only in a stronger sense than the rest, being
   absent from `src/` rather than merely unreached. *Judgment required:* decide
   once, for the set, and record the decision in the preamble as a stated
   convention rather than leaving it as 16 unexamined identical labels. Either
   answer is defensible; leaving it implicit means the set's most uniform field
   is also its least evidenced.

2. **Whether the closure store belongs to this set, to runtime-config, or to
   no scheduled pass.** This is carried unresolved, and it is now carried by
   two parts rather than one. The runtime-config evaluation raised the same
   file as its gap G2 and as its single bias, asking whether that sub-part owns
   the closure store's behaviour or only its host-facing surface
   ([../runtime-config/portfolio-evaluation.md](../runtime-config/portfolio-evaluation.md)`:294-320`).
   This set adds exactly one record, on `manifest_digest`, and routes 10 of the
   16 `tests/harness_closure.rs` checks back to runtime-config's record
   (`existing-checks.md:449`). `src/harness_closure.rs` is 1,146 lines with no
   inline test module (`existing-checks.md:90`), and it validates untrusted
   manifests and materializes filesystem trees. Two parts have now declined the
   same surface for defensible reasons. *Judgment required:* this has stopped
   being a per-part scope question and become a plan question. Either a part
   owns the behaviour, or the honest output is a note that a 1,146-line
   untrusted-input filesystem module belongs to no scheduled pass.

3. **Whether a test that returns before its first assertion is
   `Exercised: partial` or `Exercised: not yet`.** Raised by the inventory
   itself (`existing-checks.md:627-630`). Six tests return at
   `let Some(ort) = ort_library() else { return };` when
   `EIDNARA_SYNAPSE_TEST_ORT_LIBRARY` is unset:
   `tests/synapse_bundle.rs:573`, `:645`, `:687`, `:699`, `:709`, and
   `tests/synapse_roundtrip.rs:121`, with the gate at
   `tests/synapse_bundle.rs:29-42`. `ci.yml` sets no `EIDNARA` variable. *The
   evaluator's view, offered and not imposed:* a clause with no execution in
   any automated run reads as `not yet`, and `partial` implies something was
   constructed. But the decision changes `Exercised:` fields across the whole
   catalog's conventions, it interacts with the two `#[ignore]` tests the
   inventory's second open question raises, and it decides whether this set's
   `Exercised: yes` count of 8 is the right number. It belongs to a human, and
   the fault map's `u3_ort_test_skipped_without_library` marker is the right
   instrument either way.

## Verdict

**REFUTED as finished.** The set is accurate where it is anchored, and the
anchoring is the problem. Its 16 records state validation, identity,
placement, and bound properties, every one of them with a named existing
check, and they state no concurrent-lifecycle property at all in two
subsystems whose source comments are largely about concurrent lifecycle.

What is genuinely strong, and was not disputed. The CI reversal is real and
correctly verified: `ci.yml:118` and `:126` run
`cargo test --workspace --all-targets --all-features --locked` under two
toolchains on `ubuntu-latest` (`:14`), and this is the first part in the
catalog whose checks execute. The line-reference discipline holds under
spot-checking: every source count in `existing-checks.md:4-9` reproduces
exactly, `CREDENTIAL_ROW_CAP_BYTES` does have exactly zero readers
(`subprocess.rs:51`), the `#[ignore]` and ORT-gate sites are all where the
inventory puts them, and the three attenuations at `existing-checks.md:43-47`
are the whole list. The fixture census is right that every fault-needing
record has its fixture and lacks only a composition. The mechanical state is
clean: 16 records, 16 index entries, 16 evidence files, matching digest.

What is not finished. Two records carry a `Check:` an oracle would fail on a
correct build (R3), one carries an `Impact:` no defect it detects can produce
(R4), one is typed `liveness` with no bound in a part that already recorded
that no bound exists (R5), six suppress a question their own evidence raises
(R1), three carry a `Reachability:` note the crate's own constructors refute
(R2), and the two highest-impact unstated properties are both concurrent
lifecycle with zero fixture cost (G1, G2).

Ready now for test implementation, in this order. The two one-line assertions
the ranking puts first, a `readlink` on the memfd load path and a
`symlink_metadata` on `lifetime.lock`, both correct and both cheap. Then the
proof-equality loop, which R8 shows needs no fallback. Then the three one-test
compositions, all over fixtures that exist. Then the expiry boundary under
`start_paused`, which R6 shows is one attribute plus one `advance` away.

Not ready, for reasons no further work of this kind resolves. The reachability
labels wait on bias B1, and R2 shows three of them are wrong as written
whichever way B1 goes. The closure store waits on bias B2, which two parts
have now deferred. The liveness bound waits on a design decision about whether
a context request has a deadline at all. And the six suppressed questions
include one contract-versus-code disagreement in
`docs/host-wire-protocol.md` that only a human can close.

## What this evaluation says about the method

The runtime-config evaluation's guard was "what observation makes this check
fail on a defective build, and what observation makes it pass on a correct
one". That guard catches R3 and R4 immediately and catches nothing else here.
R3's check can only fail, on the input its own fixture supplies; R4's can only
pass, on every implementation. Both are the shapes that pass already has a
name for.

**This set's lesson is upstream of the check line: a record set derived from
the assertion suite inherits the suite's shape, including its silences.** All
16 records name an existing check, against 81 of 133 elsewhere; all 16 are
`always`; none is `not yet`. Those three facts are one fact. The suite asserts
invariants, so the records are invariants; the suite has no `sometimes`
marker, so no record needs one; the suite covers what it covers, so every
record is at least `partial`. The silences travel too: 109 of 237 checks map
to no record (`existing-checks.md:460`), and the inventory names the clusters
honestly, but naming an unmapped cluster in the inventory is not the same as
deciding whether it contains a property. G1 and G2 were both sitting in that
list with their tests already written.

The guard that follows is a second pass in the other direction, and it is
cheap because the inventory already produces its input: **for every cluster
the inventory lists as bearing on no record, name the property it would be,
then either write it or say why it is out of scope.** Eight clusters are
listed at `existing-checks.md:451-458`. Two of them are gaps G1 and G2. The
remaining six may well be out of scope, and the artifact does not say so,
which is why they read as inventory rather than as decisions.

The second lesson is procedural and it recurs for the fifth part in a row:
**the correction was already inside the artifact set.** Not implied by it,
written in it. Six evidence files reached a conclusion their record does not
carry (R1), and one of them, the child-environment record's, states the
reachability problem that refinement R2 spends three paragraphs
re-establishing from the code. The earlier parts' version of this was a
synthesis note contradicting the record it annotated. This part's version is
an evidence file contradicting the record it supports, and the fix is the same
one: **when an evidence file's investigation-log conclusion is `unresolved` or
`needs human input`, the record's `Open questions:` list is not `None`.** That
is a mechanical check, it can run in step 7 beside the record-index-evidence
equality, and it would have caught all six.

## Re-evaluation trigger

A fresh pass is warranted once bias B1 is answered, because the answer changes
16 labels and changes three of them in a direction refinement R2 has already
established is wrong as written. Either resolution is progress; leaving it
means the set's most uniform field stays its least evidenced.

Four further triggers, each firing independently.

- **Any record written over `finish`'s first-terminal-append-wins contract.**
  It closes gap G1, it supplies the disjunct refinement R3 needs for two
  existing records, and it is the set's cheapest large win because all six of
  its checks are already written and running.
- **Any record written over the activation-drop path.** It closes gap G2 and
  would be the set's first property over a startup-availability failure, which
  is the one impact class the current 16 do not reach.
- **Any resolution of bias B2.** Two parts have now declined the closure
  store. A third deferral should be recorded as a plan-level hole rather than
  as a per-part to-do.
- **Any change that supplies an ONNX Runtime library to CI.** Six tests pass
  without asserting today, two record clauses turn on it, and it is the only
  item on the ranking that needs a supply-chain decision rather than a test
  edit. Until then `u3_ort_test_skipped_without_library` is the honest oracle,
  and the fault map is right to place it inside `ort_library()`'s `None` arm
  rather than after the early return.

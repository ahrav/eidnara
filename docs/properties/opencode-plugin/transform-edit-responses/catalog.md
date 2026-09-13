# Transform Edit Responses: client execution (U3)

This directory is a client implementation supplement for
[#533](https://github.com/ahrav/eidnara/issues/533), not the reusable 30-record
companion named by the parent specification
[#525](https://github.com/ahrav/eidnara/issues/525). That companion is unavailable
here. These 11 records retain the parent's index slugs but do not replace its
predicates, evidence, or evaluation. The missing records are not recreated here.

Revision-bound run. These records describe the #533 change on branch
`fix/client-transform-owner`, whose parent is `b0023512`, executed on
2026-09-13 from `packages/opencode-plugin` with Node 24.18.0 first on PATH:
`bun test src/hooks/context/ src/plugin/messages-transform.test.ts
src/shared/host-client/client.test.ts` reports 1098 pass, 0 fail, 34 files
(Bun 1.3.14). Every `Existing check` below names the individual `it(` title,
its file and line in that tree, and the marker assertion that proves the
enabling state was reached before the check fired. Source references carry
line numbers from the same tree. `Guarantee` and `Check` state the obligation;
`Exercised: yes` means named tests construct the required faults and assert
the check in that run. Test adequacy is not independently reviewed here; that
verdict belongs to `/testing:invariant-test-review`.

TE21 remains partial: a kept previous-output entry that gained an accessor is
refused, but live previous-value validation before advertisement or reuse is
assigned to [#538](https://github.com/ahrav/eidnara/issues/538). TE25 is
outside these records: the 64-session `wireCaches` count cap and the 64 MiB
capture budget are not the separate 64 MiB optional-output byte budget with
byte-triggered LRU eviction. TE30 remains partial: no delta-versus-full
control comparison exists; that comparison is assigned to #538.

## Scope

- Source guard: referenceable JSON domain over the host message array.
- Capture: membership and recursive content snapshots, with await-boundary
  rechecks at preflight, permission, each ordinal scan, each transport page,
  and publication.
- Admission: one lease per session, 64 passes per process, and a 64 MiB
  aggregate pending charge reserved before the first await, not a
  whole-process RSS bound.
- Ordinals: pass-local staging, promoted only on accepted publication.
- Publication: candidate validation, current ownership, in-place replacement,
  state promotion, and lease release precede delivery awaits.
- Reachability: each evidence file traces its record through the Rust-mode
  hook and the configuration consent gate. Rust activation is explicit; a
  direct test call does not establish default-production reachability.

## Index

| Record | Type | Exercised | Check or gap |
| --- | --- | --- | --- |
| TE17 `captured-input-stays-coherent` | safety | yes | Paused directory, scan-yield, transport, retry, and pre-apply windows with mutation or hook installation |
| TE18 `ordinal-memo-promotion-is-owned` | safety | yes | Shared memo equals its pre-pass copy on every rejected path; promoted only after replacement |
| TE19 `referenceable-json-rejects-hooks-before-reading` | safety | yes | Zero trap and getter counts at the guard, the hook, the wrapper, and the rechecks |
| TE20 `publication-is-current-and-atomic` | safety | yes | Container and candidate rejection before the first write; identity preserved on every outcome |
| TE21 `previous-base-is-applied-and-live` | safety | partial | Kept-entry accessor refusal and identity; live previous-value validation awaits #538 |
| TE22 `delivery-disposition-follows-publication` | safety | yes | Per-identity ACK and NACK tuples; ACK failure leaves published output |
| TE23 `owner-admission-covers-live-captures` | safety | yes | Exact held charge, 1,000-message fixture, model-based lease sequences, release before ACK |
| TE24 `capture-charge-outlives-cancellation` | safety | yes | Charge and slot held after abort until the owner settles; release exactly once |
| TE26 `uncertain-send-never-replays-blindly` | safety | yes | One `transform` call after a thrown first send; bounded restart controls |
| TE27 `bounded-recovery-after-pressure-clears` | liveness | yes | Settlement observed at zero counters, then one `run` dispatches |
| TE30 `inbound-baseline-independent-of-output-base` | safety | partial | Two-pass scenarios only; delta-versus-full control awaits #538 |

## Records

### captured-input-stays-coherent

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - Paused directory resolution, ordinal scan yield, single and paged transport, the need_full_sync retry, a mid-series reconnect, and the pre-apply microtask are each held while the host edits, replaces, appends, rebinds, or installs an accessor, `toJSON`, or proxy on captured input; the tests assert no further dispatch or publication.
Guarantee: CK projection, native fingerprints, page bodies, and the published candidate all derive from one synchronously validated capture; any membership or content change observed after an await declines the pass.
Check: `always` - After an await, source-dependent work uses only `captured.members` (`rust-mode-transform.ts:977-979`); `recheckCapture` (`:981-989`) runs at preflight, permission, each ordinal scan, each transport page, and publish, and a failed recheck permits no further dispatch or publication, without claiming to undo an earlier dispatch.
Fault/timing angle: host mutates a message or the array between the capture and any later await boundary.
Required faults and enabling state: a controlled pause during directory resolution, an ordinal page read, a transport call, or the post-response microtask; an in-place edit, membership change, rebind, or accessor installation on a captured message.
Confidence: medium - [evidence](evidence/captured-input-stays-coherent.md). Every named witness was read and ran green on the revision-bound run; each pauses a named await and asserts a marker before the fault. The permission-verdict await has no dedicated pause witness.
Existing check: `rust-mode-transform.test.ts:2614` "permits no further dispatch or publication when an accessor is installed during preflight" (marker: `started.promise` races the pass on the directory read, then `expect(calls).toHaveLength(0)`); `:2652` "rejects publication when a message is edited in place while the transform response is pending" (marker: `expect(calls).toHaveLength(1)` before the edit; `["transform", "transform.nack"]` after); `:2255` "rejects mutation|supersession|clear|invalidation at the persisted ordinal yield with shared|distinct arrays" (8 cases; marker: `expect(pageSizes).toEqual([MODULE_ORDINAL_PAGE_SIZE])` and `expect(calls).toHaveLength(0)`); `:2342` "preserves a host member|append|rebind|metadata during transport with shared|distinct source" (8 cases; marker: `expect(calls).toHaveLength(1)` before the change; `output.messages` is the current array after); `:2387` "rejects nested accessor|toJSON|proxy installed at source-await|pre-apply without invoking it" (6 cases; markers: `directoryReached` and `expect(methods).toHaveLength(0)`, or `transportProcessedBeforeInstall`); `:2482` "does not dispatch a need_full_sync retry after mutation|supersession|clear|invalidation of the valid first send" (4 cases; marker: `expect(bodies).toHaveLength(2)` with `tail_delta` defined before the fault); `:2548` "stops a multi-page send after page zero when its source changes" (marker: `transform_page_index` 0, `transform_page_complete` false); `:1870` "stops a series restart after a mid-series reconnect when mutation|invalidation lands first" (2 cases; marker: one page-zero body and `transform_page_index` 1 pending); `hook.test.ts:301` "rejects source mutation during hook directory lookup before direct transform" (marker: `client.session.get` called once before the getter install).
Impact: a stale or partially edited array reaches the model or the daemon's inbound baseline diverges from what the host sent.
Open questions:
- The `recheckCapture("permission")` boundary at `rust-mode-transform.ts:1071` shares the mechanism the other windows exercise but has no dedicated pause-and-mutate witness.

### ordinal-memo-promotion-is-owned

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - Scan-yield faults, retry faults, a memo-copy byte decline, a throwing continuation shift, a continuation overflow, and mid-flight invalidation each compare the shared memo with an independent pre-pass copy; the module-level staging tests spy on `Map.prototype.set` and `clear`; successful publication is observed promoting entries.
Guarantee: ordinal resolution mutates only a charged pass-local memo copy; the session memo, anchor, stored count, and canonical count change only in the synchronous publication block of an accepted pass.
Check: `always` - `stagedMemo` is a charged copy (`rust-mode-transform.ts:1003-1006`, `:1124-1127`); `state.ordinals = stagedMemo` runs only after `replaceHostArrayContents` (`:1475-1477`); a declined, superseded, cleared, invalidated, or failed pass leaves `state.ordinals` equal to its value at pass start, except that clear and invalidation reset it by their own contract (`:821-827`, `:1533`).
Fault/timing angle: a pass that resolves ordinals and then fails or is rejected before publication.
Required faults and enabling state: a resolved or partly resolved pass whose response is rejected by invalidation, supersession, clear, source change, byte decline, or a throw between resolution and replacement.
Confidence: medium - [evidence](evidence/ordinal-memo-promotion-is-owned.md). Every named witness was read and ran green; the rejection oracles compare full memo objects with `toEqual(priorMemo)` or assert untouched spied maps, not sizes alone.
Existing check: `rust-mode-transform.test.ts:2255` "rejects mutation|supersession|clear|invalidation at the persisted ordinal yield with shared|distinct arrays" (8 cases; marker: `expect(pageSizes).toEqual([MODULE_ORDINAL_PAGE_SIZE])` with `ordinals.entries.size` 0 and `heldBytes` above the page's entry charge; after settlement `ordinals.entries.size` stays 0); `:2482` "does not dispatch a need_full_sync retry after ..." (4 cases; `expect(state.ordinals).toEqual(priorMemo)` for mutation and supersession, `entries.size` 0 for clear and invalidation); `:2168` "charges every existing ordinal entry and ID before copying a warm memo" (marker: decline log `ordinal memo copy`; `expect(transform.getState(sessionId).ordinals).toEqual(priorMemo)`); `:1052` "discards a partly shifted ordinal memo before host publication when shifting throws" (marker: `shiftFailed` true; `toEqual(priorMemo)`); `:1112` "rejects ordinal continuation overflow before publication and recovers on a valid response"; `:2689` "rejects publication and promotes no memo when the wire state is invalidated mid-flight" (marker: `expect(calls).toHaveLength(1)` before `invalidateWireState`); `module-wire.test.ts:892` "keeps the supplied map untouched on <fault> during <mode>" (27 cases; marker: `expect(set).not.toHaveBeenCalled()` and `expect(clear).not.toHaveBeenCalled()`); `:990` "charges row and memo storage plus short|long IDs at exact byte boundaries" (2 cases; refused budgets leave `entries` equal to `original`); `:1085` "preserves the memo through a full restart ending in <outcome>" (5 cases); promotion controls: `rust-mode-transform.test.ts:2907` (`ordinals.entries.get("m-1")` is 1 after publication) and `:3106` (`entries.size` 2).
Impact: a rejected pass leaves ordinals in the shared memo that the next pass trusts, producing mismatched ordinals or a spurious `need_full_sync`.
Open questions: None.

### referenceable-json-rejects-hooks-before-reading

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - Accessors, proxies (including revoked), `toJSON` hooks (own, hidden, inherited), class instances, symbols, bigints, non-finite numbers, cycles, sparse arrays, undefined elements, and excessive depth are rejected with trap counters at zero at the guard, through the actual hook and wrapper entries, and in rechecks after installation.
Guarantee: accessors, proxies, `toJSON` hooks, cycles, unsupported prototypes, sparse arrays, and unsupported primitive values are rejected by descriptor inspection and `util.types.isProxy` before any property read or serialization can invoke user code.
Check: `always` - Instrumented getter, proxy-trap, and serialization-hook invocation counts remain zero during validation (`transform-capture.ts:192-236`), capture, and rechecks; this asserts a forbidden effect rather than an instrumented unreachable code point.
Fault/timing angle: none for the initial guard; the recheck path covers hooks installed after capture.
Required faults and enabling state: a message tree carrying an accessor, proxy, cycle, class instance, or non-JSON primitive, at the root, a member, or a nested value.
Confidence: medium - [evidence](evidence/referenceable-json-rejects-hooks-before-reading.md). Every named witness was read and ran green; each installs an instrumented hook and asserts a zero count. The non-trapping guarantee of `util.types.isProxy` is a Node runtime property, verified by these tests on Node 24.18.0 only.
Existing check: `transform-capture.test.ts:75` "rejects symbol-keyed accessors and coercion callbacks without invocation"; `:107` "rejects proxy roots, including revoked arrays, before reflective traps"; `:212` "rejects hidden accessors without depending on consumer field names"; `:282` "rejects an accessor without invoking it"; `:291` "rejects a proxy without running its traps"; `:305` "rejects toJSON hooks, class instances, functions, symbols, bigints, and non-finite numbers"; `:338` "rejects cycles, sparse arrays, undefined elements, and excessive depth"; `:364` "reads own data properties without touching accessors or proxies"; `:549` "fails the recheck without running an accessor installed after capture"; `:891` "revalidates hooks installed between inspection and reserved capture" (all with marker `expect(counter.count).toBe(0)`); `it.each` families at `:132`, `:154`, `:168`, `:185`, `:249` cover own and inherited `toJSON`, hidden array operation overrides, and membership accessors; `rust-mode-transform.test.ts:2581` "declines an unsupported source before any dispatch and leaves the host array intact" (marker: `expect(getter).not.toHaveBeenCalled()`, `calls` 0); `:2387` "rejects nested accessor|toJSON|proxy installed at source-await|pre-apply without invoking it" (6 cases; `expect(hook).not.toHaveBeenCalled()`); `:1980` "refuses a kept previous-output entry that gained an accessor and invokes no hook"; `hook.test.ts:210` "rejects output-proxy|root-proxy|message-proxy|index-getter|nested-getter|then at the actual hook|wrapper entry without triggering reads" (12 cases; markers: `expect(trap).not.toHaveBeenCalled()`, `client.session.get` not called, `fake.calls` 0); `messages-transform.test.ts:64` and `:114` `it.each` wrapper families (marker: `getterCalls` or `trapCalls` 0).
Impact: harness-installed hooks run inside the transform, observe or alter the pass, or throw after a partial publication.
Open questions: None.

### publication-is-current-and-atomic

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - Frozen, proxied, subclassed, non-writable, and length-sealed containers are declined before dispatch; a candidate one byte over its slot charge is declined after the response with the host array unchanged; a kept entry that fails inspection is refused before the first write; clear, supersession, and invalidation before application preserve identity; the wrapper returns the current array after a throwing hook.
Guarantee: the host array is replaced in place, all or none, only after ownership, invalidation, source, and container checks pass synchronously; the wrapper never rebinds `output.messages` to captured contents.
Check: `always` - `buildNativeCandidate`, `inspectReferenceableMessages(candidate)`, `assertNativeBoundary`, `recheckCapture("publish")`, and `hostArrayReplacementRejection` all run before `replaceHostArrayContents` (`rust-mode-transform.ts:1425-1475`); the replacement is a define loop over own slots plus one length define (`transform-capture.ts:432-437`) under preconditions checked at `:400-425`; rejection preserves the current host array's identity and contents; the wrapper (`messages-transform.ts:58-84`) returns `output.messages` as it currently is and never assigns it.
Fault/timing angle: a host container that could throw mid-assignment; a candidate that fails validation after a valid prefix; a throw inside the hook after partial replacement.
Required faults and enabling state: a frozen, proxied, subclassed, or non-writable host array; a candidate whose slot charge or inspection fails; a hook error after a host edit.
Confidence: medium - [evidence](evidence/publication-is-current-and-atomic.md). Every named witness was read and ran green. The all-or-none claim rests on the define loop having no user-code path once its preconditions hold (`:427-431`); the tests exercise each precondition's rejection and the identity outcome, not an injected mid-loop throw.
Existing check: `transform-capture.test.ts:569` "bounds the candidate length at the slot budget and rejects malformed lengths"; `:606` "reports a destination slot that stopped accepting writes and reads no candidate getter" (marker: `counter.count` 0); `:621` "replaces every slot and the length of an accepted destination in place"; `:632` "rejects inherited membership and does not consult its getter"; `:653` "accepts a plain extensible array and replaces its contents in place"; `:662` "rejects containers whose element or length assignment could throw"; `rust-mode-transform.test.ts:1916` "publishes at the exact candidate charge and leaves the host array intact" and "declines one byte short of the candidate charge and leaves the host array intact" (marker: `started.promise` race, then a blocker lease reserves `remainingBytes - candidateLength * 8 + offset`; the decline log names `native candidate array` and `output.messages[0]` is still `member`); `:2600` "declines a proxied or non-replaceable host container without dispatch" (marker: `calls` 0); `:2725` "stops an in-flight pass when the session is cleared and keeps the host array intact"; `:1980` "refuses a kept previous-output entry that gained an accessor and invokes no hook" (marker: `second.messages` equals its pre-pass members); `hook.test.ts:273` "preserves host array and returned payload identity through the actual hook|wrapper" (2 cases); `messages-transform.test.ts:254` "keeps the current host contents when the inner hook mutates then throws"; `:282` "does not roll back a concurrent replace-member|rebind-array when the pending hook throws" (2 cases; marker: `started.promise` race before the host edit); `:171` "checks only the return container after the hook publishes".
Impact: a partial array reaches the model, or OpenCode's array identity is replaced by stale contents.
Open questions:
- #538's recipe operations must add a malformed-final-operation candidate case; the current candidate builder is exercised through `native_messages` and `native_messages_delta` only.

### previous-base-is-applied-and-live

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: partial - Reuse of an applied kept prefix preserves identity, and a kept entry that gained an accessor is refused before publication with no hook invoked; validation of a kept entry's live values against its approved base before advertisement or reuse is not implemented and is assigned to #538.
Guarantee: Only retained, successfully applied output whose membership and values still match its guarded revision may be advertised or reused as a previous base.
Check: `always` - Validate the previous output before request construction and again before using its keeps; mutation removes eligibility without repairing host objects, and matching a fingerprint alone is insufficient.
Fault/timing angle: Applied output is mutated before the next request or while that request is pending.
Required faults and enabling state: A retained applied output, a nested value or membership mutation, and a subsequent attempt that could reference it.
Confidence: low - [evidence](evidence/previous-base-is-applied-and-live.md). `buildNativeCandidate` checks `delta.after` against `previous.fingerprint` and the prefix range (`rust-mode-transform.ts:1425-1434`), and the candidate inspection at `:1436` refuses kept entries with hooks; no value comparison of kept entries exists.
Existing check: `rust-mode-transform.test.ts:3106` "preserves the payload identity of kept and returned messages on publication" (marker: `secondOutput.messages[0]` is `returned[0]`, `entries.size` 2); `:1980` "refuses a kept previous-output entry that gained an accessor and invokes no hook" (marker: `expect(hook).not.toHaveBeenCalled()`, NACK of `kept-attempt`, `failureCount` 1); `:1210` "applies a native_messages_delta in place and acks its note deliveries"; `:1574` "rejects a delta whose prefix fingerprint is not acknowledged" (marker: `lease.chargedBytes` 0 after the throw). No test mutates a kept entry's data values before reuse.
Impact: a delta is spliced onto output the model never saw.
Open questions:
- #538 must add guarded previous-output advertisement and reuse, with before-request and during-await value-mutation tests.

### delivery-disposition-follows-publication

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - Supersession, invalidation, clear, source change, byte decline, retry rejection, and apply errors each NACK the attempt's known IDs; applied attempts ACK only their own IDs; a paused NACK and a following pass yield per-identity `[method, transform_pass_id]` tuples; a throwing ACK leaves the published array in place.
Guarantee: Only an applied attempt's delivery IDs receive ACK attempts, discarded attempts' known IDs receive NACK attempts, and ACK failure never undoes published output.
Check: `always` - `deliveries.attempted` collects IDs per response (`rust-mode-transform.ts:1317`), `deliveries.applied` is set only in the synchronous publication block (`:1482`), and `deliverTransformNotes` (`:747-775`) sends NACK for attempted-not-applied and ACK for applied, after the lease release (`:1520-1522`); an ACK error is logged and leaves the published candidate unchanged.
Fault/timing angle: rejection after the response carried delivery IDs; ACK transport failure; a newer pass during a paused ACK or NACK.
Required faults and enabling state: a response with `note_deliveries` rejected before publication; a throwing or paused `transform.ack`; a paused `transform.nack` followed by an accepted pass.
Confidence: medium - [evidence](evidence/delivery-disposition-follows-publication.md). Every named witness was read and ran green. Calls to the fake client are attempted dispositions; daemon-side receipt is not observed by these tests.
Existing check: `rust-mode-transform.test.ts:2968` "releases rejected capture state before a paused NACK and keeps delivery IDs separate" (marker: `nackStarted.promise` race, then `[["transform.nack", "discarded"], ["transform.ack", "accepted"]]`); `:2907` "releases capture admission before the ACK so a paused ACK does not block the next pass" (marker: `ackStarted.promise` race; after the ACK throws, `firstOutput.messages[0]` is still `applied[0]`); `:1431` "nacks discarded delivery IDs and acks only IDs from the applied retry response"; `:1473` "nacks initial and retry delivery IDs when the full retry still cannot be applied"; `:1361` "disposes each duplicate delivery pass ID only once"; `:1258` "attempts every ack sequentially before reporting aggregate failures" (marker: `maxActiveCalls` 1); `:1385` "attempts every nack without replacing the original apply error"; `:1503` "nacks note deliveries and serves the input unchanged when the boundary lacks a synthetic m0"; `:1210` "applies a native_messages_delta in place and acks its note deliveries"; `:909` "nacks note deliveries when a pass is superseded while its transform response is pending" (marker: `calls` 1 before the newer call); `:2482` "does not dispatch a need_full_sync retry after ..." (4 cases; NACK of `retry-discarded`); `:1916` (2 cases; ACK of `candidate` on publish, NACK on decline); `:2037` "recovers with a full request when byte pressure rejects a full-sync retry" (NACK `discarded`, later ACK `applied`).
Impact: the daemon marks a note delivered that the model never received.
Open questions: None.

### owner-admission-covers-live-captures

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - A 1,000-message fixture holds a peak charge above its JSON size and below half the default budget; byte-only pressure declines at an exactly computed held charge; count pressure declines the third pass without queueing; a model-based test compares counts and charges to a reference across seeded lease sequences; a paused ACK and a paused NACK show zero counters after publication or rejection.
Guarantee: at most one live lease per session and 64 per process; the aggregate charge never exceeds 64 MiB; admission ends only after protected state is dropped or transferred and before delivery awaits.
Check: `always` - `admit` runs in `run` before any await (`rust-mode-transform.ts:1509-1517`); the capture charge, the wire-projection charge (`WIRE_PROJECTION_FACTOR` 4 at `:716`, `:996-1002`), and the memo-copy charge (`:1003-1006`) are reserved before the first await at `:1018`; the walk charges `TAPE_SLOT_BYTES` per retained slot, two bytes per UTF-16 unit of strings and symbol descriptions, the declared array length, and one slot per descriptor read (`transform-capture.ts:9`, `:116-124`, `:192-198`, `:240-241`); the lease releases in `.finally` before `deliverTransformNotes` (`:1520-1522`); counts and charges stay within limits and equal a reference model.
Fault/timing angle: many concurrent passes; large captures; a paused ACK or NACK.
Required faults and enabling state: a paused transport holding leases; injected small limits; a blocker lease that leaves an exact remainder.
Confidence: medium - [evidence](evidence/owner-admission-covers-live-captures.md). Every named witness was read and ran green; the byte-pressure test asserts the exact held charge composition, and the model-based test fixes its seed in the failure message. Successful publication transfers `captured.snapshots` (as `rawContentSnapshots`), the candidate array, and the promoted memo to the 64-session `wireCaches` owner and `states`; those retained bytes are count-bounded, not byte-bounded. The separate optional-output byte budget is TE25 in #538.
Existing check: `rust-mode-transform.test.ts:1813` "admits a full pass over 1,000 realistic messages under the default budget with room for a second session" (marker: `jsonBytes > 2 MiB`; `peakCharge > jsonBytes`, `peakCharge < admission.remainingBytes / 2`, `chargedBytes` 0 after); `:2805` "declines on byte pressure alone while the aggregate charge stays within the budget" (marker: `expect(admission.chargedBytes).toBe(heldCharge)` where `heldCharge` sums capture, 4x wire bytes, annotation, and memo charges; `smallInspection.estimatedBytes > remainingBytes`); `:2756` "keeps every pass within the global count limit and declines without queueing" (marker: `activePasses` 2, `calls` 2, decline log `pass_count`); `:2907` "releases capture admission before the ACK so a paused ACK does not block the next pass" (marker: `ackStarted.promise` race, then `activePasses` 0 and `remainingBytes` 64 MiB); `:2968` "releases rejected capture state before a paused NACK and keeps delivery IDs separate"; `:3052` "shares default admission across factories and cancels the earlier session owner" (marker: `defaultTransformCaptureAdmission.chargedBytes` equals `heldBytes` after the decline); `:2255` (8 cases; `heldBytes > MODULE_ORDINAL_PAGE_SIZE * ORDINAL_ENTRY_RETAINED_BYTES` at the scan yield); `transform-capture.test.ts:678` "keeps counts and charges equal to a reference model across generated lease sequences" (200 seeded sequences of 40 steps; marker: `[activePasses, chargedBytes, detail]` equals the model each step); `:760` "shares the default 64-pass and 64 MiB owner across independent borrowers"; `:787` "rejects invalid charges, isolates accounting, and cannot release a replacement lease"; `:817` "holds one lease per session, declines a newer call, and cancels the holder"; `:833` "declines beyond the global pass count without queueing"; `:843` "charges bytes against one aggregate budget and releases them exactly once"; `:867` "requires reservation before capture and refuses released or cancelled leases"; `:944` "accepts the exact byte charge and rejects one byte less"; `hook.test.ts:337` "admits before the actual hook directory await and never restores a rebound output" (marker: `get` called once, second call declines `session_busy`).
Impact: unbounded concurrent captures exhaust memory or starve the event loop.
Open questions:
- The retained `wireCaches` entries after transfer are bounded by the 64-session count, not by bytes; the byte budget for that optional-output state is TE25 in #538.

### capture-charge-outlives-cancellation

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - A newer same-session call, `clearSession`, and `invalidateWireState` each abort the lease signal while `activePasses` stays 1 and `chargedBytes` stays above zero; the counters reach zero only after the cancelled owner's promise settles; a stale lease's second release cannot release a replacement lease; `requestCancel` never changes the charge.
Guarantee: Cancellation, supersession, clear, and invalidation cannot release a charge while protected state remains live; the owner releases it exactly once after cleanup or accounted transfer.
Check: `always` - `requestCancel` only aborts the controller (`transform-capture.ts:517-519`, `:532-534`); `release` is idempotent and is the only path that subtracts the charge and frees the session slot (`:520-526`); the transform calls it in `.finally` after `execute` settles (`rust-mode-transform.ts:1520-1521`); cancellation alone leaves the charge held.
Fault/timing angle: a newer call, clear, or invalidation arrives while the owner awaits transport or an ordinal page.
Required faults and enabling state: a paused transport or page read; a second same-session call, `clearSession`, or `invalidateWireState`; a client that rejects on abort so settlement is observable.
Confidence: medium - [evidence](evidence/capture-charge-outlives-cancellation.md). Every named witness was read and ran green. The transport in `:2119` honors the abort signal; a fake that ignores cancellation (`:2874`) also holds the charge until its response settles. Real daemon transport abort is outside these tests.
Existing check: `rust-mode-transform.test.ts:2119` "propagates supersede|clear|invalidate to the transport signal and waits for rejection before release" (3 cases; marker: `started.promise` yields the signal with `aborted` false and `chargedBytes > 0`; after the fault `signal.aborted` true, `activePasses` 1, `chargedBytes > 0`; after `await pass` both 0); `:2874` "holds the charge through a slow cancellation until the cancelled owner settles" (marker: `chargedWhileHeld > 0`; after the declined newer call `activePasses` 1 and `chargedBytes` equals `chargedWhileHeld`); `:2255` (8 cases; `expect(admission.chargedBytes).toBe(heldBytes)` after the fault at the scan yield); `:3052` "shares default admission across factories and cancels the earlier session owner"; `transform-capture.test.ts:843` "charges bytes against one aggregate budget and releases them exactly once" (marker: `a.lease.requestCancel("test")` leaves `chargedBytes` 100; double release subtracts once); `:787` "rejects invalid charges, isolates accounting, and cannot release a replacement lease"; `:817` "holds one lease per session, declines a newer call, and cancels the holder"; `:678` model-based sequences including `cancel` and `stale` operations.
Impact: a slow cancelled pass's memory is double-counted as free and the budget over-admits.
Open questions:
- Real daemon transport abort through the lease signal is not exercised; the tests use in-process clients that honor or ignore the signal.

### uncertain-send-never-replays-blindly

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - A first `transform` call that throws records exactly one call and no resend; a thrown full retry after `need_full_sync` records no further send; a reconnect after a mutation or invalidation stops the bounded series restart; positive controls show one restart on attempt mismatch or reconnect alone.
Guarantee: An outcome-unknown transport failure does not trigger blind replay; application-specific bounded recovery remains distinct from a generic transport retry.
Check: `always` - exactly one `transform` call is recorded for a pass whose first call throws a transport error; `sendTransformSeries` rethrows generic errors (`rust-mode-transform.ts:1307-1311`) and only classified attempt-mismatch or reconnect results restart, once (`:1333-1356`).
Fault/timing angle: connection loss after the request was written.
Required faults and enabling state: a module client that throws on the first call; a client that throws on the full retry; a reconnect result after a source or wire fault.
Confidence: medium - [evidence](evidence/uncertain-send-never-replays-blindly.md). Every named witness was read and ran green. The thrown error is the fake's stand-in for post-write loss; whether the daemon applied the lost request is not observable in these tests.
Existing check: `rust-mode-transform.test.ts:3137` "does not resend after an outcome-unknown transport failure and recovers on the next attempt" (marker: `expect(calls).toHaveLength(1)` and `failureCount` 1 after the throw); `:2223` "retains full-sync recovery after a failed retry until a full request publishes" (marker: `bodies` 3 after the thrown full retry; `forceFullWire` true); `:1870` "stops a series restart after a mid-series reconnect when mutation|invalidation lands first" (2 cases; marker: one page-zero body before and after; `["transform", "transform", "transform.nack"]`); positive controls `:594` "restarts a paged transform series after a thrown-code|returned-code|message attempt mismatch" (3 cases; two series starts) and `:640` "restarts a paged transform series after a mid-series reconnect".
Impact: a duplicate transform mutates daemon state twice for one host turn.
Open questions:
- A real transport that writes the request and loses the response is not constructed; the fake throws before returning.

### bounded-recovery-after-pressure-clears

Type: liveness
Reachability: explicit-config-only
Status: active
Exercised: yes - After count pressure, byte pressure, a memo-copy decline, a wire-projection decline, a scan-yield rejection, supersession, and an outcome-unknown throw, each test observes settlement (`activePasses` 0 or `chargedBytes` 0, or the blocker lease released) and then a single `run` records one new `transform` dispatch.
Guarantee: once the fault stops and owned work settles, the next valid attempt is admitted and dispatched through the existing paths within one attempt.
Check: `sometimes` - a declined or failed session's next `run` records a `transform` call after `activePasses` returns to zero or the blocking charge is released; the bound is one attempt after settlement, not an unbounded eventually. Declines do not call `markFailure` (`rust-mode-transform.ts:1497-1503`), so no failure count accrues against the recovering session.
Fault/timing angle: the window between the decline and settlement.
Required faults and enabling state: a prior decline (count, byte, or session busy) or transport failure whose cause is removed before the next call; settlement observed before the single recovery call.
Confidence: medium - [evidence](evidence/bounded-recovery-after-pressure-clears.md). Every named witness was read and ran green; each asserts zero counters or the blocker's release before the one recovery call, then a `calls` length increase of one.
Existing check: `rust-mode-transform.test.ts:2756` "keeps every pass within the global count limit and declines without queueing" (marker: `activePasses` 0 and `chargedBytes` 0 after `Promise.all(passes)`, `failureCount` 0, then one `run` gives `calls` 3); `:2805` "declines on byte pressure alone while the aggregate charge stays within the budget" (marker: `chargedBytes` 0 after `await first`, then one `run` gives `calls` 2 and `initialized` true); `:2037` "recovers with a full request when byte pressure rejects a full-sync retry" (marker: `blocker.lease.release()`, then one `run` sends a full body and ACKs `applied`); `:2168` "charges every existing ordinal entry and ID before copying a warm memo" (marker: `chargedBytes` 0 after the blocker releases, then one `run` gives `bodies` 2 with `tail_delta`); `:2255` (8 cases; marker: `activePasses` 0 and `chargedBytes` 0, then one `run` gives `calls` 1 and `entries.size` equal to `rows.length`); `:3052` "shares default admission across factories and cancels the earlier session owner" (marker: default owner at 0, then the second factory dispatches once); `:3137` "does not resend after an outcome-unknown transport failure and recovers on the next attempt" (marker: `calls` 2, `consecutiveFailures` 0); `:2968` (next pass after a paused NACK dispatches once).
Impact: a session stays declined after the pressure that declined it is gone.
Open questions: None.

### inbound-baseline-independent-of-output-base

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: partial - The ACK-release test and the ack-supersession test publish output different from the input and then run later passes that dispatch; the prefix-mutation guard shows a full resend after an older input edit. No test compares a tail-delta result with a full-request control after changed output, and no optional-output-only eviction case exists.
Guarantee: the accepted submitted input (wire cache snapshots and fingerprints) remains the inbound baseline for the next delta regardless of what output was applied.
Check: `always` - After changed output and a raw-input append, the tail-delta and full-request control produce equal approved output; inbound fingerprints, frontiers, and guards still describe accepted submitted input, including after optional-output-only eviction.
Fault/timing angle: applied output differs from input, then the host appends.
Required faults and enabling state: a response that rewrites output, followed by an append, with a full-request control under equivalent daemon state.
Confidence: low - [evidence](evidence/inbound-baseline-independent-of-output-base.md). `buildWireCache` derives `rawContentSnapshots` and fingerprints from `captured.snapshots` and the submitted messages (`rust-mode-transform.ts:267-292`, `:1219-1227`) and `nativeOutput` is attached separately at `:1476`; that is source evidence, not an equality oracle.
Existing check: `rust-mode-transform.test.ts:2907` "releases capture admission before the ACK so a paused ACK does not block the next pass" (two passes; no delta/full comparison); `:1312` "keeps applied note output when a newer pass starts during the ack" (marker: third body's `tail_delta.native_replace_from` 1); `:1590` "in-place mutation of an older message forces a full send instead of a delta" (input-side guard, not an output-base comparison).
Impact: the daemon's inbound baseline drifts toward applied output and deltas mis-splice.
Open questions:
- #538 must supply the delta/full control comparison and the optional-output-only eviction case; two transform calls do not establish equality.

## Relationship map

- TE19 gates TE17: the snapshot walk requires a hook-free source domain.
- TE17 and TE18 feed TE20: publication requires a coherent capture and a
  staged memo to promote.
- TE20 precedes TE22: ACK eligibility depends on publication, while discarded
  attempts retain separate NACK obligations.
- TE23 and TE24 bound the memory that TE17's capture retains until the
  publication block transfers it to `wireCaches` and `states`.
- TE26 and TE27 describe the failure and recovery edges of the same owner.

/**
 * An in-memory stand-in for the daemon's `kernel.*` routes, driven through
 * the `KernelTransport` surface so consumers are tested against the same
 * client they ship with. It keeps the semantics the client relies on:
 * `known_as_of` tokens, the three conflict reasons, replay by operation key,
 * supersession chains, admission classes derived from `source_kind` and
 * lowered by the asserted classes, the envelope limits, disposition events
 * with the kernel's fixed table and approval-gated relaxation, previews of
 * them, and per-surface visibility: a `labeled` row serves only on
 * `explicit_search`, a non-active disposition serves labeled at most or not
 * at all, `sensitive` rows hide from the automatic surfaces, and `secret` rows
 * hide everywhere. Rows carry the project root they were written under and
 * serve only to that project. Scripted surface and commit states override the
 * row-backed replies.
 */

import { HostCallError } from "../host-client";
import {
    DISPOSITION_EVENTS,
    type DispositionEvent,
    type DispositionResult,
    type KernelMemorySnapshot,
    type KernelTransport,
    type KernelTransportCall,
    MAX_COMMIT_OPERATIONS,
    MAX_COMMIT_TOKENS,
    type MemoryState,
    parseReadResponse,
    type Surface,
    type SurfaceVisibilities,
    sha256Hex,
    type Visibility,
} from "../kernel-client";

export type FakeDisposition =
    | "active"
    | "stale"
    | "disputed"
    | "superseded"
    | "rejected"
    | "contradicted"
    | "quarantined";

export interface FakeObject {
    object_id: string;
    object_kind: string;
    domain_id: string;
    source_kind: string;
    source_id: string;
    source_revision: number;
    created_commit_seq: number;
    invalidated_commit_seq: number | null;
    superseded_by: string | null;
    sensitivity: "normal" | "sensitive" | "secret";
    labeled: boolean;
    /** The kernel's disposition ratchet; `active` unless a disposition event moved it. */
    disposition: FakeDisposition;
    /**
     * The project root the row was written under. `null` marks a seeded row
     * that serves to every project, a shape no route commit can produce.
     */
    project_root: string | null;
    decision?: { decision_kind: string; payload: { summary: string; rationale: string } };
}

interface Receipt {
    commit_seq: number;
    request_digest: string;
    tokens: { object_id: string; known_as_of: number }[];
    merged: string[];
    dispositions: DispositionResult[];
}

/** The kernel's fixed event table: the command outcome and the disposition each event asks for. */
const EVENT_EFFECT: Record<DispositionEvent, { outcome: string; disposition: FakeDisposition }> = {
    mark_stale: { outcome: "deny", disposition: "stale" },
    mark_disputed: { outcome: "deny", disposition: "disputed" },
    explicit_reject: { outcome: "reject", disposition: "rejected" },
    contradict: { outcome: "deny", disposition: "contradicted" },
    quarantine: { outcome: "quarantine", disposition: "quarantined" },
};

/** Moving to a lower rank is a relaxation and needs a valid approval. */
const DISPOSITION_RANK: Record<FakeDisposition, number> = {
    active: 0,
    stale: 1,
    disputed: 1,
    superseded: 1,
    rejected: 2,
    contradicted: 3,
    quarantined: 3,
};

type VisibilityRow = "automatic" | "explicit_labeled" | "review_only" | "audit_only";

function visibilityRow(labeled: boolean, disposition: FakeDisposition): VisibilityRow {
    switch (disposition) {
        case "active":
            return labeled ? "explicit_labeled" : "automatic";
        case "stale":
        case "disputed":
        case "superseded":
            return "explicit_labeled";
        case "rejected":
            return "review_only";
        case "contradicted":
        case "quarantined":
            return "audit_only";
    }
}

function surfaceVisibility(
    row: VisibilityRow,
    surface: Surface,
    sensitivity: Sensitivity,
): Visibility {
    if (sensitivity === "secret") return "hidden";
    if (sensitivity === "sensitive" && surface !== "explicit_search") return "hidden";
    if (row === "automatic") return "visible";
    if (row === "explicit_labeled" && surface === "explicit_search") return "labeled";
    return "hidden";
}

function surfaceVisibilities(row: VisibilityRow, sensitivity: Sensitivity): SurfaceVisibilities {
    return {
        auto_inject: surfaceVisibility(row, "auto_inject", sensitivity),
        auto_search: surfaceVisibility(row, "auto_search", sensitivity),
        explicit_search: surfaceVisibility(row, "explicit_search", sensitivity),
    };
}

type Operation = Record<string, unknown> & { op: string };

type Sensitivity = FakeObject["sensitivity"];

/** Keyed on the union so a new `Surface` member fails to typecheck here. */
const SURFACE_SET: Record<Surface, true> = {
    auto_inject: true,
    auto_search: true,
    explicit_search: true,
};
const SURFACES: readonly Surface[] = Object.keys(SURFACE_SET) as Surface[];

const SENSITIVITY_RANK: Record<Sensitivity, number> = { normal: 0, sensitive: 1, secret: 2 };

function restrictive(left: Sensitivity, right: Sensitivity): Sensitivity {
    return SENSITIVITY_RANK[left] >= SENSITIVITY_RANK[right] ? left : right;
}

/**
 * `project:` plus the sha256 of the root path bytes, the scope id the daemon
 * materializes per project. The daemon canonicalizes the root first; the fake
 * hashes `projectRoot` without canonicalizing.
 */
export function fakeProjectScopeId(projectRoot: string): string {
    return `project:${sha256Hex(projectRoot)}`;
}

function invalid(reason: string): unknown {
    return { state: { kind: "invalid", reason } };
}

function conflict(reason: string): unknown {
    return { state: { kind: "conflict", reason } };
}

/** Thrown rather than returned: the daemon answers this code as an error frame with no kernel `state`, which the client maps to `invalid:invalid_input`. commentlint: allow(JUDGE) */
function invalidParams(message: string): HostCallError {
    return new HostCallError("terminal", message, "invalid_params");
}

/** Lower ranks are more trusted; an asserted class may only use a rank equal to or above the derived class. Keyed on the daemon's serialized class names. commentlint: allow(JUDGE) */
const SOURCE_RANK: Record<string, number> = {
    explicit_user: 0,
    trusted_local_code: 1,
    trusted_tool_result: 2,
    untrusted_repo_text: 3,
    untrusted_web: 4,
    model_inference: 5,
};

const TAINT_RANK: Record<string, number> = {
    user_explicit: 0,
    current_code: 1,
    current_test: 1,
    current_config: 1,
    user_inferred: 2,
    repo_untrusted_text: 3,
    tool_untrusted_output: 3,
    assistant_inference: 4,
    dreamer_inference: 4,
    personal: 5,
    unclassifiable: 5,
};

/** Everything a plugin relays is model output about something, so the derived source class is always `model_inference`; the taint class records what it is about. commentlint: allow(JUDGE) */
const DERIVED_CLASSES: Record<string, { source: string; taint: string }> = {
    assistant: { source: "model_inference", taint: "assistant_inference" },
    model: { source: "model_inference", taint: "assistant_inference" },
    dreamer: { source: "model_inference", taint: "dreamer_inference" },
    user: { source: "model_inference", taint: "user_inferred" },
};

/** The taints `model_inference` admits; a pair outside the table is malformed input, not an over-declaration. commentlint: allow(JUDGE) */
const MODEL_INFERENCE_TAINTS: ReadonlySet<string> = new Set([
    "user_inferred",
    "assistant_inference",
    "dreamer_inference",
    "personal",
    "unclassifiable",
]);

/** An assertion above the derived class is refused rather than clamped so the caller learns its claim was not accepted. commentlint: allow(JUDGE) */
function resolveClasses(
    body: Record<string, unknown>,
): { sourceKind: string } | { reply: unknown } {
    const sourceKind = body.source_kind;
    if (typeof sourceKind !== "string") throw invalidParams("kernel.commit requires source_kind");
    const derived = DERIVED_CLASSES[sourceKind];
    if (!derived) return { reply: invalid("invalid_input") };
    let source = derived.source;
    if (body.asserted_source_class !== undefined) {
        const asserted = body.asserted_source_class;
        if (typeof asserted !== "string" || !(asserted in SOURCE_RANK)) {
            return { reply: invalid("invalid_input") };
        }
        if ((SOURCE_RANK[asserted] as number) < (SOURCE_RANK[derived.source] as number)) {
            return { reply: invalid("class_over_declared") };
        }
        source = asserted;
    }
    let taint = derived.taint;
    if (body.asserted_taint_class !== undefined) {
        const asserted = body.asserted_taint_class;
        if (typeof asserted !== "string" || !(asserted in TAINT_RANK)) {
            return { reply: invalid("invalid_input") };
        }
        if ((TAINT_RANK[asserted] as number) < (TAINT_RANK[derived.taint] as number)) {
            return { reply: invalid("class_over_declared") };
        }
        taint = asserted;
    }
    // Every derived source is `model_inference` and a lower-ranked assertion was refused above, so this is the only source row the table needs. commentlint: allow(JUDGE)
    if (source !== "model_inference" || !MODEL_INFERENCE_TAINTS.has(taint)) {
        return { reply: invalid("invalid_input") };
    }
    return { sourceKind };
}

export class FakeKernel {
    tip = 0;
    readonly objects = new Map<string, FakeObject>();
    /** Latest commit that changed each object; `kernel.commit` compares tokens against it. */
    readonly lastChange = new Map<string, number>();
    readonly receipts = new Map<string, Receipt>();
    /** Every `decision_id` the store has held, live or retired; the daemon's `decisions` primary key refuses a second insert under any of them. commentlint: allow(JUDGE) */
    readonly decisionIds = new Set<string>();
    /** Objects the kernel would honor as approval authority: a live `adr_accepted` decision admitted by an explicit user. Any other live object cited as an approval is valid to name but grants nothing, so a relaxation citing it is denied. commentlint: allow(JUDGE) */
    readonly approvals = new Set<string>();
    /** Forces every read on a surface to answer with this state instead of rows. */
    readonly surfaceStates = new Map<Surface, MemoryState>();
    /** Forces the next commit to answer with this state. */
    nextCommitState: MemoryState | null = null;
    /** Every read reply carries this `truncated` flag, standing in for a daemon that dropped rows to fit its per-read bounds. commentlint: allow(JUDGE) */
    readTruncated = false;
    /** Rows served per unfiltered read when set, standing in for the daemon's newest-rows cap; a read with an `object_ids` filter ignores it, as the daemon's cap never binds a filter-sized read. commentlint: allow(JUDGE) */
    readRowCap: number | null = null;
    /** Rows served per filtered read when set, standing in for the daemon's serialization byte budget: the `object_ids` filter bypasses the row cap but not the budget, and the budget keeps a newest-first prefix of the filtered rows. commentlint: allow(JUDGE) */
    filteredReadRowCap: number | null = null;
    /** Runs after the client's read and before the commit's token check, standing in for a concurrent writer. */
    beforeCommit: (() => void) | null = null;

    private nextSeq(): number {
        this.tip += 1;
        return this.tip;
    }

    /**
     * Seeds a live decision object as if a prior commit had written it. Route
     * writes are `labeled`; `labeled: false` stands in for a verified object
     * only a direct store commit can produce. Without `projectRoot` the row
     * serves to every project. `decision_id` defaults to `object_id`.
     */
    seedDecision(input: {
        object_id: string;
        decision_id?: string;
        decision_kind: string;
        summary: string;
        rationale?: string;
        labeled?: boolean;
        disposition?: FakeDisposition;
        sensitivity?: FakeObject["sensitivity"];
        source_revision?: number;
        domain_id?: string;
        source_kind?: string;
        source_id?: string;
        projectRoot?: string;
    }): FakeObject {
        const seq = this.nextSeq();
        const object: FakeObject = {
            object_id: input.object_id,
            object_kind: "decision",
            domain_id: input.domain_id ?? "memory",
            source_kind: input.source_kind ?? "assistant",
            source_id: input.source_id ?? "ctx_memory",
            source_revision: input.source_revision ?? 1,
            created_commit_seq: seq,
            invalidated_commit_seq: null,
            superseded_by: null,
            sensitivity: input.sensitivity ?? "normal",
            labeled: input.labeled ?? true,
            disposition: input.disposition ?? "active",
            project_root: input.projectRoot ?? null,
            decision: {
                decision_kind: input.decision_kind,
                payload: { summary: input.summary, rationale: input.rationale ?? "" },
            },
        };
        this.objects.set(object.object_id, object);
        this.lastChange.set(object.object_id, seq);
        this.decisionIds.add(input.decision_id ?? input.object_id);
        return object;
    }

    /** Seeds a live `adr_accepted` decision the fake honors as approval authority, as the route test's `seed_approval` does. */
    seedApproval(objectId: string, projectRoot?: string): FakeObject {
        const object = this.seedDecision({
            object_id: objectId,
            decision_kind: "adr_accepted",
            summary: "Approved by the user.",
            source_kind: "user",
            ...(projectRoot === undefined ? {} : { projectRoot }),
        });
        this.approvals.add(objectId);
        return object;
    }

    /** A change outside the client's view: the object's `known_as_of` advances without a payload change. */
    touch(objectId: string): void {
        const seq = this.nextSeq();
        this.lastChange.set(objectId, seq);
    }

    /**
     * The snapshot a client would hold after reading `surface` at the tip,
     * parsed through the same wire decoder, for consumers that take the
     * snapshot as a value instead of dialing. Without `projectRoot` no
     * project filter applies.
     */
    snapshot(surface: Surface = "explicit_search", projectRoot?: string): KernelMemorySnapshot {
        const parsed = parseReadResponse(
            this.readReply({ surface, gated: false }, projectRoot ?? null),
        );
        return parsed.payload
            ? {
                  state: parsed.state,
                  rows: parsed.payload.rows,
                  knownAsOf: parsed.payload.known_as_of,
                  truncated: parsed.payload.truncated,
              }
            : { state: parsed.state, rows: [], knownAsOf: null };
    }

    liveRows(): FakeObject[] {
        return [...this.objects.values()]
            .filter((object) => object.invalidated_commit_seq === null)
            .sort((left, right) => (left.object_id < right.object_id ? -1 : 1));
    }

    private static visibilityOn(object: FakeObject, surface: Surface): Visibility {
        return surfaceVisibility(
            visibilityRow(object.labeled, object.disposition),
            surface,
            object.sensitivity,
        );
    }

    private static servesOn(object: FakeObject, surface: Surface): boolean {
        return FakeKernel.visibilityOn(object, surface) !== "hidden";
    }

    /** Newest `created_commit_seq` first, then ascending `object_id` within one commit. */
    private static servingOrder(left: FakeObject, right: FakeObject): number {
        if (left.created_commit_seq !== right.created_commit_seq) {
            return right.created_commit_seq - left.created_commit_seq;
        }
        return left.object_id < right.object_id ? -1 : left.object_id > right.object_id ? 1 : 0;
    }

    /** Whether a row is in the calling project's scope; a seeded row without a root, or a call without one, passes. */
    private static inProject(object: FakeObject, projectRoot: string | null): boolean {
        return (
            projectRoot === null ||
            object.project_root === null ||
            object.project_root === projectRoot
        );
    }

    private readReply(body: Record<string, unknown>, projectRoot: string | null): unknown {
        const surface = body.surface;
        if (!(SURFACES as readonly unknown[]).includes(surface)) return invalid("invalid_input");
        const forced = this.surfaceStates.get(surface as Surface);
        if (forced && forced.kind !== "available") return { state: forced };
        const asOf = typeof body.as_of === "number" ? body.as_of : this.tip;
        if (asOf > this.tip) return { state: { kind: "unavailable", reason: "snapshot_diverged" } };
        const objectIds = Array.isArray(body.object_ids)
            ? new Set(body.object_ids.filter((id): id is string => typeof id === "string"))
            : null;
        let visible = [...this.objects.values()].filter(
            (object) =>
                object.created_commit_seq <= asOf &&
                (object.invalidated_commit_seq === null || asOf < object.invalidated_commit_seq) &&
                FakeKernel.inProject(object, projectRoot) &&
                FakeKernel.servesOn(object, surface as Surface),
        );
        if (objectIds !== null) {
            visible = visible.filter((object) => objectIds.has(object.object_id));
        }
        // The daemon serves rows newest first, then by object id, and keeps that order's prefix when a cap binds. The row cap never binds a filtered read (a filter names at most `MAX_READ_OBJECT_IDS` rows, far under the cap), so it applies to unfiltered reads alone; the filtered cap stands in for the byte budget, which binds either way. commentlint: allow(JUDGE)
        visible.sort(FakeKernel.servingOrder);
        let truncated = this.readTruncated;
        const cap = objectIds === null ? this.readRowCap : this.filteredReadRowCap;
        if (cap !== null && visible.length > cap) {
            visible = visible.slice(0, cap);
            truncated = true;
        }
        const rows = visible.map((object) => {
            const { labeled, disposition: _disposition, project_root, decision, ...row } = object;
            const visibility = FakeKernel.visibilityOn(object, surface as Surface);
            return {
                object: row,
                visibility,
                labeled: visibility === "labeled",
                scope_id: fakeProjectScopeId(project_root ?? projectRoot ?? ""),
                token: { object_id: object.object_id, known_as_of: asOf },
                decision: decision ?? null,
            };
        });
        return {
            state: { kind: "available" },
            known_as_of: asOf,
            tip: this.tip,
            gated: body.gated === true,
            truncated,
            rows,
        };
    }

    /**
     * The daemon's token check: an object the store never held or another
     * project's object is `not_found`, so foreign ids are not enumerable; a
     * token from a snapshot past the tip is refused before the object is
     * consulted; an invalidated object names the disposition that invalidated
     * it; a live object that changed after the token's `known_as_of` is
     * `known_as_of_advanced`.
     */
    private conflictFor(
        tokens: { object_id: string; known_as_of: number }[],
        projectRoot: string | null,
    ): unknown | null {
        for (const token of tokens) {
            const object = this.objects.get(token.object_id);
            if (!object || !FakeKernel.inProject(object, projectRoot)) return invalid("not_found");
            if (token.known_as_of > this.tip) {
                return { state: { kind: "unavailable", reason: "snapshot_diverged" } };
            }
            if (object.invalidated_commit_seq !== null) {
                return conflict(object.superseded_by ? "superseded" : "retracted");
            }
            if ((this.lastChange.get(token.object_id) ?? 0) > token.known_as_of) {
                return conflict("known_as_of_advanced");
            }
        }
        return null;
    }

    /**
     * The checks a commit and a preview share, in the daemon's order: the
     * scripted state, the envelope limits, class resolution, then the receipt
     * lookup, so an over-declared class cannot replay a receipt.
     */
    private commitPreflight(body: Record<string, unknown>):
        | { reply: unknown }
        | {
              operations: Operation[];
              tokens: { object_id: string; known_as_of: number }[];
              sourceKind: string;
              intent: { operation_key: string; request_digest: string };
              replayed: Receipt | null;
          } {
        if (this.nextCommitState) {
            const state = this.nextCommitState;
            this.nextCommitState = null;
            return { reply: { state } };
        }
        const operations = (body.operations as Operation[] | undefined) ?? [];
        if (operations.length > MAX_COMMIT_OPERATIONS) {
            throw invalidParams(
                `kernel.commit carries at most ${MAX_COMMIT_OPERATIONS} operations`,
            );
        }
        const tokens = (body.tokens as { object_id: string; known_as_of: number }[]) ?? [];
        if (tokens.length > MAX_COMMIT_TOKENS) {
            throw invalidParams(`kernel.commit carries at most ${MAX_COMMIT_TOKENS} tokens`);
        }
        const classes = resolveClasses(body);
        if ("reply" in classes) return classes;
        const intent = body.intent as { operation_key: string; request_digest: string };
        const replayed = this.receipts.get(intent.operation_key) ?? null;
        if (replayed && replayed.request_digest !== intent.request_digest) {
            return { reply: invalid("operation_key_reused") };
        }
        return { operations, tokens, sourceKind: classes.sourceKind, intent, replayed };
    }

    private commitReply(body: Record<string, unknown>, projectRoot: string | null): unknown {
        const preflight = this.commitPreflight(body);
        if ("reply" in preflight) return preflight.reply;
        const { operations, tokens, sourceKind, intent, replayed } = preflight;
        if (replayed) {
            return {
                state: { kind: "available" },
                receipt: { commit_seq: replayed.commit_seq, replayed: true },
                known_as_of: replayed.commit_seq,
                tokens: replayed.tokens,
                merged: replayed.merged,
                ...(replayed.dispositions.length === 0
                    ? {}
                    : { dispositions: replayed.dispositions }),
            };
        }
        this.beforeCommit?.();
        const tokenConflict = this.conflictFor(tokens, projectRoot);
        if (tokenConflict) return tokenConflict;
        // One envelope is atomic: rows change on a staged overlay in envelope order, and a refusal at any operation leaves the store and the tip untouched. commentlint: allow(JUDGE)
        const seq = this.tip + 1;
        const staged = new Map<string, FakeObject>();
        const stagedDecisionIds = new Set<string>();
        const touched = new Set<string>();
        /** Objects whose row changed structurally; the token advances for these alone. */
        const changed = new Set<string>();
        const merged = new Set<string>();
        const dispositions: DispositionResult[] = [];
        const view = (objectId: string): FakeObject | undefined =>
            staged.get(objectId) ?? this.objects.get(objectId);
        const stage = (objectId: string): FakeObject => {
            let row = staged.get(objectId);
            if (!row) {
                row = { ...(this.objects.get(objectId) as FakeObject) };
                staged.set(objectId, row);
            }
            return row;
        };
        // A commit target is looked up among this project's live objects only, so a missing, foreign, or invalidated target is `not_found` alike. commentlint: allow(JUDGE)
        const liveTarget = (objectId: string): FakeObject | null => {
            const target = view(objectId);
            if (
                !target ||
                !FakeKernel.inProject(target, projectRoot) ||
                target.invalidated_commit_seq !== null
            ) {
                return null;
            }
            return target;
        };
        // The daemon's `decisions` primary key refuses a `decision_id` any row has ever carried, live or retired, in this envelope or an earlier commit. commentlint: allow(JUDGE)
        const decisionIdHeld = (spec: Record<string, unknown>): boolean =>
            typeof spec.decision_id === "string" &&
            (this.decisionIds.has(spec.decision_id) || stagedDecisionIds.has(spec.decision_id));
        const insert = (spec: Record<string, unknown>, sensitivity: Sensitivity): void => {
            const objectId = spec.object_id as string;
            if (typeof spec.decision_id === "string") stagedDecisionIds.add(spec.decision_id);
            staged.set(objectId, {
                object_id: objectId,
                object_kind: "decision",
                domain_id: spec.domain_id as string,
                source_kind: sourceKind,
                source_id: spec.source_id as string,
                source_revision: spec.source_revision as number,
                created_commit_seq: seq,
                invalidated_commit_seq: null,
                superseded_by: null,
                sensitivity,
                labeled: true,
                disposition: "active",
                project_root: projectRoot,
                decision: {
                    decision_kind: spec.decision_kind as string,
                    payload: spec.payload as { summary: string; rationale: string },
                },
            });
            touched.add(objectId);
            changed.add(objectId);
        };
        const invalidate = (target: FakeObject, supersededBy: string | null): void => {
            const row = stage(target.object_id);
            row.invalidated_commit_seq = seq;
            row.superseded_by = supersededBy;
            touched.add(row.object_id);
            changed.add(row.object_id);
        };
        for (const operation of operations) {
            if (operation.op === "insert_decision") {
                const spec = operation.spec as Record<string, unknown>;
                // The registry's primary key refuses any held id, live or retired, this project's or another's. commentlint: allow(JUDGE)
                if (view(spec.object_id as string)) return invalid("already_exists");
                if (decisionIdHeld(spec)) return invalid("already_exists");
                insert(spec, (spec.sensitivity as Sensitivity | undefined) ?? "normal");
            } else if (operation.op === "supersede_decision") {
                const replaced = liveTarget(operation.replaced_object_id as string);
                if (!replaced) return invalid("not_found");
                const spec = operation.spec as Record<string, unknown>;
                const replacementId = spec.object_id as string;
                // A replacement id another project holds is `not_found` whether live or retired, so its state is not revealed. A live in-project replacement is a fold survivor: the spec is discarded, the survivor keeps its stored label and revision, and the predecessor is re-pointed at it. A retired in-project one is a duplicate insert. commentlint: allow(JUDGE)
                const replacement = view(replacementId);
                if (replacement && !FakeKernel.inProject(replacement, projectRoot)) {
                    return invalid("not_found");
                }
                const survivor =
                    replacement && replacement.invalidated_commit_seq === null ? replacement : null;
                if (
                    survivor &&
                    restrictive(survivor.sensitivity, replaced.sensitivity) !== survivor.sensitivity
                ) {
                    return invalid("admission_policy");
                }
                const successor = survivor ?? {
                    domain_id: spec.domain_id as string,
                    source_kind: sourceKind,
                    source_id: spec.source_id as string,
                    source_revision: spec.source_revision as number,
                };
                if (successor.source_revision <= replaced.source_revision) {
                    return invalid("revision_not_advanced");
                }
                if (
                    successor.domain_id !== replaced.domain_id ||
                    successor.source_kind !== replaced.source_kind ||
                    successor.source_id !== replaced.source_id
                ) {
                    return invalid("invalid_input");
                }
                if (replacement && !survivor) return invalid("already_exists");
                if (!survivor && decisionIdHeld(spec)) {
                    return invalid("already_exists");
                }
                if (survivor) {
                    merged.add(survivor.object_id);
                    touched.add(survivor.object_id);
                    changed.add(survivor.object_id);
                } else {
                    // A non-fold successor may raise its predecessor's label but not lower it.
                    insert(
                        spec,
                        restrictive(
                            (spec.sensitivity as Sensitivity | undefined) ?? "normal",
                            replaced.sensitivity,
                        ),
                    );
                }
                invalidate(replaced, replacementId);
            } else if (operation.op === "retire_decision") {
                const retired = liveTarget(operation.object_id as string);
                if (!retired) return invalid("not_found");
                invalidate(retired, null);
            } else if (operation.op === "disposition") {
                const judged = this.judgeDisposition(operation, projectRoot, view);
                if ("reply" in judged) return judged.reply;
                const row = stage(judged.result.object_id);
                row.disposition = judged.result.disposition as FakeDisposition;
                // An admission event is part of the receipt but does not advance the object's token. commentlint: allow(JUDGE)
                touched.add(row.object_id);
                dispositions.push(judged.result);
            } else {
                return invalid("invalid_input");
            }
        }
        this.tip = seq;
        for (const [objectId, row] of staged) {
            const existing = this.objects.get(objectId);
            if (existing) {
                Object.assign(existing, row);
            } else {
                this.objects.set(objectId, row);
            }
        }
        for (const objectId of changed) this.lastChange.set(objectId, seq);
        for (const decisionId of stagedDecisionIds) this.decisionIds.add(decisionId);
        const receipt: Receipt = {
            commit_seq: seq,
            request_digest: intent.request_digest,
            tokens: [...touched].sort().map((object_id) => ({ object_id, known_as_of: seq })),
            merged: [...merged].sort(),
            dispositions,
        };
        this.receipts.set(intent.operation_key, receipt);
        return {
            state: { kind: "available" },
            receipt: { commit_seq: seq, replayed: false },
            known_as_of: seq,
            tokens: receipt.tokens,
            merged: receipt.merged,
            ...(dispositions.length === 0 ? {} : { dispositions }),
        };
    }

    /**
     * The daemon's `disposition_request` and the kernel's ratchet: the target
     * must be a live in-project decision, a cited approval a live in-project
     * object, and a move toward a less restrictive disposition without one is
     * recorded as denied with the disposition unchanged.
     */
    private judgeDisposition(
        operation: Operation,
        projectRoot: string | null,
        view: (objectId: string) => FakeObject | undefined,
    ): { result: DispositionResult } | { reply: unknown } {
        const objectId = operation.object_id;
        const event = operation.event;
        if (
            typeof objectId !== "string" ||
            !(DISPOSITION_EVENTS as readonly unknown[]).includes(event)
        ) {
            throw invalidParams("kernel.commit disposition is malformed");
        }
        const target = view(objectId);
        if (
            !target ||
            !FakeKernel.inProject(target, projectRoot) ||
            target.invalidated_commit_seq !== null ||
            target.object_kind !== "decision"
        ) {
            return { reply: invalid("not_found") };
        }
        const approval = operation.approval_object_id;
        let approved = false;
        if (approval !== undefined) {
            if (typeof approval !== "string") {
                throw invalidParams("kernel.commit disposition approval is malformed");
            }
            const object = view(approval);
            if (
                !object ||
                !FakeKernel.inProject(object, projectRoot) ||
                object.invalidated_commit_seq !== null
            ) {
                return { reply: invalid("not_found") };
            }
            approved = this.approvals.has(approval);
        }
        const effect = EVENT_EFFECT[event as DispositionEvent];
        const relaxes = DISPOSITION_RANK[effect.disposition] < DISPOSITION_RANK[target.disposition];
        const denied = relaxes && !approved;
        return {
            result: {
                object_id: objectId,
                event: event as DispositionEvent,
                outcome: effect.outcome,
                previous_disposition: target.disposition,
                disposition: denied ? target.disposition : effect.disposition,
                denied,
            },
        };
    }

    /** `kernel.commit` with `preview: true`: a recorded identity answers its receipt; otherwise operations are judged in order on an overlay of the tip, nothing is written, and no receipt is created. commentlint: allow(JUDGE) */
    private previewReply(body: Record<string, unknown>, projectRoot: string | null): unknown {
        const tokens = (body.tokens as unknown[] | undefined) ?? [];
        if (tokens.length > 0)
            throw invalidParams("kernel.commit preview checks no tokens; send none");
        const operations = (body.operations as Operation[] | undefined) ?? [];
        if (operations.some((operation) => operation.op !== "disposition")) {
            throw invalidParams("kernel.commit preview supports disposition operations only");
        }
        const preflight = this.commitPreflight(body);
        if ("reply" in preflight) return preflight.reply;
        if (preflight.replayed) {
            return {
                state: { kind: "available" },
                known_as_of: this.tip,
                receipt: { commit_seq: preflight.replayed.commit_seq, replayed: true },
                previews: [],
            };
        }
        const previews = [];
        const staged = new Map<string, FakeObject>();
        const view = (id: string): FakeObject | undefined => staged.get(id) ?? this.objects.get(id);
        for (const operation of operations) {
            const judged = this.judgeDisposition(operation, projectRoot, view);
            if ("reply" in judged) return judged.reply;
            const target = view(judged.result.object_id) as FakeObject;
            const current = surfaceVisibilities(
                visibilityRow(target.labeled, target.disposition),
                target.sensitivity,
            );
            const projected = surfaceVisibilities(
                visibilityRow(target.labeled, judged.result.disposition as FakeDisposition),
                target.sensitivity,
            );
            const visibility_changes = SURFACES.some(
                (surface) =>
                    current[surface] !== "hidden" && current[surface] !== projected[surface],
            );
            staged.set(target.object_id, {
                ...target,
                disposition: judged.result.disposition as FakeDisposition,
            });
            previews.push({ ...judged.result, current, projected, visibility_changes });
        }
        return { state: { kind: "available" }, known_as_of: this.tip, previews };
    }

    reply(call: KernelTransportCall): unknown {
        const body = call.body as Record<string, unknown>;
        // The route is bound to the transport call's root; a body root that names another project is refused before any work. The daemon canonicalizes both roots first; the fake compares the strings. commentlint: allow(JUDGE)
        if (typeof body.project_root === "string" && body.project_root !== call.projectRoot) {
            return invalid("project_mismatch");
        }
        const projectRoot = call.projectRoot;
        switch (call.method) {
            case "kernel.read":
                return this.readReply(body, projectRoot);
            case "kernel.commit":
                return body.preview === true
                    ? this.previewReply(body, projectRoot)
                    : this.commitReply(body, projectRoot);
            default:
                return invalid("invalid_input");
        }
    }
}

/** A `KernelTransport` over a `FakeKernel` that records every call. */
export class FakeKernelTransport implements KernelTransport {
    readonly calls: KernelTransportCall[] = [];
    fileExists = true;
    rebinds = 0;
    /** Thrown by every call while set, standing in for a transport-level failure. */
    failWith: Error | null = null;

    constructor(readonly kernel: FakeKernel = new FakeKernel()) {}

    connectionFileExists(): boolean {
        return this.fileExists;
    }

    async call(args: KernelTransportCall): Promise<unknown> {
        this.calls.push(args);
        if (this.failWith) throw this.failWith;
        return this.kernel.reply(args);
    }

    async ensureRoute(): Promise<void> {
        this.rebinds += 1;
    }

    methods(): string[] {
        return this.calls.map((call) => call.method);
    }
}

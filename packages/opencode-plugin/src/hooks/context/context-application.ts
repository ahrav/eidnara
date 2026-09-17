import { BoundedSessionMap } from "../../shared/bounded-session-map";
import type { HostModuleTransport } from "./module-transport";

export type EditClass = "suppression" | "replacement" | "cross_step_reuse";
export type Outcome = "keep" | "append" | "applied_replacement" | "preparation_failure";

/** The two actions this client can carry; suppression and reuse need survivor proof it does not assemble. */
export type PackedAction = "append" | "replace";

export const TERMINALS = [
    "disabled",
    "capability_unsupported",
    "capability_undeclared",
    "receipt_unavailable",
    "stale_preparation",
    "conflict",
    "profile_unavailable",
    "profile_mismatch",
] as const;
export type Terminal = (typeof TERMINALS)[number];

/** `append` is not a gated class, so only `replace` has a class the daemon can deny. */
export function gatedClass(action: PackedAction): EditClass | undefined {
    return action === "replace" ? "replacement" : undefined;
}

function isEditClass(value: unknown): value is EditClass {
    return value === "suppression" || value === "replacement" || value === "cross_step_reuse";
}

function isTerminal(value: unknown): value is Terminal {
    return (TERMINALS as readonly unknown[]).includes(value);
}

export interface RouteKey {
    sessionId: string;
    projectRoot: string;
    routeEpoch: number;
}

/** `capability_unsupported` or `capability_undeclared` denies the class on one route until its epoch changes; `append` is never latched. An evicted denial costs one more daemon refusal before it latches again. */
export class CapabilityLatch {
    private readonly routes = new BoundedSessionMap<{ routeEpoch: number; denied: Set<EditClass> }>(
        1000,
    );

    chooseAction(intent: PackedAction, route: RouteKey): PackedAction {
        const cls = gatedClass(intent);
        return cls !== undefined && this.isDenied(cls, route) ? "append" : intent;
    }

    observeTerminal(terminal: string, cls: unknown, route: RouteKey): boolean {
        if (terminal !== "capability_unsupported" && terminal !== "capability_undeclared") {
            return false;
        }
        if (!isEditClass(cls)) return false;
        this.denied(route).add(cls);
        return true;
    }

    isDenied(cls: EditClass, route: RouteKey): boolean {
        return this.denied(route).has(cls);
    }

    private denied(route: RouteKey): Set<EditClass> {
        const key = `${route.sessionId}\u0000${route.projectRoot}`;
        const current = this.routes.get(key);
        if (current !== undefined && current.routeEpoch === route.routeEpoch) return current.denied;
        const fresh = { routeEpoch: route.routeEpoch, denied: new Set<EditClass>() };
        this.routes.set(key, fresh);
        return fresh.denied;
    }
}

export interface AccountingBinding {
    identity: string;
    revision: string;
}

export function isAccountingBinding(value: unknown): value is AccountingBinding {
    return (
        typeof value === "object" &&
        value !== null &&
        typeof (value as AccountingBinding).identity === "string" &&
        typeof (value as AccountingBinding).revision === "string" &&
        Object.keys(value).length === 2
    );
}

export const PACKED_ENTRY_ID_PREFIX = "eidnara-packed-";

export interface PackedEntry {
    info: { id: string; role: "user"; sessionID: string };
    parts: Array<{ type: "text"; text: string }>;
}

export function isPackedEntry(value: unknown): value is PackedEntry {
    if (typeof value !== "object" || value === null) return false;
    const info = (value as { info?: unknown }).info;
    return (
        typeof info === "object" &&
        info !== null &&
        typeof (info as { id?: unknown }).id === "string" &&
        (info as { id: string }).id.startsWith(PACKED_ENTRY_ID_PREFIX)
    );
}

export interface EntryEdit {
    entries: unknown[];
    outcome: Outcome;
}

/** One owned entry at most: `append` keeps an existing entry rather than adding a second; an empty `replace` leaves the slot absent and is still `applied_replacement`. */
export function editEntries(
    entries: readonly unknown[],
    action: PackedAction,
    sessionId: string,
    preparationId: string,
    body: string,
): EntryEdit {
    const owned = entries.some(isPackedEntry);
    if (action === "append") {
        if (body.length === 0 || owned) return { entries: [...entries], outcome: "keep" };
        return {
            entries: [...entries, packedEntry(sessionId, preparationId, body)],
            outcome: "append",
        };
    }
    const kept = entries.filter((entry) => !isPackedEntry(entry));
    if (body.length > 0) kept.push(packedEntry(sessionId, preparationId, body));
    return { entries: kept, outcome: "applied_replacement" };
}

function packedEntry(sessionId: string, preparationId: string, body: string): PackedEntry {
    return {
        info: {
            id: `${PACKED_ENTRY_ID_PREFIX}${preparationId}`,
            role: "user",
            sessionID: sessionId,
        },
        parts: [{ type: "text", text: body }],
    };
}

export interface WireContext {
    context_revision: string;
    representation: string;
    spans: Array<{ occurrence_id: string; buffer_len: number; span: [number, number] | null }>;
    selection: string[];
}

/** A refusal is answered before publication; the host surface is untouched. */
export type Refused = { kind: "refused"; terminal: Terminal; cls?: EditClass; reason?: string };

export type ApplicationResult =
    | { kind: "applied"; outcome: Outcome; preparationId: string; appliedIdentity: string }
    | {
          kind: "unknown";
          preparationId: string;
          forwardedIdentity?: string;
          appliedIdentity?: string;
          /** The daemon's answer to a confirm sent after publication; the host may hold the edit whatever the daemon says. */
          terminal?: Terminal;
      }
    | Refused
    | { kind: "failure"; reason: string };

export interface ApplicationTarget {
    route: RouteKey;
    context: WireContext;
    entries: () => readonly unknown[];
    body: string;
    /** Returns the identity the host observed, or `undefined` when the acknowledgment was lost; a rejection is read the same way. */
    publish: (edit: EntryEdit, forwardedIdentity: string) => Promise<string | undefined>;
    signal?: AbortSignal;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null;
}

type PrepareAnswer =
    | { kind: "prepared"; preparationId: string; profile: AccountingBinding }
    | { kind: "failure"; reason: string };

function parsePrepared(answer: unknown): PrepareAnswer {
    if (!isRecord(answer)) return { kind: "failure", reason: "malformed_prepare_answer" };
    if (answer.kind === "outcome") {
        return {
            kind: "failure",
            reason: typeof answer.reason === "string" ? answer.reason : "preparation_failure",
        };
    }
    if (
        answer.kind === "prepared" &&
        typeof answer.preparation_id === "string" &&
        isAccountingBinding(answer.accounting_profile)
    ) {
        return {
            kind: "prepared",
            preparationId: answer.preparation_id,
            profile: answer.accounting_profile,
        };
    }
    return { kind: "failure", reason: "malformed_prepare_answer" };
}

type ApplyAnswer =
    | { kind: "forwarded"; forwardedIdentity: string }
    | { kind: "receipt"; forwardedIdentity?: string }
    | { kind: "failure"; reason: string };

function parseApplied(answer: unknown): ApplyAnswer {
    if (!isRecord(answer)) return { kind: "failure", reason: "malformed_apply_answer" };
    if (answer.kind === "forwarded" && typeof answer.forwarded_identity === "string") {
        return { kind: "forwarded", forwardedIdentity: answer.forwarded_identity };
    }
    if (answer.kind === "receipt") {
        return {
            kind: "receipt",
            forwardedIdentity:
                typeof answer.forwarded_identity === "string"
                    ? answer.forwarded_identity
                    : undefined,
        };
    }
    return { kind: "failure", reason: "malformed_apply_answer" };
}

function confirmedComplete(answer: unknown): boolean {
    return isRecord(answer) && answer.kind === "receipt" && answer.state === "complete";
}

type RetrievalMethod = "retrieval.prepare" | "retrieval.apply" | "retrieval.confirm";

export class ContextApplication {
    constructor(
        private readonly transport: Pick<HostModuleTransport, "call">,
        private readonly latch: CapabilityLatch,
    ) {}

    async run(intent: PackedAction, target: ApplicationTarget): Promise<ApplicationResult> {
        let action = this.latch.chooseAction(intent, target.route);
        // Read once: the bytes the daemon authorizes are the bytes published.
        const body = target.body;
        for (;;) {
            const answer = await this.call(target, "retrieval.prepare", {
                ...target.context,
                action,
                edit_bytes: new TextEncoder().encode(body).length,
            });
            const refused = this.refusal(answer);
            if (refused === undefined) return this.applyPrepared(action, answer, target, body);
            const latched = this.latch.observeTerminal(refused.terminal, refused.cls, target.route);
            if (!latched || action === "append") return refused;
            action = "append";
        }
    }

    private async applyPrepared(
        action: PackedAction,
        answer: unknown,
        target: ApplicationTarget,
        body: string,
    ): Promise<ApplicationResult> {
        const prepared = parsePrepared(answer);
        if (prepared.kind === "failure") return prepared;
        const { preparationId, profile } = prepared;

        const applyAnswer = await this.call(target, "retrieval.apply", {
            ...target.context,
            preparation_id: preparationId,
            accounting_profile: profile,
        });
        const refusedApply = this.refusal(applyAnswer);
        if (refusedApply) {
            // The daemon gates the class again at apply; a denial there latches like one at prepare.
            this.latch.observeTerminal(refusedApply.terminal, refusedApply.cls, target.route);
            return refusedApply;
        }
        const applied = parseApplied(applyAnswer);
        if (applied.kind === "failure") return applied;
        if (applied.kind === "receipt") {
            return { kind: "unknown", preparationId, forwardedIdentity: applied.forwardedIdentity };
        }
        const { forwardedIdentity } = applied;

        const edit = editEntries(
            target.entries(),
            action,
            target.route.sessionId,
            preparationId,
            body,
        );
        let appliedIdentity: string | undefined;
        try {
            appliedIdentity = await target.publish(edit, forwardedIdentity);
        } catch {
            // The daemon has forwarded and the host may hold the edit: a rejected publication is a lost acknowledgment.
            appliedIdentity = undefined;
        }
        let confirmed: unknown;
        try {
            // The host may already hold the edit, so the confirm runs without the caller's abort signal and a transport failure is `unknown`, not an error.
            confirmed = await this.call({ ...target, signal: undefined }, "retrieval.confirm", {
                preparation_id: preparationId,
                forwarded_identity: forwardedIdentity,
                applied_identity: appliedIdentity ?? null,
                outcome: edit.outcome,
            });
        } catch {
            return { kind: "unknown", preparationId, forwardedIdentity, appliedIdentity };
        }
        const refusedConfirm = this.refusal(confirmed);
        if (refusedConfirm) {
            return {
                kind: "unknown",
                preparationId,
                forwardedIdentity,
                appliedIdentity,
                terminal: refusedConfirm.terminal,
            };
        }
        if (appliedIdentity !== undefined && confirmedComplete(confirmed)) {
            return { kind: "applied", outcome: edit.outcome, preparationId, appliedIdentity };
        }
        return { kind: "unknown", preparationId, forwardedIdentity, appliedIdentity };
    }

    private refusal(answer: unknown): Refused | undefined {
        if (!isRecord(answer) || answer.kind !== "terminal" || !isTerminal(answer.terminal)) {
            return undefined;
        }
        return {
            kind: "refused",
            terminal: answer.terminal,
            cls: isEditClass(answer.class) ? answer.class : undefined,
            reason: typeof answer.reason === "string" ? answer.reason : undefined,
        };
    }

    private call(
        target: ApplicationTarget,
        method: RetrievalMethod,
        fields: Record<string, unknown>,
    ): Promise<unknown> {
        return this.transport.call({
            sessionId: target.route.sessionId,
            projectRoot: target.route.projectRoot,
            method,
            body: {
                method,
                v: 1,
                session_id: target.route.sessionId,
                project_root: target.route.projectRoot,
                ...fields,
            },
            signal: target.signal,
        });
    }
}

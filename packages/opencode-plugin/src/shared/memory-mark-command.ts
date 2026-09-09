/**
 * OpenCode and Pi share the host-owned `/ctx-memory-mark` command. It names a
 * kernel `EventKind`, never a target disposition: the kernel's fixed table
 * decides the resulting state. The daemon previews the event first; the
 * harness asks for confirmation only when a serving surface would display a
 * different state after the event.
 */

import {
    DISPOSITION_EVENTS,
    type DispositionEvent,
    type DispositionOperation,
    type DispositionPreview,
    type DispositionResult,
    invalid,
    isAvailable,
    type KernelClient,
    type MemoryState,
    OPERATION_KEY_SEPARATOR,
    renderToolStateText,
    stateKey,
    unavailable,
} from "./kernel-client";

export const MEMORY_MARK_COMMAND = "ctx-memory-mark";

export const MEMORY_MARK_DESCRIPTION =
    "Mark a project memory stale, disputed, rejected, contradicted, or quarantined";

/** `--yes` skips the confirmation a visibility change would otherwise ask for. */
export const MEMORY_MARK_CONFIRM_FLAG = "--yes";

export const MEMORY_MARK_USAGE = `Usage: /${MEMORY_MARK_COMMAND} <${DISPOSITION_EVENTS.join("|")}> <object_id> [${MEMORY_MARK_CONFIRM_FLAG}]`;

export interface MemoryMarkArgs {
    event: DispositionEvent;
    objectId: string;
    /** Whether the user already answered the confirmation, through `--yes` or a dialog. */
    confirmed: boolean;
}

export type ParsedMemoryMarkArgs =
    | { ok: true; args: MemoryMarkArgs }
    | { ok: false; message: string };

function isDispositionEvent(value: string): value is DispositionEvent {
    return (DISPOSITION_EVENTS as readonly string[]).includes(value);
}

/** Accepts the wire event spelled with `_` or `-`; the flag may appear anywhere. */
export function parseMemoryMarkArgs(raw: string): ParsedMemoryMarkArgs {
    const words = raw
        .trim()
        .split(/\s+/)
        .filter((word) => word.length > 0);
    const confirmed = words.includes(MEMORY_MARK_CONFIRM_FLAG);
    const positional = words.filter((word) => word !== MEMORY_MARK_CONFIRM_FLAG);
    if (positional.length !== 2) return { ok: false, message: MEMORY_MARK_USAGE };
    const [rawEvent, objectId] = positional as [string, string];
    const event = rawEvent.replaceAll("-", "_");
    if (!isDispositionEvent(event)) {
        return { ok: false, message: `Unknown event "${rawEvent}". ${MEMORY_MARK_USAGE}` };
    }
    if (objectId.startsWith("-") || objectId.includes(OPERATION_KEY_SEPARATOR)) {
        return { ok: false, message: `Invalid object id "${objectId}". ${MEMORY_MARK_USAGE}` };
    }
    return { ok: true, args: { event, objectId, confirmed } };
}

export type MemoryMarkOutcome =
    /** The preview reported a visibility change and the user has not confirmed. */
    | { kind: "needs_confirmation"; preview: DispositionPreview }
    /** The user declined the confirmation; nothing was written. */
    | { kind: "declined"; preview: DispositionPreview }
    /** A relaxation without a valid approval, refused at the preview or recorded as denied by the commit; the disposition is unchanged. commentlint: allow(JUDGE) */
    | { kind: "denied"; result: DispositionResult }
    | { kind: "applied"; result: DispositionResult; commitSeq: number; replayed: boolean }
    /** The daemon answered the preview or the commit with a non-available state. */
    | { kind: "refused"; step: "preview" | "commit"; state: MemoryState };

export interface MemoryMarkInput {
    client: KernelClient;
    sessionId: string;
    /** Separator-free identity of the human command surface, such as `user:opencode`. */
    actor: string;
    args: MemoryMarkArgs;
    /**
     * Asked exactly once, when the preview reports a visibility change and
     * `args.confirmed` is false. A surface without a dialog leaves it out, and
     * the decision falls to the user's next invocation (`needs_confirmation`).
     */
    confirm?: (preview: DispositionPreview) => Promise<boolean>;
    /** Checked again before the commit, since the preview and any confirmation take time during which the session can end. */
    isCancelled?: () => boolean;
}

/** Repeating the same event on the same object in one session reuses the identity, so the daemon replays the receipt instead of recording twice. commentlint: allow(JUDGE) */
export function memoryMarkOperationId(sessionId: string, args: MemoryMarkArgs): string {
    return [sessionId, args.event, args.objectId].join(OPERATION_KEY_SEPARATOR);
}

/** The daemon returns one verdict for the single operation sent; only a verdict for `operation` may decide the confirmation and the reported outcome. commentlint: allow(JUDGE) */
function verdictFor<R extends DispositionResult>(
    results: readonly R[],
    operation: DispositionOperation,
): R | null {
    const [verdict] = results;
    if (results.length !== 1 || verdict === undefined) return null;
    if (verdict.object_id !== operation.object_id || verdict.event !== operation.event) return null;
    return verdict;
}

export async function runMemoryMarkCommand(input: MemoryMarkInput): Promise<MemoryMarkOutcome> {
    const { client, args } = input;
    const operation: DispositionOperation = {
        op: "disposition",
        object_id: args.objectId,
        event: args.event,
    };
    const intent = {
        actor: input.actor,
        operationId: memoryMarkOperationId(input.sessionId, args),
        cause: `/${MEMORY_MARK_COMMAND} ${args.event} ${args.objectId}`,
        sourceKind: "user" as const,
    };
    const previewed = await client.previewDispositions({ ...intent, operations: [operation] });
    if (!isAvailable(previewed))
        return { kind: "refused", step: "preview", state: previewed.state };
    // A recorded identity replays its receipt whatever the object's state is now, so the preview has nothing to judge and asks nothing. commentlint: allow(JUDGE)
    if (previewed.receipt === undefined) {
        const preview = verdictFor(previewed.previews, operation);
        // The reply decoded but does not answer the operation sent: a daemon contract violation, and nothing was written. commentlint: allow(JUDGE)
        if (!preview) return { kind: "refused", step: "preview", state: invalid("internal") };
        if (preview.denied) return { kind: "denied", result: preview };
        if (preview.visibility_changes && !args.confirmed) {
            if (!input.confirm) return { kind: "needs_confirmation", preview };
            if (!(await input.confirm(preview))) return { kind: "declined", preview };
        }
    }
    if (input.isCancelled?.())
        return { kind: "refused", step: "commit", state: { kind: "cancelled" } };
    const committed = await client.commit({ ...intent, operations: [operation] });
    if (!isAvailable(committed)) return { kind: "refused", step: "commit", state: committed.state };
    const result = verdictFor(committed.dispositions, operation);
    // The receipt exists, so the commit may have been applied; a verdict list that does not answer the operation leaves its effect unknown rather than refused. commentlint: allow(JUDGE)
    if (!result) return { kind: "refused", step: "commit", state: unavailable("outcome_unknown") };
    // The object can move between the preview and the commit; the commit's verdict is the one recorded. commentlint: allow(JUDGE)
    if (result.denied) return { kind: "denied", result };
    return {
        kind: "applied",
        result,
        commitSeq: committed.receipt.commit_seq,
        replayed: committed.receipt.replayed,
    };
}

const SURFACES = ["auto_inject", "auto_search", "explicit_search"] as const;

/** One line per surface whose verdict the event would change. */
export function formatVisibilityDelta(preview: DispositionPreview): string {
    return SURFACES.filter((surface) => preview.current[surface] !== preview.projected[surface])
        .map(
            (surface) =>
                `- ${surface}: ${preview.current[surface]} -> ${preview.projected[surface]}`,
        )
        .join("\n");
}

export function formatMemoryMarkConfirmation(preview: DispositionPreview): string {
    return [
        `${preview.event} on ${preview.object_id} changes what is served (${preview.previous_disposition} -> ${preview.disposition}):`,
        formatVisibilityDelta(preview),
    ].join("\n");
}

export function formatMemoryMarkOutcome(outcome: MemoryMarkOutcome, args: MemoryMarkArgs): string {
    const title = "## Eidnara Memory";
    switch (outcome.kind) {
        case "needs_confirmation":
            return [
                `${title} — Confirmation Needed`,
                "",
                formatMemoryMarkConfirmation(outcome.preview),
                "",
                `Nothing was written. Re-run with ${MEMORY_MARK_CONFIRM_FLAG} to apply: \`/${MEMORY_MARK_COMMAND} ${args.event} ${args.objectId} ${MEMORY_MARK_CONFIRM_FLAG}\``,
            ].join("\n");
        case "declined":
            return `${title} — Not Applied\n\n${args.event} on ${args.objectId} was declined; nothing was written.`;
        case "denied":
            return [
                `${title} — Denied`,
                "",
                `${args.event} on ${args.objectId} moves toward a less restrictive disposition and needs a valid approval; the disposition stays ${outcome.result.disposition}.`,
            ].join("\n");
        case "applied": {
            const receipt = outcome.replayed
                ? `replayed receipt #${outcome.commitSeq}`
                : `receipt #${outcome.commitSeq}`;
            return [
                `${title} — Applied`,
                "",
                `${outcome.result.event} on ${outcome.result.object_id}: outcome ${outcome.result.outcome}, disposition ${outcome.result.previous_disposition} -> ${outcome.result.disposition} (${receipt}).`,
            ].join("\n");
        }
        case "refused":
            return [
                `${title} — ${outcome.step === "preview" ? "Preview" : "Commit"} Refused`,
                "",
                `${renderToolStateText(outcome.state)} (${stateKey(outcome.state)})`,
            ].join("\n");
    }
}

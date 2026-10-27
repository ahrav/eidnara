/**
 * OpenCode and Pi share the host-owned `/eidnara-memory-mark` command. It names a
 * kernel `EventKind`, never a target disposition: the kernel's fixed table
 * decides the resulting state. The daemon previews the event first; the
 * harness asks for confirmation only when a serving surface would display a
 * different state after the event.
 *
 * A dispute (`mark_disputed` or `contradict`) cites evidence: `--evidence <id>`
 * names a live Kernel evidence id, sent byte for byte, and `--path <path>@<oid>`
 * names a repository file at a git object id, split at the last `@`. The daemon
 * validates both; the runner forwards them.
 */

import {
    DISPOSITION_EVENTS,
    type DispositionEvent,
    type DispositionOperation,
    type DispositionPreview,
    type DispositionResult,
    type EvidenceRef,
    invalid,
    isAvailable,
    type KernelClient,
    type MemoryState,
    OPERATION_KEY_SEPARATOR,
    renderToolStateText,
    stateKey,
    unavailable,
} from "./kernel-client";

export const MEMORY_MARK_COMMAND = "eidnara-memory-mark";

export const MEMORY_MARK_DESCRIPTION =
    "Mark a project memory stale, disputed, rejected, contradicted, or quarantined; a dispute or contradiction cites --evidence <id> or --path <path>@<oid>";

/** `--yes` skips the confirmation a visibility change would otherwise ask for. */
export const MEMORY_MARK_CONFIRM_FLAG = "--yes";

/** `--evidence <id>` cites a live Kernel evidence id. */
const MEMORY_MARK_EVIDENCE_FLAG = "--evidence";

/** `--path <path>@<oid>` cites a repository file at a git object id. */
const MEMORY_MARK_PATH_FLAG = "--path";

/** The events that must cite evidence. */
const DISPUTE_EVENTS: readonly DispositionEvent[] = ["mark_disputed", "contradict"];

export const MEMORY_MARK_USAGE = [
    `Usage: /${MEMORY_MARK_COMMAND} <${DISPOSITION_EVENTS.join("|")}> <object_id> [${MEMORY_MARK_EVIDENCE_FLAG} <id>] [${MEMORY_MARK_PATH_FLAG} <path>@<oid>] [${MEMORY_MARK_CONFIRM_FLAG}]`,
    `${DISPUTE_EVENTS.join(" and ")} cite evidence with ${MEMORY_MARK_EVIDENCE_FLAG}, ${MEMORY_MARK_PATH_FLAG}, or both; the other events take neither.`,
    `${MEMORY_MARK_EVIDENCE_FLAG} names a live evidence id of this project; ${MEMORY_MARK_PATH_FLAG} names a relative repository path without spaces and a 40- or 64-character lowercase hex git object id.`,
    "Refusals: invalid_input (missing or malformed evidence), not_found (unknown, retired, or foreign evidence or object), admission_policy, class_over_declared, operation_key_reused.",
    "The preview reports the named memory's visibility only; a dispute on an approval also demotes the memories that rest on it in the same commit.",
].join("\n");

export interface MemoryMarkArgs {
    event: DispositionEvent;
    objectId: string;
    /** A live Kernel evidence id, exactly as typed. */
    evidenceId?: string;
    evidenceRef?: EvidenceRef;
    /** Whether the user already answered the confirmation, through `--yes` or a dialog. */
    confirmed: boolean;
}

export type ParsedMemoryMarkArgs =
    | { ok: true; args: MemoryMarkArgs }
    | { ok: false; message: string };

function isDispositionEvent(value: string): value is DispositionEvent {
    return (DISPOSITION_EVENTS as readonly string[]).includes(value);
}

/** Splits `<path>@<oid>` at the last `@`; the oid is hex, so it holds none. */
function parseEvidenceRef(value: string): EvidenceRef | null {
    const at = value.lastIndexOf("@");
    if (at <= 0 || at === value.length - 1) return null;
    const path = value.slice(0, at);
    if (path.includes(OPERATION_KEY_SEPARATOR)) return null;
    return { path, git_object_id: value.slice(at + 1) };
}

/** Accepts the wire event spelled with `_` or `-`; the flags may appear anywhere, each at most once. */
export function parseMemoryMarkArgs(raw: string): ParsedMemoryMarkArgs {
    const words = raw
        .trim()
        .split(/\s+/)
        .filter((word) => word.length > 0);
    let confirmed = false;
    const valued = new Map<string, string>();
    const positional: string[] = [];
    for (let index = 0; index < words.length; index += 1) {
        const word = words[index] as string;
        if (word === MEMORY_MARK_CONFIRM_FLAG) {
            confirmed = true;
        } else if (word === MEMORY_MARK_EVIDENCE_FLAG || word === MEMORY_MARK_PATH_FLAG) {
            const value = words[index + 1];
            if (value === undefined || value.startsWith("--") || valued.has(word)) {
                return { ok: false, message: MEMORY_MARK_USAGE };
            }
            valued.set(word, value);
            index += 1;
        } else {
            positional.push(word);
        }
    }
    const evidenceId = valued.get(MEMORY_MARK_EVIDENCE_FLAG);
    const evidencePath = valued.get(MEMORY_MARK_PATH_FLAG);
    if (positional.length !== 2) return { ok: false, message: MEMORY_MARK_USAGE };
    const [rawEvent, objectId] = positional as [string, string];
    const event = rawEvent.replaceAll("-", "_");
    if (!isDispositionEvent(event)) {
        return { ok: false, message: `Unknown event "${rawEvent}". ${MEMORY_MARK_USAGE}` };
    }
    if (objectId.startsWith("-") || objectId.includes(OPERATION_KEY_SEPARATOR)) {
        return { ok: false, message: `Invalid object id "${objectId}". ${MEMORY_MARK_USAGE}` };
    }
    if (evidenceId?.includes(OPERATION_KEY_SEPARATOR)) {
        return { ok: false, message: `Invalid evidence id "${evidenceId}". ${MEMORY_MARK_USAGE}` };
    }
    const evidenceRef = evidencePath === undefined ? undefined : parseEvidenceRef(evidencePath);
    if (evidenceRef === null) {
        return {
            ok: false,
            message: `Invalid evidence path "${evidencePath}". ${MEMORY_MARK_USAGE}`,
        };
    }
    const cites = evidenceId !== undefined || evidenceRef !== undefined;
    if (cites !== DISPUTE_EVENTS.includes(event)) return { ok: false, message: MEMORY_MARK_USAGE };
    return {
        ok: true,
        args: {
            event,
            objectId,
            ...(evidenceId === undefined ? {} : { evidenceId }),
            ...(evidenceRef === undefined ? {} : { evidenceRef }),
            confirmed,
        },
    };
}

/** The evidence a dispute cites, as the command line spells it: `--evidence <id>` and `--path <path>@<oid>`, in that order. */
function evidenceFlags(args: MemoryMarkArgs): string[] {
    return [
        ...(args.evidenceId === undefined ? [] : [MEMORY_MARK_EVIDENCE_FLAG, args.evidenceId]),
        ...(args.evidenceRef === undefined
            ? []
            : [
                  MEMORY_MARK_PATH_FLAG,
                  `${args.evidenceRef.path}@${args.evidenceRef.git_object_id}`,
              ]),
    ];
}

/** The command line that names `args`, as a user would type it again. */
function commandLine(args: MemoryMarkArgs): string {
    return [`/${MEMORY_MARK_COMMAND}`, args.event, args.objectId, ...evidenceFlags(args)].join(" ");
}

export type MemoryMarkOutcome =
    /** The preview reported a visibility change and the user has not confirmed. */
    | { kind: "needs_confirmation"; preview: DispositionPreview }
    /** The user declined the confirmation; nothing was written. */
    | { kind: "declined"; preview: DispositionPreview }
    /** A relaxation without a valid approval, refused at the preview or recorded as denied by the commit; the disposition is unchanged. */
    | { kind: "denied"; result: DispositionResult }
    | { kind: "applied"; result: DispositionResult; commitSeq: number; replayed: boolean }
    /** The daemon answered the preview or the commit with a non-available state. */
    | { kind: "refused"; step: "preview" | "commit"; state: MemoryState };

export interface MemoryMarkInput {
    client: KernelClient;
    /** The operation's stable identity; the command derives it with `memoryMarkOperationId`. */
    operationId: string;
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

/** Repeating the same event on the same object with the same evidence in one session reuses the identity, so the daemon replays the receipt instead of recording twice; other evidence is another dispute. */
export function memoryMarkOperationId(sessionId: string, args: MemoryMarkArgs): string {
    return [sessionId, args.event, args.objectId, ...evidenceFlags(args)].join(
        OPERATION_KEY_SEPARATOR,
    );
}

/** The daemon returns one verdict for the single operation sent; only a verdict for `operation` may decide the confirmation and the reported outcome. */
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
        ...(args.evidenceId === undefined ? {} : { evidence_id: args.evidenceId }),
        // A copy, so the commit sends the reference the preview judged even if the caller edits its args during the confirmation wait.
        ...(args.evidenceRef === undefined ? {} : { evidence_ref: { ...args.evidenceRef } }),
    };
    const intent = {
        actor: input.actor,
        operationId: input.operationId,
        cause: commandLine(args),
        sourceKind: "user" as const,
    };
    const previewed = await client.previewDispositions({ ...intent, operations: [operation] });
    if (!isAvailable(previewed))
        return { kind: "refused", step: "preview", state: previewed.state };
    // A recorded identity replays its receipt whatever the object's state is now, so the preview has nothing to judge and asks nothing.
    if (previewed.receipt === undefined) {
        const preview = verdictFor(previewed.previews, operation);
        // The reply decoded but does not answer the operation sent: a daemon contract violation, and nothing was written.
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
    // The receipt exists, so the commit may have been applied; a verdict list that does not answer the operation leaves its effect unknown rather than refused.
    if (!result) return { kind: "refused", step: "commit", state: unavailable("outcome_unknown") };
    // The object can move between the preview and the commit; the commit's verdict is the one recorded.
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
            // The re-run line comes first: OpenCode shows this reply as a toast that keeps only its opening characters, and the surface list below can run past that cut.
            return [
                `${title} — Confirmation Needed`,
                "",
                `Nothing was written. Re-run with ${MEMORY_MARK_CONFIRM_FLAG} to apply: \`${commandLine(args)} ${MEMORY_MARK_CONFIRM_FLAG}\``,
                "",
                formatMemoryMarkConfirmation(outcome.preview),
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

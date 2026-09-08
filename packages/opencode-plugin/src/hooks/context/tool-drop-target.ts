import { isRecord } from "../../shared/record-type-guard";
import { markPartMutated } from "./read-session-true-raw-tokens";
import type { MessageLike, ThinkingLikePart } from "./tag-content-primitives";
import { stripTagPrefix } from "./tag-content-primitives";

export type ToolDropResult = "removed" | "truncated" | "absent" | "incomplete";

interface ToolCallObservation {
    callId: string;
    kind: "invocation" | "result";
}

export interface IndexedOccurrence {
    message: MessageLike;
    part: unknown;
    kind: "invocation" | "result";
}

export interface ToolCallIndexEntry {
    occurrences: IndexedOccurrence[];
    hasResult: boolean;
}

export type ToolCallIndex = Map<string, ToolCallIndexEntry>;

/** `setContent` treats only this exact marker as a drop request. */
const DROP_SENTINEL = /^\[dropped \u00a7\d+\u00a7\]$/;
/** Part types omitted from OpenCode message content. */
const IGNORE_PART_TYPES = new Set([
    "thinking",
    "reasoning",
    "redacted_thinking",
    "step-start",
    "step-finish",
    "snapshot",
    "patch",
    "agent",
    "retry",
]);

function isToolCallId(value: unknown): value is string {
    return typeof value === "string" && value.length > 0;
}

interface FieldRef {
    owner: Record<string, unknown>;
    key: string;
}

interface ToolPartFields {
    state: Record<string, unknown> | null;
    status: string | undefined;
    result: FieldRef;
    staleResults: FieldRef[];
    input: FieldRef | null;
}

const RESULT_KEYS = ["output", "error"] as const;

function firstPresent(refs: FieldRef[]): FieldRef | undefined {
    return refs.find((ref) => ref.key in ref.owner);
}

function toolPartFields(part: Record<string, unknown>): ToolPartFields {
    const state = isRecord(part.state) ? part.state : null;
    const nestedStatus = state?.status;
    const statusValue = typeof nestedStatus === "string" ? nestedStatus : part.status;
    const status = typeof statusValue === "string" ? statusValue : undefined;

    const resultCandidates: FieldRef[] = [];
    if (state !== null) for (const key of RESULT_KEYS) resultCandidates.push({ owner: state, key });
    for (const key of RESULT_KEYS) resultCandidates.push({ owner: part, key });
    const presentResults = resultCandidates.filter((ref) => ref.key in ref.owner);
    const result = presentResults[0] ?? { owner: state ?? part, key: "output" };

    const inputCandidates: FieldRef[] = [];
    if (state !== null) inputCandidates.push({ owner: state, key: "input" });
    inputCandidates.push({ owner: part, key: "input" }, { owner: part, key: "args" });

    return {
        state,
        status,
        result,
        staleResults: presentResults.slice(1),
        input: firstPresent(inputCandidates) ?? null,
    };
}

function clearToolAttachments(part: Record<string, unknown>, fields: ToolPartFields): boolean {
    const had = (fields.state !== null && "attachments" in fields.state) || "attachments" in part;
    if (fields.state !== null) delete fields.state.attachments;
    delete part.attachments;
    return had;
}

const TOOL_RESULT_PAYLOAD_KEYS = ["content", "output", "result"] as const;

function toolResultPayload(part: Record<string, unknown>): {
    ref: FieldRef;
    stale: FieldRef[];
} {
    const present = TOOL_RESULT_PAYLOAD_KEYS.filter((key) => key in part).map((key) => ({
        owner: part,
        key,
    }));
    return { ref: present[0] ?? { owner: part, key: "content" }, stale: present.slice(1) };
}

function readRef(ref: FieldRef): unknown {
    return ref.owner[ref.key];
}

function writeRef(ref: FieldRef, stale: FieldRef[], value: unknown): void {
    ref.owner[ref.key] = value;
    for (const s of stale) delete s.owner[s.key];
}

function setToolContent(part: unknown, content: string): boolean {
    if (!isRecord(part)) return false;
    let changed = false;
    if (part.type === "tool") {
        const fields = toolPartFields(part);
        const textChanged = readRef(fields.result) !== content || fields.staleResults.length > 0;
        const attachmentsCleared = clearToolAttachments(part, fields);
        writeRef(fields.result, fields.staleResults, content);
        changed = textChanged || attachmentsCleared;
    } else if (part.type === "tool_result") {
        const payload = toolResultPayload(part);
        changed = readRef(payload.ref) !== content || payload.stale.length > 0;
        writeRef(payload.ref, payload.stale, content);
    }
    if (changed) markPartMutated(part);
    return changed;
}

/**
 * When cloning succeeds, replacing the array entry isolates clamp mutations from the original part.
 */
function clonePart(part: unknown): unknown {
    if (part === null || typeof part !== "object") return part;
    try {
        return structuredClone(part);
    } catch {
        try {
            return JSON.parse(JSON.stringify(part));
        } catch {
            // If both cloning strategies fail, the subsequent clamp mutates the original part.
            return part;
        }
    }
}

/**
 */
function clampCloneInPlace(occurrence: IndexedOccurrence, clamp: (part: unknown) => void): void {
    const clone = clonePart(occurrence.part);
    clamp(clone);
    // `structuredClone` keeps enumerable harness versions, so the clamped clone needs its own stamp.
    if (clone !== null && typeof clone === "object") markPartMutated(clone);
    const parts = occurrence.message.parts;
    const index = parts.indexOf(occurrence.part);
    if (index >= 0) {
        parts[index] = clone;
        occurrence.part = clone;
    }
}

/** An object input is clamped per value; an array input collapses to `[N items]`; scalars pass through. */
function clampInput(ref: FieldRef): void {
    const value = readRef(ref);
    if (estimateInputSize(value) <= INPUT_CLAMP_BYTES) return;
    if (Array.isArray(value)) ref.owner[ref.key] = `[${value.length} items]`;
    else if (isRecord(value)) truncateInputValues(value);
}

function inputRefOf(part: Record<string, unknown>): FieldRef | null {
    if (part.type === "tool") return toolPartFields(part).input;
    if (part.type === "tool-invocation")
        return firstPresent([{ owner: part, key: "args" }]) ?? null;
    if (part.type === "tool_use") return firstPresent([{ owner: part, key: "input" }]) ?? null;
    return null;
}

function truncateToolPart(part: unknown, tagId: number): void {
    if (!isRecord(part)) return;

    const sentinel = `[dropped \u00a7${tagId}\u00a7]`;

    if (part.type === "tool") {
        const fields = toolPartFields(part);
        writeRef(fields.result, fields.staleResults, sentinel);
        clearToolAttachments(part, fields);
        if (fields.input !== null) clampInput(fields.input);
        return;
    }

    if (part.type === "tool_result") {
        const payload = toolResultPayload(part);
        writeRef(payload.ref, payload.stale, sentinel);
        return;
    }

    const input = inputRefOf(part);
    if (input !== null) clampInput(input);
}

/** Maximum JSON-serialized input size before truncation. */
const INPUT_CLAMP_BYTES = 500;

function estimateInputSize(input: unknown): number {
    try {
        return Buffer.byteLength(JSON.stringify(input) ?? "", "utf8");
    } catch {
        return 0;
    }
}

function readToolPartInput(part: unknown): Record<string, unknown> | null {
    if (!isRecord(part)) return null;
    const ref = inputRefOf(part);
    if (ref === null) return null;
    const value = readRef(ref);
    return isRecord(value) ? value : null;
}

const TRUNCATION_SENTINEL = "...[truncated]";
const SKELETON_ARG_LEN = 5;

// Iterating by Unicode scalar preserves surrogate pairs and matches the daemon's `chars().count()` clamp measure.
function scalarPrefix(str: string, count: number): string | null {
    let prefix = "";
    let seen = 0;
    for (const scalar of str) {
        if (seen === count) return prefix;
        prefix += scalar;
        seen += 1;
    }
    return null;
}

function isClampedArg(value: string): boolean {
    if (!value.endsWith(TRUNCATION_SENTINEL)) return false;
    const head = value.slice(0, value.length - TRUNCATION_SENTINEL.length);
    // The clamp emits exactly `SKELETON_ARG_LEN` scalars before the sentinel.
    return (
        scalarPrefix(head, SKELETON_ARG_LEN) === null &&
        scalarPrefix(head, SKELETON_ARG_LEN - 1) !== null
    );
}

function truncateInputValues(input: Record<string, unknown>): void {
    for (const key of Object.keys(input)) {
        const value = input[key];
        if (typeof value === "string") {
            if (isClampedArg(value) || value === "[object]" || /^\[\d+ items\]$/.test(value)) {
                continue;
            }
            const prefix = scalarPrefix(value, SKELETON_ARG_LEN);
            if (prefix !== null) input[key] = `${prefix}${TRUNCATION_SENTINEL}`;
        } else if (Array.isArray(value)) {
            input[key] = `[${value.length} items]`;
        } else if (value !== null && typeof value === "object") {
            input[key] = "[object]";
        }
    }
}

export function hasMeaningfulPart(part: unknown): boolean {
    if (!isRecord(part)) return false;
    const type = part.type;
    if (type === "text") {
        if (typeof part.text !== "string") return false;
        // `crates/daemon/src/codec/opencode.rs` skips text parts with `ignored: true` when decoding.
        if (part.ignored === true) return false;
        return stripTagPrefix(part.text).trim().length > 0;
    }
    if (typeof type !== "string") return false;
    if (IGNORE_PART_TYPES.has(type)) return false;
    if (type === "meta") return Object.keys(part).length > 1;
    return true;
}

function containsSignature(value: unknown): boolean {
    if (Array.isArray(value)) return value.some(containsSignature);
    if (!isRecord(value)) return false;
    if (typeof value.signature === "string") return true;
    return Object.values(value).some(containsSignature);
}

function isProtectedReasoning(part: ThinkingLikePart): boolean {
    const record: unknown = part;
    if (!isRecord(record)) return false;
    if (typeof record.signature === "string" || containsSignature(record.metadata)) return true;
    if (typeof record.data === "string" || typeof record.redacted === "string") return true;
    return isRecord(record.metadata) && typeof record.metadata.redacted === "string";
}

function clearThinkingParts(thinkingParts: ThinkingLikePart[]): void {
    for (const part of thinkingParts) {
        if (isProtectedReasoning(part)) continue;
        let changed = false;
        if (part.thinking !== undefined && part.thinking !== "[cleared]") {
            part.thinking = "[cleared]";
            changed = true;
        }
        if (part.text !== undefined && part.text !== "[cleared]") {
            part.text = "[cleared]";
            changed = true;
        }
        if (changed) markPartMutated(part);
    }
}

export function partHasCompletedResult(part: unknown): boolean {
    if (!isRecord(part)) return false;
    if (part.type === "tool") {
        const fields = toolPartFields(part);
        // A `running` tool may already hold partial streamed output.
        if (fields.status !== undefined) {
            return fields.status === "completed" || fields.status === "error";
        }
        return fields.result.key === "output" && typeof readRef(fields.result) === "string";
    }
    return part.type === "tool_result";
}

/** Call-id aliases in the order the codec resolves them for an OpenCode `tool` part. */
const OPENCODE_TOOL_CALL_ID_KEYS = ["callID", "callId", "id"] as const;

function openCodeToolCallId(part: Record<string, unknown>): string | null {
    for (const key of OPENCODE_TOOL_CALL_ID_KEYS) {
        const value = part[key];
        if (isToolCallId(value)) return value;
    }
    return null;
}

export function extractToolCallObservation(part: unknown): ToolCallObservation | null {
    if (!isRecord(part)) return null;
    if (part.type === "tool") {
        const callId = openCodeToolCallId(part);
        return callId === null ? null : { callId, kind: "result" };
    }
    if (part.type === "tool-invocation" && isToolCallId(part.callID)) {
        return { callId: part.callID, kind: "invocation" };
    }
    if (part.type === "tool_use" && isToolCallId(part.id)) {
        return { callId: part.id, kind: "invocation" };
    }
    if (part.type === "tool_result" && isToolCallId(part.tool_use_id)) {
        return { callId: part.tool_use_id, kind: "result" };
    }
    return null;
}

function isDropContent(content: string): boolean {
    return DROP_SENTINEL.test(content);
}

export class ToolMutationBatch {
    private partsToRemove = new Set<unknown>();
    private affectedMessages = new Set<MessageLike>();
    private messages: MessageLike[];

    constructor(messages: MessageLike[]) {
        this.messages = messages;
    }

    markForRemoval(occurrence: IndexedOccurrence): void {
        this.partsToRemove.add(occurrence.part);
        this.affectedMessages.add(occurrence.message);
    }

    finalize(): void {
        if (this.partsToRemove.size === 0) return;

        for (const message of this.affectedMessages) {
            message.parts = message.parts.filter((p) => !this.partsToRemove.has(p));
        }

        // Only a message this batch emptied is pruned; a message that arrived partless is left in place.
        for (let i = this.messages.length - 1; i >= 0; i -= 1) {
            const message = this.messages[i];
            if (this.affectedMessages.has(message) && !message.parts.some(hasMeaningfulPart)) {
                this.messages.splice(i, 1);
            }
        }

        this.partsToRemove.clear();
        this.affectedMessages.clear();
    }
}

/**
 * (`<ownerMsgId>\x00<callId>`).
 *
 *
 */
export function createToolDropTarget(
    compositeKey: string,
    thinkingParts: ThinkingLikePart[],
    index: ToolCallIndex,
    batch: ToolMutationBatch,
    tagId: number,
): {
    setContent: (content: string) => boolean;
    drop: () => ToolDropResult;
    truncate: () => ToolDropResult;
    /**
     */
    canDrop: () => boolean;
    readInput: () => Record<string, unknown> | null;
} {
    const drop = (): ToolDropResult => {
        const entry = index.get(compositeKey);
        if (!entry || entry.occurrences.length === 0) return "absent";
        if (!entry.hasResult) return "incomplete";

        for (const occurrence of entry.occurrences) {
            batch.markForRemoval(occurrence);
        }
        clearThinkingParts(thinkingParts);
        index.delete(compositeKey);
        return "removed";
    };

    const truncate = (): ToolDropResult => {
        const entry = index.get(compositeKey);
        if (!entry || entry.occurrences.length === 0) return "absent";
        if (!entry.hasResult) return "incomplete";

        for (const occurrence of entry.occurrences) {
            clampCloneInPlace(occurrence, (part) => truncateToolPart(part, tagId));
        }
        clearThinkingParts(thinkingParts);
        return "truncated";
    };

    return {
        setContent: (content: string): boolean => {
            if (isDropContent(content)) {
                return drop() === "removed";
            }

            const entry = index.get(compositeKey);
            // A pending or running tool has no result to replace; writing `output` would mark it completed.
            if (!entry?.hasResult) return false;

            let changed = false;
            for (const occurrence of entry.occurrences) {
                if (occurrence.kind !== "result") continue;
                if (setToolContent(occurrence.part, content)) changed = true;
            }
            return changed;
        },
        drop,
        truncate,
        canDrop: (): boolean => {
            const entry = index.get(compositeKey);
            return !!entry && entry.occurrences.length > 0 && entry.hasResult;
        },
        readInput: (): Record<string, unknown> | null => {
            const entry = index.get(compositeKey);
            if (!entry) return null;
            for (const occurrence of entry.occurrences) {
                if (occurrence.kind !== "invocation") continue;
                const input = readToolPartInput(occurrence.part);
                if (input) return input;
            }
            for (const occurrence of entry.occurrences) {
                const input = readToolPartInput(occurrence.part);
                if (input) return input;
            }
            return null;
        },
    };
}

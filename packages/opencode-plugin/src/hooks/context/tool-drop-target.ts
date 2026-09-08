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
    "meta",
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

interface ToolPartFields {
    state: Record<string, unknown> | null;
    status: string | undefined;
    outputOwner: Record<string, unknown>;
    errorOwner: Record<string, unknown>;
    input: Record<string, unknown> | null;
}

/** Preserves nested versus top-level field placement when rewriting tool results. */
function toolPartFields(part: Record<string, unknown>): ToolPartFields {
    const state = isRecord(part.state) ? part.state : null;
    const owner = (key: string): Record<string, unknown> => {
        if (state !== null && key in state) return state;
        if (key in part) return part;
        return state ?? part;
    };
    const statusOwner = owner("status");
    const status = typeof statusOwner.status === "string" ? statusOwner.status : undefined;
    let input: Record<string, unknown> | null = null;
    if (state !== null && isRecord(state.input)) input = state.input;
    else if (isRecord(part.input)) input = part.input;
    else if (isRecord(part.args)) input = part.args;
    return { state, status, outputOwner: owner("output"), errorOwner: owner("error"), input };
}

/** A failed OpenCode tool carries its result in `error`; the wire serializes that field. */
function isErrorResult(fields: ToolPartFields): boolean {
    return fields.status === "error" && typeof fields.errorOwner.error === "string";
}

function clearToolAttachments(part: Record<string, unknown>, fields: ToolPartFields): boolean {
    const had = (fields.state !== null && "attachments" in fields.state) || "attachments" in part;
    if (fields.state !== null) delete fields.state.attachments;
    delete part.attachments;
    return had;
}

function getToolContent(part: unknown): string | undefined {
    if (!isRecord(part)) return undefined;
    if (part.type === "tool") {
        const fields = toolPartFields(part);
        const output = fields.outputOwner.output;
        if (typeof output === "string") return output;
        return isErrorResult(fields) ? (fields.errorOwner.error as string) : undefined;
    }
    if (part.type === "tool_result") {
        return typeof part.content === "string" ? part.content : undefined;
    }
    return undefined;
}

function writeToolResult(fields: ToolPartFields, content: string): void {
    fields.outputOwner.output = content;
    if (isErrorResult(fields)) fields.errorOwner.error = content;
}

function setToolContent(part: unknown, content: string): boolean {
    if (!isRecord(part)) return false;
    let changed = false;
    if (part.type === "tool") {
        const fields = toolPartFields(part);
        const textChanged = getToolContent(part) !== content;
        const attachmentsCleared = clearToolAttachments(part, fields);
        writeToolResult(fields, content);
        changed = textChanged || attachmentsCleared;
    } else if (part.type === "tool_result") {
        changed = part.content !== content;
        part.content = content;
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
    const parts = occurrence.message.parts;
    const index = parts.indexOf(occurrence.part);
    if (index >= 0) {
        parts[index] = clone;
        occurrence.part = clone;
    }
}

function clampInput(input: Record<string, unknown>): void {
    if (estimateInputSize(input) > INPUT_CLAMP_BYTES) truncateInputValues(input);
}

function truncateToolPart(part: unknown, tagId: number): void {
    if (!isRecord(part)) return;

    const sentinel = `[dropped \u00a7${tagId}\u00a7]`;

    if (part.type === "tool") {
        const fields = toolPartFields(part);
        writeToolResult(fields, sentinel);
        clearToolAttachments(part, fields);
        if (fields.input !== null) clampInput(fields.input);
        return;
    }

    if (part.type === "tool_result") {
        part.content = sentinel;
        return;
    }

    const input = readToolPartInput(part);
    if (input !== null) clampInput(input);
}

/** Maximum JSON-serialized input size before truncation. */
const INPUT_CLAMP_BYTES = 500;

function estimateInputSize(input: Record<string, unknown>): number {
    try {
        return Buffer.byteLength(JSON.stringify(input), "utf8");
    } catch {
        return 0;
    }
}

function readToolPartInput(part: unknown): Record<string, unknown> | null {
    if (!isRecord(part)) return null;
    if (part.type === "tool") return toolPartFields(part).input;
    if (part.type === "tool-invocation" && isRecord(part.args)) return part.args;
    if (part.type === "tool_use" && isRecord(part.input)) return part.input;
    return null;
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
        if (fields.state === null && fields.status === undefined) return false;
        return (
            fields.status === "completed" ||
            fields.status === "error" ||
            typeof fields.outputOwner.output === "string"
        );
    }
    return part.type === "tool_result";
}

export function extractToolCallObservation(part: unknown): ToolCallObservation | null {
    if (!isRecord(part)) return null;
    if (part.type === "tool" && isToolCallId(part.callID)) {
        return { callId: part.callID, kind: "result" };
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

/**
 * Pi message conversion synthesizes OpenCode-compatible `parts` so shared formatting, chunking, and trigger logic can consume Pi messages unchanged.
 *
 * Mapping:
 * Each user and assistant message maps to one `RawMessage` with OpenCode-compatible `parts`.
 * The mapping folds each `ToolResultMessage` into the immediately following user `RawMessage` so its `callID` pairs with the assistant's `tool_use` part.
 * A run of tool results before an assistant message, or trailing the branch, produces a synthetic user `RawMessage`.
 *
 * Ordinals:
 * Branch-order ordinals start at 1; append-only active-branch entries keep the mapping stable for the session.
 *
 * The conversion skips every `getBranch()` entry except `SessionMessageEntry` because only that type provides `parts`.
 */

import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { RawMessage } from "@eidnara/opencode/hooks/context/read-session-raw";

/**
 * `SYNTH_USER_ID_PREFIX` prefixes synthetic-user `RawMessage` ids created from `toolResult` runs.
 * `id` is `${SYNTH_USER_ID_PREFIX}${firstRealToolResultEntryId}`, not a real `SessionEntry.id`.
 * Pi `getBranch()` and compaction replay match real `entry.id` values only.
 * Consumers that require replay-safe entry IDs must strip `SYNTH_USER_ID_PREFIX` to recover the underlying `toolResult` entry ID.
 */
const SYNTH_USER_ID_PREFIX = "synth-user-";

/**
 * Returns an OpenCode-shape `RawMessage[]` synthesized from the active Pi session branch.
 */
export function readPiSessionMessages(ctx: ExtensionContext): RawMessage[] {
    const sm = ctx.sessionManager;
    if (sm === undefined) return [];
    const getBranch = (sm as { getBranch?: (fromId?: string) => unknown[] }).getBranch;
    if (typeof getBranch !== "function") return [];

    let entries: unknown[];
    try {
        entries = getBranch.call(sm);
    } catch {
        return [];
    }
    if (!Array.isArray(entries)) return [];

    return convertEntriesToRawMessages(entries);
}

function rawEntryVersion(entry: MessageEntry): string | number {
    const record = entry as unknown as Record<string, unknown>;
    const updated = record.updatedAt ?? record.updated_at ?? record.timestamp;
    return typeof updated === "string" || typeof updated === "number" ? updated : entry.id;
}

function attachPiPartVersion(parts: unknown[], version: string | number): unknown[] {
    return parts.map((part) => {
        if (part === null || typeof part !== "object" || Array.isArray(part)) return part;
        try {
            Object.defineProperty(part, "__eidnaraPartUpdatedAt", {
                value: version,
                enumerable: false,
                configurable: true,
            });
        } catch {}
        return part;
    });
}

export function convertEntriesToRawMessages(entries: unknown[]): RawMessage[] {
    const result: RawMessage[] = [];
    let nextOrdinal = 1;

    // The pending buffer holds tool-result parts until the next user message.
    let pendingToolParts: unknown[] = [];
    // A synthetic user ID uses its first contributing `toolResult` entry ID for session lookup.
    let pendingFirstRealId = "";
    let pendingFirstRealVersion: string | number = "";

    for (const entry of entries) {
        if (!isMessageEntry(entry)) {
            continue;
        }

        const msg = entry.message;
        const role = (msg as { role?: string }).role;

        if (role === "toolResult") {
            const version = rawEntryVersion(entry);
            pendingToolParts.push(...attachPiPartVersion(synthesizeToolResultParts(msg), version));
            if (pendingFirstRealId === "") {
                pendingFirstRealId = entry.id;
                pendingFirstRealVersion = version;
            }
            continue;
        }

        if (role === "user") {
            // Tool-result parts precede user content in conversation order.
            const version = rawEntryVersion(entry);
            const parts: unknown[] = [
                ...pendingToolParts,
                ...attachPiPartVersion(synthesizeUserParts(msg), version),
            ];
            pendingToolParts = [];
            pendingFirstRealId = "";
            pendingFirstRealVersion = "";
            result.push({
                ordinal: nextOrdinal++,
                id: entry.id,
                role: "user",
                parts,
                version,
            });
            continue;
        }

        if (role === "assistant") {
            if (pendingToolParts.length > 0) {
                result.push({
                    ordinal: nextOrdinal++,
                    id: `${SYNTH_USER_ID_PREFIX}${pendingFirstRealId}`,
                    role: "user",
                    parts: pendingToolParts,
                    version: pendingFirstRealVersion,
                });
                pendingToolParts = [];
                pendingFirstRealId = "";
                pendingFirstRealVersion = "";
            }

            const version = rawEntryVersion(entry);
            result.push({
                ordinal: nextOrdinal++,
                id: entry.id,
                role: "assistant",
                parts: attachPiPartVersion(synthesizeAssistantParts(msg), version),
                version,
            });
            continue;
        }

        result.push({
            ordinal: nextOrdinal++,
            id: entry.id,
            role: typeof role === "string" ? role : "unknown",
            parts: [],
            version: rawEntryVersion(entry),
        });
    }

    // Trailing tool results produce a synthetic user turn.
    if (pendingToolParts.length > 0) {
        result.push({
            ordinal: nextOrdinal,
            id: `${SYNTH_USER_ID_PREFIX}${pendingFirstRealId}`,
            role: "user",
            parts: pendingToolParts,
            version: pendingFirstRealVersion,
        });
    }

    return result;
}

interface MessageEntry {
    type: "message";
    id: string;
    message: unknown;
}

function isMessageEntry(value: unknown): value is MessageEntry {
    if (value === null || typeof value !== "object") return false;
    const v = value as Record<string, unknown>;
    if (v.type !== "message") return false;
    if (typeof v.id !== "string") return false;
    if (v.message === null || typeof v.message !== "object") return false;
    return true;
}

function synthesizeUserParts(msg: unknown): unknown[] {
    const m = msg as { content?: unknown };
    if (typeof m.content === "string") {
        if (m.content.trim().length === 0) return [];
        return [{ type: "text", text: m.content }];
    }
    if (!Array.isArray(m.content)) return [];

    const parts: unknown[] = [];
    for (const c of m.content) {
        if (c === null || typeof c !== "object") continue;
        const cc = c as Record<string, unknown>;
        if (cc.type === "text" && typeof cc.text === "string") {
            parts.push({ type: "text", text: cc.text });
        }
    }
    return parts;
}

function synthesizeAssistantParts(msg: unknown): unknown[] {
    const m = msg as { content?: unknown };
    if (!Array.isArray(m.content)) return [];

    const parts: unknown[] = [];
    for (const c of m.content) {
        if (c === null || typeof c !== "object") continue;
        const cc = c as Record<string, unknown>;
        if (cc.type === "text" && typeof cc.text === "string") {
            parts.push({ type: "text", text: cc.text });
        } else if (cc.type === "toolCall" && typeof cc.id === "string") {
            parts.push({
                type: "tool",
                tool: typeof cc.name === "string" ? cc.name : "unknown",
                callID: cc.id,
                state: {
                    input: cc.arguments ?? {},
                },
            });
        }
    }
    return parts;
}

function synthesizeToolResultParts(msg: unknown): unknown[] {
    const m = msg as {
        toolCallId?: unknown;
        toolName?: unknown;
        content?: unknown;
    };
    const callID = typeof m.toolCallId === "string" ? m.toolCallId : "";
    const tool = typeof m.toolName === "string" ? m.toolName : "unknown";

    if (!callID) return []; // no useful pairing handle

    let output = "";
    if (Array.isArray(m.content)) {
        const fragments: string[] = [];
        for (const c of m.content) {
            if (c === null || typeof c !== "object") continue;
            const cc = c as Record<string, unknown>;
            if (cc.type === "text" && typeof cc.text === "string") {
                fragments.push(cc.text);
            }
        }
        output = fragments.join("\n");
    }

    return [
        {
            type: "tool",
            tool,
            callID,
            state: {
                output,
            },
        },
    ];
}

/**
 * The CK encoder for Pi `AgentMessage` rows. Its output equals what the daemon's closed Pi codec
 * decodes the same rows to (`decode_pi_rows` in `crates/daemon/src/codec/pi.rs`), less the
 * daemon's own block identity stamps and ordinals; `__fixtures__/pi-codec-parity.json` pins both
 * sides to one set of values.
 */

import { createHash } from "node:crypto";

import { serdeJsonCompact } from "@eidnara/opencode/hooks/context/module-wire";

/** One window row: the message's persisted entry id, or a reserved id, and the message. */
export interface PiRow {
    readonly id: string;
    readonly message: Record<string, unknown>;
}

export interface PiCkMessage {
    mid: string;
    ck: Record<string, unknown>;
}

/** Prefix of every reserved row id; Pi entry ids are hexadecimal, so none carries the `:`. */
export const PI_RESERVED_ID_PREFIX = "eidnara:";

/** Pi 0.80.2's `AgentMessage` roles, closed as the daemon codec closes them. */
export const PI_ROLES = [
    "user",
    "assistant",
    "toolResult",
    "bashExecution",
    "custom",
    "branchSummary",
    "compactionSummary",
] as const;
export type PiRole = (typeof PI_ROLES)[number];

const COMPACTION_SUMMARY_PREFIX =
    "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
const COMPACTION_SUMMARY_SUFFIX = "\n</summary>";
const BRANCH_SUMMARY_PREFIX =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";
const BRANCH_SUMMARY_SUFFIX = "</summary>";
const OPAQUE_SOURCE = { harness: "pi", type: "harness" } as const;

type Json = Record<string, unknown>;

function isRecord(value: unknown): value is Json {
    return value !== null && typeof value === "object" && !Array.isArray(value);
}

function stringField(value: Json, key: string): string | undefined {
    const field = value[key];
    return typeof field === "string" ? field : undefined;
}

/** Pi 0.80.2 `bashExecutionToText`. */
function bashExecutionText(message: Json): string {
    let text = `Ran \`${stringField(message, "command") ?? ""}\`\n`;
    const output = stringField(message, "output");
    text += output ? `\`\`\`\n${output}\n\`\`\`` : "(no output)";
    const exitCode = message.exitCode;
    if (message.cancelled === true) text += "\n\n(command cancelled)";
    else if (typeof exitCode === "number" && Number.isInteger(exitCode) && exitCode !== 0)
        text += `\n\nCommand exited with code ${exitCode}`;
    const fullOutputPath = stringField(message, "fullOutputPath");
    if (message.truncated === true && fullOutputPath)
        text += `\n\n[Output truncated. Full output: ${fullOutputPath}]`;
    return text;
}

function mediaKind(mediaType: string): string {
    if (mediaType.startsWith("image/")) return "image";
    if (mediaType.startsWith("audio/")) return "audio";
    if (mediaType.startsWith("video/")) return "video";
    if (mediaType === "application/pdf") return "document";
    return "file";
}

function media(part: Json): Json {
    const mediaType =
        stringField(part, "mimeType") ?? stringField(part, "mime") ?? "application/octet-stream";
    const data = stringField(part, "data");
    const url = stringField(part, "url");
    const filename = stringField(part, "filename");
    return {
        kind: mediaKind(mediaType),
        media_type: mediaType,
        ...(filename !== undefined ? { filename } : {}),
        source:
            data !== undefined
                ? { type: "data_base64", data }
                : url !== undefined
                  ? { type: "url", url }
                  : { type: "opaque", raw: part },
    };
}

function opaque(kind: string, raw: unknown, arc?: Json): Json {
    return {
        type: "opaque",
        source: OPAQUE_SOURCE,
        kind,
        raw,
        ...(arc !== undefined ? { arc } : {}),
    };
}

function block(kind: Json, pi?: Json): Json {
    return pi && Object.keys(pi).length > 0 ? { kind, provider_extras: { pi } } : { kind };
}

function textExtras(part: Json): Json {
    const signature = stringField(part, "textSignature");
    return signature !== undefined ? { textSignature: signature } : {};
}

/** A split `call|item` id is the canonical call id plus its item id. */
function canonicalToolId(nativeId: string): [string, string | undefined] {
    const split = nativeId.indexOf("|");
    return split < 0
        ? [nativeId, undefined]
        : [nativeId.slice(0, split), nativeId.slice(split + 1)];
}

function stableHashPrefix(value: unknown, chars: number): string {
    return createHash("sha256").update(serdeJsonCompact(value)).digest("hex").slice(0, chars);
}

function userContent(message: Json): Json[] {
    const content = message.content;
    if (typeof content === "string") return [block({ type: "text", text: content })];
    if (!Array.isArray(content)) return [];
    return content.map((part: unknown) => {
        const record = isRecord(part) ? part : {};
        const type = stringField(record, "type");
        if (type === "text")
            return block(
                { type: "text", text: stringField(record, "text") ?? "" },
                textExtras(record),
            );
        if (type === "image") return block({ type: "media", ...media(record) });
        return block(opaque(type ?? "unknown", part));
    });
}

function assistantContent(message: Json, ordinal: number): Json[] {
    const content = message.content;
    if (!Array.isArray(content)) return [];
    return content.map((part: unknown, partIndex) => {
        const record = isRecord(part) ? part : {};
        const type = stringField(record, "type");
        if (type === "text")
            return block(
                { type: "text", text: stringField(record, "text") ?? "" },
                textExtras(record),
            );
        if (type === "thinking") {
            if (record.redacted === true)
                return block({
                    type: "redacted_reasoning",
                    data:
                        stringField(record, "thinkingSignature") ??
                        stringField(record, "thinking") ??
                        "",
                });
            const signature = stringField(record, "thinkingSignature");
            return block({
                type: "reasoning",
                text: stringField(record, "thinking") ?? "",
                ...(signature !== undefined ? { signature } : {}),
            });
        }
        if (type === "toolCall") {
            const input = record.arguments !== undefined ? record.arguments : {};
            const name = stringField(record, "name") ?? "tool";
            const nativeId =
                stringField(record, "id") ??
                stringField(record, "callId") ??
                `synth-tool-${ordinal}-${partIndex}-${name}-${stableHashPrefix(input, 12)}`;
            const [id, itemId] = canonicalToolId(nativeId);
            const thoughtSignature = stringField(record, "thoughtSignature");
            return block(
                { type: "tool_call", id, name, input },
                {
                    ...(thoughtSignature !== undefined ? { thoughtSignature } : {}),
                    ...(itemId !== undefined ? { itemId } : {}),
                    nativeToolCallId: nativeId,
                },
            );
        }
        const approvalId = stringField(record, "approvalId");
        const arc =
            approvalId !== undefined
                ? {
                      kind: "Approval",
                      id: approvalId,
                      role: (type ?? "").includes("response") ? "Response" : "Request",
                  }
                : undefined;
        return block(opaque(type ?? "unknown", part, type === undefined ? undefined : arc));
    });
}

function toolOutput(message: Json, isError: boolean): Json {
    const parts = message.content;
    if (!Array.isArray(parts)) return { kind: { type: isError ? "error_text" : "text", text: "" } };
    if (parts.length === 1) {
        const only = parts[0];
        if (
            isRecord(only) &&
            only.type === "text" &&
            Object.keys(only).every((key) => key === "type" || key === "text")
        )
            return {
                kind: {
                    type: isError ? "error_text" : "text",
                    text: stringField(only, "text") ?? "",
                },
            };
    }
    const blocks = parts.map((part: unknown) => {
        const record = isRecord(part) ? part : {};
        const type = stringField(record, "type");
        const kind =
            type === "text"
                ? { type: "text", text: stringField(record, "text") ?? "" }
                : type === "image" || type === "file"
                  ? { type: "media", media: media(record) }
                  : { type: "opaque", opaque: opaque(type ?? "unknown", part) };
        if (kind.type === "opaque") delete (kind.opaque as Json).type;
        return { kind, provider_extras: { pi: { rawResultPart: part } } };
    });
    return { kind: { type: isError ? "error_content" : "content", blocks } };
}

function toolResultContent(message: Json): Json[] {
    const nativeId = stringField(message, "toolCallId") ?? "tool";
    const [id, itemId] = canonicalToolId(nativeId);
    const isError = message.isError === true;
    return [
        block(
            {
                type: "tool_result",
                id,
                tool_name: stringField(message, "toolName") ?? "tool",
                output: toolOutput(message, isError),
            },
            itemId !== undefined ? { itemId, nativeToolCallId: nativeId } : undefined,
        ),
    ];
}

function content(role: PiRole, message: Json, ordinal: number): Json[] {
    switch (role) {
        case "user":
        case "custom":
            return userContent(message);
        case "assistant":
            return assistantContent(message, ordinal);
        case "toolResult":
            return toolResultContent(message);
        case "bashExecution":
            return message.excludeFromContext === true
                ? []
                : [block({ type: "text", text: bashExecutionText(message) })];
        case "branchSummary":
            return [
                block({
                    type: "text",
                    text: `${BRANCH_SUMMARY_PREFIX}${stringField(message, "summary") ?? ""}${BRANCH_SUMMARY_SUFFIX}`,
                }),
            ];
        case "compactionSummary":
            return [
                block({
                    type: "text",
                    text: `${COMPACTION_SUMMARY_PREFIX}${stringField(message, "summary") ?? ""}${COMPACTION_SUMMARY_SUFFIX}`,
                }),
            ];
    }
}

function wireRole(role: PiRole): string {
    return role === "assistant" ? "assistant" : role === "toolResult" ? "tool" : "user";
}

export function isPiRole(role: unknown): role is PiRole {
    return typeof role === "string" && (PI_ROLES as readonly string[]).includes(role);
}

/**
 * Encodes rows as the CK window the daemon reads beside them. A row outside the closed role
 * set throws, so the adapter declines the pass instead of sending a window the daemon refuses.
 */
export function encodePiRowsToCk(rows: readonly PiRow[]): PiCkMessage[] {
    return rows.map((row, index) => {
        const role = row.message.role;
        if (!isPiRole(role)) throw new Error(`pi message ${row.id} has role ${String(role)}`);
        const message = row.message;
        const origin =
            role === "assistant" &&
            typeof message.api === "string" &&
            typeof message.provider === "string" &&
            typeof message.model === "string"
                ? { api: message.api, provider: message.provider, model: message.model }
                : undefined;
        const timestamp = message.timestamp;
        return {
            mid: row.id,
            ck: {
                role: wireRole(role),
                content: content(role, message, index + 1),
                ...(origin !== undefined ? { origin } : {}),
                meta: {
                    harness_id: row.id,
                    ...(role === "assistant" && message.stopReason === "error"
                        ? { errored: true }
                        : {}),
                    ...(typeof timestamp === "number" && Number.isSafeInteger(timestamp)
                        ? { created_at_ms: timestamp }
                        : {}),
                },
            },
        };
    });
}

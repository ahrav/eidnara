import * as crypto from "node:crypto";
import {
    getRawSessionStoredMessageCount,
    readRawSessionMessageOrdinalPage,
} from "./read-session-chunk";
import { isRawCompactionSummaryInfo, type RawMessageOrdinalAnchor } from "./read-session-raw";
import type { MessageLike } from "./tag-content-primitives";

/** The module facade accepts request pages up to 512 KiB. */
export const MODULE_PAGE_MAX_BYTES = 512 * 1024;
/** Chunking splits large values so each message fits one page. */
export const MODULE_ITEM_CONTINUATION_CHUNK_BYTES = 64 * 1024;
// The module reassembles this envelope for live transform requests.
export const MODULE_ITEM_CONTINUATION_KEY = "__shadow_item_continuation";
export const MODULE_ORDINAL_PAGE_SIZE = 500;

export interface ModuleNormalizationRecord {
    kind: "tag_prefix" | "ctx_search_hint" | "summary_message";
    message_id: string | null;
    part_index: number;
    field: string;
    tag_number?: number;
    removed: string;
}

function yieldToEventLoop(): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, 0));
}

function canonicalJson(value: unknown): string {
    if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
    if (value !== null && typeof value === "object") {
        const record = value as Record<string, unknown>;
        return `{${Object.keys(record)
            .sort()
            .map((key) => `${JSON.stringify(key)}:${canonicalJson(record[key])}`)
            .join(",")}}`;
    }
    const encoded = JSON.stringify(value);
    return encoded === undefined ? "null" : encoded;
}

function transformPageDigest(arrays: Record<string, unknown[]>): string {
    let wireArrays: Record<string, unknown[]>;
    try {
        wireArrays = JSON.parse(JSON.stringify(arrays)) as Record<string, unknown[]>;
    } catch (error) {
        throw new Error("module transform page is not JSON-serializable", { cause: error });
    }
    return crypto.createHash("sha256").update(canonicalJson(wireArrays)).digest("hex");
}

function getMessageId(message: MessageLike): string | null {
    return typeof message.info.id === "string" && message.info.id.length > 0
        ? message.info.id
        : null;
}

function isSyntheticWireMessage(message: MessageLike): boolean {
    if ((message.info as { synthetic?: unknown }).synthetic === true) return true;
    return message.parts.some(
        (part) =>
            part !== null &&
            typeof part === "object" &&
            (part as { synthetic?: unknown }).synthetic === true,
    );
}

/**
 * The session's durable ordinal-memo state, passed as one bundle. Callers
 * project their session-state fields into this shape exactly once so the
 * field-to-field rename map cannot drift between call sites. The resolver
 * mutates `entries` in place (clear/set): pass the live session Map, never
 * a copy, or resolved ordinals silently detach from the session state.
 */
export interface ModuleOrdinalMemo {
    generation: number;
    memoGeneration: number;
    entries: Map<string, number>;
    anchor?: RawMessageOrdinalAnchor | null;
    storedCount?: number | null;
    canonicalCount?: number;
    /** Highest ordinal from the prior lineage; priming assigns persisted rows ordinals starting at `continuationBase + 1`. */
    continuationBase?: number;
}

/**
 * Resolve OpenCode message ids to the absolute ordinals used by the module.
 */
export async function resolveOrdinalsForModule(args: {
    sessionId: string;
    messages: MessageLike[];
    memo: ModuleOrdinalMemo;
    /** Absolute ordinal immediately before a sliced unresolved tail. */
    provisionalBase?: number;
}): Promise<
    | {
          ok: true;
          annotatedInput: unknown[];
          memoGeneration: number;
          memoAnchor: RawMessageOrdinalAnchor | null;
          memoStoredCount: number;
          memoCanonicalCount: number;
          normalizations: ModuleNormalizationRecord[];
      }
    | {
          ok: false;
          reason: "unresolved" | "mismatch";
          messageId?: string;
          messageIndex?: number;
          messageRole?: string;
      }
> {
    const memo = args.memo.entries;
    const generationChanged = args.memo.memoGeneration !== args.memo.generation;
    if (generationChanged) memo.clear();

    const continuationBase = Math.max(0, args.memo.continuationBase ?? 0);
    let anchor = generationChanged ? null : (args.memo.anchor ?? null);
    let storedCount = generationChanged ? null : (args.memo.storedCount ?? null);
    let canonicalCount = generationChanged
        ? continuationBase
        : (args.memo.canonicalCount ?? continuationBase);
    const priming = storedCount === null;
    if (priming) {
        memo.clear();
        anchor = null;
        canonicalCount = continuationBase;
    }

    const newEntries: Array<ReturnType<typeof readRawSessionMessageOrdinalPage>[number]> = [];
    let pageAnchor = anchor;
    while (true) {
        const page = readRawSessionMessageOrdinalPage(
            args.sessionId,
            pageAnchor,
            MODULE_ORDINAL_PAGE_SIZE,
        );
        if (page.length === 0) break;
        newEntries.push(...page);
        const last = page[page.length - 1];
        pageAnchor = { timeCreated: last.timeCreated, id: last.id };
        if (page.length < MODULE_ORDINAL_PAGE_SIZE) break;
        await yieldToEventLoop();
    }

    const currentStoredCount = getRawSessionStoredMessageCount(args.sessionId);
    const expectedStoredCount = (storedCount ?? 0) + newEntries.length;
    if (currentStoredCount !== expectedStoredCount) {
        memo.clear();
        return { ok: false, reason: "mismatch" };
    }

    for (const entry of newEntries) {
        if (!entry.contributesOrdinal) continue;
        canonicalCount += 1;
        const prior = memo.get(entry.id);
        if (prior !== undefined && prior !== canonicalCount) {
            memo.clear();
            return { ok: false, reason: "mismatch", messageId: entry.id };
        }
        memo.set(entry.id, canonicalCount);
    }
    anchor = pageAnchor;
    storedCount = currentStoredCount;

    const normalizations: ModuleNormalizationRecord[] = [];
    const visibleIndexes: number[] = [];
    const visibleMessages = args.messages.filter((message, index) => {
        if (!isRawCompactionSummaryInfo(message.info)) {
            visibleIndexes.push(index);
            return true;
        }
        normalizations.push({
            kind: "summary_message",
            message_id: getMessageId(message),
            part_index: -1,
            field: "input",
            removed: JSON.stringify(message),
        });
        return false;
    });

    // The encoder must not mutate caller-owned OpenCode objects.
    // A shallow root projection is sufficient because the encoder only reads nested fields.
    // The shallow root projection avoids walking or duplicating the full message tree on every pass.
    const annotated: Array<Record<string, unknown>> = new Array(visibleMessages.length);
    const resolved: Array<number | undefined> = new Array(annotated.length);
    let firstUnresolved:
        | {
              messageId: string;
              messageIndex: number;
              messageRole: string;
          }
        | undefined;
    for (let index = 0; index < annotated.length; index += 1) {
        const messageId = getMessageId(visibleMessages[index]);
        if (!messageId) {
            return {
                ok: false,
                reason: "unresolved",
                messageIndex: visibleIndexes[index],
                messageRole: visibleMessages[index].info.role ?? "unknown",
            };
        }
        const ordinal = memo.get(messageId);
        if (ordinal === undefined && firstUnresolved === undefined) {
            firstUnresolved = {
                messageId,
                messageIndex: visibleIndexes[index],
                messageRole: visibleMessages[index].info.role ?? "unknown",
            };
        }
        resolved[index] = ordinal;
    }

    /**
     * An unpersisted explicit synthetic message borrows its preceding canonical ordinal.
     * Persisted unpaged messages remain unresolved and are rejected.
     */
    for (let index = 0; index < resolved.length; index += 1) {
        if (resolved[index] !== undefined || !isSyntheticWireMessage(visibleMessages[index])) {
            continue;
        }
        const hasResolvedMessageAfter = resolved
            .slice(index + 1)
            .some((ordinal) => ordinal !== undefined);
        if (!hasResolvedMessageAfter) continue;
        let priorIndex = index - 1;
        while (priorIndex >= 0 && resolved[priorIndex] === undefined) priorIndex -= 1;
        resolved[index] = priorIndex >= 0 ? (resolved[priorIndex] as number) : 0;
    }

    let suffixStart = annotated.length;
    while (suffixStart > 0 && resolved[suffixStart - 1] === undefined) suffixStart -= 1;
    for (let index = 0; index < suffixStart; index += 1) {
        if (resolved[index] === undefined) {
            return { ok: false, reason: "unresolved", ...firstUnresolved };
        }
    }
    if (suffixStart < annotated.length) {
        const base =
            suffixStart > 0
                ? (resolved[suffixStart - 1] as number)
                : Math.max(0, args.provisionalBase ?? canonicalCount);
        for (let index = suffixStart; index < annotated.length; index += 1) {
            resolved[index] = base + (index - suffixStart) + 1;
        }
    }

    for (let index = 0; index < annotated.length; index += 1) {
        const messageId = getMessageId(visibleMessages[index]) as string;
        const ordinal = resolved[index] as number;
        const prior = memo.get(messageId);
        if (prior !== undefined && prior !== ordinal) {
            return {
                ok: false,
                reason: "mismatch",
                messageId,
                messageIndex: visibleIndexes[index],
                messageRole: visibleMessages[index].info.role ?? "unknown",
            };
        }
        memo.set(messageId, ordinal);
        annotated[index] = { ...visibleMessages[index], absolute_ordinal: ordinal };
    }

    return {
        ok: true,
        annotatedInput: annotated,
        memoGeneration: args.memo.generation,
        memoAnchor: anchor,
        memoStoredCount: storedCount,
        memoCanonicalCount: canonicalCount,
        normalizations,
    };
}

/** Flatten the typed builder shape to the module's top-level wire envelope. */
function toFlatModuleWireBody(payload: {
    method: string;
    params: Record<string, unknown>;
}): Record<string, unknown> {
    return { method: payload.method, ...payload.params };
}

export function moduleWireBodyBytes(payload: {
    method: string;
    params: Record<string, unknown>;
}): number {
    return Buffer.byteLength(JSON.stringify(toFlatModuleWireBody(payload)));
}

/**
 * Paging must preserve every message value.
 * The module understands continuation markers for a single item that exceeds the page limit.
 */
export interface ModuleTransformWirePage {
    page: Record<string, unknown>;
    /** The byte count must equal the UTF-8 length of the serialized page. */
    bytes: number;
}

export function buildPagedModuleTransformPayloads(
    body: Record<string, unknown>,
): ModuleTransformWirePage[] {
    // The returned serialized length prevents transport telemetry from serializing the body twice.
    const unpagedBytes = Buffer.byteLength(JSON.stringify(body));
    if (unpagedBytes <= MODULE_PAGE_MAX_BYTES) return [{ page: body, bytes: unpagedBytes }];

    const arrayFields = [
        "input",
        "messages",
        "native_messages",
        "ts_output",
        "ts_messages",
        "normalizations",
    ].filter((field) => Array.isArray(body[field]));
    if (arrayFields.length === 0) {
        throw new Error("module transform body has no pageable message arrays");
    }
    const scalarFields = { ...body };
    for (const field of arrayFields) delete scalarFields[field];
    const transformPageId = crypto.randomUUID();
    const items = arrayFields.flatMap((field) =>
        (body[field] as unknown[]).map((value, itemIndex) => ({ field, value, itemIndex })),
    );
    const emptyArrays = (): Record<string, unknown[]> =>
        Object.fromEntries(arrayFields.map((field) => [field, []]));
    const makePage = (args: {
        index: number;
        total: number;
        complete: boolean;
        arrays: Record<string, unknown[]>;
    }): ModuleTransformWirePage => {
        const pageArrays = Object.fromEntries(
            arrayFields.map((field) => [field, args.arrays[field] ?? []]),
        );
        const page: Record<string, unknown> = {
            method: body.method,
            session_id: body.session_id,
            shadow_generation: body.shadow_generation,
            transform_page_id: transformPageId,
            // A stable transform generation keeps every page of one request under the same all-or-none paging contract.
            transform_generation: body.shadow_generation ?? 0,
            transform_page_index: args.index,
            transform_page_total: args.total,
            transform_page_complete: args.complete,
            transform_page_digest: transformPageDigest(pageArrays),
            ...pageArrays,
        };
        if (args.complete) Object.assign(page, scalarFields);
        // The returned `bytes` value is the serialized page's exact UTF-8 length.
        return { page, bytes: Buffer.byteLength(JSON.stringify(page)) };
    };
    const hasItems = (arrays: Record<string, unknown[]>): boolean =>
        Object.values(arrays).some((values) => values.length > 0);

    // The encoder assigns digests only to emitted pages.
    const serializedItemBytes = (value: unknown): number =>
        Buffer.byteLength(JSON.stringify(value) ?? "null");
    const pageByteLength = (args: {
        index: number;
        total: number;
        complete: boolean;
        arrayBytes: Record<string, number>;
    }): number => {
        const skeleton: Record<string, unknown> = {
            method: body.method,
            session_id: body.session_id,
            shadow_generation: body.shadow_generation,
            transform_page_id: transformPageId,
            transform_generation: body.shadow_generation ?? 0,
            transform_page_index: args.index,
            transform_page_total: args.total,
            transform_page_complete: args.complete,
            transform_page_digest: "0".repeat(64),
            ...Object.fromEntries(arrayFields.map((field) => [field, []])),
        };
        if (args.complete) Object.assign(skeleton, scalarFields);
        const emptyArrayBytes = 2 * arrayFields.length;
        const contentsBytes = arrayFields.reduce(
            (sum, field) => sum + (args.arrayBytes[field] ?? 2),
            0,
        );
        return Buffer.byteLength(JSON.stringify(skeleton)) - emptyArrayBytes + contentsBytes;
    };

    let assumedTotal = 1;
    for (let attempt = 0; attempt < 10; attempt += 1) {
        const pages: ModuleTransformWirePage[] = [];
        let current = emptyArrays();
        let currentBytes = Object.fromEntries(arrayFields.map((field) => [field, 2]));
        const appendUnit = (field: string, value: unknown): boolean => {
            const valueBytes = serializedItemBytes(value);
            const previousBytes = currentBytes[field] ?? 2;
            current[field].push(value);
            currentBytes[field] = previousBytes + valueBytes + (current[field].length > 1 ? 1 : 0);
            if (
                pageByteLength({
                    index: pages.length,
                    total: assumedTotal,
                    complete: false,
                    arrayBytes: currentBytes,
                }) <= MODULE_PAGE_MAX_BYTES
            ) {
                return true;
            }
            current[field].pop();
            currentBytes[field] = previousBytes;
            if (hasItems(current)) {
                pages.push(
                    makePage({
                        index: pages.length,
                        total: assumedTotal,
                        complete: false,
                        arrays: current,
                    }),
                );
                current = emptyArrays();
                currentBytes = Object.fromEntries(arrayFields.map((name) => [name, 2]));
            }
            current[field].push(value);
            currentBytes[field] = 2 + valueBytes;
            if (
                pageByteLength({
                    index: pages.length,
                    total: assumedTotal,
                    complete: false,
                    arrayBytes: currentBytes,
                }) > MODULE_PAGE_MAX_BYTES
            ) {
                current[field].pop();
                currentBytes[field] = 2;
                return false;
            }
            return true;
        };

        for (const item of items) {
            if (appendUnit(item.field, item.value)) continue;
            const serialized = JSON.stringify(item.value) ?? "null";
            const bytes = Buffer.from(serialized, "utf8");
            const chunks: string[] = [];
            for (let start = 0; start < bytes.length; ) {
                let end = Math.min(start + MODULE_ITEM_CONTINUATION_CHUNK_BYTES, bytes.length);
                while (end < bytes.length && (bytes[end] & 0xc0) === 0x80) end -= 1;
                chunks.push(bytes.subarray(start, end).toString("utf8"));
                start = end;
            }
            const chunkTotal = chunks.length;
            for (const [chunkIndex, chunk] of chunks.entries()) {
                const marker = {
                    [MODULE_ITEM_CONTINUATION_KEY]: {
                        field: item.field,
                        item_index: item.itemIndex,
                        chunk_index: chunkIndex,
                        chunk_total: chunkTotal,
                    },
                    chunk,
                };
                if (!appendUnit(item.field, marker)) {
                    throw new Error("module transform continuation exceeds the 512 KiB page limit");
                }
            }
        }

        let finalPage = makePage({
            index: pages.length,
            total: assumedTotal,
            complete: true,
            arrays: current,
        });
        if (finalPage.bytes > MODULE_PAGE_MAX_BYTES) {
            if (!hasItems(current)) {
                throw new Error("module transform scalar tail exceeds the 512 KiB page limit");
            }
            pages.push(
                makePage({
                    index: pages.length,
                    total: assumedTotal,
                    complete: false,
                    arrays: current,
                }),
            );
            current = emptyArrays();
            currentBytes = Object.fromEntries(arrayFields.map((field) => [field, 2]));
            finalPage = makePage({
                index: pages.length,
                total: assumedTotal,
                complete: true,
                arrays: current,
            });
            if (finalPage.bytes > MODULE_PAGE_MAX_BYTES) {
                throw new Error("module transform scalar tail exceeds the 512 KiB page limit");
            }
        }
        pages.push(finalPage);
        if (pages.length === assumedTotal) return pages;
        assumedTotal = pages.length;
    }
    throw new Error("module transform page count did not stabilize");
}

export const __moduleWireTest = {
    buildPagedModuleTransformPayloads,
    encodeOpenCodeMessagesToCk,
    moduleWireBodyBytes,
    resolveOrdinalsForModule,
    toFlatModuleWireBody,
};

export function encodeOpenCodeMessagesToCk(messages: unknown[]): Array<{
    mid: string;
    ordinal: number;
    ck: Record<string, unknown>;
}> {
    return messages.map((message, index) => {
        const raw =
            message !== null && typeof message === "object"
                ? (message as Record<string, unknown>)
                : {};
        const info =
            raw.info !== null && typeof raw.info === "object"
                ? (raw.info as Record<string, unknown>)
                : raw;
        const id =
            (typeof info.id === "string" && info.id.length > 0 && info.id) ||
            `opencode-${crypto.createHash("sha256").update(JSON.stringify(message)).digest("hex").slice(0, 24)}`;
        const ordinal =
            (typeof raw.absolute_ordinal === "number" && raw.absolute_ordinal) ||
            (typeof info.absolute_ordinal === "number" && info.absolute_ordinal) ||
            index + 1;
        const role = typeof info.role === "string" ? info.role : "user";
        const parts = Array.isArray(raw.parts) ? raw.parts : [];
        const synthetic =
            parts.length > 0 &&
            parts.every(
                (part) =>
                    part !== null &&
                    typeof part === "object" &&
                    ((part as Record<string, unknown>).synthetic === true ||
                        (part as Record<string, unknown>).syntheticTodoMarker === true),
            );
        const content: Record<string, unknown>[] = [];
        for (const partValue of parts) {
            if (partValue === null || typeof partValue !== "object") continue;
            const part = partValue as Record<string, unknown>;
            const type = typeof part.type === "string" ? part.type : "unknown";
            if (type === "text" && part.ignored !== true) {
                content.push({
                    kind: { type: "text", text: typeof part.text === "string" ? part.text : "" },
                });
            } else if (type === "reasoning" || type === "thinking") {
                const signature = typeof part.signature === "string" ? part.signature : undefined;
                content.push({
                    kind: {
                        type: "reasoning",
                        text:
                            typeof part.text === "string"
                                ? part.text
                                : typeof part.thinking === "string"
                                  ? part.thinking
                                  : "",
                        ...(signature ? { signature } : {}),
                    },
                    ...(part.cache_control !== undefined
                        ? {
                              provider_extras: {
                                  opencode: { cache_control: part.cache_control },
                              },
                          }
                        : {}),
                });
            } else if (type === "redacted_thinking") {
                content.push({
                    kind: {
                        type: "redacted_reasoning",
                        data:
                            typeof part.data === "string"
                                ? part.data
                                : typeof part.redacted === "string"
                                  ? part.redacted
                                  : "",
                    },
                    ...(part.cache_control !== undefined
                        ? {
                              provider_extras: {
                                  opencode: { cache_control: part.cache_control },
                              },
                          }
                        : {}),
                });
            } else if (type === "tool") {
                const state =
                    part.state !== null && typeof part.state === "object"
                        ? (part.state as Record<string, unknown>)
                        : {};
                const callId =
                    (typeof part.callID === "string" && part.callID) ||
                    (typeof part.callId === "string" && part.callId) ||
                    (typeof part.id === "string" && part.id) ||
                    `${id}#${content.length}`;
                const toolName = typeof part.tool === "string" ? part.tool : "unknown";
                const input = state.input ?? part.input ?? part.args;
                const finished = state.status === "completed" || state.status === "error";
                // Pi folds a tool result into the next message as a finished part without input.
                if (input !== undefined || !finished) {
                    content.push({
                        kind: { type: "tool_call", id: callId, name: toolName, input: input ?? {} },
                    });
                }
                if (finished) {
                    const output =
                        typeof state.output === "string"
                            ? state.output
                            : typeof state.error === "string"
                              ? state.error
                              : "";
                    content.push({
                        kind: {
                            type: "tool_result",
                            id: callId,
                            tool_name: toolName,
                            output: {
                                kind: {
                                    type: state.status === "error" ? "error_text" : "text",
                                    text: output,
                                },
                            },
                        },
                    });
                }
            } else if (
                !["compaction", "step-finish", "snapshot", "patch", "agent", "retry"].includes(type)
            ) {
                content.push({
                    kind: {
                        type: "opaque",
                        source: "opencode",
                        kind: type,
                        raw: part,
                    },
                });
            }
        }
        return {
            mid: id,
            ordinal,
            ck: {
                role,
                content,
                meta: {
                    harness_id: id,
                    ordinal,
                    synthetic,
                    summary: info.summary === true,
                    errored: info.error !== undefined && info.error !== null,
                    ...(typeof info.finish === "string" ? { finish: info.finish } : {}),
                    ...(typeof info.time_created === "number"
                        ? { created_at_ms: info.time_created }
                        : typeof info.timeCreated === "number"
                          ? { created_at_ms: info.timeCreated }
                          : {}),
                },
            },
        };
    });
}

/** Every method name the module transport can carry on the wire. */
export type ModuleMethod =
    | "transform"
    | "session.status"
    | "session.delete"
    | "session.flush"
    | "session.recomp"
    | "session.wrapup"
    | "todo_state.set"
    | "agent_drops.append"
    | "ctx_note"
    | "note.evaluation.register"
    | "note.evaluation.heartbeat"
    | "note.evaluation.unregister"
    | "note.evaluation.next"
    | "note.evaluation.renew"
    | "note.evaluation.complete"
    | "note.evaluation.abandon"
    | "transform.ack"
    | "transform.nack"
    | KernelMethod;

/** The daemon's `kernel.*` routes, issued only through the shared kernel client. */
export type KernelMethod =
    | "kernel.read"
    | "kernel.commit"
    | "kernel.eligibility.batch"
    | "kernel.egress.decide"
    | "kernel.artifact.ingest.begin"
    | "kernel.artifact.ingest.page"
    | "kernel.artifact.ingest.finish";

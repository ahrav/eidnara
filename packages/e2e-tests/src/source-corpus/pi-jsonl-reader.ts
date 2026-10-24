import type { ExpectedRecord, ExpectedUnit, NonUnit } from "./expected.ts";

type Json = Record<string, unknown>;

function object(value: unknown, at: string): Json {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
        throw new Error(`${at}: not an object`);
    }
    return value as Json;
}

function text(value: unknown, at: string): string {
    if (typeof value !== "string") throw new Error(`${at}: not a string`);
    return value;
}

function utf8Length(value: string): number {
    return Buffer.byteLength(value, "utf8");
}

function parseLine(line: string, at: string): Json {
    let value: unknown;
    try {
        value = JSON.parse(line);
    } catch (error) {
        throw new Error(`${at}: not JSON: ${String(error)}`);
    }
    return object(value, at);
}

/**
 * The expected-unit table of one Pi session file, one record per JSONL line, with the classification
 * `read_pi_session` in `crates/daemon/src/source_corpus.rs` applies.
 */
export function readPiSession(jsonl: string): ExpectedRecord[] {
    const records: ExpectedRecord[] = [];
    jsonl.split("\n").forEach((line, number) => {
        if (line.trim() === "") return;
        const at = `line ${number + 1}`;
        const entry = parseLine(line, at);
        const kind = text(entry.type, `${at} type`);
        const id = text(entry.id, `${at} id`);
        const units: ExpectedUnit[] = [];
        const nonUnits: NonUnit[] = [];
        if (kind !== "message") {
            nonUnits.push({ index: -1, partId: id, reason: "non_message_entry", kind });
            records.push({ id, role: "", units, nonUnits });
            return;
        }
        const message = object(entry.message, `${at} message`);
        const role = text(message.role, `${at} role`);
        if (role !== "user" && role !== "assistant" && role !== "toolResult") {
            nonUnits.push({ index: -1, partId: id, reason: "uncaptured_role", kind: role });
            records.push({ id, role, units, nonUnits });
            return;
        }
        const content = message.content;
        if (typeof content === "string" && role !== "toolResult") {
            units.push({
                index: 0,
                partId: id,
                block: 0,
                byteLen: utf8Length(content),
                tool: null,
            });
            records.push({ id, role, units, nonUnits });
            return;
        }
        if (!Array.isArray(content)) throw new Error(`${at}: content has no supported shape`);
        const items = content.map((item, index) => object(item, `${at} item ${index}`));
        if (role !== "toolResult") {
            items.forEach((item, index) => {
                const itemKind = text(item.type, `${at} item ${index} type`);
                if (itemKind === "text") {
                    const body = text(item.text, `${at} item ${index} text`);
                    units.push({
                        index,
                        partId: id,
                        block: index,
                        byteLen: utf8Length(body),
                        tool: null,
                    });
                } else {
                    nonUnits.push({ index, partId: id, reason: "skipped_item", kind: itemKind });
                }
            });
            records.push({ id, role, units, nonUnits });
            return;
        }
        const nonText = items.find((item) => item.type !== "text");
        if (nonText) {
            const itemKind = text(nonText.type, `${at} item type`);
            nonUnits.push({ index: -1, partId: id, reason: "non_text_item", kind: itemKind });
            records.push({ id, role, units, nonUnits });
            return;
        }
        if (typeof message.isError !== "boolean")
            throw new Error(`${at}: isError is not a boolean`);
        const details = message.details as { truncation?: { truncated?: unknown } } | undefined;
        const truncated = details?.truncation?.truncated;
        const tool = {
            name: text(message.toolName, `${at} toolName`),
            callId: text(message.toolCallId, `${at} toolCallId`),
            isError: message.isError,
            exit: null,
            truncated: typeof truncated === "boolean" ? truncated : null,
        };
        items.forEach((item, index) => {
            const body = text(item.text, `${at} item ${index} text`);
            units.push({ index, partId: id, block: index, byteLen: utf8Length(body), tool });
        });
        records.push({ id, role, units, nonUnits });
    });
    return records;
}

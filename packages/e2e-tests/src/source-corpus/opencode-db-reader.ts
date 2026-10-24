import type { Database } from "bun:sqlite";

export type NonUnitReason =
    | "non_text_part"
    | "ignored_text"
    | "unsettled_text"
    | "unsettled_tool"
    | "non_text_item"
    | "skipped_item"
    | "non_message_entry"
    | "uncaptured_role";

export interface ExpectedTool {
    readonly name: string;
    readonly callId: string;
    readonly isError: boolean;
    readonly exit: number | null;
    readonly truncated: boolean | null;
}

export interface ExpectedUnit {
    readonly index: number;
    readonly partId: string;
    readonly byteLen: number;
    readonly tool: ExpectedTool | null;
}

export interface NonUnit {
    readonly index: number;
    readonly partId: string;
    readonly reason: NonUnitReason;
    readonly kind: string;
}

export interface ExpectedRecord {
    readonly id: string;
    readonly role: string;
    readonly units: readonly ExpectedUnit[];
    readonly nonUnits: readonly NonUnit[];
}

type Json = Record<string, unknown>;

function isObject(value: unknown): value is Json {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function object(value: unknown, at: string): Json {
    if (!isObject(value)) throw new Error(`${at}: not an object`);
    return value;
}

function text(value: unknown, at: string): string {
    if (typeof value !== "string") throw new Error(`${at}: not a string`);
    return value;
}

function parse(data: string, at: string): Json {
    let value: unknown;
    try {
        value = JSON.parse(data);
    } catch (error) {
        throw new Error(`${at}: data is not JSON: ${String(error)}`);
    }
    return object(value, at);
}

function utf8Length(value: string): number {
    return Buffer.byteLength(value, "utf8");
}

export function readOpenCodeSession(db: Database, sessionId: string): ExpectedRecord[] {
    const messages = db
        .prepare("SELECT id, data FROM message WHERE session_id = ? ORDER BY time_created, id")
        .all(sessionId) as Array<{ id: string; data: string }>;
    const parts = db.prepare(
        "SELECT id, data FROM part WHERE session_id = ? AND message_id = ? ORDER BY time_created, id",
    );
    return messages.flatMap((row) => {
        const info = parse(row.data, `message ${row.id}`);
        if (info.summary === true && info.finish === "stop") return [];
        const role = text(info.role, `message ${row.id} role`);
        const time = object(info.time, `message ${row.id} time`);
        const settled = time.completed !== undefined && time.completed !== null;
        const units: ExpectedUnit[] = [];
        const nonUnits: NonUnit[] = [];
        const partRows = parts.all(sessionId, row.id) as Array<{ id: string; data: string }>;
        partRows.forEach((partRow, index) => {
            const at = `part ${partRow.id}`;
            const part = parse(partRow.data, at);
            const kind = text(part.type, `${at} type`);
            const nonUnit = (reason: NonUnitReason) =>
                nonUnits.push({ index, partId: partRow.id, reason, kind });
            if (kind === "text") {
                const body = text(part.text, `${at} text`);
                if (part.ignored === true) nonUnit("ignored_text");
                else if (role === "assistant" && !settled) nonUnit("unsettled_text");
                else
                    units.push({
                        index,
                        partId: partRow.id,
                        byteLen: utf8Length(body),
                        tool: null,
                    });
            } else if (kind === "tool") {
                const state = object(part.state, `${at} state`);
                const status = text(state.status, `${at} status`);
                const metadata = isObject(state.metadata) ? state.metadata : {};
                const unsettled =
                    (status !== "completed" && status !== "error") ||
                    (metadata.interrupted === true && !settled);
                if (unsettled) {
                    nonUnit("unsettled_tool");
                } else {
                    const isError = status === "error";
                    const output = text(isError ? state.error : state.output, `${at} output`);
                    const name = text(part.tool, `${at} tool`);
                    units.push({
                        index,
                        partId: partRow.id,
                        byteLen: utf8Length(output),
                        tool: {
                            name,
                            callId: text(part.callID, `${at} callID`),
                            isError,
                            exit:
                                name === "bash" && Number.isInteger(metadata.exit)
                                    ? (metadata.exit as number)
                                    : null,
                            truncated:
                                typeof metadata.truncated === "boolean" ? metadata.truncated : null,
                        },
                    });
                }
            } else {
                nonUnit("non_text_part");
            }
        });
        return [{ id: row.id, role, units, nonUnits }];
    });
}

import type { Database } from "../../../shared/sqlite";
import { jsonField } from "../../../shared/sqlite-helpers";
import { isMachineAuthoredPart, isMeaningfulUserText } from "../read-session-formatting";

export const MID_TURN_REFERENCE_SHA = "7ed1e9845af1a76ff04c31d95ea811367a926bb0";

interface AssistantMidTurnRow {
    id?: string;
    finish?: string | null;
    timeCreated?: number;
    timeCompleted?: number | null;
}

interface MessageIdRow {
    id?: string;
}

interface PartDataRow {
    data?: string | null;
}

export function frozenIsMidTurnFromOpenCodeDb(db: Database, sessionId: string): boolean {
    const latestAssistant = db
        .prepare(
            `SELECT id,
                    json_extract(data, '$.finish') as finish,
                    json_extract(data, '$.time.completed') as timeCompleted,
                    time_created as timeCreated
             FROM message
             WHERE session_id = ?
               AND ${jsonField("data", "$.role")} = 'assistant'
               AND NOT (
                 COALESCE(${jsonField("data", "$.summary")}, 0) = 1
                 AND COALESCE(${jsonField("data", "$.finish")}, '') = 'stop'
               )
             ORDER BY time_created DESC, id DESC
             LIMIT 1`,
        )
        .get(sessionId) as AssistantMidTurnRow | null;

    if (
        hasNewerRealUserMessage(
            db,
            sessionId,
            latestAssistant?.id ?? "",
            latestAssistant?.timeCreated ?? -1,
        )
    ) {
        return true;
    }
    if (typeof latestAssistant?.id !== "string") return false;
    if (typeof latestAssistant.timeCompleted !== "number") return true;
    if (latestAssistant.finish === "tool-calls") return true;

    const partRows = db
        .prepare("SELECT data FROM part WHERE session_id = ? AND message_id = ?")
        .all(sessionId, latestAssistant.id) as PartDataRow[];

    return partRows.some((row) => {
        const part = parsePart(row);
        return (
            part !== null &&
            part.type === "tool" &&
            !isProviderExecuted(part) &&
            !isMachineAuthoredPart(part)
        );
    });
}

function isProviderExecuted(part: Record<string, unknown>): boolean {
    if (part.providerExecuted === true) return true;
    const metadata = part.metadata;
    return (
        metadata !== null &&
        typeof metadata === "object" &&
        (metadata as Record<string, unknown>).providerExecuted === true
    );
}

function hasNewerRealUserMessage(
    db: Database,
    sessionId: string,
    latestAssistantId: string,
    latestAssistantTimeCreated: number,
): boolean {
    const candidates = db
        .prepare(
            `SELECT m.id
             FROM message m
             WHERE m.session_id = ?
               AND (m.time_created > ? OR (m.time_created = ? AND m.id > ?))
               AND ${jsonField("m.data", "$.role")} = 'user'
               AND NOT EXISTS (
                 SELECT 1 FROM part p
                 WHERE p.message_id = m.id
                   AND ${jsonField("p.data", "$.type")} = 'compaction'
               )
             ORDER BY m.time_created ASC, m.id ASC`,
        )
        .all(
            sessionId,
            latestAssistantTimeCreated,
            latestAssistantTimeCreated,
            latestAssistantId,
        ) as MessageIdRow[];

    const selectParts = db.prepare("SELECT data FROM part WHERE message_id = ?");
    for (const candidate of candidates) {
        if (typeof candidate.id !== "string") continue;
        const partRows = selectParts.all(candidate.id) as PartDataRow[];
        if (isRealUserMessage(partRows)) return true;
    }
    return false;
}

function isRealUserMessage(partRows: PartDataRow[]): boolean {
    if (partRows.length === 0) return true;
    return partRows.some((row) => {
        const part = parsePart(row);
        return part !== null && isRealUserPart(part);
    });
}

function isRealUserPart(part: Record<string, unknown>): boolean {
    if (typeof part.type !== "string") return false;
    if (isMachineAuthoredPart(part)) return false;
    if (part.type === "text") {
        return typeof part.text === "string" && isMeaningfulUserText(part.text);
    }
    return true;
}

function parsePart(row: PartDataRow): Record<string, unknown> | null {
    if (typeof row.data !== "string" || row.data.length === 0) return null;
    try {
        const parsed: unknown = JSON.parse(row.data);
        return parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)
            ? (parsed as Record<string, unknown>)
            : null;
    } catch {
        return null;
    }
}

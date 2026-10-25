import { Database } from "bun:sqlite";

import { loadFrozenOpenCode } from "./freeze.ts";
import { CANARY } from "./workspace.ts";

/** The R1 tool and result-kind cells, in the order the fixture README lists them. */
export const R1_CELLS = [
    "tool:read",
    "tool:bash",
    "tool:edit",
    "tool:write",
    "tool:grep",
    "tool:glob",
    "tool:todowrite",
    "tool:mcp",
    "tool:subagent",
    "tool:aborted",
    "result:text",
    "result:json_text",
    "result:pi_image_item",
    "result:opencode_non_text_part",
    "result:over_64_kib",
    "result:invalid_utf8",
    "result:canary",
] as const;

export type Cell = (typeof R1_CELLS)[number];

/** Cells a harness's pinned closure cannot produce, with the reason the README records. */
export const CANNOT_PRODUCE: Record<"opencode" | "pi", Partial<Record<Cell, string>>> = {
    opencode: {
        "result:pi_image_item": "a Pi content item; OpenCode carries images as file parts",
        "result:over_64_kib":
            "OpenCode 1.18.22 truncates every tool result before it reaches the session store",
    },
    pi: {
        "tool:glob": "Pi 0.80.2 has no glob tool; its `find` takes a glob pattern",
        "tool:todowrite": "Pi 0.80.2 has no to-do tool",
        "tool:mcp": "Pi 0.80.2 has no MCP client; the extension tool `corpus_probe` stands in",
        "tool:subagent": "Pi 0.80.2 has no subagent tool",
        "result:opencode_non_text_part": "an OpenCode part kind",
    },
};

/** Both harnesses decode invalid UTF-8 tool output to U+FFFD before storing it. */
const REPLACEMENT = "\uFFFD";

function parse<T>(text: string, at: string): T {
    try {
        return JSON.parse(text) as T;
    } catch (error) {
        throw new Error(`${at}: not JSON: ${String(error)}`);
    }
}

interface Witness {
    readonly cell: Cell;
    readonly at: string;
}

function textCells(text: string, at: string, out: Witness[]): void {
    out.push({ cell: "result:text", at });
    if (text.includes(CANARY)) out.push({ cell: "result:canary", at });
    if (text.includes(REPLACEMENT)) out.push({ cell: "result:invalid_utf8", at });
    if (Buffer.byteLength(text) > 64 * 1024) out.push({ cell: "result:over_64_kib", at });
    try {
        const value = JSON.parse(text) as unknown;
        if (value !== null && typeof value === "object") out.push({ cell: "result:json_text", at });
    } catch {
        // Not JSON text.
    }
}

const OPENCODE_TOOLS: Record<string, Cell> = {
    read: "tool:read",
    bash: "tool:bash",
    edit: "tool:edit",
    write: "tool:write",
    grep: "tool:grep",
    glob: "tool:glob",
    todowrite: "tool:todowrite",
    task: "tool:subagent",
    corpus_probe: "tool:mcp",
};

/** The witness of every cell the frozen OpenCode fixture holds. */
export function openCodeWitnesses(fixture: string): Witness[] {
    const db = new Database(":memory:");
    const sessionId = loadFrozenOpenCode(db, fixture);
    const rows = db
        .prepare(
            "SELECT m.id AS message, p.id AS part, p.data AS data FROM part p JOIN message m ON p.message_id = m.id WHERE m.session_id = ? ORDER BY m.time_created, p.id",
        )
        .all(sessionId) as { message: string; part: string; data: string }[];
    db.close();
    const out: Witness[] = [];
    for (const row of rows) {
        const at = `${row.message}/${row.part}`;
        const part = parse<{
            type: string;
            tool?: string;
            state?: { output?: string; error?: string };
        }>(row.data, at);
        if (part.type === "file") out.push({ cell: "result:opencode_non_text_part", at });
        if (part.type !== "tool") continue;
        const tool = OPENCODE_TOOLS[part.tool ?? ""];
        if (tool) out.push({ cell: tool, at });
        const output = part.state?.output ?? part.state?.error ?? "";
        if (output.includes("User aborted the command")) out.push({ cell: "tool:aborted", at });
        textCells(output, at, out);
    }
    return out;
}

const PI_TOOLS: Record<string, Cell> = {
    read: "tool:read",
    bash: "tool:bash",
    edit: "tool:edit",
    write: "tool:write",
    grep: "tool:grep",
};

/** The witness of every cell the frozen Pi fixture holds. */
export function piWitnesses(fixture: string): Witness[] {
    const frozen = parse<{
        entries: {
            id: string;
            type: string;
            message?: {
                role: string;
                toolName?: string;
                isError?: boolean;
                content: { type: string; text?: string }[] | string;
            };
        }[];
    }>(fixture, "fixture");
    const out: Witness[] = [];
    for (const entry of frozen.entries) {
        const message = entry.message;
        if (entry.type !== "message" || message?.role !== "toolResult") continue;
        const items = Array.isArray(message.content) ? message.content : [];
        const tool = PI_TOOLS[message.toolName ?? ""];
        if (tool) out.push({ cell: tool, at: entry.id });
        if (items.some((item) => item.type === "image")) {
            out.push({ cell: "result:pi_image_item", at: entry.id });
        }
        const text = items.map((item) => item.text ?? "").join("");
        if (message.isError && / aborted$/.test(text))
            out.push({ cell: "tool:aborted", at: entry.id });
        textCells(text, entry.id, out);
    }
    return out;
}

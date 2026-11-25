import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai/compat";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { ReferenceableAllowance } from "@eidnara/opencode/hooks/context/transform-capture";

import { createTestAgentSession, type TestAgentSession } from "../__tests__/agent-session";
import { encodePiRowsToCk } from "./pi-ck";
import { entryRow } from "./pi-transform";

type Json = Record<string, unknown>;

const FAILURES_RS_REFUSAL_HEADING = 'Validation failed for tool "';
const FAILURES_RS_EXIT_LINE = "Command exited with code ";

let root: string;
let harness: TestAgentSession | undefined;

beforeEach(() => {
    root = mkdtempSync(join(tmpdir(), "eidnara-pi-results-"));
});

afterEach(async () => {
    await harness?.dispose();
    harness = undefined;
    rmSync(root, { recursive: true, force: true });
});

function results(session: TestAgentSession): Map<string, SessionEntry> {
    const found = new Map<string, SessionEntry>();
    for (const entry of session.sessionManager.getEntries()) {
        const message = (entry as unknown as { message?: Json }).message;
        if (entry.type === "message" && message?.role === "toolResult")
            found.set(String(message.toolCallId), entry);
    }
    return found;
}

function textOf(entry: SessionEntry | undefined): string {
    const content = ((entry as unknown as { message: Json } | undefined)?.message.content ??
        []) as { text?: string }[];
    return content.map((part) => part.text ?? "").join("");
}

describe("the tool results Pi writes", () => {
    it("mark a refused call and a nonzero exit with the texts `begins_refusal` and `EXIT_LINE` of failures.rs read", async () => {
        harness = await createTestAgentSession({
            cwd: root,
            extensionFactories: [],
            tools: ["bash"],
        });
        harness.respond([
            fauxAssistantMessage(
                fauxToolCall("bash", { command: "true", timeout: "soon" }, { id: "call-refused" }),
            ),
            fauxAssistantMessage(
                fauxToolCall("bash", { command: "echo boom; exit 3" }, { id: "call-failed" }),
            ),
            fauxAssistantMessage(fauxToolCall("bash", { command: "echo fine" }, { id: "call-ok" })),
            fauxAssistantMessage("done"),
        ]);
        await harness.session.prompt("Run the commands.");
        const found = results(harness);
        const mark = (id: string) =>
            (found.get(id) as unknown as { message: Json } | undefined)?.message.isError;
        expect([mark("call-refused"), mark("call-failed"), mark("call-ok")]).toEqual([
            true,
            true,
            false,
        ]);
        const refused = textOf(found.get("call-refused"));
        expect(refused.startsWith(`${FAILURES_RS_REFUSAL_HEADING}bash":\n`)).toBe(true);
        expect(refused).toContain('"timeout": "soon"');
        const failed = textOf(found.get("call-failed")).split("\n");
        expect(failed.at(0)).toBe("boom");
        expect(failed.slice(-2)).toEqual(["", `${FAILURES_RS_EXIT_LINE}3`]);

        const allowance = new ReferenceableAllowance();
        const reader = {
            getLeafId: () => harness?.sessionManager.getLeafId(),
            getEntry: (id: string) => harness?.sessionManager.getEntry(id),
        };
        for (const [call, entry] of found) {
            const cloned = structuredClone((entry as unknown as { message: Json }).message);
            const row = entryRow(entry.id, cloned, reader, allowance);
            expect([call, row.type, row.parentId]).toEqual([call, "message", entry.parentId]);
            const [encoded] = encodePiRowsToCk([row]);
            const output = ((encoded?.ck.content as Json[])[0]?.kind as Json).output as Json;
            expect([call, (output.kind as Json).type]).toEqual([
                call,
                call === "call-ok" ? "text" : "error_text",
            ]);
        }
    });
});

import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { validateToolArguments } from "@earendil-works/pi-ai";
import cap from "@eidnara/opencode/hooks/context/__fixtures__/window-cap.json";
import { Type } from "typebox";

import { encodePiRowsToCk, type PiRow, piRowSize } from "./pi-ck";

const fixture = JSON.parse(
    readFileSync(join(import.meta.dir, "__fixtures__", "pi-codec-parity.json"), "utf8"),
) as { rows: PiRow[]; expected: unknown[] };

describe("Pi CK encoder", () => {
    it("produces the CK the daemon's Pi codec decodes from the same rows", () => {
        const encoded = encodePiRowsToCk(fixture.rows);
        expect(encoded).toHaveLength(fixture.expected.length);
        for (const [index, message] of encoded.entries())
            expect(message, fixture.rows[index]?.id).toStrictEqual(
                fixture.expected[index] as never,
            );
    });

    it("covers every role of the closed set", () => {
        const roles = new Set(fixture.rows.map((row) => row.message.role));
        for (const role of [
            "user",
            "assistant",
            "toolResult",
            "bashExecution",
            "custom",
            "branchSummary",
            "compactionSummary",
        ])
            expect(roles.has(role)).toBe(true);
    });

    it("refuses a role outside the closed set", () => {
        expect(() =>
            encodePiRowsToCk([{ id: "a1", message: { role: "systemNotice", content: "x" } }]),
        ).toThrow("systemNotice");
    });

    it("sends Pi's report of refused arguments as a marked result under its heading", () => {
        const tool = {
            name: "bash",
            description: "Runs a command line.",
            parameters: Type.Object({
                command: Type.String(),
                timeout: Type.Optional(Type.Number()),
            }),
        };
        let report = "";
        try {
            validateToolArguments(tool, {
                type: "toolCall",
                id: "call-1",
                name: "bash",
                arguments: { command: "make", timeout: "soon" },
            });
        } catch (error) {
            report = (error as Error).message;
        }
        // The daemon withholds the text of a marked result that begins with this heading
        // (`begins_refusal` in `crates/daemon/src/memory_capture/failures.rs`).
        expect(report.startsWith('Validation failed for tool "bash":\n')).toBe(true);
        expect(report).toContain('"command": "make"');
        const [message] = encodePiRowsToCk([
            {
                id: "e1",
                type: "message",
                parentId: null,
                message: {
                    role: "toolResult",
                    toolCallId: "call-1",
                    toolName: "bash",
                    content: [{ type: "text", text: report }],
                    isError: true,
                    timestamp: 1,
                },
            },
        ]);
        expect(message?.ck.content).toEqual([
            {
                kind: {
                    type: "tool_result",
                    id: "call-1",
                    tool_name: "bash",
                    output: { kind: { type: "error_text", text: report } },
                },
            },
        ]);
    });
});

describe("Pi window-cap size", () => {
    it("counts the daemon's blocks and bounds its bytes for every parity row", () => {
        for (const [index, parity] of cap.parity.pi.entries()) {
            const size = piRowSize(parity.message as PiRow);
            expect({ index, blocks: size?.blocks }).toEqual({ index, blocks: parity.blocks });
            expect(size?.bytes ?? -1).toBeGreaterThanOrEqual(parity.bytes);
        }
    });

    it("has no size for a row outside the closed role set", () => {
        expect(piRowSize({ id: "a1", message: { role: "system", content: "x" } })).toBeUndefined();
    });
});

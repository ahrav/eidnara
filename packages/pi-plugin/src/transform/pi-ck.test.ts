import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import cap from "@eidnara/opencode/hooks/context/__fixtures__/window-cap.json";

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

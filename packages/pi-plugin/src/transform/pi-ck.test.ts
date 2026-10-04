import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { encodePiRowsToCk, type PiRow } from "./pi-ck";

const fixture = JSON.parse(
    readFileSync(join(import.meta.dir, "__fixtures__", "pi-ck-parity.json"), "utf8"),
) as { rows: PiRow[]; expected: unknown[] };

describe("Pi CK encoder", () => {
    it("produces the CK the daemon's Pi codec decodes from the same rows", () => {
        const encoded = encodePiRowsToCk(fixture.rows);
        expect(encoded).toHaveLength(fixture.expected.length);
        for (const [index, message] of encoded.entries())
            expect(message, fixture.rows[index]?.id).toEqual(fixture.expected[index] as never);
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

import { describe, expect, it } from "bun:test";
import { isCompactionEnabled } from "./agent-disable";
import { EidnaraConfigSchema } from "./schema/eidnara";

describe("compaction config back-compat (issue #266 S1)", () => {
    it("defaults compaction.enabled on for absent and empty blocks and honors explicit values", () => {
        const cases: Array<[string, Record<string, unknown>, boolean]> = [
            ["{}", {}, true],
            ["no compaction block", { memory: { enabled: false } }, true],
            ["{ compaction: {} }", { compaction: {} }, true],
            ["explicit true", { compaction: { enabled: true } }, true],
            ["explicit user-tier false", { compaction: { enabled: false } }, false],
        ];
        for (const [title, raw, enabled] of cases) {
            const parsed = EidnaraConfigSchema.parse(raw);
            expect([title, parsed.compaction.enabled]).toEqual([title, enabled]);
            expect([title, isCompactionEnabled(parsed)]).toEqual([title, enabled]);
        }
    });

    it("absent block is byte-identical to default block for compaction (no behavior gated yet)", () => {
        const absent = EidnaraConfigSchema.parse({ memory: { enabled: true } });
        const empty = EidnaraConfigSchema.parse({ compaction: {} });
        expect(absent.compaction).toEqual(empty.compaction);
    });
});

import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { budgetPressure, tierAt, tierWindows } from "./decay-oracle";

const GOLDEN = resolve(
    import.meta.dir,
    "../../../../crates/context-core/testdata/decay-golden.json",
);

interface Golden {
    tier_cases: {
        index: number;
        importance: number;
        pressure: number;
        rendered: number;
    }[];
    pressure_cases: { importances: number[]; budget: number; one_pass: number }[];
}

describe("decay oracle", () => {
    it("agrees with the decay ladder's cross-language golden vectors", () => {
        const golden = JSON.parse(readFileSync(GOLDEN, "utf8")) as Golden;
        expect(golden.tier_cases.length).toBeGreaterThan(0);
        expect(golden.pressure_cases.length).toBeGreaterThan(0);
        for (const c of golden.tier_cases) {
            const served: string = tierAt(c.index, c.importance, c.pressure);
            expect([c, served]).toEqual([c, `p${c.rendered}`]);
        }
        for (const c of golden.pressure_cases) {
            expect(budgetPressure(c.importances, c.budget)).toBeCloseTo(c.one_pass, 9);
        }
    });

    it("partitions newer-row counts into consecutive tier windows", () => {
        const windows = [...tierWindows(70, 13, 30, 562)];
        expect(windows.map(([tier]) => tier)).toEqual(["p1", "p2", "p3", "p4", "p5"]);
        for (let i = 1; i < windows.length; i += 1) {
            expect(windows[i]?.[1].first).toBe((windows[i - 1]?.[1].last ?? 0) + 1);
        }
    });
});

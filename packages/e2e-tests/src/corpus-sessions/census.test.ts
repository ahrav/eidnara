import { Database } from "bun:sqlite";
import { describe, expect, it } from "bun:test";
import { join } from "node:path";
import { SOURCE_CORPUS_DIR } from "../source-corpus/corpus.ts";

import { readOpenCodeSession } from "../source-corpus/opencode-db-reader.ts";
import { readPiSession } from "../source-corpus/pi-jsonl-reader.ts";
import { CANNOT_PRODUCE, openCodeWitnesses, piWitnesses, R1_CELLS } from "./census.ts";
import { frozenPiJsonl, loadFrozenOpenCode, OPENCODE_CLOSURE, PI_CLOSURE } from "./freeze.ts";

const { readFileSync } = process.getBuiltinModule("node:fs");

const fixture = (closure: string) =>
    readFileSync(join(SOURCE_CORPUS_DIR, "sessions", closure, "mix.json"), "utf8");

describe("frozen session census", () => {
    for (const [harness, closure, witnesses] of [
        ["opencode", OPENCODE_CLOSURE, openCodeWitnesses],
        ["pi", PI_CLOSURE, piWitnesses],
    ] as const) {
        it(`witnesses every R1 cell ${harness} can produce and names the rest`, () => {
            const found = new Set(witnesses(fixture(closure)).map((witness) => witness.cell));
            const cannot = CANNOT_PRODUCE[harness];
            for (const cell of R1_CELLS) {
                if (cannot[cell]) expect(found.has(cell)).toBe(false);
                else
                    expect({ cell, witnessed: found.has(cell) }).toEqual({ cell, witnessed: true });
            }
        });
    }

    it("reads the totals the Rust Pi reader pins from the frozen Pi session", () => {
        const records = readPiSession(frozenPiJsonl(fixture(PI_CLOSURE)));
        const units = records.flatMap((record) => record.units);
        expect([records.length, units.length, units.filter((unit) => unit.tool).length]).toEqual([
            57, 39, 12,
        ]);
        expect(
            records
                .filter((record) => record.nonUnits.some((n) => n.reason === "non_text_item"))
                .map((record) => record.id),
        ).toEqual(["e0041"]);
    });

    it("lists each witness record in the fixture README census", () => {
        const readme = readFileSync(join(SOURCE_CORPUS_DIR, "README.md"), "utf8");
        const start = readme.indexOf("Census of the R1 cells");
        const census = readme.slice(start, readme.indexOf("OQ9 observations", start));
        const opencode = openCodeWitnesses(fixture(OPENCODE_CLOSURE));
        const pi = piWitnesses(fixture(PI_CLOSURE));
        for (const cell of R1_CELLS) {
            const first = opencode.find((witness) => witness.cell === cell);
            if (first) expect(census).toContain(`\`${first.at.split("/")[0]}\``);
            const piFirst = pi.find((witness) => witness.cell === cell);
            if (piFirst) expect(census).toContain(`\`${piFirst.at}\``);
        }
    });

    it("reads the totals the Rust SQL reader pins from the frozen OpenCode session", () => {
        const db = new Database(":memory:");
        const records = readOpenCodeSession(db, loadFrozenOpenCode(db, fixture(OPENCODE_CLOSURE)));
        const units = records.flatMap((record) => record.units);
        const nonUnits = records.flatMap((record) => record.nonUnits);
        expect([records.length, units.length, units.filter((unit) => unit.tool).length]).toEqual([
            42, 42, 13,
        ]);
        expect([nonUnits.length, nonUnits.filter((n) => n.kind === "file").length]).toEqual([
            56, 1,
        ]);
    });

    it("names each cannot-produce cell in the fixture README", () => {
        const readme = readFileSync(join(SOURCE_CORPUS_DIR, "README.md"), "utf8");
        for (const reasons of Object.values(CANNOT_PRODUCE)) {
            for (const reason of Object.values(reasons)) expect(readme).toContain(reason);
        }
    });
});

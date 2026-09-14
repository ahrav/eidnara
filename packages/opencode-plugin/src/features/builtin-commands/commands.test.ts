import { describe, expect, it } from "bun:test";
import { getEidnaraBuiltinCommands } from "./commands";

const EXPECTED_KEYS = [
    "eidnara-status",
    "eidnara-recomp",
    "eidnara-wrapup",
    "eidnara-flush",
    "eidnara-aug",
    "eidnara-memory-mark",
];
const UNAVAILABLE = "Unavailable when compaction.enabled is false";

describe("getEidnaraBuiltinCommands", () => {
    it("exposes exactly the six built-in commands", () => {
        expect(Object.keys(getEidnaraBuiltinCommands()).sort()).toEqual([...EXPECTED_KEYS].sort());
        expect(Object.keys(getEidnaraBuiltinCommands(false)).sort()).toEqual(
            [...EXPECTED_KEYS].sort(),
        );
    });

    it("gives every command a template matching its key", () => {
        for (const [key, command] of Object.entries(getEidnaraBuiltinCommands())) {
            expect(command.template).toBe(key);
        }
    });

    it("marks only the compacted-history commands unavailable when compaction is off", () => {
        const commands = getEidnaraBuiltinCommands(false);
        for (const key of ["eidnara-recomp", "eidnara-wrapup", "eidnara-flush"]) {
            expect(commands[key]?.description).toContain(UNAVAILABLE);
            expect(commands[key]?.description).toContain(`/${key}`);
        }
        for (const key of ["eidnara-status", "eidnara-aug", "eidnara-memory-mark"]) {
            expect(commands[key]?.description).not.toContain(UNAVAILABLE);
        }
    });

    it("uses the regular descriptions when compaction is on", () => {
        for (const command of Object.values(getEidnaraBuiltinCommands(true))) {
            expect(command.description).not.toContain(UNAVAILABLE);
        }
    });

    it("does not advertise a message range for /eidnara-recomp", () => {
        const description = getEidnaraBuiltinCommands(true)["eidnara-recomp"]?.description ?? "";
        expect(description).not.toContain("<start>-<end>");
        expect(description).not.toMatch(/range/i);
    });
});

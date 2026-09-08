import { describe, expect, it } from "bun:test";
import { getEidnaraBuiltinCommands } from "./commands";

const EXPECTED_KEYS = ["ctx-status", "ctx-recomp", "ctx-wrapup", "ctx-flush", "ctx-aug"];
const UNAVAILABLE = "Unavailable when compaction.enabled is false";

describe("getEidnaraBuiltinCommands", () => {
    it("exposes exactly the five built-in commands", () => {
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
        for (const key of ["ctx-recomp", "ctx-wrapup", "ctx-flush"]) {
            expect(commands[key]?.description).toContain(UNAVAILABLE);
            expect(commands[key]?.description).toContain(`/${key}`);
        }
        for (const key of ["ctx-status", "ctx-aug"]) {
            expect(commands[key]?.description).not.toContain(UNAVAILABLE);
        }
    });

    it("uses the regular descriptions when compaction is on", () => {
        for (const command of Object.values(getEidnaraBuiltinCommands(true))) {
            expect(command.description).not.toContain(UNAVAILABLE);
        }
    });

    it("does not advertise a message range for /ctx-recomp", () => {
        const description = getEidnaraBuiltinCommands(true)["ctx-recomp"]?.description ?? "";
        expect(description).not.toContain("<start>-<end>");
        expect(description).not.toMatch(/range/i);
    });
});

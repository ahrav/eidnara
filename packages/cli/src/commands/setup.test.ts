import { describe, expect, it } from "bun:test";
import { unknownSetupArguments } from "./setup";

describe("setup argument validation", () => {
    it("accepts the supported flags and the harness value", () => {
        expect(unknownSetupArguments([])).toEqual([]);
        expect(unknownSetupArguments(["--dry-run"])).toEqual([]);
        expect(unknownSetupArguments(["--harness", "pi", "--dry-run"])).toEqual([]);
    });

    it("reports misspelled or unsupported flags instead of ignoring them", () => {
        expect(unknownSetupArguments(["--dryrun"])).toEqual(["--dryrun"]);
        expect(unknownSetupArguments(["--dry-run=true"])).toEqual(["--dry-run=true"]);
        expect(unknownSetupArguments(["--harness=pi"])).toEqual(["--harness=pi"]);
        expect(unknownSetupArguments(["--harness", "pi", "extra"])).toEqual(["extra"]);
    });

    it("reports a repeated --harness instead of silently using the first", () => {
        expect(unknownSetupArguments(["--harness", "pi", "--harness", "omp"])).toEqual([
            "--harness omp (repeated)",
        ]);
        expect(unknownSetupArguments(["--harness", "pi", "--harness"])).toEqual([
            "--harness (repeated)",
        ]);
    });

    it("leaves a flag-shaped harness value for the harness parser to reject", () => {
        expect(unknownSetupArguments(["--harness", "--dry-run"])).toEqual([]);
        expect(unknownSetupArguments(["--harness"])).toEqual([]);
    });
});

import { describe, expect, it } from "bun:test";
import { parseArgs, selectedTestFiles, UsageError } from "./run-test-selection";

describe("test selection arguments", () => {
    it("accepts --mode rust with timeout and concurrency", () => {
        expect(
            parseArgs(["--mode", "rust", "--timeout", "600000", "--max-concurrency", "1"]),
        ).toEqual({
            mode: "rust",
            timeoutMs: 600_000,
            maxConcurrency: 1,
        });
        expect(parseArgs(["--mode", "rust"])).toEqual({
            mode: "rust",
            timeoutMs: 120_000,
            maxConcurrency: null,
        });
    });

    it("rejects every mode other than rust as a usage error", () => {
        for (const value of ["ts", "pi", "", "RUST"]) {
            expect(() => parseArgs(["--mode", value])).toThrow(UsageError);
            expect(() => parseArgs(["--mode", value])).toThrow(/--mode accepts only rust/);
        }
        expect(() => parseArgs([])).toThrow(/--mode rust is required/);
        expect(() => parseArgs(["--harness", "all", "--mode", "rust"])).toThrow(/unknown argument/);
    });

    it("rejects non-positive timeout and concurrency", () => {
        expect(() => parseArgs(["--mode", "rust", "--timeout", "0"])).toThrow(UsageError);
        expect(() => parseArgs(["--mode", "rust", "--max-concurrency", "x"])).toThrow(UsageError);
    });

    it("selects exactly the manifest's files", () => {
        const files = selectedTestFiles("rust");
        expect(files.length).toBe(9);
        expect(files).toEqual([...files].sort());
        expect(files.every((file) => file.startsWith("tests/") && file.endsWith(".test.ts"))).toBe(
            true,
        );
    });
});

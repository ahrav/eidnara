import { describe, expect, it } from "bun:test";
import { standaloneVersion } from "./semver";

describe("standaloneVersion", () => {
    it("returns the version from a line that is only a version", () => {
        expect(standaloneVersion("0.74.0")).toBe("0.74.0");
        expect(standaloneVersion("v1.18.0\n")).toBe("1.18.0");
        expect(standaloneVersion("  2.0.0-beta.1  ")).toBe("2.0.0-beta.1");
    });

    it("skips warning lines that mention another tool's version", () => {
        expect(standaloneVersion("Node 24.15.0 is deprecated\n0.70.0")).toBe("0.70.0");
        expect(standaloneVersion("warning: config at /home/alice token=abc123\n0.74.0")).toBe(
            "0.74.0",
        );
    });

    it("accepts an allowed prefix such as omp/", () => {
        expect(standaloneVersion("omp/17.0.0", "omp/")).toBe("17.0.0");
        expect(standaloneVersion("17.0.0", "omp/")).toBe("17.0.0");
        expect(standaloneVersion("omp/17.0.0")).toBeNull();
    });

    it("returns null when no line is a bare version", () => {
        expect(standaloneVersion(null)).toBeNull();
        expect(standaloneVersion("")).toBeNull();
        expect(standaloneVersion("Error: requires node 22.1.0 or newer")).toBeNull();
        expect(standaloneVersion("pi 0.74.0")).toBeNull();
    });
});

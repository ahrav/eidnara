import { describe, expect, it } from "bun:test";
import { compareVersionStrings } from "./version";

describe("compareVersionStrings", () => {
    it("orders semver triples", () => {
        expect(compareVersionStrings("1.14.9", "1.15.0")).toBe(-1);
        expect(compareVersionStrings("1.15.0", "1.15.0")).toBe(0);
        expect(compareVersionStrings("2.0.0", "1.99.99")).toBe(1);
    });

    it("reads the first triple out of a version banner", () => {
        expect(compareVersionStrings("opencode 1.18.22 (abc123)", "1.15.0")).toBe(1);
        expect(compareVersionStrings("v1.15.0 (abc123)", "1.15.0")).toBe(0);
    });

    it("orders a prerelease below its release", () => {
        expect(compareVersionStrings("1.15.0-beta.1", "1.15.0")).toBe(-1);
        expect(compareVersionStrings("1.15.0", "1.15.0-rc.2")).toBe(1);
        expect(compareVersionStrings("1.15.0-beta.1", "1.15.0-beta.2")).toBe(-1);
        expect(compareVersionStrings("1.15.1-beta.1", "1.15.0")).toBe(1);
    });

    it("treats an unparsable side as equal so it never blocks", () => {
        expect(compareVersionStrings("dev", "1.15.0")).toBe(0);
        expect(compareVersionStrings("1.15.0", "")).toBe(0);
    });
});

import { describe, expect, it } from "bun:test";
import {
    filesForMode,
    type ModeManifest,
    validateManifestDocument,
    validateModeManifest,
    validateTestSource,
} from "./validate-mode-manifest";

const validation = validateModeManifest();

function manifestWith(entries: ModeManifest["entries"]): ModeManifest {
    return {
        schema: 1,
        header: "test manifest",
        entries,
    };
}

const VALID_TEST_SOURCE = 'import { it } from "bun:test";\nit("x", () => {});\n';

describe("mode manifest validator", () => {
    it("covers every live e2e test exactly once", () => {
        expect(validation.files.length).toBe(18);
        expect(validation.manifest.entries).toHaveLength(validation.files.length);
        expect(new Set(validation.manifest.entries.map((entry) => entry.path)).size).toBe(
            validation.files.length,
        );
        expect(validation.manifest.entries.map((entry) => entry.path).sort()).toEqual(
            validation.files,
        );
    });

    it("selects every entry for rust; only the Pi smoke entry is pi-smoke", () => {
        expect(filesForMode(validation, "rust")).toEqual(validation.files);
        const tiers = new Map(validation.manifest.entries.map((entry) => [entry.path, entry.tier]));
        expect(tiers.get("tests/pi-smoke.test.ts")).toBe("pi-smoke");
        tiers.delete("tests/pi-smoke.test.ts");
        expect([...tiers.values()].every((tier) => tier === "rust-only")).toBe(true);
    });

    it("fails when a test file lacks an entry", () => {
        const entries = validation.manifest.entries;
        expect(() =>
            validateManifestDocument(manifestWith(entries.slice(0, -1)), validation.files),
        ).toThrow(/missing manifest entries: tests\/thinking-block-safety\.test\.ts/);
    });

    it("rejects a duplicated or dead manifest path", () => {
        const entries = validation.manifest.entries;
        expect(() =>
            validateManifestDocument(manifestWith([...entries, entries[0]!]), validation.files),
        ).toThrow(/duplicate manifest entry/);
        expect(() =>
            validateManifestDocument(
                manifestWith([
                    ...entries.slice(0, -1),
                    { ...entries.at(-1)!, path: "tests/not-live.test.ts" },
                ]),
                validation.files,
            ),
        ).toThrow(/dead or out-of-scope/);
    });

    it("rejects unknown tiers and unknown entry fields", () => {
        const entries = validation.manifest.entries;
        expect(() =>
            validateManifestDocument(
                manifestWith([
                    { ...entries[0]!, tier: "both-modes" as never },
                    ...entries.slice(1),
                ]),
                validation.files,
            ),
        ).toThrow(/invalid classification/);
        expect(() =>
            validateManifestDocument(
                manifestWith([
                    { ...entries[0]!, invocation: { rust: true } } as never,
                    ...entries.slice(1),
                ]),
                validation.files,
            ),
        ).toThrow(/must contain exactly path, tier, rationale, contract_refs/);
        expect(() =>
            validateManifestDocument(
                manifestWith([{ ...entries[0]!, contract_refs: [] }, ...entries.slice(1)]),
                validation.files,
            ),
        ).toThrow(/non-empty string-array contract_refs/);
    });

    it("parses each entry's source as a bun:test module", () => {
        const entries = validation.manifest.entries;
        expect(() =>
            validateManifestDocument(manifestWith(entries), validation.files, (path) =>
                path === entries[0]!.path ? "describe(" : VALID_TEST_SOURCE,
            ),
        ).toThrow(/does not parse as TypeScript/);
        expect(() =>
            validateManifestDocument(manifestWith(entries), validation.files, (path) =>
                path === entries[0]!.path ? 'import { x } from "./x";\n' : VALID_TEST_SOURCE,
            ),
        ).toThrow(/does not import bun:test/);
        expect(() => validateTestSource("tests/ok.test.ts", VALID_TEST_SOURCE)).not.toThrow();
    });
});

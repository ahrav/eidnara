import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { projectModeOverrides, readEidnaraModes } from "./eidnara-modes";

const roots: string[] = [];
afterEach(() => {
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function write(name: string, body: string): string {
    const root = mkdtempSync(join(tmpdir(), "eidnara-modes-"));
    roots.push(root);
    const path = join(root, name);
    writeFileSync(path, body);
    return path;
}

describe("readEidnaraModes", () => {
    it("derives every mode from the shared config and defaults to enabled", () => {
        expect(readEidnaraModes(join(tmpdir(), "missing-eidnara.jsonc"))).toEqual({
            enabled: true,
            compactionEnabled: true,
            memoryEnabled: true,
        });
        expect(readEidnaraModes(write("a.jsonc", `{"compaction":{"enabled":false}}`))).toEqual({
            enabled: true,
            compactionEnabled: false,
            memoryEnabled: true,
        });
        expect(readEidnaraModes(write("b.jsonc", `{"enabled":false}`))).toEqual({
            enabled: false,
            compactionEnabled: false,
            memoryEnabled: false,
        });
    });
});

describe("projectModeOverrides", () => {
    const shared = { enabled: true, compactionEnabled: false, memoryEnabled: true };

    it("reports enabled, compaction, and memory disagreements", () => {
        const project = write(
            "p.jsonc",
            `{"enabled":false,"compaction":{"enabled":true},"memory":{"enabled":false}}`,
        );
        expect(projectModeOverrides(project, shared)).toEqual([
            "enabled: false",
            "compaction.enabled: true",
            "memory.enabled: false",
        ]);
    });

    it("stays silent when the project agrees or says nothing", () => {
        expect(
            projectModeOverrides(write("q.jsonc", `{"compaction":{"enabled":false}}`), shared),
        ).toEqual([]);
        expect(projectModeOverrides(join(tmpdir(), "missing.jsonc"), shared)).toEqual([]);
    });
});

import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    compactionEnabledWithSummarizer,
    projectModeOverrides,
    readEidnaraModes,
} from "./eidnara-modes";

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
    const admitted = { status: "admitted" } as const;
    const model = `"history_summarizer":{"model":"anthropic/claude-haiku-4-5"}`;

    it("derives every mode from the shared config through the plugin loader", () => {
        expect(readEidnaraModes(join(tmpdir(), "missing-eidnara.jsonc"))).toEqual({
            enabled: true,
            compactionEnabled: false,
            memoryEnabled: true,
            admission: admitted,
        });
        expect(readEidnaraModes(write("m.jsonc", `{${model}}`))).toEqual({
            enabled: true,
            compactionEnabled: true,
            memoryEnabled: true,
            admission: admitted,
        });
        expect(
            readEidnaraModes(write("a.jsonc", `{"compaction":{"enabled":false},${model}}`)),
        ).toEqual({
            enabled: true,
            compactionEnabled: false,
            memoryEnabled: true,
            admission: admitted,
        });
        expect(readEidnaraModes(write("b.jsonc", `{"enabled":false,${model}}`))).toEqual({
            enabled: false,
            compactionEnabled: false,
            memoryEnabled: false,
            admission: admitted,
        });
    });

    it("forwards the summarizer chain after substitution and reference exclusion", () => {
        const previous = process.env.EIDNARA_MODES_MODEL;
        process.env.EIDNARA_MODES_MODEL = "anthropic/claude-haiku-4-5";
        try {
            const referenced = readEidnaraModes(
                write("r.jsonc", `{"history_summarizer":{"model":"{env:EIDNARA_MODES_MODEL}"}}`),
            );
            expect(referenced.compactionEnabled).toBe(false);
            expect(referenced.admission).toEqual(admitted);
        } finally {
            if (previous === undefined) delete process.env.EIDNARA_MODES_MODEL;
            else process.env.EIDNARA_MODES_MODEL = previous;
        }
    });

    it("reports a rejected user tier as unresolved", () => {
        for (const [name, body] of [
            ["malformed.jsonc", `{${model}`],
            ["unknown.jsonc", `{"history_summarizer":{"model":"a/b","modle":"c/d"}}`],
            ["string-flag.jsonc", `{"compaction":{"enabled":"false"},${model}}`],
        ] as const) {
            expect([name, readEidnaraModes(write(name, body)).admission.status]).toEqual([
                name,
                "unresolved",
            ]);
        }
    });
});

describe("compactionEnabledWithSummarizer", () => {
    const picked = "anthropic/claude-haiku-4-5";

    it("resolves the mode the shared config reaches once setup writes the picked summarizer", () => {
        expect(
            compactionEnabledWithSummarizer(join(tmpdir(), "missing-eidnara.jsonc"), picked),
        ).toBe(true);
        expect(
            compactionEnabledWithSummarizer(
                write("keep.jsonc", `{"memory":{"enabled":true}}`),
                picked,
            ),
        ).toBe(true);
        expect(
            compactionEnabledWithSummarizer(
                write("opt-out.jsonc", `{"compaction":{"enabled":false}}`),
                picked,
            ),
        ).toBe(false);
        expect(
            compactionEnabledWithSummarizer(write("off.jsonc", `{"enabled":false}`), picked),
        ).toBe(false);
    });

    it("keeps the configured chain when setup writes no summarizer", () => {
        expect(compactionEnabledWithSummarizer(write("none.jsonc", "{}"), null)).toBe(false);
    });
});

describe("projectModeOverrides", () => {
    const shared = {
        enabled: true,
        compactionEnabled: false,
        memoryEnabled: true,
        admission: { status: "admitted" } as const,
    };

    it("reports enabled and memory disagreements, not the stripped compaction mode", () => {
        const project = write(
            "p.jsonc",
            `{"enabled":false,"compaction":{"enabled":true},"memory":{"enabled":false}}`,
        );
        expect(projectModeOverrides(project, shared)).toEqual([
            "enabled: false",
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

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
            const modes = readEidnaraModes(write(name, body));
            expect([name, modes.admission.status, modes.compactionEnabled]).toEqual([
                name,
                "unresolved",
                false,
            ]);
        }
    });

    it("keeps a rejected tier's memory opt-out", () => {
        const modes = readEidnaraModes(
            write(
                "opt-out.jsonc",
                `{"memory":{"enabled":false},"compaction":{"enabled":"false"},${model}}`,
            ),
        );
        expect(modes).toEqual({
            enabled: true,
            compactionEnabled: false,
            memoryEnabled: false,
            admission: expect.objectContaining({ status: "unresolved" }),
        });
    });

    it("keeps the opt-outs of a tier the schema refuses for unknown keys", () => {
        for (const [name, body] of [
            [
                "authority-typo.jsonc",
                `{"enabled":false,"memory":{"enabled":false},"history_summarizer":{"model":"a/b","modle":"c/d"}}`,
            ],
            [
                "two-typos.jsonc",
                `{"enabled":false,"memroy":1,"memory":{"enabled":false,"auto_promot":true},"history_summarizer":{"modle":"c/d"}}`,
            ],
        ] as const) {
            expect([name, readEidnaraModes(write(name, body))]).toEqual([
                name,
                {
                    enabled: false,
                    compactionEnabled: false,
                    memoryEnabled: false,
                    admission: expect.objectContaining({ status: "unresolved" }),
                },
            ]);
        }
    });

    it("resolves the compaction mode setup produces once it writes the summarizer model", () => {
        const planned = "anthropic/claude-haiku-4-5";
        const missing = join(tmpdir(), "missing-eidnara.jsonc");
        expect(readEidnaraModes(missing, { summarizerModel: planned }).compactionEnabled).toBe(
            true,
        );
        expect(
            readEidnaraModes(write("empty.jsonc", "{}"), { summarizerModel: planned })
                .compactionEnabled,
        ).toBe(true);
        expect(
            readEidnaraModes(write("off.jsonc", `{"compaction":{"enabled":false}}`), {
                summarizerModel: planned,
            }).compactionEnabled,
        ).toBe(false);
        expect(
            readEidnaraModes(write("disabled.jsonc", `{"enabled":false}`), {
                summarizerModel: planned,
            }).compactionEnabled,
        ).toBe(false);
        expect(
            readEidnaraModes(write("unresolved.jsonc", `{"compaction":{"enabled":"x"}}`), {
                summarizerModel: planned,
            }).compactionEnabled,
        ).toBe(false);
        expect(
            readEidnaraModes(
                write("module.jsonc", `{"history_summarizer":{"module_model":"m/n"}}`),
                {
                    summarizerModel: planned,
                },
            ).compactionEnabled,
        ).toBe(true);
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

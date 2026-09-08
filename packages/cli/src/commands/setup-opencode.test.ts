import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    type ConflictResult,
    DCP_CONFLICT_REASON,
} from "@eidnara/opencode/shared/conflict-detector";
import { parse as parseJsonc } from "comment-json";
import { assertJsoncConfigsParseable } from "../lib/jsonc-config";
import {
    addPluginToOpenCodeConfig,
    addPluginToTuiConfig,
    assertPluginListShape,
    findDcpPluginIndexes,
    hasAnthropicModel,
    preflightConfigPaths,
    withClaudeMaxCacheTtl,
    withoutDcpConflict,
    writeEidnaraConfig,
} from "./setup-opencode";

const tempDirs: string[] = [];

function tempDir(): string {
    const path = mkdtempSync(join(tmpdir(), "eidnara-opencode-setup-"));
    tempDirs.push(path);
    return path;
}

afterEach(() => {
    for (const path of tempDirs.splice(0)) rmSync(path, { recursive: true, force: true });
});

describe("setup-opencode config safety", () => {
    it("leaves malformed existing config unchanged", () => {
        const path = join(tempDir(), "eidnara.jsonc");
        const malformed = `{\n  "historian": {\n`;
        writeFileSync(path, malformed);

        expect(() =>
            writeEidnaraConfig(path, {
                historianModel: "anthropic/claude-sonnet-4-6",
                sidekickEnabled: false,
                sidekickModel: null,
                claudeMax: false,
            }),
        ).toThrow(`Refusing to overwrite unparseable config ${path}`);
        expect(readFileSync(path, "utf-8")).toBe(malformed);
    });

    it("writes the schema URL, historian model, and sidekick block into a new config", () => {
        const path = join(tempDir(), "eidnara.jsonc");

        writeEidnaraConfig(path, {
            historianModel: "anthropic/claude-haiku-4-5",
            sidekickEnabled: true,
            sidekickModel: "openai/gpt-5-mini",
            claudeMax: false,
        });

        const written = parseJsonc(readFileSync(path, "utf-8")) as Record<string, unknown>;
        expect(written.$schema).toBe(
            "https://raw.githubusercontent.com/ahrav/eidnara/main/assets/eidnara.schema.json",
        );
        expect(written.historian).toEqual({ model: "anthropic/claude-haiku-4-5" });
        expect(written.sidekick).toEqual({ model: "openai/gpt-5-mini" });

        writeEidnaraConfig(path, {
            historianModel: "anthropic/claude-haiku-4-5",
            sidekickEnabled: true,
            sidekickModel: "openai/gpt-5-nano",
            claudeMax: false,
        });

        const rewritten = parseJsonc(readFileSync(path, "utf-8")) as Record<string, unknown>;
        expect(rewritten.$schema).toBe(written.$schema);
        expect(rewritten.sidekick).toEqual({ model: "openai/gpt-5-nano" });
    });

    it("lifts a scalar cache_ttl into the record default when Claude Max is selected", () => {
        const path = join(tempDir(), "eidnara.jsonc");
        writeFileSync(path, `{"cache_ttl":"10m","historian":{"model":"openai/gpt-5"}}`);

        writeEidnaraConfig(path, {
            historianModel: "anthropic/claude-haiku-4-5",
            sidekickEnabled: false,
            sidekickModel: null,
            claudeMax: true,
        });

        const written = parseJsonc(readFileSync(path, "utf-8")) as Record<string, unknown>;
        expect(written.cache_ttl).toEqual({
            default: "10m",
            "anthropic/claude-sonnet-4-6": "59m",
            "anthropic/claude-opus-4-6": "59m",
            "anthropic/claude-haiku-4-5": "59m",
        });
    });

    it("clears a historian opt-out when a historian model is chosen", () => {
        const path = join(tempDir(), "eidnara.jsonc");
        writeFileSync(
            path,
            `{"historian":{"disable":true,"enabled":false,"model":"old"},"sidekick":{"disable":true}}`,
        );

        writeEidnaraConfig(path, {
            historianModel: "anthropic/claude-haiku-4-5",
            sidekickEnabled: false,
            sidekickModel: null,
            claudeMax: false,
        });

        const written = parseJsonc(readFileSync(path, "utf-8")) as Record<string, unknown>;
        expect(written.historian).toEqual({ model: "anthropic/claude-haiku-4-5" });
        expect(written.sidekick).toEqual({ disable: true });
    });

    it("drops schema-invalid agent fields so the runtime keeps the block", () => {
        const path = join(tempDir(), "eidnara.jsonc");
        writeFileSync(
            path,
            `{"historian":{"temperature":"hot","top_p":0.5},"sidekick":{"color":"red","prompt":"keep"}}`,
        );

        writeEidnaraConfig(path, {
            historianModel: "anthropic/claude-haiku-4-5",
            sidekickEnabled: true,
            sidekickModel: "openai/gpt-5-mini",
            claudeMax: false,
        });

        const written = parseJsonc(readFileSync(path, "utf-8")) as Record<string, unknown>;
        expect(written.historian).toEqual({ model: "anthropic/claude-haiku-4-5", top_p: 0.5 });
        expect(written.sidekick).toEqual({ model: "openai/gpt-5-mini", prompt: "keep" });
    });

    it("replaces schema-invalid agent blocks instead of throwing on them", () => {
        const path = join(tempDir(), "eidnara.jsonc");
        writeFileSync(path, `{"historian":"old-model","sidekick":["stale"]}`);

        writeEidnaraConfig(path, {
            historianModel: "anthropic/claude-haiku-4-5",
            sidekickEnabled: true,
            sidekickModel: "openai/gpt-5-mini",
            claudeMax: false,
        });

        const written = parseJsonc(readFileSync(path, "utf-8")) as Record<string, unknown>;
        expect(written.historian).toEqual({ model: "anthropic/claude-haiku-4-5" });
        expect(written.sidekick).toEqual({ model: "openai/gpt-5-mini" });
    });

    it("normalizes every cache_ttl shape before adding the Claude Max overrides", () => {
        const overrides = {
            "anthropic/claude-sonnet-4-6": "59m",
            "anthropic/claude-opus-4-6": "59m",
        };
        expect(withClaudeMaxCacheTtl(undefined)).toEqual({ default: "5m", ...overrides });
        expect(withClaudeMaxCacheTtl("never")).toEqual({ default: "never", ...overrides });
        expect(withClaudeMaxCacheTtl({ "openai/gpt-5": "1h" })).toEqual({
            default: "5m",
            "openai/gpt-5": "1h",
            ...overrides,
        });
        expect(withClaudeMaxCacheTtl(["5m"])).toEqual({ default: "5m", ...overrides });
        // Non-string members would fail the schema for the whole record; they are dropped.
        expect(withClaudeMaxCacheTtl({ default: 10, "openai/gpt-5": 30, "x/y": "1h" })).toEqual({
            default: "5m",
            "x/y": "1h",
            ...overrides,
        });
    });

    it("extends the Claude Max overrides to the selected Anthropic models only", () => {
        expect(
            withClaudeMaxCacheTtl(undefined, ["anthropic/claude-haiku-4-5", "openai/gpt-5", null]),
        ).toEqual({
            default: "5m",
            "anthropic/claude-sonnet-4-6": "59m",
            "anthropic/claude-opus-4-6": "59m",
            "anthropic/claude-haiku-4-5": "59m",
        });
    });

    it("offers the Claude Max prompt for a manually entered Anthropic model", () => {
        expect(hasAnthropicModel([])).toBe(false);
        expect(hasAnthropicModel(["openai/gpt-5", null])).toBe(false);
        expect(hasAnthropicModel(["anthropic/claude-haiku-4-5"])).toBe(true);
        expect(hasAnthropicModel(["openai/gpt-5", "anthropic/claude-haiku-4-5", null])).toBe(true);
    });

    it("appends the bare plugin name once and leaves a second run unchanged", () => {
        const path = join(tempDir(), "opencode.jsonc");
        writeFileSync(path, `{"plugin":["other"]}`);

        addPluginToOpenCodeConfig(path, "jsonc");
        const afterFirst = readFileSync(path, "utf-8");
        expect(parseJsonc(afterFirst)).toMatchObject({
            plugin: ["other", "@eidnara/opencode"],
        });

        addPluginToOpenCodeConfig(path, "jsonc");
        expect(readFileSync(path, "utf-8")).toBe(afterFirst);
    });

    it("re-detects targets created after discovery and merges them", () => {
        const root = tempDir();
        const opencodePath = join(root, "opencode.jsonc");
        const tuiPath = join(root, "tui.jsonc");
        writeFileSync(opencodePath, `{"theme":"dark","plugin":["other"]}`);
        writeFileSync(tuiPath, `{"layout":"wide","plugin":["other-tui"]}`);

        // "none" is the stale pre-prompt detection result.
        addPluginToOpenCodeConfig(opencodePath, "none");
        addPluginToTuiConfig(tuiPath, "none");

        expect(parseJsonc(readFileSync(opencodePath, "utf-8"))).toMatchObject({
            theme: "dark",
            plugin: ["other", "@eidnara/opencode"],
        });
        expect(parseJsonc(readFileSync(tuiPath, "utf-8"))).toMatchObject({
            layout: "wide",
            plugin: ["other-tui", "@eidnara/opencode"],
        });
    });

    it("creates a missing config and merges a valid config", () => {
        const root = tempDir();
        const missingPath = join(root, "opencode.json");
        addPluginToOpenCodeConfig(missingPath, "none");
        expect(parseJsonc(readFileSync(missingPath, "utf-8"))).toMatchObject({
            compaction: { auto: false, prune: false },
        });

        const validPath = join(root, "existing.jsonc");
        writeFileSync(
            validPath,
            `{"theme":"dark","plugin":["other","@tarquinen/opencode-dcp@latest"]}`,
        );
        addPluginToOpenCodeConfig(validPath, "jsonc", true);
        const merged = parseJsonc(readFileSync(validPath, "utf-8")) as {
            theme?: string;
            plugin?: string[];
            compaction?: { auto?: boolean; prune?: boolean };
        };
        expect(merged).toMatchObject({
            theme: "dark",
            compaction: { auto: false, prune: false },
        });
        expect(merged.plugin).toContain("other");
        expect(merged.plugin).not.toContain("@tarquinen/opencode-dcp@latest");
    });
});

describe("setup-opencode preflight targets", () => {
    it("checks only the effective member of each project config pair", () => {
        const root = tempDir();
        mkdirSync(join(root, ".opencode"), { recursive: true });
        writeFileSync(join(root, ".opencode", "opencode.jsonc"), "{}");
        writeFileSync(join(root, ".opencode", "opencode.json"), "{ malformed");
        writeFileSync(join(root, "opencode.json"), "{}");
        const userPaths = {
            configDir: join(root, "user"),
            opencodeConfig: join(root, "user", "opencode.jsonc"),
            opencodeConfigFormat: "none" as const,
            eidnaraConfig: join(root, "user", "eidnara.jsonc"),
            omoConfig: null,
            tuiConfig: join(root, "user", "tui.jsonc"),
            tuiConfigFormat: "none" as const,
        };

        const targets = preflightConfigPaths(userPaths, root, { firstTimeOmoRepair: false });

        expect(targets).toContain(join(root, ".opencode", "opencode.jsonc"));
        expect(targets).not.toContain(join(root, ".opencode", "opencode.json"));
        expect(targets).toContain(join(root, "opencode.json"));
        expect(targets.slice(0, 3)).toEqual([
            userPaths.opencodeConfig,
            userPaths.eidnaraConfig,
            userPaths.tuiConfig,
        ]);
        expect(() => assertJsoncConfigsParseable(targets)).not.toThrow();
    });

    it("includes OMO configs only when the fixer can reach them", () => {
        const root = tempDir();
        mkdirSync(join(root, ".omo"), { recursive: true });
        writeFileSync(join(root, "oh-my-opencode.jsonc"), "{}");
        writeFileSync(join(root, ".omo", "omo.json"), "{ malformed");
        const userPaths = {
            configDir: join(root, "user"),
            opencodeConfig: join(root, "user", "opencode.jsonc"),
            opencodeConfigFormat: "none" as const,
            eidnaraConfig: join(root, "user", "eidnara.jsonc"),
            omoConfig: null,
            tuiConfig: join(root, "user", "tui.jsonc"),
            tuiConfigFormat: "none" as const,
        };

        // No OMO plugin entry and no first-time repair: the stale file is not a target.
        const unrelated = preflightConfigPaths(userPaths, root, { firstTimeOmoRepair: false });
        expect(unrelated).not.toContain(join(root, ".omo", "omo.json"));
        expect(() => assertJsoncConfigsParseable(unrelated)).not.toThrow();

        // The first-time branch edits OMO configs, so they are checked.
        const firstTime = preflightConfigPaths(userPaths, root, { firstTimeOmoRepair: true });
        expect(firstTime).toContain(join(root, "oh-my-opencode.jsonc"));
        expect(firstTime).toContain(join(root, ".omo", "omo.json"));
        expect(() => assertJsoncConfigsParseable(firstTime)).toThrow(/omo\.json/);

        // A project OMO plugin entry drives the conflict pass, so they are checked too.
        writeFileSync(join(root, "opencode.json"), `{"plugin":["oh-my-opencode"]}`);
        const withPlugin = preflightConfigPaths(userPaths, root, { firstTimeOmoRepair: false });
        expect(withPlugin).toContain(join(root, ".omo", "omo.json"));
    });
});

describe("setup-opencode plugin list shape", () => {
    it("refuses a scalar or object plugin value and accepts arrays or absence", () => {
        const root = tempDir();
        const scalar = join(root, "scalar.json");
        const object = join(root, "object.json");
        const array = join(root, "array.json");
        writeFileSync(scalar, `{"plugin":"@other/plugin"}`);
        writeFileSync(object, `{"plugin":{"name":"@other/plugin"}}`);
        writeFileSync(array, `{"plugin":["@other/plugin"]}`);

        expect(() => assertPluginListShape([scalar])).toThrow(/"plugin" must be an array/);
        expect(() => assertPluginListShape([object])).toThrow(/object\.json/);
        expect(() => assertPluginListShape([array, join(root, "missing.json")])).not.toThrow();
    });
});

describe("setup-opencode DCP preflight", () => {
    it("is tuple-safe and only matches canonical opencode-dcp entries", () => {
        const plugins: unknown[] = [
            ["@plannotator/opencode@latest", { workflow: "plan-agent" }],
            "@some-fork/opencode-dcp-fork",
            ["@tarquinen/opencode-dcp@latest", { enabled: true }],
            "file:///tmp/opencode-dcp-dev",
        ];

        expect(() => findDcpPluginIndexes(plugins)).not.toThrow();
        expect(findDcpPluginIndexes(plugins)).toEqual([2]);
    });

    it("keeps a retained DCP plugin out of the broader automatic-fix pass", () => {
        const detected: ConflictResult = {
            hasConflict: true,
            reasons: [
                "OpenCode auto-compaction is enabled (compaction.auto=true)",
                DCP_CONFLICT_REASON,
            ],
            conflicts: {
                compactionAuto: true,
                compactionPrune: false,
                dcpPlugin: true,
                omoPreemptiveCompaction: false,
                omoContextWindowMonitor: false,
                omoAnthropicRecovery: false,
            },
            nativeCompaction: { auto: true, prune: false },
        };

        const masked = withoutDcpConflict(detected);
        expect(masked.conflicts.dcpPlugin).toBe(false);
        expect(masked.conflicts.compactionAuto).toBe(true);
        expect(masked.reasons).toEqual([
            "OpenCode auto-compaction is enabled (compaction.auto=true)",
        ]);
        expect(masked.hasConflict).toBe(true);
        expect(detected.conflicts.dcpPlugin).toBe(true);

        const dcpOnly = withoutDcpConflict({
            ...detected,
            reasons: [DCP_CONFLICT_REASON],
            conflicts: { ...detected.conflicts, compactionAuto: false },
        });
        expect(dcpOnly.hasConflict).toBe(false);
        expect(dcpOnly.reasons).toEqual([]);
    });
});

describe("setup-opencode compaction-off writer (issue #266)", () => {
    it("skips the compaction.auto=false write when compactionEnabled=false", () => {
        const root = tempDir();
        const configPath = join(root, "opencode.jsonc");
        writeFileSync(configPath, JSON.stringify({ compaction: { auto: true, prune: true } }));

        addPluginToOpenCodeConfig(configPath, "jsonc", false, false);

        const merged = parseJsonc(readFileSync(configPath, "utf-8")) as {
            compaction?: { auto?: boolean; prune?: boolean };
        };
        expect(merged.compaction).toEqual({ auto: true, prune: true });
    });

    it("writes compaction.auto=false when compactionEnabled=true (default mode-on)", () => {
        const root = tempDir();
        const configPath = join(root, "opencode.jsonc");
        writeFileSync(configPath, JSON.stringify({ compaction: { auto: true, prune: true } }));

        addPluginToOpenCodeConfig(configPath, "jsonc", false, true);

        const merged = parseJsonc(readFileSync(configPath, "utf-8")) as {
            compaction?: { auto?: boolean; prune?: boolean };
        };
        expect(merged.compaction).toEqual({ auto: false, prune: false });
    });

    it("does not create a compaction block when compactionEnabled=false and none exists", () => {
        const root = tempDir();
        const configPath = join(root, "opencode.jsonc");
        addPluginToOpenCodeConfig(configPath, "jsonc", false, false);

        const merged = parseJsonc(readFileSync(configPath, "utf-8")) as {
            compaction?: unknown;
        };
        expect(merged.compaction).toBeUndefined();
    });

    it("mutation direction: same config gets auto=false when mode forced on", () => {
        const root = tempDir();
        const configPath = join(root, "opencode.jsonc");
        writeFileSync(configPath, JSON.stringify({ compaction: { auto: true } }));

        addPluginToOpenCodeConfig(configPath, "jsonc", false, false);
        const afterOff = parseJsonc(readFileSync(configPath, "utf-8")) as {
            compaction?: { auto?: boolean };
        };
        expect(afterOff.compaction?.auto).toBe(true);

        addPluginToOpenCodeConfig(configPath, "jsonc", false, true);
        const afterOn = parseJsonc(readFileSync(configPath, "utf-8")) as {
            compaction?: { auto?: boolean };
        };
        expect(afterOn.compaction?.auto).toBe(false);
    });
});

describe("setup-opencode JSONC byte preservation", () => {
    it("removes DCP and updates compaction without reformatting the existing config", () => {
        const configPath = join(tempDir(), "opencode.jsonc");
        const original =
            "// leading comment\r\n" +
            "{\r\n" +
            '\t"plugin": [\r\n' +
            "\t\t// first plugin\r\n" +
            '\t\t"@keep/first",\r\n' +
            "\t\t// removed DCP plugin\r\n" +
            '\t\t"@tarquinen/opencode-dcp@latest",\r\n' +
            "\t\t// Eidnara stays\r\n" +
            '\t\t"@eidnara/opencode@latest",\r\n' +
            "\t], // array comment\r\n" +
            '\t"compaction": {\r\n' +
            "\t\t// preserve nested comment\r\n" +
            '\t\t"auto": true,\r\n' +
            '\t\t"prune": true,\r\n' +
            "\t},\r\n" +
            '\t"theme": "dark",\r\n' +
            "}\r\n";
        const expected =
            "// leading comment\r\n" +
            "{\r\n" +
            '\t"plugin": [\r\n' +
            "\t\t// first plugin\r\n" +
            '\t\t"@keep/first",\r\n' +
            "\t\t// Eidnara stays\r\n" +
            '\t\t"@eidnara/opencode@latest",\r\n' +
            "\t], // array comment\r\n" +
            '\t"compaction": {\r\n' +
            "\t\t// preserve nested comment\r\n" +
            '\t\t"auto": false,\r\n' +
            '\t\t"prune": false,\r\n' +
            "\t},\r\n" +
            '\t"theme": "dark",\r\n' +
            "}\r\n";
        writeFileSync(configPath, original);

        addPluginToOpenCodeConfig(configPath, "jsonc", true);

        expect(readFileSync(configPath, "utf-8")).toBe(expected);
        expect(readFileSync(configPath, "utf-8")).not.toContain("removed DCP plugin");
    });
});

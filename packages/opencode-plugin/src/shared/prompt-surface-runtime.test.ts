import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { modelKeyLookupOrder, resolvePromptSurface } from "./prompt-surface";
import {
    createPromptSurfaceGuidanceEpochCache,
    createPromptSurfaceRuntime,
    LIGHT_TOOL_DESCRIPTIONS,
    MAX_GUIDANCE_OVERRIDE_BYTES,
} from "./prompt-surface-runtime";

const tempDirs: string[] = [];

function tempDir(): string {
    const directory = mkdtempSync(join(tmpdir(), "prompt-surface-runtime-"));
    tempDirs.push(directory);
    return directory;
}

afterEach(() => {
    for (const directory of tempDirs) {
        rmSync(directory, { recursive: true, force: true });
    }
    tempDirs.length = 0;
});

describe("model key lookup order", () => {
    it("walks the dash-stripped bare ladder for a bare model key", () => {
        expect(modelKeyLookupOrder("claude-sonnet-4-6")).toEqual([
            { key: "claude-sonnet-4-6", source: "bare" },
            { key: "claude-sonnet-4", source: "bare" },
            { key: "claude-sonnet", source: "bare" },
            { key: "claude", source: "bare" },
        ]);
    });

    it("resolves the same bare models entry whether or not the provider prefix is present", () => {
        const config = {
            default: "full" as const,
            models: { "claude-sonnet-4-6": "light" as const },
        };

        expect(resolvePromptSurface(config, "anthropic/claude-sonnet-4-6")).toEqual({
            preset: "light",
            source: "bare",
        });
        expect(resolvePromptSurface(config, "claude-sonnet-4-6")).toEqual({
            preset: "light",
            source: "bare",
        });
    });

    it("keeps provider-qualified and wildcard rungs off the bare ladder", () => {
        const config = {
            default: "full" as const,
            models: {
                "anthropic/claude-sonnet-4-6": "light" as const,
                "anthropic/*": "light" as const,
            },
        };

        expect(resolvePromptSurface(config, "claude-sonnet-4-6")).toEqual({
            preset: "full",
            source: "default",
        });
    });

    it("yields no candidates for an empty provider or model segment", () => {
        expect(modelKeyLookupOrder("/claude-sonnet-4-6")).toEqual([]);
        expect(modelKeyLookupOrder("anthropic/")).toEqual([]);
        expect(modelKeyLookupOrder("")).toEqual([]);
    });
});

describe("prompt-surface runtime", () => {
    it("does not resolve a relative override against the CWD when HOME is relative", () => {
        const saved = { XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME, HOME: process.env.HOME };
        const cwd = process.cwd();
        const directory = tempDir();
        writeFileSync(join(directory, "guidance.md"), "## Eidnara\n\nFrom the project tree");
        try {
            delete process.env.XDG_CONFIG_HOME;
            process.env.HOME = "relative/home";
            process.chdir(directory);
            const warnings: string[] = [];
            const runtime = createPromptSurfaceRuntime({
                warn: (warning) => warnings.push(warning),
            });

            const selection = runtime.resolveGuidance(
                { default: "full", guidance_override_path: "guidance.md" },
                "provider/model",
            );

            expect(selection.primaryOverride).toBeUndefined();
            expect(warnings).toHaveLength(1);
            expect(warnings[0]).toContain("no user configuration directory");
        } finally {
            process.chdir(cwd);
            for (const [key, value] of Object.entries(saved)) {
                if (value === undefined) delete process.env[key];
                else process.env[key] = value;
            }
        }
    });

    it("rejects a guidance override larger than the 1 MiB cap", () => {
        const directory = tempDir();
        const oversized = `## Eidnara\n\n${"x".repeat(MAX_GUIDANCE_OVERRIDE_BYTES)}`;
        writeFileSync(join(directory, "big.md"), oversized);
        const warnings: string[] = [];
        const runtime = createPromptSurfaceRuntime({
            userConfigDirectory: directory,
            warn: (warning) => warnings.push(warning),
        });

        const selection = runtime.resolveGuidance(
            { default: "full", guidance_override_path: "big.md" },
            "provider/model",
        );

        expect(selection.primaryOverride).toBeUndefined();
        expect(warnings).toHaveLength(1);
        expect(warnings[0]).toContain(`exceeds ${MAX_GUIDANCE_OVERRIDE_BYTES} bytes`);
    });

    it("accepts a guidance override exactly at the 1 MiB cap", () => {
        const directory = tempDir();
        const marker = "## Eidnara\n\n";
        const content = `${marker}${"x".repeat(MAX_GUIDANCE_OVERRIDE_BYTES - marker.length)}`;
        writeFileSync(join(directory, "max.md"), content);
        const warnings: string[] = [];
        const runtime = createPromptSurfaceRuntime({
            userConfigDirectory: directory,
            warn: (warning) => warnings.push(warning),
        });

        const selection = runtime.resolveGuidance(
            { default: "full", guidance_override_path: "max.md" },
            "provider/model",
        );

        expect(selection.primaryOverride).toBe(content);
        expect(warnings).toEqual([]);
    });

    it.skipIf(process.platform === "win32")("does not follow a symlinked guidance override", () => {
        const directory = tempDir();
        const target = join(directory, "target.md");
        writeFileSync(target, "## Eidnara\n\nBehind a symlink");
        symlinkSync(target, join(directory, "link.md"));
        const warnings: string[] = [];
        const runtime = createPromptSurfaceRuntime({
            userConfigDirectory: directory,
            warn: (warning) => warnings.push(warning),
        });

        const selection = runtime.resolveGuidance(
            { default: "full", guidance_override_path: "link.md" },
            "provider/model",
        );

        expect(selection.primaryOverride).toBeUndefined();
        expect(warnings).toHaveLength(1);
        expect(warnings[0]).toContain("could not be read");
    });

    it("rejects guidance overrides with zero or two section markers", () => {
        for (const [name, content, expectedCount] of [
            ["zero.md", "Custom guidance without a section marker", 0],
            ["two.md", "## Eidnara\n\nFirst\n\n## Eidnara\n\nSecond", 2],
        ] as const) {
            const directory = tempDir();
            writeFileSync(join(directory, name), content);
            const warnings: string[] = [];
            const runtime = createPromptSurfaceRuntime({
                userConfigDirectory: directory,
                warn: (warning) => warnings.push(warning),
            });

            const selection = runtime.resolveGuidance(
                { default: "full", guidance_override_path: name },
                "provider/model",
            );

            expect(selection.primaryOverride).toBeUndefined();
            expect(warnings).toHaveLength(1);
            expect(warnings[0]).toContain("must contain exactly one");
            expect(warnings[0]).toContain(`found ${expectedCount}`);
        }
    });

    it("captures a valid relative override once per model-key epoch", () => {
        const directory = tempDir();
        const path = join(directory, "guidance.md");
        const first = "## Eidnara\n\nFirst epoch";
        const second = "## Eidnara\n\nSecond epoch";
        writeFileSync(path, first);
        const runtime = createPromptSurfaceRuntime({
            userConfigDirectory: directory,
            warn: () => undefined,
        });
        const epochs = createPromptSurfaceGuidanceEpochCache(runtime);
        const config = {
            default: "full" as const,
            models: { "provider/light": "light" as const },
            guidance_override_path: "guidance.md",
        };

        const initial = epochs.resolve("session", config, "provider/full");
        writeFileSync(path, second);
        const fiveDeferred = Array.from({ length: 5 }, () =>
            epochs.resolve("session", config, "provider/full"),
        );
        const changedModel = epochs.resolve("session", config, "provider/light");

        expect(initial.primaryOverride).toBe(first);
        expect(fiveDeferred.every((selection) => selection === initial)).toBe(true);
        expect(changedModel.preset).toBe("light");
        expect(changedModel.primaryOverride).toBe(second);
    });

    it("uses registration default, applies known overrides, and reports invalid IDs", () => {
        const warnings: string[] = [];
        const runtime = createPromptSurfaceRuntime({
            userConfigDirectory: tempDir(),
            warn: (warning) => warnings.push(warning),
        });
        const registration = runtime.resolveRegistration({
            default: "full",
            models: { "provider/model": "light" },
            tool_descriptions: {
                ctx_search: "Custom search description",
                unknown_tool: "Not allowed",
            },
        });

        expect(registration.preset).toBe("full");
        expect(registration.descriptionFor("ctx_search", "Full search")).toBe(
            "Custom search description",
        );
        expect(registration.descriptionFor("ctx_reduce", "Full reduce")).toBe("Full reduce");
        expect(warnings).toHaveLength(1);
        expect(warnings[0]).toContain("unknown_tool");
    });

    it("lets a user description override the built-in light catalog", () => {
        const runtime = createPromptSurfaceRuntime({
            userConfigDirectory: tempDir(),
            warn: () => undefined,
        });
        const registration = runtime.resolveRegistration({
            default: "light",
            tool_descriptions: { ctx_search: "User light search" },
        });

        expect(registration.descriptionFor("ctx_search", "Full search")).toBe("User light search");
        expect(registration.descriptionFor("ctx_reduce", "Full reduce")).toBe(
            LIGHT_TOOL_DESCRIPTIONS.ctx_reduce,
        );
    });

    it("serves built-in light descriptions without a fallback notice", () => {
        const warnings: string[] = [];
        const runtime = createPromptSurfaceRuntime({
            userConfigDirectory: tempDir(),
            warn: (warning) => warnings.push(warning),
        });
        const config = { default: "light" as const };

        const registration = runtime.resolveRegistration(config);
        const guidance = runtime.resolveGuidance(config, "provider/model");
        runtime.resolveGuidance(config, "provider/other");

        expect(registration.descriptionFor("ctx_search", "Full search")).toBe(
            LIGHT_TOOL_DESCRIPTIONS.ctx_search,
        );
        expect(guidance.preset).toBe("light");
        expect(warnings).toEqual([]);
    });
});

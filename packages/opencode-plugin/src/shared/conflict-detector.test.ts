/// <reference types="bun-types" />

import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    detectConflicts,
    omoConfigCandidatePaths,
    openCodeConfigLayerPaths,
    resolveCompactionForBoot,
} from "./conflict-detector";
import { getOpenCodeConfigPaths } from "./opencode-config-dir";

/**
 */
describe("detectConflicts", () => {
    let root: string;
    let projectDir: string;
    let userConfigDir: string;
    let homeDir: string;
    let originalEnv: Record<string, string | undefined>;

    beforeEach(() => {
        root = mkdtempSync(join(tmpdir(), "eidnara-conflict-"));
        projectDir = join(root, "project");
        mkdirSync(projectDir, { recursive: true });
        userConfigDir = join(root, "user-config", "opencode");
        mkdirSync(userConfigDir, { recursive: true });
        homeDir = join(root, "home");
        mkdirSync(homeDir, { recursive: true });

        // The test isolates config-path resolution from inherited environment variables.
        // `OPENCODE_CONFIG_DIR` overrides `XDG_CONFIG_HOME`.
        // Clearing `XDG_CONFIG_HOME` prevents inherited config paths from affecting the test.
        // test-leaked state.
        originalEnv = {
            OPENCODE_CONFIG_DIR: process.env.OPENCODE_CONFIG_DIR,
            XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
            OPENCODE_DISABLE_AUTOCOMPACT: process.env.OPENCODE_DISABLE_AUTOCOMPACT,
            OPENCODE_DISABLE_PRUNE: process.env.OPENCODE_DISABLE_PRUNE,
            OPENCODE_DISABLE_PROJECT_CONFIG: process.env.OPENCODE_DISABLE_PROJECT_CONFIG,
            OPENCODE_CONFIG: process.env.OPENCODE_CONFIG,
            OPENCODE_CONFIG_CONTENT: process.env.OPENCODE_CONFIG_CONTENT,
            HOME: process.env.HOME,
        };
        process.env.OPENCODE_CONFIG_DIR = userConfigDir;
        process.env.HOME = homeDir;
        delete process.env.XDG_CONFIG_HOME;
        // Setting `OPENCODE_DISABLE_AUTOCOMPACT=1` isolates plugin detection from compaction detection.
        process.env.OPENCODE_DISABLE_AUTOCOMPACT = "1";
        // An inherited `OPENCODE_DISABLE_PRUNE` would hide every prune conflict under test.
        delete process.env.OPENCODE_DISABLE_PRUNE;
        // An inherited `OPENCODE_DISABLE_PROJECT_CONFIG` would hide every project-layer fixture.
        delete process.env.OPENCODE_DISABLE_PROJECT_CONFIG;
        // An inherited `OPENCODE_CONFIG` or `OPENCODE_CONFIG_CONTENT` would add a layer the test did not write.
        delete process.env.OPENCODE_CONFIG;
        delete process.env.OPENCODE_CONFIG_CONTENT;
    });

    afterEach(() => {
        for (const [k, v] of Object.entries(originalEnv)) {
            if (v === undefined) delete process.env[k];
            else process.env[k] = v;
        }
        try {
            rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
        } catch {
            /* */
        }
    });

    function writeProjectConfig(plugins: Array<string | [string, unknown]>): void {
        writeFileSync(join(projectDir, "opencode.json"), JSON.stringify({ plugin: plugins }));
    }

    describe("DCP detection", () => {
        it("matches the canonical @tarquinen/opencode-dcp package", () => {
            writeProjectConfig(["@tarquinen/opencode-dcp"]);
            const result = detectConflicts(projectDir);
            expect(result.conflicts.dcpPlugin).toBe(true);
        });

        it("matches the canonical package with a version suffix", () => {
            writeProjectConfig(["@tarquinen/opencode-dcp@latest"]);
            const result = detectConflicts(projectDir);
            expect(result.conflicts.dcpPlugin).toBe(true);
        });

        it("matches with a semver range suffix", () => {
            writeProjectConfig(["@tarquinen/opencode-dcp@^3.1.0"]);
            const result = detectConflicts(projectDir);
            expect(result.conflicts.dcpPlugin).toBe(true);
        });

        it("does NOT match a fork with a different package name", () => {
            writeProjectConfig(["@some-fork/opencode-dcp-fork"]);
            const result = detectConflicts(projectDir);
            expect(result.conflicts.dcpPlugin).toBe(false);
        });

        it("does NOT match a file:// path that contains 'opencode-dcp'", () => {
            writeProjectConfig(["file:///home/user/work/opencode-dcp-fork"]);
            const result = detectConflicts(projectDir);
            expect(result.conflicts.dcpPlugin).toBe(false);
        });
    });

    describe("OMO detection", () => {
        it("matches the canonical oh-my-opencode package", () => {
            writeProjectConfig(["oh-my-opencode"]);
            const result = detectConflicts(projectDir);
            // Without OMO config, the detector flags all three default-active hooks.
            expect(result.conflicts.omoPreemptiveCompaction).toBe(true);
            expect(result.conflicts.omoContextWindowMonitor).toBe(true);
            expect(result.conflicts.omoAnthropicRecovery).toBe(true);
        });

        it("matches the canonical oh-my-openagent package alias", () => {
            writeProjectConfig(["oh-my-openagent"]);
            const result = detectConflicts(projectDir);
            expect(result.conflicts.omoPreemptiveCompaction).toBe(true);
        });

        it("matches a canonical OMO with a version suffix", () => {
            writeProjectConfig(["oh-my-opencode@3.17.5", "oh-my-openagent@latest"]);
            const result = detectConflicts(projectDir);
            expect(result.conflicts.omoPreemptiveCompaction).toBe(true);
            expect(result.conflicts.omoContextWindowMonitor).toBe(true);
            expect(result.conflicts.omoAnthropicRecovery).toBe(true);
        });

        it("does NOT match oh-my-opencode-slim (issue #43)", () => {
            writeProjectConfig(["oh-my-opencode-slim"]);
            const result = detectConflicts(projectDir);
            expect(result.hasConflict).toBe(false);
            expect(result.conflicts.omoPreemptiveCompaction).toBe(false);
            expect(result.conflicts.omoContextWindowMonitor).toBe(false);
            expect(result.conflicts.omoAnthropicRecovery).toBe(false);
        });

        it("does NOT match oh-my-opencode-slim with a version suffix (issue #43)", () => {
            writeProjectConfig(["oh-my-opencode-slim@latest", "oh-my-opencode-slim@1.0.3"]);
            const result = detectConflicts(projectDir);
            expect(result.hasConflict).toBe(false);
        });

        it("does NOT match a file:// path containing 'oh-my-opencode' (issue #43)", () => {
            writeProjectConfig(["file:///home/user/workspace/oh-my-opencode-slim-dev"]);
            const result = detectConflicts(projectDir);
            expect(result.hasConflict).toBe(false);
        });

        it("does NOT match other forks under different package names", () => {
            writeProjectConfig([
                "oh-my-opencode-cli",
                "@some-org/oh-my-opencode-fork",
                "my-oh-my-opencode-customizations",
            ]);
            const result = detectConflicts(projectDir);
            expect(result.hasConflict).toBe(false);
        });

        it("still detects canonical OMO when slim is also installed", () => {
            // The detector flags canonical OMO when the config also contains slim OMO.
            writeProjectConfig(["oh-my-opencode-slim", "oh-my-opencode@latest"]);
            const result = detectConflicts(projectDir);
            expect(result.conflicts.omoPreemptiveCompaction).toBe(true);
        });

        // Each row writes an OMO config that disables all three hooks at one
        // supported config location/format and asserts the conflict clears
        // (the wrapped "[opencode]" form is the unified omo.jsonc of
        // oh-my-openagent >= 4.19.0).
        // Project-scoped rows avoid relying on user config-path resolution,
        // which can be leaked across files by `spyOn(getOpenCodeConfigPaths)`
        // mocks in sibling tests.
        it.each([
            [
                "respects disabled_hooks in project-level .opencode/oh-my-opencode.json (old format)",
                "project-opencode",
                "oh-my-opencode.json",
                false,
            ],
            [
                "respects disabled_hooks in user-level oh-my-openagent.jsonc (old format)",
                "user-config",
                "oh-my-openagent.jsonc",
                false,
            ],
            [
                "detects disabled_hooks in new ~/.omo/omo.jsonc (user-level)",
                "home-omo",
                "omo.jsonc",
                true,
            ],
            [
                "detects disabled_hooks in new .omo/omo.jsonc (project-level)",
                "project-omo",
                "omo.jsonc",
                true,
            ],
            [
                "reads omo.json (fallback) when omo.jsonc does not exist",
                "home-omo",
                "omo.json",
                true,
            ],
        ] as Array<
            [string, string, string, boolean]
        >)("%s", (_title, location, filename, wrapped) => {
            writeProjectConfig(["oh-my-opencode"]);
            const disabled = {
                disabled_hooks: [
                    "preemptive-compaction",
                    "context-window-monitor",
                    "anthropic-context-window-limit-recovery",
                ],
            };
            const dirs: Record<string, string> = {
                "project-opencode": join(projectDir, ".opencode"),
                "user-config": userConfigDir,
                "project-omo": join(projectDir, ".omo"),
                "home-omo": join(homeDir, ".omo"),
            };
            const dir = dirs[location];
            if (!dir) throw new Error(`unknown location ${location}`);
            mkdirSync(dir, { recursive: true });
            writeFileSync(
                join(dir, filename),
                JSON.stringify(wrapped ? { "[opencode]": disabled } : disabled),
            );
            const result = detectConflicts(projectDir);
            expect(result.hasConflict).toBe(false);
        });

        it("ignores a legacy oh-my-opencode.json at the project root, which OMO never reads", () => {
            writeProjectConfig(["oh-my-opencode"]);
            writeFileSync(
                join(projectDir, "oh-my-opencode.json"),
                JSON.stringify({
                    disabled_hooks: [
                        "preemptive-compaction",
                        "context-window-monitor",
                        "anthropic-context-window-limit-recovery",
                    ],
                }),
            );
            const result = detectConflicts(projectDir);
            expect(result.conflicts.omoPreemptiveCompaction).toBe(true);
            expect(result.conflicts.omoContextWindowMonitor).toBe(true);
            expect(result.conflicts.omoAnthropicRecovery).toBe(true);
        });

        it("reads only omo.jsonc when omo.json sits beside it, like OMO does", () => {
            writeProjectConfig(["oh-my-opencode"]);
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            // The active file leaves preemptive-compaction enabled.
            writeFileSync(
                join(omoDir, "omo.jsonc"),
                JSON.stringify({
                    "[opencode]": {
                        disabled_hooks: [
                            "context-window-monitor",
                            "anthropic-context-window-limit-recovery",
                        ],
                    },
                }),
            );
            // The stale fallback file disables it; OMO ignores this file, so the detector must too.
            writeFileSync(
                join(omoDir, "omo.json"),
                JSON.stringify({
                    "[opencode]": { disabled_hooks: ["preemptive-compaction"] },
                }),
            );
            const result = detectConflicts(projectDir);
            expect(result.conflicts.omoPreemptiveCompaction).toBe(true);
            expect(result.conflicts.omoContextWindowMonitor).toBe(false);
            expect(result.conflicts.omoAnthropicRecovery).toBe(false);
        });

        it("detects hooks as active when new omo.jsonc has no disabled_hooks", () => {
            writeProjectConfig(["oh-my-opencode"]);
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            writeFileSync(
                join(omoDir, "omo.jsonc"),
                JSON.stringify({
                    "[opencode]": {
                        // Without `disabled_hooks`, OMO activates hooks by default.
                    },
                }),
            );
            const result = detectConflicts(projectDir);
            expect(result.conflicts.omoPreemptiveCompaction).toBe(true);
            expect(result.conflicts.omoContextWindowMonitor).toBe(true);
            expect(result.conflicts.omoAnthropicRecovery).toBe(true);
        });

        it("reads disabled_hooks from both old and new config paths", () => {
            writeProjectConfig(["oh-my-opencode"]);
            // The legacy config path disables only preemptive-compaction.
            const legacyDir = join(projectDir, ".opencode");
            mkdirSync(legacyDir, { recursive: true });
            writeFileSync(
                join(legacyDir, "oh-my-opencode.json"),
                JSON.stringify({
                    disabled_hooks: ["preemptive-compaction"],
                }),
            );
            // The unified config path disables the hooks not disabled by the legacy config.
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            writeFileSync(
                join(omoDir, "omo.jsonc"),
                JSON.stringify({
                    "[opencode]": {
                        disabled_hooks: [
                            "context-window-monitor",
                            "anthropic-context-window-limit-recovery",
                        ],
                    },
                }),
            );
            const result = detectConflicts(projectDir);
            // Together, the legacy and unified configs disable all three OMO hooks.
            expect(result.hasConflict).toBe(false);
        });

        it("ignores new omo.jsonc when OMO is not installed", () => {
            writeProjectConfig([]);
            const omoDir = join(homeDir, ".omo");
            mkdirSync(omoDir, { recursive: true });
            writeFileSync(
                join(omoDir, "omo.jsonc"),
                JSON.stringify({
                    "[opencode]": {
                        disabled_hooks: ["preemptive-compaction"],
                    },
                }),
            );
            const result = detectConflicts(projectDir);
            expect(result.hasConflict).toBe(false);
        });
    });

    it("returns no conflicts for an empty plugin list", () => {
        writeProjectConfig([]);
        const result = detectConflicts(projectDir);
        expect(result.hasConflict).toBe(false);
    });

    it("returns no conflicts for unrelated plugins", () => {
        writeProjectConfig(["@eidnara/opencode@latest", "some-other-plugin"]);
        const result = detectConflicts(projectDir);
        expect(result.hasConflict).toBe(false);
    });

    // `readJsoncFile<T>` asserts a TypeScript type but never checks the runtime shape, so a
    // repo-controlled config can hand the detector any JSON value where an array is expected.
    describe("malformed config shapes are ignored, not thrown", () => {
        it.each([
            5,
            true,
            {},
            { "0": "@tarquinen/opencode-dcp" },
            "oh-my-opencode",
        ])("non-array `plugin` value %j in project config does not throw", (plugin) => {
            writeFileSync(join(projectDir, "opencode.json"), JSON.stringify({ plugin }));
            mkdirSync(join(projectDir, ".opencode"), { recursive: true });
            writeFileSync(
                join(projectDir, ".opencode", "opencode.json"),
                JSON.stringify({ plugin }),
            );
            let result: ReturnType<typeof detectConflicts> | undefined;
            expect(() => {
                result = detectConflicts(projectDir);
            }).not.toThrow();
            expect(result?.hasConflict).toBe(false);
        });

        it.each([
            [
                "project legacy",
                (dir: string) => join(dir, ".opencode", "oh-my-opencode.json"),
                false,
            ],
            ["home unified", (_dir: string, home: string) => join(home, ".omo", "omo.json"), true],
            ["project unified", (dir: string) => join(dir, ".omo", "omo.json"), true],
        ] as Array<
            [string, (dir: string, home: string) => string, boolean]
        >)("non-array `disabled_hooks` in %s OMO config does not throw and leaves hooks active", (_label, pathFor, wrapped) => {
            writeProjectConfig(["oh-my-opencode"]);
            const target = pathFor(projectDir, homeDir);
            mkdirSync(join(target, ".."), { recursive: true });
            const body = { disabled_hooks: { "preemptive-compaction": true } };
            writeFileSync(target, JSON.stringify(wrapped ? { "[opencode]": body } : body));
            let result: ReturnType<typeof detectConflicts> | undefined;
            expect(() => {
                result = detectConflicts(projectDir);
            }).not.toThrow();
            expect(result?.conflicts.omoPreemptiveCompaction).toBe(true);
        });

        it("non-string entries inside `disabled_hooks` are skipped", () => {
            writeProjectConfig(["oh-my-opencode"]);
            const legacyDir = join(projectDir, ".opencode");
            mkdirSync(legacyDir, { recursive: true });
            writeFileSync(
                join(legacyDir, "oh-my-opencode.json"),
                JSON.stringify({ disabled_hooks: [42, null, "preemptive-compaction", {}] }),
            );
            const result = detectConflicts(projectDir);
            expect(result.conflicts.omoPreemptiveCompaction).toBe(false);
            expect(result.conflicts.omoContextWindowMonitor).toBe(true);
        });
    });

    // OpenCode supports ["pkg@version", { ...options }] tuple form.
    // `matchesPackageName` accepts package strings, so the detector normalizes tuple entries first.

    describe("tuple plugin entries (issue #49)", () => {
        it("does not crash when a plugin is defined as a [name, options] tuple", () => {
            writeProjectConfig([
                "@eidnara/opencode@latest",
                ["@plannotator/opencode@latest", { workflow: "plan-agent" }],
            ]);
            expect(() => detectConflicts(projectDir)).not.toThrow();
        });

        it("detects DCP conflict when DCP is expressed as a tuple", () => {
            writeProjectConfig([
                "@eidnara/opencode@latest",
                ["@tarquinen/opencode-dcp@latest", {}],
            ]);
            const result = detectConflicts(projectDir);
            expect(result.conflicts.dcpPlugin).toBe(true);
        });

        it("detects OMO conflict when OMO is expressed as a tuple", () => {
            writeProjectConfig([["oh-my-opencode@latest", {}]]);
            const result = detectConflicts(projectDir);
            expect(result.conflicts.omoPreemptiveCompaction).toBe(true);
        });

        it("does not crash on mixed string and tuple entries with unrelated packages", () => {
            writeProjectConfig([
                "oh-my-opencode-slim",
                [
                    "@plannotator/opencode@latest",
                    { workflow: "plan-agent", planningAgents: ["plan"] },
                ],
                "@eidnara/opencode@latest",
            ]);
            const result = detectConflicts(projectDir);
            expect(result.hasConflict).toBe(false);
        });
    });

    // When Eidnara compaction is off, the detector must not treat native compaction.auto or compaction.prune as a plugin-disabling conflict.
    // When Eidnara compaction is on, the detector treats `compaction.auto` and `compaction.prune` as conflicts.
    //
    describe("compaction-off mode matrix (issue #266)", () => {
        // Tests that exercise compaction detection clear `OPENCODE_DISABLE_AUTOCOMPACT`, which the suite setup sets to `1`.
        function writeCompactionConfig(auto: boolean, prune = false): void {
            const prev = process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ compaction: { auto, prune } }),
            );
            if (prev !== undefined) process.env.OPENCODE_DISABLE_AUTOCOMPACT = prev;
        }

        function detectWithMode(compactionEnabled: boolean) {
            const prev = process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            try {
                return detectConflicts(projectDir, { compactionEnabled });
            } finally {
                if (prev !== undefined) process.env.OPENCODE_DISABLE_AUTOCOMPACT = prev;
            }
        }

        it("Eidnara ON + auto=true → conflict fires, plugin would be disabled", () => {
            writeCompactionConfig(true);
            const result = detectWithMode(true);
            expect(result.hasConflict).toBe(true);
            expect(result.conflicts.compactionAuto).toBe(true);
        });

        it("Eidnara ON + auto=false → no compaction conflict, plugin stays enabled", () => {
            writeCompactionConfig(false);
            const result = detectWithMode(true);
            expect(result.conflicts.compactionAuto).toBe(false);
            expect(result.hasConflict).toBe(false);
        });

        it("Eidnara OFF + auto=true → NO conflict, plugin stays enabled (native compaction active)", () => {
            writeCompactionConfig(true);
            const result = detectWithMode(false);
            expect(result.conflicts.compactionAuto).toBe(false);
            expect(result.conflicts.compactionPrune).toBe(false);
            expect(result.hasConflict).toBe(false);
            expect(result.nativeCompaction.auto).toBe(true);
        });

        it("Eidnara OFF + auto=false → NO conflict, no-manager configuration reported honestly", () => {
            writeCompactionConfig(false);
            const result = detectWithMode(false);
            expect(result.conflicts.compactionAuto).toBe(false);
            expect(result.hasConflict).toBe(false);
            // When Eidnara compaction and native compaction are disabled, neither manages compaction.
            expect(result.nativeCompaction.auto).toBe(false);
            expect(result.nativeCompaction.prune).toBe(false);
        });

        it("mutation direction: same auto=true config DOES conflict when mode forced on", () => {
            writeCompactionConfig(true);
            const offResult = detectWithMode(false);
            const onResult = detectWithMode(true);
            expect(offResult.hasConflict).toBe(false);
            expect(onResult.hasConflict).toBe(true);
            expect(onResult.conflicts.compactionAuto).toBe(true);
        });

        // `compaction.prune=true` is a conflict only when Eidnara compaction is enabled.
        it("Eidnara OFF + prune=true → NO conflict (prune is not a conflict in compaction-off mode)", () => {
            writeCompactionConfig(false, true);
            const result = detectWithMode(false);
            expect(result.conflicts.compactionPrune).toBe(false);
            expect(result.hasConflict).toBe(false);
            expect(result.nativeCompaction.prune).toBe(true);
        });

        // DCP and OMO conflicts disable the plugin whether compactionEnabled is true or false.
        it("Eidnara OFF + DCP plugin → DCP conflict still fires (compaction-off does not broaden compatibility)", () => {
            writeProjectConfig(["@tarquinen/opencode-dcp"]);
            const result = detectWithMode(false);
            expect(result.conflicts.dcpPlugin).toBe(true);
            expect(result.hasConflict).toBe(true);
        });

        it("Eidnara OFF + OMO hooks → OMO conflicts still fire in both modes", () => {
            writeProjectConfig(["oh-my-opencode"]);
            const result = detectWithMode(false);
            expect(result.conflicts.omoPreemptiveCompaction).toBe(true);
            expect(result.hasConflict).toBe(true);
        });

        // `detectConflicts` defaults `compactionEnabled` to `true` when options omit it.
        // `detectConflicts` uses file-based compaction detection when `resolvedCompaction` is absent.
        it("default (no options) treats compaction.auto=true as a conflict (fail toward mode-on)", () => {
            writeCompactionConfig(true);
            const prev = process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            try {
                const result = detectConflicts(projectDir);
                expect(result.conflicts.compactionAuto).toBe(true);
                expect(result.hasConflict).toBe(true);
            } finally {
                if (prev !== undefined) process.env.OPENCODE_DISABLE_AUTOCOMPACT = prev;
            }
        });
    });

    // The plugin boot consumes the host's resolved config instead of re-deriving compaction from files.
    describe("resolved-config arm (issue #309)", () => {
        function withoutAutoCompactEnv<T>(fn: () => T): T {
            const prev = process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            try {
                return fn();
            } finally {
                if (prev !== undefined) process.env.OPENCODE_DISABLE_AUTOCOMPACT = prev;
            }
        }

        it("resolved auto=false + file layer that would default true → NO conflict (#309)", () => {
            // Without a compaction block or environment override, file-based detection defaults `auto` to `true`.
            // `resolvedCompaction.auto=false` overrides the file-based default.
            withoutAutoCompactEnv(() => {
                const result = detectConflicts(projectDir, {
                    compactionEnabled: true,
                    resolvedCompaction: { auto: false, prune: false },
                });
                expect(result.conflicts.compactionAuto).toBe(false);
                expect(result.hasConflict).toBe(false);
                expect(result.nativeCompaction.auto).toBe(false);
            });
        });

        it("resolved auto=true → conflict, message carries '(resolved config)'", () => {
            withoutAutoCompactEnv(() => {
                const result = detectConflicts(projectDir, {
                    compactionEnabled: true,
                    resolvedCompaction: { auto: true, prune: false },
                });
                expect(result.conflicts.compactionAuto).toBe(true);
                expect(result.hasConflict).toBe(true);
                expect(result.reasons.join("; ")).toContain("(resolved config)");
            });
        });

        it("resolved prune=true → conflict, message carries '(resolved config)'", () => {
            withoutAutoCompactEnv(() => {
                const result = detectConflicts(projectDir, {
                    compactionEnabled: true,
                    resolvedCompaction: { auto: false, prune: true },
                });
                expect(result.conflicts.compactionPrune).toBe(true);
                expect(result.hasConflict).toBe(true);
                expect(result.reasons.join("; ")).toContain("(resolved config)");
            });
        });

        it("resolved arm is skipped when resolvedCompaction is absent (file-based fallback unchanged)", () => {
            withoutAutoCompactEnv(() => {
                const result = detectConflicts(projectDir, { compactionEnabled: true });
                expect(result.conflicts.compactionAuto).toBe(true);
                expect(result.hasConflict).toBe(true);
                // `resolvedCompaction` is not reported for file-based detection.
                expect(result.reasons.join("; ")).not.toContain("(resolved config)");
            });
        });

        it("OPENCODE_DISABLE_AUTOCOMPACT does not override the resolved arm (host already applied it)", () => {
            // The host folds the flag into the resolved config before the plugin sees it,
            // so the resolved block is authoritative for both `auto` and `prune`.
            process.env.OPENCODE_DISABLE_AUTOCOMPACT = "1";
            try {
                const result = detectConflicts(projectDir, {
                    compactionEnabled: true,
                    resolvedCompaction: { auto: false, prune: true },
                });
                expect(result.conflicts.compactionAuto).toBe(false);
                expect(result.conflicts.compactionPrune).toBe(true);
                expect(result.hasConflict).toBe(true);
                expect(result.nativeCompaction.auto).toBe(false);
                expect(result.nativeCompaction.prune).toBe(true);
            } finally {
                delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            }
        });

        it("compaction-off mode: resolved auto=true is NOT a conflict (native compaction active)", () => {
            withoutAutoCompactEnv(() => {
                const result = detectConflicts(projectDir, {
                    compactionEnabled: false,
                    resolvedCompaction: { auto: true, prune: false },
                });
                expect(result.conflicts.compactionAuto).toBe(false);
                expect(result.hasConflict).toBe(false);
                // Native compaction state remains reported when compactionEnabled is false.
                expect(result.nativeCompaction.auto).toBe(true);
            });
        });
    });

    // The host reads OPENCODE_DISABLE_AUTOCOMPACT through `truthy()`: only "true" or "1"
    // (case-insensitive) count, and the flag zeroes `compaction.auto` only. `compaction.prune`
    // has its own OPENCODE_DISABLE_PRUNE flag. The detector must mirror that or it reports a
    // compaction state the host is not actually running.
    // The host deep-merges every config layer (user `opencode.json` then `opencode.jsonc`,
    // then project root, then `.opencode/`) and applies `{auto: true, prune: false}` defaults
    // only to keys no layer set. File-based detection must reproduce that or it reports a
    // compaction state the host is not running.
    describe("file-based compaction resolution mirrors host layer merging", () => {
        function detect() {
            const prev = process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            try {
                return detectConflicts(projectDir, { compactionEnabled: true });
            } finally {
                if (prev !== undefined) process.env.OPENCODE_DISABLE_AUTOCOMPACT = prev;
            }
        }

        it("a layer that sets only prune leaves auto at the host default (true)", () => {
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ compaction: { prune: false } }),
            );
            const result = detect();
            expect(result.nativeCompaction).toEqual({ auto: true, prune: false });
            expect(result.conflicts.compactionAuto).toBe(true);
        });

        it("a layer that sets only auto leaves prune at the host default (false)", () => {
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ compaction: { auto: false } }),
            );
            const result = detect();
            expect(result.nativeCompaction).toEqual({ auto: false, prune: false });
            expect(result.hasConflict).toBe(false);
        });

        it("merges keys across layers: project auto=false + user prune=true → prune conflict only", () => {
            mkdirSync(join(projectDir, ".opencode"), { recursive: true });
            writeFileSync(
                join(projectDir, ".opencode", "opencode.json"),
                JSON.stringify({ compaction: { auto: false } }),
            );
            writeFileSync(
                join(userConfigDir, "opencode.json"),
                JSON.stringify({ compaction: { prune: true } }),
            );
            const result = detect();
            expect(result.nativeCompaction).toEqual({ auto: false, prune: true });
            expect(result.conflicts.compactionAuto).toBe(false);
            expect(result.conflicts.compactionPrune).toBe(true);
        });

        it("reads opencode.json even when a sibling opencode.jsonc exists", () => {
            writeFileSync(join(projectDir, "opencode.jsonc"), JSON.stringify({ theme: "dark" }));
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ compaction: { auto: false } }),
            );
            const result = detect();
            expect(result.nativeCompaction.auto).toBe(false);
            expect(result.conflicts.compactionAuto).toBe(false);
        });

        it("higher layer wins: user auto=true is overridden by .opencode auto=false", () => {
            writeFileSync(
                join(userConfigDir, "opencode.json"),
                JSON.stringify({ compaction: { auto: true } }),
            );
            mkdirSync(join(projectDir, ".opencode"), { recursive: true });
            writeFileSync(
                join(projectDir, ".opencode", "opencode.json"),
                JSON.stringify({ compaction: { auto: false } }),
            );
            const result = detect();
            expect(result.conflicts.compactionAuto).toBe(false);
        });

        it("within one directory opencode.jsonc overrides opencode.json", () => {
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ compaction: { auto: true } }),
            );
            writeFileSync(
                join(projectDir, "opencode.jsonc"),
                JSON.stringify({ compaction: { auto: false } }),
            );
            const result = detect();
            expect(result.conflicts.compactionAuto).toBe(false);
        });

        it("ignores non-boolean compaction values instead of coercing them", () => {
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ compaction: { auto: "false", prune: 1 } }),
            );
            const result = detect();
            expect(result.nativeCompaction).toEqual({ auto: true, prune: false });
        });
    });

    describe("OPENCODE_DISABLE_PROJECT_CONFIG removes the project layers, like the host", () => {
        function withFlag<T>(run: () => T): T {
            const prev = process.env.OPENCODE_DISABLE_PROJECT_CONFIG;
            process.env.OPENCODE_DISABLE_PROJECT_CONFIG = "1";
            try {
                return run();
            } finally {
                if (prev === undefined) delete process.env.OPENCODE_DISABLE_PROJECT_CONFIG;
                else process.env.OPENCODE_DISABLE_PROJECT_CONFIG = prev;
            }
        }

        it("lists only the user layers", () => {
            const user = getOpenCodeConfigPaths({ binary: "opencode" });
            expect(withFlag(() => openCodeConfigLayerPaths(projectDir))).toEqual([
                user.configJson,
                user.configJsonc,
            ]);
        });

        it("ignores a DCP entry in a project file the host does not load", () => {
            writeProjectConfig(["@tarquinen/opencode-dcp"]);
            mkdirSync(join(projectDir, ".opencode"), { recursive: true });
            writeFileSync(
                join(projectDir, ".opencode", "opencode.jsonc"),
                JSON.stringify({ plugin: ["oh-my-opencode"] }),
            );
            const result = withFlag(() => detectConflicts(projectDir));
            expect(result.hasConflict).toBe(false);
        });

        it("still reads the user layer", () => {
            writeFileSync(
                join(userConfigDir, "opencode.json"),
                JSON.stringify({ plugin: ["@tarquinen/opencode-dcp"] }),
            );
            const result = withFlag(() => detectConflicts(projectDir));
            expect(result.conflicts.dcpPlugin).toBe(true);
        });
    });

    describe("OPENCODE_CONFIG adds a file layer between the user and project layers", () => {
        function withCustom<T>(filePath: string, run: () => T): T {
            const prev = process.env.OPENCODE_CONFIG;
            process.env.OPENCODE_CONFIG = filePath;
            try {
                return run();
            } finally {
                if (prev === undefined) delete process.env.OPENCODE_CONFIG;
                else process.env.OPENCODE_CONFIG = prev;
            }
        }

        it("places the custom file after the user layers and before the project layers", () => {
            const custom = join(root, "custom.jsonc");
            const user = getOpenCodeConfigPaths({ binary: "opencode" });
            const layers = withCustom(custom, () => openCodeConfigLayerPaths(projectDir));
            expect(layers.slice(0, 3)).toEqual([user.configJson, user.configJsonc, custom]);
            expect(layers[3]).toBe(join(projectDir, "opencode.json"));
        });

        it("detects a DCP plugin supplied only through the custom file", () => {
            const custom = join(root, "custom.jsonc");
            writeFileSync(custom, JSON.stringify({ plugin: ["@tarquinen/opencode-dcp"] }));
            writeProjectConfig([]);
            const result = withCustom(custom, () => detectConflicts(projectDir));
            expect(result.conflicts.dcpPlugin).toBe(true);
        });

        it("lets the custom file override the user layer and the project layer override it", () => {
            const prevAuto = process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            try {
                const custom = join(root, "custom.jsonc");
                writeFileSync(
                    join(userConfigDir, "opencode.json"),
                    JSON.stringify({ compaction: { auto: true, prune: true } }),
                );
                writeFileSync(
                    custom,
                    JSON.stringify({ compaction: { auto: false, prune: false } }),
                );
                writeFileSync(
                    join(projectDir, "opencode.json"),
                    JSON.stringify({ compaction: { prune: true } }),
                );
                const result = withCustom(custom, () =>
                    detectConflicts(projectDir, { compactionEnabled: true }),
                );
                expect(result.nativeCompaction).toEqual({ auto: false, prune: true });
            } finally {
                if (prevAuto === undefined) delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;
                else process.env.OPENCODE_DISABLE_AUTOCOMPACT = prevAuto;
            }
        });
    });

    describe("OPENCODE_CONFIG_CONTENT is the highest file-arm layer", () => {
        function withInline<T>(content: string | undefined, run: () => T): T {
            const prev = process.env.OPENCODE_CONFIG_CONTENT;
            if (content === undefined) delete process.env.OPENCODE_CONFIG_CONTENT;
            else process.env.OPENCODE_CONFIG_CONTENT = content;
            try {
                return run();
            } finally {
                if (prev === undefined) delete process.env.OPENCODE_CONFIG_CONTENT;
                else process.env.OPENCODE_CONFIG_CONTENT = prev;
            }
        }

        it("detects a DCP plugin supplied only through the inline config", () => {
            writeProjectConfig(["@eidnara/opencode"]);
            const result = withInline(
                JSON.stringify({ plugin: ["@tarquinen/opencode-dcp@latest"] }),
                () => detectConflicts(projectDir),
            );
            expect(result.conflicts.dcpPlugin).toBe(true);
        });

        it("detects OMO supplied only through the inline config", () => {
            writeProjectConfig([]);
            const result = withInline(JSON.stringify({ plugin: ["oh-my-opencode"] }), () =>
                detectConflicts(projectDir),
            );
            expect(result.conflicts.omoPreemptiveCompaction).toBe(true);
        });

        it("inline compaction overrides every file layer, like the host merge order", () => {
            const prevAuto = process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            try {
                mkdirSync(join(projectDir, ".opencode"), { recursive: true });
                writeFileSync(
                    join(projectDir, ".opencode", "opencode.jsonc"),
                    JSON.stringify({ compaction: { auto: true, prune: true } }),
                );
                const result = withInline(
                    JSON.stringify({ compaction: { auto: false, prune: false } }),
                    () => detectConflicts(projectDir, { compactionEnabled: true }),
                );
                expect(result.nativeCompaction).toEqual({ auto: false, prune: false });
                expect(result.conflicts.compactionAuto).toBe(false);
                expect(result.conflicts.compactionPrune).toBe(false);
            } finally {
                if (prevAuto === undefined) delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;
                else process.env.OPENCODE_DISABLE_AUTOCOMPACT = prevAuto;
            }
        });

        it.each([
            "{ not json",
            "[]",
            "42",
            "",
        ])("malformed or non-object inline content %j contributes nothing", (content) => {
            writeProjectConfig(["@tarquinen/opencode-dcp"]);
            let result: ReturnType<typeof detectConflicts> | undefined;
            expect(() => {
                result = withInline(content, () => detectConflicts(projectDir));
            }).not.toThrow();
            expect(result?.conflicts.dcpPlugin).toBe(true);
        });
    });

    describe("OPENCODE_DISABLE_AUTOCOMPACT host semantics (file-based arm)", () => {
        function detectWithEnv(value: string | undefined) {
            return detectWithFlag("OPENCODE_DISABLE_AUTOCOMPACT", value);
        }

        it.each([
            "0",
            "false",
            "no",
            "off",
            "yes",
        ])("value %j is NOT a disable signal, so auto=true still conflicts", (value) => {
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ compaction: { auto: true, prune: false } }),
            );
            const result = detectWithEnv(value);
            expect(result.conflicts.compactionAuto).toBe(true);
            expect(result.nativeCompaction.auto).toBe(true);
        });

        it.each([
            "1",
            "true",
            "TRUE",
            "True",
        ])("value %j disables auto exactly like the host", (value) => {
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ compaction: { auto: true, prune: false } }),
            );
            const result = detectWithEnv(value);
            expect(result.conflicts.compactionAuto).toBe(false);
            expect(result.nativeCompaction.auto).toBe(false);
        });

        it("leaves prune untouched: OPENCODE_DISABLE_AUTOCOMPACT=1 + prune=true still conflicts on prune", () => {
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ compaction: { auto: true, prune: true } }),
            );
            const result = detectWithEnv("1");
            expect(result.conflicts.compactionAuto).toBe(false);
            expect(result.conflicts.compactionPrune).toBe(true);
            expect(result.hasConflict).toBe(true);
            expect(result.nativeCompaction).toEqual({ auto: false, prune: true });
        });
    });

    describe("OPENCODE_DISABLE_PRUNE host semantics (file-based arm)", () => {
        function detectWithEnv(value: string | undefined) {
            return detectWithFlag("OPENCODE_DISABLE_PRUNE", value);
        }

        it.each([
            "0",
            "false",
            "no",
            "off",
            "yes",
        ])("value %j is NOT a disable signal, so prune=true still conflicts", (value) => {
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ compaction: { auto: false, prune: true } }),
            );
            const result = detectWithEnv(value);
            expect(result.conflicts.compactionPrune).toBe(true);
            expect(result.nativeCompaction.prune).toBe(true);
        });

        it.each([
            "1",
            "true",
            "TRUE",
            "True",
        ])("value %j disables prune exactly like the host", (value) => {
            writeFileSync(
                join(projectDir, "opencode.json"),
                JSON.stringify({ compaction: { auto: false, prune: true } }),
            );
            const result = detectWithEnv(value);
            expect(result.conflicts.compactionPrune).toBe(false);
            expect(result.nativeCompaction.prune).toBe(false);
            expect(result.hasConflict).toBe(false);
        });

        it("leaves auto untouched: OPENCODE_DISABLE_PRUNE=1 + auto=true still conflicts on auto", () => {
            // The suite's `beforeEach` sets OPENCODE_DISABLE_AUTOCOMPACT=1; clear it so auto is
            // decided by the file layer alone.
            const prevAuto = process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;
            try {
                writeFileSync(
                    join(projectDir, "opencode.json"),
                    JSON.stringify({ compaction: { auto: true, prune: true } }),
                );
                const result = detectWithEnv("1");
                expect(result.conflicts.compactionAuto).toBe(true);
                expect(result.conflicts.compactionPrune).toBe(false);
                expect(result.nativeCompaction).toEqual({ auto: true, prune: false });
            } finally {
                if (prevAuto === undefined) delete process.env.OPENCODE_DISABLE_AUTOCOMPACT;
                else process.env.OPENCODE_DISABLE_AUTOCOMPACT = prevAuto;
            }
        });

        it("does not override the resolved arm (host already applied it)", () => {
            const prev = process.env.OPENCODE_DISABLE_PRUNE;
            process.env.OPENCODE_DISABLE_PRUNE = "1";
            try {
                const result = detectConflicts(projectDir, {
                    compactionEnabled: true,
                    resolvedCompaction: { auto: false, prune: true },
                });
                expect(result.conflicts.compactionPrune).toBe(true);
            } finally {
                if (prev === undefined) delete process.env.OPENCODE_DISABLE_PRUNE;
                else process.env.OPENCODE_DISABLE_PRUNE = prev;
            }
        });
    });

    function detectWithFlag(
        name: "OPENCODE_DISABLE_AUTOCOMPACT" | "OPENCODE_DISABLE_PRUNE",
        value: string | undefined,
    ) {
        const prev = process.env[name];
        if (value === undefined) delete process.env[name];
        else process.env[name] = value;
        try {
            return detectConflicts(projectDir, { compactionEnabled: true });
        } finally {
            if (prev === undefined) delete process.env[name];
            else process.env[name] = prev;
        }
    }

    describe("omoConfigCandidatePaths", () => {
        it("returns nothing when no OMO config file exists", () => {
            expect(omoConfigCandidatePaths(projectDir)).toEqual([]);
        });

        it("selects one existing file per location and marks unified entries by location", () => {
            writeFileSync(join(userConfigDir, "oh-my-opencode.jsonc"), "{}");
            writeFileSync(join(userConfigDir, "oh-my-openagent.json"), "{}");
            writeFileSync(join(userConfigDir, "oh-my-openagent.jsonc"), "{}");
            const projectLegacyDir = join(projectDir, ".opencode");
            mkdirSync(projectLegacyDir, { recursive: true });
            writeFileSync(join(projectLegacyDir, "oh-my-opencode.json"), "{}");
            writeFileSync(join(projectDir, "oh-my-opencode.json"), "{}");
            const homeOmoDir = join(homeDir, ".omo");
            mkdirSync(homeOmoDir, { recursive: true });
            writeFileSync(join(homeOmoDir, "omo.json"), "{}");
            writeFileSync(join(homeOmoDir, "omo.jsonc"), "{}");
            const projectOmoDir = join(projectDir, ".omo");
            mkdirSync(projectOmoDir, { recursive: true });
            writeFileSync(join(projectOmoDir, "omo.json"), "{}");

            expect(omoConfigCandidatePaths(projectDir)).toEqual([
                { path: join(userConfigDir, "oh-my-openagent.jsonc"), unified: false },
                { path: join(projectLegacyDir, "oh-my-opencode.json"), unified: false },
                { path: join(homeOmoDir, "omo.jsonc"), unified: true },
                { path: join(projectOmoDir, "omo.json"), unified: true },
            ]);
        });
    });

    describe("resolveCompactionForBoot", () => {
        it("returns the resolved compaction block from the client", async () => {
            const client = {
                config: {
                    get: async () => ({
                        data: { compaction: { auto: false, prune: true } },
                    }),
                },
            };
            const result = await resolveCompactionForBoot(client);
            expect(result).toEqual({ auto: false, prune: true });
        });

        it("returns null when the compaction block is absent (file-based fallback, not host defaults)", async () => {
            // An absent compaction block is not evidence that the host resolved default values.
            const client = {
                config: {
                    get: async () => ({ data: {} }),
                },
            };
            const result = await resolveCompactionForBoot(client);
            expect(result).toBeNull();
        });

        it("returns null when compaction values are not explicit booleans", async () => {
            const client = {
                config: {
                    get: async () => ({ data: { compaction: { auto: "true", prune: null } } }),
                },
            };
            const result = await resolveCompactionForBoot(client);
            expect(result).toBeNull();
        });

        it("returns null when the response data is missing entirely", async () => {
            const client = {
                config: {
                    get: async () => ({}) as { data?: Record<string, unknown> },
                },
            };
            const result = await resolveCompactionForBoot(client);
            expect(result).toBeNull();
        });

        it("returns null when the client throws (file-based fallback used)", async () => {
            const client = {
                config: {
                    get: async () => {
                        throw new Error("boom");
                    },
                },
            };
            const result = await resolveCompactionForBoot(client);
            expect(result).toBeNull();
        });

        it("returns null when the client times out (boot never hangs)", async () => {
            const client = {
                config: {
                    get: () => new Promise<never>(() => {}), // never resolves
                },
            };
            const result = await resolveCompactionForBoot(client, 20);
            expect(result).toBeNull();
        });
    });
});

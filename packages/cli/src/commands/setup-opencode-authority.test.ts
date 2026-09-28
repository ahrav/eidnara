import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import {
    chmodSync,
    existsSync,
    mkdirSync,
    mkdtempSync,
    readFileSync,
    rmSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadUserTierConfigDetailed } from "@eidnara/opencode/config";
import { detectConflicts } from "@eidnara/opencode/shared/conflict-detector";
import { parse as parseJsonc } from "comment-json";
import { foldAuthorityOf } from "../lib/eidnara-modes";
import type { PromptIO, PromptSpinner, SelectOption } from "../lib/prompts";
import { proposeEidnaraConfig, runSetup } from "./setup-opencode";

const MODEL = "anthropic/claude-haiku-4-5";
const ENV_KEYS = [
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "PATH",
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_DISABLE_AUTOCOMPACT",
    "OPENCODE_DISABLE_PRUNE",
    "OPENCODE_DISABLE_PROJECT_CONFIG",
] as const;

class ScriptedPrompts implements PromptIO {
    readonly messages: string[] = [];
    constructor(
        private readonly confirms: boolean[],
        private readonly selections: string[],
        private readonly beforePrompt: (message: string) => void = () => {},
    ) {}
    readonly log = {
        info: (m: string) => this.messages.push(`info:${m}`),
        success: (m: string) => this.messages.push(`success:${m}`),
        warn: (m: string) => this.messages.push(`warn:${m}`),
        error: (m: string) => this.messages.push(`error:${m}`),
        message: (m: string) => this.messages.push(`message:${m}`),
        step: (m: string) => this.messages.push(`step:${m}`),
    };
    intro(m: string): void {
        this.messages.push(`intro:${m}`);
    }
    outro(m: string): void {
        this.messages.push(`outro:${m}`);
    }
    note(m: string, title?: string): void {
        this.messages.push(`note:${title ?? ""}:${m}`);
    }
    spinner(): PromptSpinner {
        return { start() {}, stop() {}, message() {} };
    }
    // `resolveDcpConflictBeforeSetup` destructures `confirm` off the PromptIO, so it is bound here.
    confirm = async (message: string): Promise<boolean> => {
        this.beforePrompt(message);
        const answer = this.confirms.shift();
        if (answer === undefined) throw new Error(`unexpected confirm: ${message}`);
        return answer;
    };
    async text(message: string): Promise<string> {
        throw new Error(`unexpected text: ${message}`);
    }
    async selectOne(message: string, options: SelectOption[]): Promise<string> {
        this.beforePrompt(message);
        const value = this.selections.shift();
        if (value === undefined || !options.some((option) => option.value === value)) {
            throw new Error(`unexpected select: ${message} -> ${value}`);
        }
        return value;
    }
    async selectMany(message: string): Promise<string[]> {
        throw new Error(`unexpected selectMany: ${message}`);
    }
    async selectAutocomplete(message: string, options: SelectOption[]): Promise<string> {
        return this.selectOne(message, options);
    }
    transcript(): string {
        return this.messages.join("\n");
    }
}

describe("runSetup derives the fold authority from the proposed document", () => {
    const saved = new Map<string, string | undefined>();
    let root: string;
    let opencodeConfig: string;
    let eidnaraConfig: string;
    let cwd: string;

    beforeEach(() => {
        for (const key of ENV_KEYS) saved.set(key, process.env[key]);
        root = mkdtempSync(join(tmpdir(), "eidnara-setup-authority-"));
        const bin = join(root, "bin");
        mkdirSync(bin);
        const fake = join(bin, "opencode");
        writeFileSync(
            fake,
            `#!/bin/sh\nif [ "$1" = "models" ]; then echo ${MODEL}; else echo 1.18.0; fi\n`,
        );
        chmodSync(fake, 0o755);
        const configHome = join(root, ".config");
        mkdirSync(join(configHome, "opencode"), { recursive: true });
        mkdirSync(join(configHome, "eidnara"), { recursive: true });
        process.env.HOME = root;
        process.env.XDG_CONFIG_HOME = configHome;
        process.env.XDG_DATA_HOME = join(root, ".local", "share");
        process.env.PATH = bin;
        for (const key of ENV_KEYS.slice(4)) delete process.env[key];
        opencodeConfig = join(configHome, "opencode", "opencode.jsonc");
        eidnaraConfig = join(configHome, "eidnara", "eidnara.jsonc");
        const project = join(root, "project");
        mkdirSync(project);
        cwd = process.cwd();
        process.chdir(project);
    });

    afterEach(() => {
        process.chdir(cwd);
        for (const [key, value] of saved) {
            if (value === undefined) delete process.env[key];
            else process.env[key] = value;
        }
        rmSync(root, { recursive: true, force: true });
    });

    const readJson = (path: string) =>
        parseJsonc(readFileSync(path, "utf-8")) as Record<string, Record<string, unknown>>;

    it("a fresh setup without a summarizer turns OpenCode's compaction on and says why", async () => {
        const prompts = new ScriptedPrompts([false, false], ["remove"]);
        expect(await runSetup(false, { io: prompts })).toBe(0);

        expect(readJson(opencodeConfig).compaction).toEqual({ auto: true });
        expect(readJson(eidnaraConfig).history_summarizer).toBeUndefined();
        const text = prompts.transcript();
        expect(text).toContain(
            "Fold authority: OpenCode's native compaction folds (no summarizer model is configured)",
        );
        expect(text).toContain("compaction.auto=true, compaction.prune left as found");
        expect(text).toContain("Written, restart required");
        expect(text).not.toContain("applied");
    });

    it("a dry run prints the same native proposal and writes nothing", async () => {
        const prompts = new ScriptedPrompts([false, false], ["remove"]);
        expect(await runSetup(true, { io: prompts })).toBe(0);

        expect(existsSync(opencodeConfig)).toBe(false);
        expect(existsSync(eidnaraConfig)).toBe(false);
        const text = prompts.transcript();
        expect(text).toContain("[dry-run] would add the plugin");
        expect(text).toContain("compaction.auto=true, compaction.prune left as found");
        expect(text).toContain(
            "OpenCode's native compaction folds (no summarizer model is configured)",
        );
    });

    it("a setup that picks a model turns OpenCode's compaction off as before", async () => {
        const prompts = new ScriptedPrompts([false, false], ["model", MODEL]);
        expect(await runSetup(false, { io: prompts })).toBe(0);

        expect(readJson(opencodeConfig).compaction).toEqual({ auto: false, prune: false });
        expect(readJson(eidnaraConfig).history_summarizer?.model).toBe(MODEL);
        expect(prompts.transcript()).toContain(
            `Fold authority: Eidnara folds (summarizer chain: ${MODEL})`,
        );
    });

    it("repairs an existing native-folds host for the summarizer it is about to write", async () => {
        // The current file leaves the folds to OpenCode, which would target `auto = true`; the
        // repair follows the proposed document instead.
        writeFileSync(
            opencodeConfig,
            JSON.stringify({ plugin: ["@tarquinen/opencode-dcp"], compaction: { auto: false } }),
        );
        const prompts = new ScriptedPrompts([false, false, true, true], ["model", MODEL]);
        expect(await runSetup(false, { io: prompts })).toBe(0);

        const written = readJson(opencodeConfig);
        expect(written.compaction).toEqual({ auto: false, prune: false });
        expect(written.plugin).not.toContain("@tarquinen/opencode-dcp");
        expect(detectConflicts(process.cwd(), { compactionEnabled: true }).disposition).toBe(
            "none",
        );
        const text = prompts.transcript();
        expect(text).toContain(`Fold authority: Eidnara folds (summarizer chain: ${MODEL})`);
        expect(text).not.toContain("Conflicts remain");
    });

    it("keeps prune as found and re-enables auto under native folds", async () => {
        writeFileSync(opencodeConfig, JSON.stringify({ compaction: { auto: false, prune: true } }));
        const prompts = new ScriptedPrompts([false, false, true], ["remove"]);
        expect(await runSetup(false, { io: prompts })).toBe(0);

        expect(readJson(opencodeConfig).compaction).toEqual({ auto: true, prune: true });
        expect(prompts.transcript()).toContain("no fold authority");
    });

    it("keep and remove derive the authority from the resulting document", async () => {
        const chain = {
            model: "openai/gpt-5.6",
            fallback_models: ["openai/gpt-5.6-mini"],
            module_model: MODEL,
            module_fallback_models: "google/gemini-3.5-flash",
            variant: "high",
        };
        writeFileSync(eidnaraConfig, JSON.stringify({ history_summarizer: chain }));
        const keep = new ScriptedPrompts([false, false], ["keep"]);
        expect(await runSetup(false, { io: keep })).toBe(0);
        expect(readJson(eidnaraConfig).history_summarizer).toEqual(chain);
        expect(keep.transcript()).toContain(
            `Fold authority: Eidnara folds (summarizer chain: ${MODEL}, google/gemini-3.5-flash)`,
        );
        expect(readJson(opencodeConfig).compaction).toEqual({ auto: false, prune: false });

        const remove = new ScriptedPrompts([false, false, true], ["remove"]);
        expect(await runSetup(false, { io: remove })).toBe(0);
        expect(readJson(eidnaraConfig).history_summarizer).toEqual({ variant: "high" });
        expect(readJson(opencodeConfig).compaction).toEqual({ auto: true, prune: false });
    });

    it("a dry run prints the proposed document itself", async () => {
        const prompts = new ScriptedPrompts([false, false], ["model", MODEL]);
        expect(await runSetup(true, { io: prompts })).toBe(0);
        const text = prompts.transcript();
        expect(text).toContain(`[dry-run] proposed ${eidnaraConfig}:`);
        expect(text).toContain(`"model": "${MODEL}"`);
        expect(text).toContain("compaction.auto=false and compaction.prune=false");
    });

    it("an edit made while a prompt is open stops setup before any write", async () => {
        writeFileSync(opencodeConfig, JSON.stringify({ compaction: { auto: false } }));
        const prompts = new ScriptedPrompts([false, false, true], ["remove"], (message) => {
            if (message.startsWith("Apply automatic conflict fixes")) {
                writeFileSync(eidnaraConfig, JSON.stringify({ language: "fr" }));
            }
        });
        expect(await runSetup(false, { io: prompts })).toBe(1);

        expect(readJson(eidnaraConfig)).toEqual({ language: "fr" });
        expect(readJson(opencodeConfig).compaction).toEqual({ auto: false });
        expect(prompts.transcript()).toContain("changed while setup was running");
    });

    it("a project edit made while a prompt is open stops setup before any write", async () => {
        writeFileSync(opencodeConfig, JSON.stringify({ compaction: { auto: false } }));
        const project = process.cwd();
        const prompts = new ScriptedPrompts([false, false, true], ["remove"], (message) => {
            if (message.startsWith("Apply automatic conflict fixes")) {
                mkdirSync(join(project, ".eidnara"));
                writeFileSync(
                    join(project, ".eidnara", "eidnara.jsonc"),
                    JSON.stringify({ compaction: { bogus: true } }),
                );
            }
        });
        expect(await runSetup(false, { io: prompts })).toBe(1);

        expect(readJson(opencodeConfig).compaction).toEqual({ auto: false });
        expect(existsSync(eidnaraConfig)).toBe(false);
        expect(prompts.transcript()).toContain("the project Eidnara config does not load");
    });

    it("enablement follows the proposal when the file changes before it is built", async () => {
        writeFileSync(opencodeConfig, JSON.stringify({ compaction: { auto: false } }));
        writeFileSync(eidnaraConfig, JSON.stringify({ enabled: false }));
        const prompts = new ScriptedPrompts([false, false, true], ["remove"], (message) => {
            if (message === "History summarizer") writeFileSync(eidnaraConfig, "{}");
        });
        expect(await runSetup(false, { io: prompts })).toBe(0);

        expect(readJson(opencodeConfig).compaction).toEqual({ auto: true });
    });

    it("OMO repair follows the proposal's enablement", async () => {
        writeFileSync(eidnaraConfig, JSON.stringify({ enabled: false }));
        const omo = join(root, ".config", "opencode", "oh-my-opencode.json");
        writeFileSync(omo, JSON.stringify({ disabled_hooks: [] }));
        const prompts = new ScriptedPrompts([false, false, true], ["remove"], (message) => {
            if (message === "History summarizer") writeFileSync(eidnaraConfig, "{}");
        });
        expect(await runSetup(false, { io: prompts })).toBe(0);

        expect(prompts.transcript()).toContain("Found oh-my-opencode config");
        expect(readJson(omo).disabled_hooks).toEqual([
            "context-window-monitor",
            "preemptive-compaction",
            "anthropic-context-window-limit-recovery",
        ]);
    });

    it("a project tier that stops the plugin blocks every host edit", async () => {
        const project = process.cwd();
        mkdirSync(join(project, ".eidnara"));
        writeFileSync(
            join(project, ".eidnara", "eidnara.jsonc"),
            JSON.stringify({ history_summarizer: { bogus: true } }),
        );
        const prompts = new ScriptedPrompts([false, false], ["model", MODEL]);
        expect(await runSetup(false, { io: prompts })).toBe(1);

        expect(existsSync(opencodeConfig)).toBe(false);
        expect(prompts.transcript()).toContain("the project Eidnara config does not load");
    });

    it("declining the OpenCode edit leaves the authority and the host settings as they are", async () => {
        writeFileSync(opencodeConfig, JSON.stringify({ compaction: { auto: false } }));
        const prompts = new ScriptedPrompts([false, false, false], ["remove"]);
        expect(await runSetup(false, { io: prompts })).toBe(1);

        expect(readJson(opencodeConfig).compaction).toEqual({ auto: false });
        expect(readJson(eidnaraConfig).history_summarizer).toBeUndefined();
        expect(prompts.transcript()).toContain("Eidnara runs with a warning");
    });

    it("a declined repair under Eidnara folds leaves OpenCode's compaction on and reports it", async () => {
        writeFileSync(opencodeConfig, JSON.stringify({ compaction: { auto: true } }));
        writeFileSync(eidnaraConfig, JSON.stringify({ history_summarizer: { model: MODEL } }));
        const prompts = new ScriptedPrompts([false, false, false], ["keep"]);
        expect(await runSetup(false, { io: prompts })).toBe(1);

        expect(readJson(opencodeConfig).compaction).toEqual({ auto: true });
        expect(prompts.transcript()).toContain("Eidnara stays disabled");
    });

    it("reports an environment-forced auto=false as unresolved by its source", async () => {
        writeFileSync(opencodeConfig, JSON.stringify({ compaction: { auto: true } }));
        process.env.OPENCODE_DISABLE_AUTOCOMPACT = "1";
        const prompts = new ScriptedPrompts([false, false, true], ["remove"]);
        expect(await runSetup(false, { io: prompts })).toBe(1);

        const text = prompts.transcript();
        expect(text).toContain("compaction.auto is set by OPENCODE_DISABLE_AUTOCOMPACT");
        expect(text).toContain("Eidnara runs with a warning");
        expect(text).toContain("No additional conflict changes were needed");
    });

    it("an unresolved proposed document blocks every host edit", async () => {
        writeFileSync(eidnaraConfig, JSON.stringify({ compaction: { enabled: "false" } }));
        const prompts = new ScriptedPrompts([false, false], ["remove"]);
        expect(await runSetup(false, { io: prompts })).toBe(1);

        expect(existsSync(opencodeConfig)).toBe(false);
        expect(prompts.transcript()).toContain("Fold authority: unresolved");
        expect(prompts.transcript()).toContain("edits no host setting");
    });

    function killSetupBetweenWrites(confirms: boolean[], selections: string[]): void {
        const child = join(root, "setup-child.ts");
        writeFileSync(
            child,
            `import { runSetup } from ${JSON.stringify(join(import.meta.dir, "setup-opencode.ts"))};
const confirms = ${JSON.stringify(confirms)};
const selections = ${JSON.stringify(selections)};
const noop = () => {};
const io = {
    intro: noop, outro: noop, note: noop,
    log: { info: noop, success: noop, warn: noop, error: noop, message: noop, step: noop },
    spinner: () => ({ start: noop, stop: noop, message: noop }),
    confirm: async () => confirms.shift() ?? false,
    text: async () => "",
    selectOne: async () => selections.shift() ?? "",
    selectMany: async () => [],
    selectAutocomplete: async () => selections.shift() ?? "",
};
await runSetup(false, { io, betweenWrites: () => process.kill(process.pid, "SIGKILL") });
`,
        );
        const result = Bun.spawnSync([process.execPath, child], {
            cwd: process.cwd(),
            env: process.env,
        });
        expect(result.signalCode).toBe("SIGKILL");
    }

    it("a process killed while removing the summarizer leaves a mismatch detection reports", async () => {
        writeFileSync(
            opencodeConfig,
            JSON.stringify({ compaction: { auto: false, prune: false } }),
        );
        writeFileSync(eidnaraConfig, JSON.stringify({ history_summarizer: { model: MODEL } }));
        killSetupBetweenWrites([false, false, true], ["remove"]);

        expect(readJson(opencodeConfig).compaction).toEqual({ auto: true, prune: false });
        expect(readJson(eidnaraConfig).history_summarizer).toEqual({ model: MODEL });
        const authority = foldAuthorityOf(loadUserTierConfigDetailed(eidnaraConfig));
        expect(authority.kind).toBe("eidnara");
        expect(detectConflicts(process.cwd(), { compactionEnabled: true }).disposition).toBe(
            "disable",
        );
    });

    it("a process killed while adding a summarizer leaves OpenCode's compaction folding", async () => {
        writeFileSync(opencodeConfig, JSON.stringify({ compaction: { auto: true } }));
        writeFileSync(eidnaraConfig, "{}");
        killSetupBetweenWrites([false, false, true], ["model", MODEL]);

        expect(readJson(opencodeConfig).compaction).toEqual({ auto: true });
        expect(readJson(eidnaraConfig).history_summarizer).toEqual({ model: MODEL });
        const authority = foldAuthorityOf(loadUserTierConfigDetailed(eidnaraConfig));
        expect(authority.kind).toBe("eidnara");
        const detected = detectConflicts(process.cwd(), { compactionEnabled: true });
        expect(detected.conflicts.noFoldAuthority).toBe(false);
        expect(detected.disposition).toBe("disable");
    });

    it("a rolled-back rerun whose files already match the proposal is not reported as written", async () => {
        writeFileSync(
            opencodeConfig,
            JSON.stringify({ compaction: { auto: false, prune: false } }),
        );
        const previous = proposeEidnaraConfig(eidnaraConfig, {
            summarizer: { kind: "model", model: MODEL },
            context_researcherEnabled: false,
            context_researcherModel: null,
            claudeMax: false,
        });
        writeFileSync(eidnaraConfig, previous);
        const prompts = new ScriptedPrompts([false, false], ["model", MODEL]);
        const code = await runSetup(false, {
            io: prompts,
            betweenWrites: () => {
                throw new Error("simulated failure between the two writes");
            },
        });
        expect(code).toBe(1);
        const text = prompts.transcript();
        expect(text).toContain("Read back:");
        expect(text).toContain("rolled back");
        expect(text).not.toContain("Written, restart required");
        expect(readFileSync(eidnaraConfig, "utf-8")).toBe(previous);
    });

    it("a failure between the two writes reports the read-back of both files", async () => {
        writeFileSync(
            opencodeConfig,
            JSON.stringify({ compaction: { auto: false, prune: false } }),
        );
        const prompts = new ScriptedPrompts([false, false, true], ["remove"]);
        const code = await runSetup(false, {
            io: prompts,
            betweenWrites: () => {
                throw new Error("simulated failure between the two writes");
            },
        });
        expect(code).toBe(1);

        const text = prompts.transcript();
        expect(text).toContain("simulated failure between the two writes");
        expect(text).toContain("Read back:");
        expect(text).toContain("rolled back");
        expect(text).not.toContain("Written, restart required");
        expect(readJson(opencodeConfig).compaction).toEqual({ auto: false, prune: false });
    });
});

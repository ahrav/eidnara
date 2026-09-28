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
import { parse as parseJsonc } from "comment-json";
import type { PromptIO, PromptSpinner, SelectOption } from "../lib/prompts";
import { runSetup } from "./setup-opencode";

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
    async confirm(message: string): Promise<boolean> {
        const answer = this.confirms.shift();
        if (answer === undefined) throw new Error(`unexpected confirm: ${message}`);
        return answer;
    }
    async text(message: string): Promise<string> {
        throw new Error(`unexpected text: ${message}`);
    }
    async selectOne(message: string, options: SelectOption[]): Promise<string> {
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

    it("keeps prune as found and re-enables auto under native folds", async () => {
        writeFileSync(opencodeConfig, JSON.stringify({ compaction: { auto: false, prune: true } }));
        const prompts = new ScriptedPrompts([false, false, true], ["remove"]);
        expect(await runSetup(false, { io: prompts })).toBe(0);

        expect(readJson(opencodeConfig).compaction).toEqual({ auto: true, prune: true });
        expect(prompts.transcript()).toContain("no fold authority");
    });

    it("keep and remove derive the authority from the resulting document", async () => {
        writeFileSync(
            eidnaraConfig,
            JSON.stringify({
                history_summarizer: {
                    model: MODEL,
                    fallback_models: ["openai/gpt-5.6-mini"],
                    variant: "high",
                },
            }),
        );
        const keep = new ScriptedPrompts([false, false], ["keep"]);
        expect(await runSetup(true, { io: keep })).toBe(0);
        expect(keep.transcript()).toContain(
            `Fold authority: Eidnara folds (summarizer chain: ${MODEL}, openai/gpt-5.6-mini)`,
        );
        expect(keep.transcript()).toContain("compaction.auto=false and compaction.prune=false");

        const remove = new ScriptedPrompts([false, false], ["remove"]);
        expect(await runSetup(false, { io: remove })).toBe(0);
        expect(readJson(eidnaraConfig).history_summarizer).toEqual({ variant: "high" });
        expect(readJson(opencodeConfig).compaction).toEqual({ auto: true });
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
        expect(prompts.transcript()).toContain("setup edits no host setting");
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

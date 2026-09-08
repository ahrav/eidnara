import { beforeEach, describe, expect, it, mock, spyOn } from "bun:test";
import { EventEmitter } from "node:events";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, isAbsolute, join } from "node:path";
import { PassThrough } from "node:stream";
import * as loggerModule from "@eidnara/opencode/shared/logger";
import type { SubagentRunOptions } from "@eidnara/opencode/shared/subagent-runner";

import { __test, PiSubagentRunner } from "./subagent-runner";

const baseOptions: SubagentRunOptions = {
    agent: "sidekick",
    systemPrompt: "system guidance",
    userMessage: "summarize this session",
};
const TEST_SYSTEM_PROMPT_PATH = "/tmp/eidnara-pi-system-prompt.txt";
const COLLISION_STDERR =
    "Agent is already processing. Specify streamingBehavior ('steer' or 'followUp') to queue the message.";
const ISOLATED_RETRY_LOG_MESSAGE =
    "pi subagent: a loaded Pi extension started an agent turn before the child's prompt could run; retrying with an isolated extension set (user extensions disabled for this run)";
const ISOLATED_RETRY_MODEL_UNAVAILABLE_LOG_MESSAGE =
    "model unavailable in isolated retry: it is provided by a disabled extension; configure it through models.json or add a built-in/provider-configured fallback";
const ISOLATED_RETRY_SILENT_LOG_MESSAGE =
    "pi subagent: child exited successfully but emitted no protocol output (no agent_end, zero stdout); a loaded Pi extension likely broke print mode; retrying with an isolated extension set (user extensions disabled for this run)";
const OMP_ALLOWLISTABLE_TOOLS: Readonly<Record<string, true>> = {
    read: true,
    grep: true,
    glob: true,
    bash: true,
    edit: true,
    write: true,
};

beforeEach(() => {
    __test.resetProviderFormCache();
});

type MockChild = ReturnType<typeof createMockChild>;

function createMockChild({ stdout = true }: { stdout?: boolean } = {}) {
    const events = new EventEmitter();
    const stdinStream = new PassThrough();
    stdinStream.setEncoding("utf8");
    const stdoutStream = stdout ? new PassThrough() : null;
    const stderrStream = new PassThrough();
    let stdinText = "";
    const stdinEnded = new Promise<void>((resolve) => {
        stdinStream.on("data", (chunk) => {
            stdinText += chunk;
        });
        stdinStream.on("end", () => resolve());
    });
    let killed = false;
    let exitCode: number | null = null;
    let signalCode: NodeJS.Signals | null = null;
    const killSignals: Array<NodeJS.Signals | number | undefined> = [];

    const child = {
        pid: 42,
        stdin: stdinStream,
        stdout: stdoutStream,
        stderr: stderrStream,
        get killed() {
            return killed;
        },
        get exitCode() {
            return exitCode;
        },
        get signalCode() {
            return signalCode;
        },
        get stdinText() {
            return stdinText;
        },
        kill: mock((signal?: NodeJS.Signals | number) => {
            killSignals.push(signal);
            killed = true;
            return true;
        }),
        on: events.on.bind(events),
        once: events.once.bind(events),
        emitClose: (code: number | null = 0, signal: NodeJS.Signals | null = null) => {
            exitCode = code;
            signalCode = signal;
            stdoutStream?.end();
            stderrStream.end();
            if (!stdinStream.writableEnded) stdinStream.end();
            setTimeout(() => events.emit("close", code, signal), 0);
        },
        emitExit: (code: number | null = 0, signal: NodeJS.Signals | null = null) => {
            exitCode = code;
            signalCode = signal;
            if (!stdinStream.writableEnded) stdinStream.end();
            events.emit("exit", code, signal);
        },
        emitError: (error: Error) => events.emit("error", error),
        writeStdoutLine: (event: unknown) => {
            if (!stdoutStream) throw new Error("stdout disabled");
            stdoutStream.write(`${JSON.stringify(event)}\n`);
        },
        writeRawStdoutLine: (line: string) => {
            if (!stdoutStream) throw new Error("stdout disabled");
            stdoutStream.write(`${line}\n`);
        },
        writeStderr: (text: string) => {
            stderrStream.write(text);
        },
        waitForStdinEnd: () => stdinEnded,
        killSignals,
    };

    return child;
}

function runnerWith(
    childOrChildren: MockChild | MockChild[],
    {
        piBinary = "pi-test",
        platform,
        extraArgs,
        subagentExtensions,
    }: {
        piBinary?: string;
        platform?: NodeJS.Platform;
        extraArgs?: readonly string[];
        subagentExtensions?: readonly string[];
    } = {},
) {
    const remainingChildren = Array.isArray(childOrChildren) ? [...childOrChildren] : null;
    const spawnImpl = mock(() => {
        if (remainingChildren === null) return childOrChildren as never;
        const nextChild = remainingChildren.shift();
        if (!nextChild) throw new Error("unexpected extra spawn");
        return nextChild as never;
    });
    const runner = new PiSubagentRunner({
        piBinary,
        platform,
        extraArgs,
        subagentExtensions,
        spawnImpl: spawnImpl as never,
    });
    return { runner, spawnImpl };
}

function buildArgsForTest(
    options: SubagentRunOptions,
    opts?: Parameters<typeof __test.buildArgs>[1],
) {
    return __test.buildArgs(options, {
        systemPromptPath: TEST_SYSTEM_PROMPT_PATH,
        ...opts,
    });
}

function requirePromptPath(promptPath: string | undefined): string {
    if (!promptPath) throw new Error("expected system prompt path");
    return promptPath;
}

function agentEnd(messages: unknown[]) {
    return { type: "agent_end", messages };
}

function nextTick() {
    return new Promise((resolve) => setTimeout(resolve, 0));
}

describe("subagent-runner pure helpers", () => {
    it("extracts the last assistant text and status from mixed messages", () => {
        const result = __test.extractFinalAssistant([
            { role: "assistant", content: [{ type: "text", text: "old" }] },
            { role: "user", content: [{ type: "text", text: "prompt" }] },
            {
                role: "assistant",
                content: [
                    { type: "toolCall", id: "ignored" },
                    { type: "text", text: "hello " },
                    { type: "text", text: "world" },
                ],
                stopReason: "stop",
                errorMessage: "ignored on success but preserved",
            },
        ]);

        expect(result).toEqual({
            text: "hello world",
            stopReason: "stop",
            errorMessage: "ignored on success but preserved",
        });
    });

    it("returns null text when no assistant message exists", () => {
        expect(__test.extractFinalAssistant([{ role: "user", content: [] }, null])).toEqual({
            text: null,
            stopReason: null,
            errorMessage: null,
        });
    });

    it("builds argv with system prompt, primary model, and prompt last", () => {
        expect(
            buildArgsForTest({
                ...baseOptions,
                model: "anthropic/claude-sonnet",
            }),
        ).toEqual([
            "--print",
            "--mode",
            "json",
            // `--no-session` keeps sidekick child sessions out of `pi resume` and Pi's session picker.
            "--no-session",
            "--no-skills",
            "--no-prompt-templates",
            "--no-context-files",
            "--tools",
            "read,grep,find,ls,ctx_search",
            "--system-prompt",
            TEST_SYSTEM_PROMPT_PATH,
            "--model",
            "anthropic/claude-sonnet",
            // `baseOptions` leaves `thinkingLevel` unset, so Pi resolves thinking for Anthropic.
            "summarize this session",
        ]);
    });

    it("keeps extension discovery enabled so provider and AFT extensions can load", () => {
        const args = buildArgsForTest({
            ...baseOptions,
            model: "google/antigravity-gemini-3.5-flash",
        });

        expect(args).not.toContain("--no-extensions");
        expect(args).toContain("--no-skills");
        expect(args).toContain("--no-prompt-templates");
    });

    it("isolated retry disables discovered extensions but keeps explicit --extension paths", () => {
        const args = buildArgsForTest(
            {
                ...baseOptions,
                agent: "sidekick",
                model: "anthropic/claude-sonnet",
            },
            {
                disableDiscoveredExtensions: true,
                subagentEntryPath: "/tmp/subagent-entry.js",
            },
        );

        expect(args).toEqual(
            expect.arrayContaining(["--no-extensions", "--extension", "/tmp/subagent-entry.js"]),
        );
    });

    it("uses the configured extension allowlist in order and resolves relative paths from Pi settings", () => {
        const args = buildArgsForTest(
            { ...baseOptions, model: "anthropic/claude-sonnet" },
            {
                subagentExtensions: [
                    "provider-package",
                    "./extensions/provider.ts",
                    "../shared/provider.ts",
                ],
            },
        );

        const firstExtension = args.indexOf("--extension");
        expect(args.slice(firstExtension, firstExtension + 6)).toEqual([
            "--extension",
            join(homedir(), ".pi/agent/provider-package"),
            "--extension",
            join(homedir(), ".pi/agent/extensions/provider.ts"),
            "--extension",
            join(homedir(), ".pi/shared/provider.ts"),
        ]);
        expect(args).toContain("--no-extensions");
    });

    it("keeps plain Pi relative allowlist entries rooted at the stock agent dir", () => {
        const previous = process.env.PI_CODING_AGENT_DIR;
        process.env.PI_CODING_AGENT_DIR = "/tmp/plain-pi-custom-agent";
        try {
            const args = buildArgsForTest(
                { ...baseOptions, model: "anthropic/claude-sonnet" },
                { subagentExtensions: ["provider-package"] },
            );
            const firstExtension = args.indexOf("--extension");
            expect(args.slice(firstExtension, firstExtension + 2)).toEqual([
                "--extension",
                join(homedir(), ".pi/agent/provider-package"),
            ]);
        } finally {
            if (previous === undefined) delete process.env.PI_CODING_AGENT_DIR;
            else process.env.PI_CODING_AGENT_DIR = previous;
        }
    });

    it("uses PI_CODING_AGENT_DIR only for a positively identified OMP host", () => {
        const root = mkdtempSync(join(homedir(), ".eidnara-omp-host-test-"));
        const previousAgentDir = process.env.PI_CODING_AGENT_DIR;
        const previousPackageDir = process.env.PI_PACKAGE_DIR;
        writeFileSync(
            join(root, "package.json"),
            JSON.stringify({ name: "@oh-my-pi/pi-coding-agent" }),
        );
        process.env.PI_PACKAGE_DIR = root;
        process.env.PI_CODING_AGENT_DIR = "/tmp/omp-profile/agent";
        try {
            const args = buildArgsForTest(
                { ...baseOptions, model: "anthropic/claude-sonnet" },
                { subagentExtensions: ["provider-package"] },
            );
            const firstExtension = args.indexOf("--extension");
            expect(args.slice(firstExtension, firstExtension + 2)).toEqual([
                "--extension",
                "/tmp/omp-profile/agent/provider-package",
            ]);
        } finally {
            rmSync(root, { recursive: true, force: true });
            if (previousAgentDir === undefined) delete process.env.PI_CODING_AGENT_DIR;
            else process.env.PI_CODING_AGENT_DIR = previousAgentDir;
            if (previousPackageDir === undefined) delete process.env.PI_PACKAGE_DIR;
            else process.env.PI_PACKAGE_DIR = previousPackageDir;
        }
    });

    it("uses the OMP default agent dir when PI_CODING_AGENT_DIR is unset", () => {
        const root = mkdtempSync(join(homedir(), ".eidnara-omp-default-host-test-"));
        const previous = {
            agentDir: process.env.PI_CODING_AGENT_DIR,
            packageDir: process.env.PI_PACKAGE_DIR,
            configDir: process.env.PI_CONFIG_DIR,
            ompProfile: process.env.OMP_PROFILE,
            piProfile: process.env.PI_PROFILE,
        };
        writeFileSync(
            join(root, "package.json"),
            JSON.stringify({ name: "@oh-my-pi/pi-coding-agent" }),
        );
        process.env.PI_PACKAGE_DIR = root;
        delete process.env.PI_CODING_AGENT_DIR;
        delete process.env.PI_CONFIG_DIR;
        delete process.env.OMP_PROFILE;
        delete process.env.PI_PROFILE;
        try {
            const args = buildArgsForTest(
                { ...baseOptions, model: "anthropic/claude-sonnet" },
                { subagentExtensions: ["provider-package"] },
            );
            const firstExtension = args.indexOf("--extension");
            expect(args.slice(firstExtension, firstExtension + 2)).toEqual([
                "--extension",
                join(homedir(), ".omp/agent/provider-package"),
            ]);
        } finally {
            rmSync(root, { recursive: true, force: true });
            for (const [key, value] of [
                ["PI_CODING_AGENT_DIR", previous.agentDir],
                ["PI_PACKAGE_DIR", previous.packageDir],
                ["PI_CONFIG_DIR", previous.configDir],
                ["OMP_PROFILE", previous.ompProfile],
                ["PI_PROFILE", previous.piProfile],
            ] as const) {
                if (value === undefined) delete process.env[key];
                else process.env[key] = value;
            }
        }
    });

    it("gives a named OMP profile precedence over a stale agent-dir override", () => {
        const root = mkdtempSync(join(homedir(), ".eidnara-omp-profile-host-test-"));
        const previous = {
            agentDir: process.env.PI_CODING_AGENT_DIR,
            packageDir: process.env.PI_PACKAGE_DIR,
            configDir: process.env.PI_CONFIG_DIR,
            ompProfile: process.env.OMP_PROFILE,
            piProfile: process.env.PI_PROFILE,
        };
        writeFileSync(
            join(root, "package.json"),
            JSON.stringify({ name: "@oh-my-pi/pi-coding-agent" }),
        );
        process.env.PI_PACKAGE_DIR = root;
        process.env.PI_CODING_AGENT_DIR = "/tmp/stale-omp-agent";
        process.env.PI_CONFIG_DIR = ".omp-test";
        process.env.OMP_PROFILE = "work";
        delete process.env.PI_PROFILE;
        try {
            const args = buildArgsForTest(
                { ...baseOptions, model: "anthropic/claude-sonnet" },
                { subagentExtensions: ["provider-package"] },
            );
            const firstExtension = args.indexOf("--extension");
            expect(args.slice(firstExtension, firstExtension + 2)).toEqual([
                "--extension",
                join(homedir(), ".omp-test/profiles/work/agent/provider-package"),
            ]);
        } finally {
            rmSync(root, { recursive: true, force: true });
            for (const [key, value] of [
                ["PI_CODING_AGENT_DIR", previous.agentDir],
                ["PI_PACKAGE_DIR", previous.packageDir],
                ["PI_CONFIG_DIR", previous.configDir],
                ["OMP_PROFILE", previous.ompProfile],
                ["PI_PROFILE", previous.piProfile],
            ] as const) {
                if (value === undefined) delete process.env[key];
                else process.env[key] = value;
            }
        }
    });

    it("keeps the current all-extension argv shape when no allowlist is configured", () => {
        const args = buildArgsForTest({
            ...baseOptions,
            model: "anthropic/claude-sonnet",
        });

        expect(args).not.toContain("--no-extensions");
        expect(args).not.toContain("--extension");
    });

    it("disables project context files so hidden subagents see only our prompt", () => {
        const args = buildArgsForTest({
            ...baseOptions,
            model: "anthropic/claude-sonnet",
        });

        expect(args).toContain("--no-context-files");
        expect(args.indexOf("--no-context-files")).toBeLessThan(args.indexOf("--tools"));
    });

    it("emits only OMP-supported startup flags and tool names on an OMP host", () => {
        const root = mkdtempSync(join(homedir(), ".eidnara-omp-argv-test-"));
        const previousPackageDir = process.env.PI_PACKAGE_DIR;
        writeFileSync(
            join(root, "package.json"),
            JSON.stringify({ name: "@oh-my-pi/pi-coding-agent" }),
        );
        process.env.PI_PACKAGE_DIR = `~/${basename(root)}`;
        try {
            const sidekickArgs = buildArgsForTest({
                ...baseOptions,
                agent: "sidekick",
            });
            expect(sidekickArgs).toContain("--no-rules");
            expect(sidekickArgs).not.toContain("--no-prompt-templates");
            expect(sidekickArgs).not.toContain("--no-context-files");
            expect(sidekickArgs).toEqual(expect.arrayContaining(["--tools", "read,grep,glob"]));
        } finally {
            rmSync(root, { recursive: true, force: true });
            if (previousPackageDir === undefined) delete process.env.PI_PACKAGE_DIR;
            else process.env.PI_PACKAGE_DIR = previousPackageDir;
        }
    });

    it("always includes --no-session so child sessions don't appear in pi resume", () => {
        // Hidden sidekick subagents must not appear in Pi's session list or `pi resume`.
        const args = buildArgsForTest({
            ...baseOptions,
            model: "anthropic/claude-sonnet",
        });
        expect(args).toContain("--no-session");
        const noSessionIdx = args.indexOf("--no-session");
        const modelIdx = args.indexOf("--model");
        expect(noSessionIdx).toBeLessThan(modelIdx);
    });

    it("builds a single --model; runner handles fallback with fresh children", () => {
        const args = buildArgsForTest({
            ...baseOptions,
            model: "anthropic/primary",
            fallbackModels: ["openai/fallback", "google/last"],
        });

        expect(args).toContain("--model");
        expect(args).not.toContain("--models");
        expect(args).toContain("anthropic/primary");
        expect(args).not.toContain("openai/fallback");
        expect(args.at(-1)).toBe("summarize this session");
    });

    it("translates the canonical (OpenCode) provider to Pi's form at --model", () => {
        // `--model` receives Pi provider IDs rather than canonical provider IDs.
        expect(buildArgsForTest({ ...baseOptions, model: "openai/gpt-5.5" })).toEqual(
            expect.arrayContaining(["--model", "openai-codex/gpt-5.5"]),
        );
        expect(
            buildArgsForTest({
                ...baseOptions,
                model: "google/antigravity-gemini-3.5-flash",
            }),
        ).toEqual(
            expect.arrayContaining(["--model", "google-antigravity/antigravity-gemini-3.5-flash"]),
        );
        expect(buildArgsForTest({ ...baseOptions, model: "anthropic/claude-opus-4-8" })).toEqual(
            expect.arrayContaining(["--model", "anthropic/claude-opus-4-8"]),
        );
    });

    it("passes prompt last without a -- sentinel", () => {
        const args = buildArgsForTest({
            ...baseOptions,
            model: "anthropic/claude-sonnet",
            userMessage: "ordinary prompt",
        });

        expect(args.at(-1)).toBe("ordinary prompt");
        expect(args).not.toContain("--");
    });

    it("locks sidekick to an explicit read-only allow-list", () => {
        const sidekickArgs = buildArgsForTest({
            ...baseOptions,
            agent: "sidekick",
        });
        expect(sidekickArgs).toEqual(
            expect.arrayContaining(["--tools", "read,grep,find,ls,ctx_search"]),
        );
    });

    it("translates every strict Pi allow-list into valid OMP built-ins", () => {
        expect(__test.resolveHostToolAllowlist(["read", "grep", "find", "ls"], true)).toEqual([
            "read",
            "grep",
            "glob",
        ]);
        expect(__test.resolveHostToolAllowlist(["read", "aft_search", "ctx_search"], true)).toEqual(
            ["read"],
        );
        expect(
            __test.resolveHostToolAllowlist(["read", "find", "ls", "aft_search"], false),
        ).toEqual(["read", "find", "ls", "aft_search"]);

        for (const [agent, tools] of __test.STRICT_TOOL_ALLOWLIST) {
            const resolved = __test.resolveHostToolAllowlist(tools, true);
            for (const tool of resolved) {
                expect(OMP_ALLOWLISTABLE_TOOLS[tool] === true, agent).toBe(true);
            }
            expect(new Set(resolved).size, agent).toBe(resolved.length);
        }
    });

    it("emits an explicit tool gate for every known Pi subagent agent", () => {
        for (const agent of __test.KNOWN_PI_SUBAGENT_AGENTS) {
            const args = buildArgsForTest({ ...baseOptions, agent });
            const hasTools = args.includes("--tools");
            const hasNoTools = args.includes("--no-tools");
            expect(__test.STRICT_TOOL_ALLOWLIST.has(agent)).toBe(true);
            expect(hasTools || hasNoTools).toBe(true);
            expect(hasTools && hasNoTools).toBe(false);
        }
    });

    it("fails closed to --no-tools for unknown agent ids", () => {
        const args = buildArgsForTest({ ...baseOptions, agent: "future-agent" });
        expect(args).toContain("--no-tools");
        expect(args).not.toContain("--tools");
    });

    it("parses JSON event lines and normalizes parse errors", () => {
        expect(__test.parsePiEventLine('{"type":"agent_start"}')).toEqual({
            ok: true,
            event: { type: "agent_start" },
        });

        const parsed = __test.parsePiEventLine("{not-json");
        expect(parsed.ok).toBe(false);
        if (!parsed.ok && "error" in parsed) {
            expect(parsed.error).toContain("failed to parse event");
            expect(parsed.error).toContain("line={not-json");
        } else {
            throw new Error("malformed JSON must be an error, not noise");
        }

        // Ignore non-event stdout only when an intact terminal `message_end` arrives.
        const noise = __test.parsePiEventLine("[Worker] Ready");
        expect(noise.ok).toBe(false);
        if (!noise.ok) expect("noise" in noise).toBe(true);
    });

    // Ignore non-event stdout when a terminal `message_end` arrives intact.
    it("ignores non-JSON stdout noise from co-loaded extensions", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeRawStdoutLine("[Worker] Ready");
        child.writeStdoutLine({
            type: "message_end",
            message: {
                role: "assistant",
                content: [{ type: "text", text: "done" }],
                stopReason: "stop",
            },
        });
        child.writeRawStdoutLine("[Worker] Shutting down");
        child.emitClose(0);

        const result = await resultPromise;
        expect(result.ok).toBe(true);
        if (result.ok) {
            expect(result.assistantText).toBe("done");
        }
    });

    // `dist/subagent-entry.js` is absent in unit tests unless `bun run build` runs.
    // The source build omits `--extension` because `dist/subagent-entry.js` is absent.

    it("dev mode (no bundle): does NOT pass --extension flag, so ctx_* tools are unavailable", () => {
        // In dev mode (running .ts source), there's no dist/subagent-entry.js
        // extensions still load; only Eidnara's explicit ctx_* entry is absent.
        const args = buildArgsForTest({
            ...baseOptions,
            agent: "sidekick",
            model: "anthropic/claude-sonnet",
        });
        // `-x` hard-fails in Pi 0.71+.
        expect(args).not.toContain("--extension");
        expect(args).not.toContain("-x");
    });
});

describe("PiSubagentRunner spawn lifecycle", () => {
    it("treats a terminal stop turn as success even when drain SIGTERM closes the child", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeStdoutLine({
            type: "message_end",
            message: {
                role: "assistant",
                content: [{ type: "text", text: "looks done" }],
                stopReason: "stop",
            },
        });
        child.emitClose(null, "SIGTERM");

        expect(await resultPromise).toEqual({
            ok: true,
            assistantText: "looks done",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
    });
    it("replaces the hard timeout with the drain grace period after agent_end", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run({ ...baseOptions, timeoutMs: 30 });
        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "answered" }],
                    stopReason: "stop",
                },
            ]),
        );
        // The child outlives `timeoutMs`; the captured answer must not become a timeout.
        const outcome = await Promise.race([
            resultPromise.then(() => "settled"),
            new Promise<string>((resolve) => setTimeout(() => resolve("pending"), 90)),
        ]);
        expect(outcome).toBe("pending");
        child.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: true,
            assistantText: "answered",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
    });
    it("counts toolCall content parts from assistant message_end into toolCallCount (grounding gate)", async () => {
        // `toolCallCount` counts `toolCall` content parts on assistant `message_end` turns, not tool event names.
        // Pi emits `tool_execution_end`, not `tool_result_end`, so counting event names would miss calls.
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeStdoutLine({
            type: "message_end",
            message: {
                role: "assistant",
                content: [{ type: "toolCall", toolName: "read", toolCallId: "c1" }],
                stopReason: "toolUse",
            },
        });
        // A toolResult message_end (role: "tool") must NOT be counted.
        child.writeStdoutLine({
            type: "message_end",
            message: {
                role: "tool",
                content: [{ type: "toolResult", text: "read ok" }],
            },
        });
        child.writeStdoutLine({
            type: "message_end",
            message: {
                role: "assistant",
                content: [{ type: "toolCall", toolName: "grep", toolCallId: "c2" }],
                stopReason: "toolUse",
            },
        });
        child.writeStdoutLine({
            type: "message_end",
            message: {
                role: "assistant",
                content: [{ type: "text", text: "grounded answer" }],
                stopReason: "stop",
            },
        });
        child.emitClose(0);

        const result = await resultPromise;
        expect(result.ok).toBe(true);
        if (result.ok) expect(result.toolCallCount).toBe(2);
    });

    it("spawns pi, parses stdout, trims assistant text, and captures stderr", async () => {
        const child = createMockChild();
        const { runner, spawnImpl } = runnerWith(child, { piBinary: "custom-pi" });

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/claude-sonnet",
            cwd: "/tmp/project",
        });
        child.writeStderr("warning from pi");
        child.writeStdoutLine({ type: "session", id: "s1" });
        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "  final answer  " }],
                    stopReason: "stop",
                },
            ]),
        );
        child.emitClose(0);

        const result = await resultPromise;

        expect(spawnImpl).toHaveBeenCalledWith(
            "custom-pi",
            expect.arrayContaining(["--model", "anthropic/claude-sonnet"]),
            expect.objectContaining({
                cwd: "/tmp/project",
                env: expect.objectContaining({
                    EIDNARA_PI_SUBAGENT: "1",
                    PATH: process.env.PATH,
                }),
                stdio: ["ignore", "pipe", "pipe"],
            }),
        );
        expect(result).toEqual({
            ok: true,
            assistantText: "final answer",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: "warning from pi" },
        });
    });

    it("with no piBinary override, spawns the host runtime + cli.js (Windows-safe, #177)", async () => {
        // Default resolution must not spawn bare `pi`, which ENOENTs on Windows.
        // npm installs a `pi.cmd` shim rather than a literal `pi` executable on Windows.
        const child = createMockChild();
        const spawnImpl = mock(() => child as never);
        const { PiSubagentRunner } = await import("./subagent-runner");
        const runner = new PiSubagentRunner({ spawnImpl: spawnImpl as never });

        const resultPromise = runner.run(baseOptions);
        child.writeStdoutLine({ type: "session", id: "s1" });
        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "ok" }],
                    stopReason: "stop",
                },
            ]),
        );
        child.emitClose(0);
        await resultPromise;

        expect(spawnImpl).toHaveBeenCalledTimes(1);
        const [command, spawnArgs, opts] = (spawnImpl.mock.calls as unknown[][])[0] as [
            string,
            string[],
            { shell?: boolean },
        ];
        expect(command).toBe(process.execPath);
        expect(spawnArgs[0]).toBe(process.argv[1]);
        expect(spawnArgs).toContain("--no-session");
        // `shell: false` prevents `cmd.exe` from interpreting prompt or task text.
        expect(opts.shell).toBeFalsy();
        expect(command).not.toBe("pi");
    });

    it("returns model_failed promptly for live terminal error stopReason", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run({ ...baseOptions, timeoutMs: 60_000 });
        child.writeStdoutLine({
            type: "message_end",
            message: {
                role: "assistant",
                content: [{ type: "text", text: "partial" }],
                stopReason: "error",
                errorMessage: "provider exploded",
            },
        });
        child.emitClose(null, "SIGTERM");

        expect(await resultPromise).toEqual({
            ok: false,
            reason: "model_failed",
            error: "provider exploded",
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
    });

    it("returns model_failed when the final assistant stopReason is error", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeStderr("provider failed");
        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "partial" }],
                    stopReason: "error",
                    errorMessage: "model overloaded",
                },
            ]),
        );
        child.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: false,
            reason: "model_failed",
            error: "model overloaded",
            durationMs: expect.any(Number),
            meta: { stderr: "provider failed" },
        });
    });

    it("returns model_failed when the final assistant stopReason is aborted", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "partial" }],
                    stopReason: "aborted",
                },
            ]),
        );
        child.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: false,
            reason: "model_failed",
            error: 'pi assistant stopped with reason "aborted"',
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
    });

    it("returns model_failed with the errorMessage when an error turn has no text", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeStdoutLine({
            type: "message_end",
            message: {
                role: "assistant",
                content: [],
                stopReason: "error",
                errorMessage: "No API key found for anthropic",
            },
        });
        child.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: false,
            reason: "model_failed",
            error: "No API key found for anthropic",
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
    });

    it("returns truncated when the final assistant stopReason is length", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "partial" }],
                    stopReason: "length",
                },
            ]),
        );
        child.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: false,
            reason: "truncated",
            error: 'pi assistant stopped with reason "length"',
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
    });

    it("returns spawn_failed when spawn throws synchronously", async () => {
        const spawnImpl = mock(() => {
            throw new Error("ENOENT pi");
        });
        const runner = new PiSubagentRunner({ spawnImpl: spawnImpl as never });

        expect(await runner.run(baseOptions)).toEqual({
            ok: false,
            reason: "spawn_failed",
            error: "ENOENT pi",
            durationMs: expect.any(Number),
        });
    });

    it("writes the system prompt to a temp file path and removes it after success", async () => {
        const child = createMockChild();
        let promptPath: string | undefined;
        const spawnImpl = mock((_command: string, args: string[]) => {
            const promptFlagIndex = args.indexOf("--system-prompt");
            expect(promptFlagIndex).toBeGreaterThan(-1);
            promptPath = args[promptFlagIndex + 1];
            const systemPromptPath = requirePromptPath(promptPath);
            expect(systemPromptPath).not.toBe(baseOptions.systemPrompt);
            expect(isAbsolute(systemPromptPath)).toBe(true);
            expect(existsSync(systemPromptPath)).toBe(true);
            expect(readFileSync(systemPromptPath, "utf8")).toBe(baseOptions.systemPrompt);
            return child as never;
        });
        const runner = new PiSubagentRunner({
            piBinary: "pi-test",
            spawnImpl: spawnImpl as never,
        });

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/claude-sonnet",
        });
        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "ok" }],
                    stopReason: "stop",
                },
            ]),
        );
        child.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: true,
            assistantText: "ok",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
        const systemPromptPath = requirePromptPath(promptPath);
        expect(existsSync(systemPromptPath)).toBe(false);
    });

    it("removes the temp system prompt file when spawn throws", async () => {
        let promptPath: string | undefined;
        const spawnImpl = mock((_command: string, args: string[]) => {
            const promptFlagIndex = args.indexOf("--system-prompt");
            expect(promptFlagIndex).toBeGreaterThan(-1);
            promptPath = args[promptFlagIndex + 1];
            expect(existsSync(requirePromptPath(promptPath))).toBe(true);
            throw new Error("ENOENT pi");
        });
        const runner = new PiSubagentRunner({
            piBinary: "pi-test",
            spawnImpl: spawnImpl as never,
        });

        expect(await runner.run(baseOptions)).toEqual({
            ok: false,
            reason: "spawn_failed",
            error: "ENOENT pi",
            durationMs: expect.any(Number),
        });
        const systemPromptPath = requirePromptPath(promptPath);
        expect(existsSync(systemPromptPath)).toBe(false);
    });

    it("pipes small win32 user messages through stdin instead of argv", async () => {
        const child = createMockChild();
        const { runner, spawnImpl } = runnerWith(child, { platform: "win32" });
        const userMessage = "small win32 prompt";

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/claude-sonnet",
            userMessage,
        });
        await child.waitForStdinEnd();

        const spawnArgs = spawnImpl.mock.calls[0]?.[1] as string[] | undefined;
        const spawnOptions = spawnImpl.mock.calls[0]?.[2] as
            | { stdio?: [string, string, string] }
            | undefined;
        expect(spawnArgs).not.toContain(userMessage);
        expect(child.stdinText).toBe(userMessage);
        expect(spawnOptions?.stdio).toEqual(["pipe", "pipe", "pipe"]);

        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "done" }],
                    stopReason: "stop",
                },
            ]),
        );
        child.emitClose(0);
        await resultPromise;
    });

    it("keeps small linux user messages positional", async () => {
        const child = createMockChild();
        const { runner, spawnImpl } = runnerWith(child, { platform: "linux" });
        const userMessage = "small linux prompt";

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/claude-sonnet",
            userMessage,
        });

        const spawnArgs = spawnImpl.mock.calls[0]?.[1] as string[] | undefined;
        const spawnOptions = spawnImpl.mock.calls[0]?.[2] as
            | { stdio?: [string, string, string] }
            | undefined;
        expect(spawnArgs?.at(-1)).toBe(userMessage);
        expect(child.stdinText).toBe("");
        expect(spawnOptions?.stdio).toEqual(["ignore", "pipe", "pipe"]);

        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "done" }],
                    stopReason: "stop",
                },
            ]),
        );
        child.emitClose(0);
        await resultPromise;
    });

    it("keeps win32 argv well under the CreateProcess limit with a large system prompt", async () => {
        const child = createMockChild();
        const { runner, spawnImpl } = runnerWith(child, { platform: "win32" });
        const systemPrompt = "h".repeat(60 * 1024);

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/claude-sonnet",
            systemPrompt,
            userMessage: "hi",
        });
        await child.waitForStdinEnd();

        const spawnArgs = spawnImpl.mock.calls[0]?.[1] as string[] | undefined;
        expect(spawnArgs).toBeDefined();
        expect(spawnArgs?.join(" ").length ?? 0).toBeLessThan(32_767);
        expect(spawnArgs).not.toContain(systemPrompt);
        expect(spawnArgs).not.toContain("hi");

        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "done" }],
                    stopReason: "stop",
                },
            ]),
        );
        child.emitClose(0);
        await resultPromise;
    });

    it("returns spawn_failed when the child emits an error", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.emitError(new Error("permission denied"));

        expect(await resultPromise).toEqual({
            ok: false,
            reason: "spawn_failed",
            error: "permission denied",
            durationMs: expect.any(Number),
        });
    });

    it("returns parse_failed for malformed stdout without agent_end", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeStderr("bad json emitted");
        child.writeRawStdoutLine("{not-json");
        child.emitClose(0);

        const result = await resultPromise;

        expect(result.ok).toBe(false);
        if (!result.ok) {
            expect(result.reason).toBe("parse_failed");
            expect(result.error).toContain("failed to parse event");
            expect(result.meta).toEqual({
                stderr: "bad json emitted",
                exitCode: 0,
                signal: null,
            });
        }
    });

    it("ignores malformed lines if a later agent_end succeeds", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeRawStdoutLine("not json");
        child.writeStdoutLine(
            agentEnd([{ role: "assistant", content: [{ type: "text", text: "recovered" }] }]),
        );
        child.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: true,
            assistantText: "recovered",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
    });

    it("returns no_assistant for agent_end without assistant messages", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeStdoutLine(agentEnd([{ role: "user", content: [] }]));
        child.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: false,
            reason: "no_assistant",
            error: "pi agent_end did not include an assistant message",
            durationMs: expect.any(Number),
            meta: { stderr: undefined, sawProtocolOutput: true },
        });
    });

    it("returns no_assistant for empty assistant text", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "   " }],
                    stopReason: "stop",
                },
            ]),
        );
        child.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: false,
            reason: "no_assistant",
            error: "pi assistant produced empty text",
            durationMs: expect.any(Number),
            meta: { stderr: undefined, sawProtocolOutput: true },
        });
    });

    it("returns no_assistant for empty stdout and successful exit", async () => {
        // An exit-0 primary with no stdout triggers one isolated retry. If that retry also exits 0 with no output, the run returns no_assistant with the no-protocol-output marker.
        const first = createMockChild();
        const second = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second]);

        const resultPromise = runner.run(baseOptions);
        first.emitClose(0);
        await nextTick();
        second.emitClose(0);

        const result = await resultPromise;

        expect(result.ok).toBe(false);
        if (!result.ok) {
            expect(result.reason).toBe("no_assistant");
            expect(result.error).toContain("without emitting agent_end");
            expect(result.meta).toEqual({
                stderr: undefined,
                exitCode: 0,
                signal: null,
                sawProtocolOutput: false,
            });
        }
        expect(spawnImpl).toHaveBeenCalledTimes(2);
        expect(spawnImpl.mock.calls[0]?.[1]).not.toContain("--no-extensions");
        expect(spawnImpl.mock.calls[1]?.[1]).toContain("--no-extensions");
    });

    it("returns non_zero_exit with stderr and exit metadata", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeStderr("auth missing");
        child.emitClose(7);

        const result = await resultPromise;

        expect(result.ok).toBe(false);
        if (!result.ok) {
            expect(result.reason).toBe("non_zero_exit");
            expect(result.error).toContain("code=7");
            expect(result.error).toContain("auth missing");
            expect(result.meta).toEqual({
                stderr: "auth missing",
                exitCode: 7,
                signal: null,
            });
        }
    });

    it("retries a translated provider with the canonical form after a missing-key exit", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second]);

        const resultPromise = runner.run({
            ...baseOptions,
            model: "openai/gpt-5.5",
        });
        first.writeStderr("No API key found for openai-codex. Use /login to authenticate.");
        first.emitClose(1);
        await nextTick();
        second.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "direct API success" }],
                    stopReason: "stop",
                },
            ]),
        );
        second.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: true,
            assistantText: "direct API success",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
        expect(spawnImpl).toHaveBeenCalledTimes(2);
        expect(spawnImpl.mock.calls[0]?.[1]).toEqual(
            expect.arrayContaining(["--model", "openai-codex/gpt-5.5"]),
        );
        expect(spawnImpl.mock.calls[1]?.[1]).toEqual(
            expect.arrayContaining(["--model", "openai/gpt-5.5"]),
        );
    });

    it("caches the provider form that succeeds for later spawns", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const third = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second, third]);

        const firstRun = runner.run({ ...baseOptions, model: "openai/gpt-5.5" });
        first.writeStderr("No API key found for openai-codex. Use /login to authenticate.");
        first.emitClose(1);
        await nextTick();
        second.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "first direct success" }],
                    stopReason: "stop",
                },
            ]),
        );
        second.emitClose(0);
        await firstRun;

        const secondRun = runner.run({ ...baseOptions, model: "openai/gpt-5.4" });
        third.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "cached direct success" }],
                    stopReason: "stop",
                },
            ]),
        );
        third.emitClose(0);
        await secondRun;

        expect(spawnImpl).toHaveBeenCalledTimes(3);
        expect(spawnImpl.mock.calls[2]?.[1]).toEqual(
            expect.arrayContaining(["--model", "openai/gpt-5.4"]),
        );
    });

    it("retries the translated form when the cached canonical form loses its credentials", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const third = createMockChild();
        const fourth = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second, third, fourth]);

        const firstRun = runner.run({ ...baseOptions, model: "openai/gpt-5.5" });
        first.writeStderr("No API key found for openai-codex. Use /login to authenticate.");
        first.emitClose(1);
        await nextTick();
        second.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "direct" }],
                    stopReason: "stop",
                },
            ]),
        );
        second.emitClose(0);
        await firstRun;

        const secondRun = runner.run({ ...baseOptions, model: "openai/gpt-5.5" });
        third.writeStderr("No API key found for openai. Use /login to authenticate.");
        third.emitClose(1);
        await nextTick();
        fourth.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "codex success" }],
                    stopReason: "stop",
                },
            ]),
        );
        fourth.emitClose(0);

        const result = await secondRun;
        expect(result.ok).toBe(true);
        expect(spawnImpl).toHaveBeenCalledTimes(4);
        expect(spawnImpl.mock.calls[2]?.[1]).toEqual(
            expect.arrayContaining(["--model", "openai/gpt-5.5"]),
        );
        expect(spawnImpl.mock.calls[3]?.[1]).toEqual(
            expect.arrayContaining(["--model", "openai-codex/gpt-5.5"]),
        );
    });

    it("does not provider-retry an unrelated stderr failure", async () => {
        const first = createMockChild();
        const { runner, spawnImpl } = runnerWith(first);

        const resultPromise = runner.run({
            ...baseOptions,
            model: "openai/gpt-5.5",
        });
        first.writeStderr("No API key found for another-provider. Check configuration.");
        first.emitClose(1);

        const result = await resultPromise;
        expect(result.ok).toBe(false);
        expect(spawnImpl).toHaveBeenCalledTimes(1);
    });

    it("retries google's translated provider with canonical google", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second]);

        const resultPromise = runner.run({
            ...baseOptions,
            model: "google/gemini-2.5-pro",
        });
        first.writeStderr("No API key found for google-antigravity. Use /login to authenticate.");
        first.emitClose(1);
        await nextTick();
        second.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "google API success" }],
                    stopReason: "stop",
                },
            ]),
        );
        second.emitClose(0);
        await resultPromise;

        expect(spawnImpl).toHaveBeenCalledTimes(2);
        expect(spawnImpl.mock.calls[0]?.[1]).toEqual(
            expect.arrayContaining(["--model", "google-antigravity/gemini-2.5-pro"]),
        );
        expect(spawnImpl.mock.calls[1]?.[1]).toEqual(
            expect.arrayContaining(["--model", "google/gemini-2.5-pro"]),
        );
    });

    it("bounds provider and extension retries to three spawns", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const third = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second, third]);
        const logSpy = spyOn(loggerModule, "sessionLog").mockImplementation(() => {});

        try {
            const resultPromise = runner.run({
                ...baseOptions,
                model: "openai/gpt-5.5",
            });
            first.writeStderr("No API key found for openai-codex. Use /login to authenticate.");
            first.emitClose(1);
            await nextTick();
            second.writeStderr(COLLISION_STDERR);
            second.emitClose(1);
            await nextTick();
            third.writeStdoutLine(
                agentEnd([
                    {
                        role: "assistant",
                        content: [{ type: "text", text: "bounded success" }],
                        stopReason: "stop",
                    },
                ]),
            );
            third.emitClose(0);

            expect(await resultPromise).toEqual({
                ok: true,
                assistantText: "bounded success",
                toolCallCount: 0,
                durationMs: expect.any(Number),
                meta: { stderr: undefined },
            });
            expect(spawnImpl).toHaveBeenCalledTimes(3);
            expect(spawnImpl.mock.calls[0]?.[1]).toEqual(
                expect.arrayContaining(["--model", "openai-codex/gpt-5.5"]),
            );
            expect(spawnImpl.mock.calls[1]?.[1]).toEqual(
                expect.arrayContaining(["--model", "openai/gpt-5.5"]),
            );
            expect(spawnImpl.mock.calls[1]?.[1]).not.toContain("--no-extensions");
            expect(spawnImpl.mock.calls[2]?.[1]).toEqual(
                expect.arrayContaining(["--model", "openai/gpt-5.5"]),
            );
            expect(spawnImpl.mock.calls[2]?.[1]).toContain("--no-extensions");
        } finally {
            logSpy.mockRestore();
        }
    });

    it("retries the canonical primary before advancing to fallback models", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const third = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second, third]);

        const resultPromise = runner.run({
            ...baseOptions,
            model: "openai/gpt-5.5",
            fallbackModels: ["anthropic/fallback"],
        });
        first.writeStderr("No API key found for openai-codex. Use /login to authenticate.");
        first.emitClose(1);
        await nextTick();
        second.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "partial" }],
                    stopReason: "error",
                    errorMessage: "rate limited",
                },
            ]),
        );
        second.emitClose(0);
        await nextTick();
        third.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "fallback success" }],
                    stopReason: "stop",
                },
            ]),
        );
        third.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: true,
            assistantText: "fallback success",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
        expect(spawnImpl).toHaveBeenCalledTimes(3);
        expect(spawnImpl.mock.calls[0]?.[1]).toEqual(
            expect.arrayContaining(["--model", "openai-codex/gpt-5.5"]),
        );
        expect(spawnImpl.mock.calls[1]?.[1]).toEqual(
            expect.arrayContaining(["--model", "openai/gpt-5.5"]),
        );
        expect(spawnImpl.mock.calls[2]?.[1]).toEqual(
            expect.arrayContaining(["--model", "anthropic/fallback"]),
        );
    });

    it("retries the canonical form for a translated fallback model after a missing-key exit", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const third = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second, third]);

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/primary",
            fallbackModels: ["openai/fallback"],
        });
        first.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "partial" }],
                    stopReason: "error",
                    errorMessage: "rate limited",
                },
            ]),
        );
        first.emitClose(0);
        await nextTick();
        second.writeStderr("No API key found for openai-codex. Use /login to authenticate.");
        second.emitClose(1);
        await nextTick();
        third.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "direct fallback success" }],
                    stopReason: "stop",
                },
            ]),
        );
        third.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: true,
            assistantText: "direct fallback success",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
        expect(spawnImpl).toHaveBeenCalledTimes(3);
        expect(spawnImpl.mock.calls[1]?.[1]).toEqual(
            expect.arrayContaining(["--model", "openai-codex/fallback"]),
        );
        expect(spawnImpl.mock.calls[2]?.[1]).toEqual(
            expect.arrayContaining(["--model", "openai/fallback"]),
        );
    });

    it("reuses the provider form the primary settled on for a fallback on the same provider", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const third = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second, third]);

        const resultPromise = runner.run({
            ...baseOptions,
            model: "openai/gpt-5.5",
            fallbackModels: ["openai/fallback"],
        });
        first.writeStderr("No API key found for openai-codex. Use /login to authenticate.");
        first.emitClose(1);
        await nextTick();
        second.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "partial" }],
                    stopReason: "error",
                    errorMessage: "rate limited",
                },
            ]),
        );
        second.emitClose(0);
        await nextTick();
        third.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "fallback success" }],
                    stopReason: "stop",
                },
            ]),
        );
        third.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: true,
            assistantText: "fallback success",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
        // The fallback skips the openai-codex form that already failed the credential check.
        expect(spawnImpl).toHaveBeenCalledTimes(3);
        expect(spawnImpl.mock.calls[2]?.[1]).toEqual(
            expect.arrayContaining(["--model", "openai/fallback"]),
        );
    });

    it("resumes the isolated retry at the fallback that hit the extension collision", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const third = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second, third]);
        const logSpy = spyOn(loggerModule, "sessionLog").mockImplementation(() => {});

        try {
            const resultPromise = runner.run({
                ...baseOptions,
                model: "anthropic/primary",
                fallbackModels: ["anthropic/fallback"],
            });
            first.writeStdoutLine(
                agentEnd([
                    {
                        role: "assistant",
                        content: [{ type: "text", text: "partial" }],
                        stopReason: "error",
                        errorMessage: "rate limited",
                    },
                ]),
            );
            first.emitClose(0);
            await nextTick();
            second.writeStderr(COLLISION_STDERR);
            second.emitClose(1);
            await nextTick();
            third.writeStdoutLine(
                agentEnd([
                    {
                        role: "assistant",
                        content: [{ type: "text", text: "isolated fallback success" }],
                        stopReason: "stop",
                    },
                ]),
            );
            third.emitClose(0);

            expect(await resultPromise).toEqual({
                ok: true,
                assistantText: "isolated fallback success",
                toolCallCount: 0,
                durationMs: expect.any(Number),
                meta: { stderr: undefined },
            });
            // The rejected primary is not spawned again; the isolated retry targets the fallback.
            expect(spawnImpl).toHaveBeenCalledTimes(3);
            expect(spawnImpl.mock.calls[2]?.[1]).toEqual(
                expect.arrayContaining(["--model", "anthropic/fallback", "--no-extensions"]),
            );
        } finally {
            logSpy.mockRestore();
        }
    });

    it("retries once with --no-extensions after an extension turn collision", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second]);
        const logSpy = spyOn(loggerModule, "sessionLog").mockImplementation(() => {});

        try {
            const resultPromise = runner.run({
                ...baseOptions,
                model: "anthropic/claude-sonnet",
            });
            first.writeStderr(COLLISION_STDERR);
            first.emitClose(1);
            await nextTick();
            second.writeStdoutLine(
                agentEnd([
                    {
                        role: "assistant",
                        content: [{ type: "text", text: "isolated success" }],
                        stopReason: "stop",
                    },
                ]),
            );
            second.emitClose(0);

            expect(await resultPromise).toEqual({
                ok: true,
                assistantText: "isolated success",
                toolCallCount: 0,
                durationMs: expect.any(Number),
                meta: { stderr: undefined },
            });
            expect(spawnImpl).toHaveBeenCalledTimes(2);
            expect(spawnImpl.mock.calls[0]?.[1]).not.toContain("--no-extensions");
            expect(spawnImpl.mock.calls[1]?.[1]).toContain("--no-extensions");
            expect(
                logSpy.mock.calls.some(
                    (call) => call[0] === "pi-subagent" && call[1] === ISOLATED_RETRY_LOG_MESSAGE,
                ),
            ).toBe(true);
        } finally {
            logSpy.mockRestore();
        }
    });

    it("does not retry forever when the isolated retry hits the same collision", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second]);

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/claude-sonnet",
        });
        first.writeStderr(COLLISION_STDERR);
        first.emitClose(1);
        await nextTick();
        second.writeStderr(COLLISION_STDERR);
        second.emitClose(1);

        const result = await resultPromise;
        expect(result.ok).toBe(false);
        if (!result.ok) {
            expect(result.reason).toBe("non_zero_exit");
            expect(result.meta).toEqual({
                stderr: COLLISION_STDERR,
                exitCode: 1,
                signal: null,
            });
        }
        expect(spawnImpl).toHaveBeenCalledTimes(2);
        expect(spawnImpl.mock.calls[1]?.[1]).toContain("--no-extensions");
    });

    it("does not insert an isolated retry for unrelated failures", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second]);

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/primary",
            fallbackModels: ["openai/fallback"],
        });
        first.writeStderr("auth missing");
        first.emitClose(1);
        await nextTick();
        second.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "fallback success" }],
                    stopReason: "stop",
                },
            ]),
        );
        second.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: true,
            assistantText: "fallback success",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
        expect(spawnImpl).toHaveBeenCalledTimes(2);
        expect(spawnImpl.mock.calls[0]?.[1]).not.toContain("--no-extensions");
        expect(spawnImpl.mock.calls[1]?.[1]).not.toContain("--no-extensions");
        expect(spawnImpl.mock.calls[1]?.[1]).toEqual(
            expect.arrayContaining(["--model", "openai-codex/fallback"]),
        );
    });

    it("does not start a retry loop when the allowlist already disables discovery", async () => {
        const first = createMockChild();
        const { runner, spawnImpl } = runnerWith(first, {
            subagentExtensions: ["provider-package", "./provider.ts"],
        });
        const logSpy = spyOn(loggerModule, "sessionLog").mockImplementation(() => {});

        try {
            const resultPromise = runner.run({
                ...baseOptions,
                model: "anthropic/primary",
            });
            first.writeStderr(COLLISION_STDERR);
            first.emitClose(1);

            const result = await resultPromise;
            expect(result.ok).toBe(false);
            expect(spawnImpl).toHaveBeenCalledTimes(1);
            const args = spawnImpl.mock.calls[0]?.[1] as string[];
            expect(args.filter((arg) => arg === "--no-extensions")).toHaveLength(1);
            const firstExtension = args.indexOf("--extension");
            expect(args.slice(firstExtension, firstExtension + 4)).toEqual([
                "--extension",
                join(homedir(), ".pi/agent/provider-package"),
                "--extension",
                join(homedir(), ".pi/agent/provider.ts"),
            ]);
            expect(logSpy.mock.calls.some((call) => call[1] === ISOLATED_RETRY_LOG_MESSAGE)).toBe(
                false,
            );
        } finally {
            logSpy.mockRestore();
        }
    });

    it("does not start a retry loop when the spawn already disables extensions", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second], {
            extraArgs: ["--no-extensions"],
        });
        const logSpy = spyOn(loggerModule, "sessionLog").mockImplementation(() => {});

        try {
            const resultPromise = runner.run({
                ...baseOptions,
                model: "anthropic/primary",
                fallbackModels: ["openai/fallback"],
            });
            first.writeStderr(COLLISION_STDERR);
            first.emitClose(1);
            await nextTick();
            second.writeStdoutLine(
                agentEnd([
                    {
                        role: "assistant",
                        content: [{ type: "text", text: "fallback without retry loop" }],
                        stopReason: "stop",
                    },
                ]),
            );
            second.emitClose(0);

            expect(await resultPromise).toEqual({
                ok: true,
                assistantText: "fallback without retry loop",
                toolCallCount: 0,
                durationMs: expect.any(Number),
                meta: { stderr: undefined },
            });
            expect(spawnImpl).toHaveBeenCalledTimes(2);
            expect(spawnImpl.mock.calls[0]?.[1]).toContain("--no-extensions");
            expect(spawnImpl.mock.calls[1]?.[1]).toContain("--no-extensions");
            expect(logSpy.mock.calls.some((call) => call[1] === ISOLATED_RETRY_LOG_MESSAGE)).toBe(
                false,
            );
        } finally {
            logSpy.mockRestore();
        }
    });

    it("logs model-unavailable guidance when the isolated retry loses an extension-only model", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const { runner } = runnerWith([first, second]);
        const logSpy = spyOn(loggerModule, "sessionLog").mockImplementation(() => {});

        try {
            const resultPromise = runner.run({
                ...baseOptions,
                model: "openai/extension-model",
            });
            first.writeStderr(COLLISION_STDERR);
            first.emitClose(1);
            await nextTick();
            second.writeStderr("Unknown model openai-codex/extension-model");
            second.emitClose(1);

            const result = await resultPromise;
            expect(result.ok).toBe(false);
            if (!result.ok) {
                expect(result.reason).toBe("non_zero_exit");
                expect(result.error).toContain(ISOLATED_RETRY_MODEL_UNAVAILABLE_LOG_MESSAGE);
                expect(result.error).toContain("Original failure:");
            }
            expect(logSpy.mock.calls.some((call) => call[1] === ISOLATED_RETRY_LOG_MESSAGE)).toBe(
                true,
            );
            expect(
                logSpy.mock.calls.some(
                    (call) => call[1] === ISOLATED_RETRY_MODEL_UNAVAILABLE_LOG_MESSAGE,
                ),
            ).toBe(true);
        } finally {
            logSpy.mockRestore();
        }
    });

    it("does not keep isolated mode for the next run", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const third = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second, third]);

        const degradedRun = runner.run({
            ...baseOptions,
            model: "anthropic/claude-sonnet",
        });
        first.writeStderr(COLLISION_STDERR);
        first.emitClose(1);
        await nextTick();
        second.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "isolated success" }],
                    stopReason: "stop",
                },
            ]),
        );
        second.emitClose(0);
        await degradedRun;

        const freshRun = runner.run({
            ...baseOptions,
            model: "anthropic/claude-sonnet",
        });
        third.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "extensions restored" }],
                    stopReason: "stop",
                },
            ]),
        );
        third.emitClose(0);

        expect(await freshRun).toEqual({
            ok: true,
            assistantText: "extensions restored",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
        expect(spawnImpl.mock.calls[0]?.[1]).not.toContain("--no-extensions");
        expect(spawnImpl.mock.calls[1]?.[1]).toContain("--no-extensions");
        expect(spawnImpl.mock.calls[2]?.[1]).not.toContain("--no-extensions");
    });

    it("retries once with --no-extensions after a silent exit-0 primary (no agent_end, zero stdout)", async () => {
        // An exit-0 primary with no stdout triggers one isolated retry with discovered extensions disabled.
        const first = createMockChild();
        const second = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second]);
        const logSpy = spyOn(loggerModule, "sessionLog").mockImplementation(() => {});

        try {
            const resultPromise = runner.run({
                ...baseOptions,
                model: "anthropic/claude-sonnet",
            });
            first.emitClose(0);
            await nextTick();
            second.writeStdoutLine(
                agentEnd([
                    {
                        role: "assistant",
                        content: [{ type: "text", text: "isolated success" }],
                        stopReason: "stop",
                    },
                ]),
            );
            second.emitClose(0);

            expect(await resultPromise).toEqual({
                ok: true,
                assistantText: "isolated success",
                toolCallCount: 0,
                durationMs: expect.any(Number),
                meta: { stderr: undefined },
            });
            expect(spawnImpl).toHaveBeenCalledTimes(2);
            expect(spawnImpl.mock.calls[0]?.[1]).not.toContain("--no-extensions");
            expect(spawnImpl.mock.calls[1]?.[1]).toContain("--no-extensions");
            expect(
                logSpy.mock.calls.some(
                    (call) =>
                        call[0] === "pi-subagent" && call[1] === ISOLATED_RETRY_SILENT_LOG_MESSAGE,
                ),
            ).toBe(true);
        } finally {
            logSpy.mockRestore();
        }
    });

    it("does not fire the isolated retry when agent_end arrived with empty assistant text", async () => {
        // An observed `agent_end` with whitespace-only assistant text is protocol output.
        // A whitespace-only assistant response returns `no_assistant`.
        // Protocol output falls through to fallback models instead of using the isolated retry.
        // Protocol output suppresses the one-shot isolated retry.
        const child = createMockChild();
        const { runner, spawnImpl } = runnerWith(child);
        const logSpy = spyOn(loggerModule, "sessionLog").mockImplementation(() => {});

        try {
            const resultPromise = runner.run({
                ...baseOptions,
                model: "anthropic/claude-sonnet",
            });
            child.writeStdoutLine(
                agentEnd([
                    {
                        role: "assistant",
                        content: [{ type: "text", text: "   " }],
                        stopReason: "stop",
                    },
                ]),
            );
            child.emitClose(0);

            const result = await resultPromise;
            expect(result.ok).toBe(false);
            if (!result.ok) {
                expect(result.reason).toBe("no_assistant");
                expect(result.meta).toEqual({
                    stderr: undefined,
                    sawProtocolOutput: true,
                });
            }
            expect(spawnImpl).toHaveBeenCalledTimes(1);
            expect(spawnImpl.mock.calls[0]?.[1]).not.toContain("--no-extensions");
            expect(
                logSpy.mock.calls.some((call) => call[1] === ISOLATED_RETRY_SILENT_LOG_MESSAGE),
            ).toBe(false);
        } finally {
            logSpy.mockRestore();
        }
    });

    it("isolated retry argv keeps explicit --extension entries while dropping discovered extensions", () => {
        // `disableDiscoveredExtensions: true` adds `--no-extensions` for discovered user extensions.
        // Explicit `--extension` entries include the subagent-entry extension and user-tier allowlist entries.
        // The subagent-entry and allowlisted user extensions supply the child models and tools.
        const args = buildArgsForTest(
            {
                ...baseOptions,
                agent: "sidekick",
                model: "anthropic/claude-sonnet",
            },
            {
                disableDiscoveredExtensions: true,
                subagentEntryPath: "/tmp/subagent-entry.js",
                subagentExtensions: ["provider-package"],
            },
        );

        expect(args).toContain("--no-extensions");
        expect(args).toEqual(expect.arrayContaining(["--extension", "/tmp/subagent-entry.js"]));
        expect(args).toEqual(
            expect.arrayContaining(["--extension", join(homedir(), ".pi/agent/provider-package")]),
        );
    });

    it("returns parse_failed when stdout is missing", async () => {
        const child = createMockChild({ stdout: false });
        const { runner } = runnerWith(child);

        expect(await runner.run(baseOptions)).toEqual({
            ok: false,
            reason: "parse_failed",
            error: "pi child process did not expose stdout (stdio misconfigured)",
            durationMs: expect.any(Number),
        });
    });

    it("passes fallback models, cwd, prompt arguments, and merged subagent env through spawn", async () => {
        const child = createMockChild();
        const { runner, spawnImpl } = runnerWith(child);

        const resultPromise = runner.run({
            ...baseOptions,
            // Sidekick's --tools allow-list must not alter the model, cwd, prompt, or env passed to spawn.
            agent: "sidekick",
            model: "anthropic/primary",
            fallbackModels: ["openai/fallback"],
            cwd: "/workspace/project",
            timeoutMs: 500,
        });
        child.writeStdoutLine(
            agentEnd([{ role: "assistant", content: [{ type: "text", text: "done" }] }]),
        );
        child.emitClose(0);
        await resultPromise;

        expect(spawnImpl).toHaveBeenCalledWith(
            "pi-test",
            expect.any(Array),
            expect.objectContaining({
                cwd: "/workspace/project",
                env: expect.objectContaining({
                    ...process.env,
                    EIDNARA_PI_SUBAGENT: "1",
                }),
            }),
        );
        const spawnArgs = spawnImpl.mock.calls[0]?.[1] as string[] | undefined;
        expect(spawnArgs).toEqual([
            "--print",
            "--mode",
            "json",
            "--no-session",
            "--no-skills",
            "--no-prompt-templates",
            "--no-context-files",
            "--tools",
            "read,grep,find,ls,ctx_search",
            "--system-prompt",
            expect.stringMatching(/system-prompt\.txt$/),
            "--model",
            "anthropic/primary",
            "summarize this session",
        ]);
        const spawnOptions = spawnImpl.mock.calls[0]?.[2] as
            | { env?: NodeJS.ProcessEnv }
            | undefined;
        expect(spawnOptions?.env).not.toBe(process.env);
    });

    it("keeps the Eidnara env guard when the extension allowlist is active", async () => {
        const child = createMockChild();
        const { runner, spawnImpl } = runnerWith(child, {
            subagentExtensions: ["provider-package"],
        });

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/model",
        });
        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "allowlisted" }],
                    stopReason: "stop",
                },
            ]),
        );
        child.emitClose(0);
        await resultPromise;

        const spawnOptions = spawnImpl.mock.calls[0]?.[2] as
            | { env?: NodeJS.ProcessEnv }
            | undefined;
        expect(spawnOptions?.env).toEqual(expect.objectContaining({ EIDNARA_PI_SUBAGENT: "1" }));
    });

    it("does not let a post-terminal child signal override captured success", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const resultPromise = runner.run(baseOptions);
        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "looks done" }],
                    stopReason: "stop",
                },
            ]),
        );
        child.writeStderr("process reported late noise");
        child.emitClose(null, "SIGTERM");

        expect(await resultPromise).toEqual({
            ok: true,
            assistantText: "looks done",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: "process reported late noise" },
        });
    });

    it("keeps the host default model as the first attempt when model is omitted", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second]);

        const resultPromise = runner.run({
            ...baseOptions,
            model: undefined,
            fallbackModels: ["anthropic/fallback"],
        });
        first.writeStderr("default model exploded");
        first.emitClose(1);
        await nextTick();
        second.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "fallback success" }],
                    stopReason: "stop",
                },
            ]),
        );
        second.emitClose(0);

        const result = await resultPromise;
        expect(result.ok).toBe(true);
        expect(spawnImpl).toHaveBeenCalledTimes(2);
        expect(spawnImpl.mock.calls[0]?.[1]).not.toContain("--model");
        expect(spawnImpl.mock.calls[1]?.[1]).toEqual(
            expect.arrayContaining(["--model", "anthropic/fallback"]),
        );
    });

    it("retries fallback models by spawning fresh children", async () => {
        const first = createMockChild();
        const second = createMockChild();
        let spawnCount = 0;
        const spawnImpl = mock(() => {
            spawnCount += 1;
            return (spawnCount === 1 ? first : second) as never;
        });
        const runner = new PiSubagentRunner({
            piBinary: "pi-test",
            spawnImpl: spawnImpl as never,
        });

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/primary",
            fallbackModels: ["openai/fallback"],
        });
        first.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "bad" }],
                    stopReason: "error",
                },
            ]),
        );
        first.emitClose(0);
        await new Promise((resolve) => setTimeout(resolve, 0));
        second.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "good" }],
                    stopReason: "stop",
                },
            ]),
        );
        second.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: true,
            assistantText: "good",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
        expect(spawnImpl).toHaveBeenCalledTimes(2);
        expect(spawnImpl.mock.calls[0]?.[1]).toEqual(
            expect.arrayContaining(["--model", "anthropic/primary"]),
        );
        // The spawn boundary translates OpenCode's canonical `openai/` provider prefix to Pi's `openai-codex/` prefix.
        expect(spawnImpl.mock.calls[1]?.[1]).toEqual(
            expect.arrayContaining(["--model", "openai-codex/fallback"]),
        );
    });

    it("retries fallback models after empty assistant text", async () => {
        const first = createMockChild();
        const second = createMockChild();
        let spawnCount = 0;
        const spawnImpl = mock(() => {
            spawnCount += 1;
            return (spawnCount === 1 ? first : second) as never;
        });
        const runner = new PiSubagentRunner({
            piBinary: "pi-test",
            spawnImpl: spawnImpl as never,
        });

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/primary",
            fallbackModels: ["openai/fallback"],
        });
        first.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: " " }],
                    stopReason: "stop",
                },
            ]),
        );
        first.emitClose(0);
        await new Promise((resolve) => setTimeout(resolve, 0));
        second.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "fallback text" }],
                    stopReason: "stop",
                },
            ]),
        );
        second.emitClose(0);

        expect(await resultPromise).toEqual({
            ok: true,
            assistantText: "fallback text",
            toolCallCount: 0,
            durationMs: expect.any(Number),
            meta: { stderr: undefined },
        });
        expect(spawnImpl).toHaveBeenCalledTimes(2);
    });

    it("returns timeout and terminates a child that never closes", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);

        const result = await runner.run({ ...baseOptions, timeoutMs: 20 });

        expect(result.ok).toBe(false);
        if (!result.ok) {
            expect(result.reason).toBe("timeout");
            expect(result.error).toContain("20ms");
        }
        expect(child.kill).toHaveBeenCalledWith("SIGTERM");
        expect(child.killSignals).toEqual(["SIGTERM"]);
    });

    it("does not emit child_exit after a timeout has already settled the run", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);
        const eventTypes: string[] = [];

        const result = await runner.run({
            ...baseOptions,
            timeoutMs: 20,
            onProgress: (event) => {
                eventTypes.push(event.type);
            },
        });
        expect(result.ok).toBe(false);
        child.emitClose(null, "SIGTERM");
        await nextTick();

        expect(eventTypes).not.toContain("child_exit");
    });

    it("measures durationMs across every attempt in the retry chain", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const { runner } = runnerWith([first, second]);

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/primary",
            fallbackModels: ["anthropic/fallback"],
        });
        // The first attempt is the slow one; the fallback settles on the next tick.
        await new Promise((resolve) => setTimeout(resolve, 40));
        first.writeStderr("boom");
        first.emitClose(1);
        await nextTick();
        second.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "late success" }],
                    stopReason: "stop",
                },
            ]),
        );
        second.emitClose(0);

        const result = await resultPromise;
        expect(result.ok).toBe(true);
        expect(result.durationMs).toBeGreaterThanOrEqual(35);
    });

    it("drops stderr and stdout progress events after a timeout has settled the run", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);
        const eventTypes: string[] = [];

        const result = await runner.run({
            ...baseOptions,
            timeoutMs: 20,
            onProgress: (event) => {
                eventTypes.push(event.type);
            },
        });
        expect(result.ok).toBe(false);
        const settledEventCount = eventTypes.length;
        child.writeStderr("late diagnostics while terminating");
        child.writeStdoutLine({ type: "agent_start" });
        child.emitClose(null, "SIGTERM");
        await nextTick();

        expect(eventTypes).toHaveLength(settledEventCount);
    });

    it("gives later attempts only the time left under one run deadline", async () => {
        const first = createMockChild();
        const second = createMockChild();
        const { runner, spawnImpl } = runnerWith([first, second]);

        const resultPromise = runner.run({
            ...baseOptions,
            model: "anthropic/primary",
            fallbackModels: ["anthropic/fallback"],
            timeoutMs: 120,
        });
        // The primary spends most of the budget before failing; the fallback never answers.
        await new Promise((resolve) => setTimeout(resolve, 70));
        first.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "partial" }],
                    stopReason: "error",
                    errorMessage: "provider exploded",
                },
            ]),
        );
        first.emitClose(0);

        const result = await resultPromise;
        expect(result.ok).toBe(false);
        if (!result.ok) {
            expect(result.reason).toBe("timeout");
            expect(result.error).toContain("120ms");
        }
        expect(spawnImpl).toHaveBeenCalledTimes(2);
        // A fresh per-attempt budget would run to at least 70 + 120 ms.
        expect(result.durationMs).toBeLessThan(170);
        expect(second.kill).toHaveBeenCalledWith("SIGTERM");
    });

    it("returns invalid_prompt for an unknown agent with an empty system prompt", async () => {
        const child = createMockChild();
        const { runner, spawnImpl } = runnerWith(child);

        const result = await runner.run({
            ...baseOptions,
            agent: "future-agent",
            systemPrompt: "   ",
        });

        expect(result).toEqual({
            ok: false,
            reason: "invalid_prompt",
            error: 'zero-tool Pi subagent "future-agent" requires a non-empty system prompt',
            durationMs: expect.any(Number),
            transient: true,
        });
        expect(spawnImpl).not.toHaveBeenCalled();
    });

    it("returns abort without spawning when caller signal is already aborted", async () => {
        const child = createMockChild();
        const { runner, spawnImpl } = runnerWith(child);
        const controller = new AbortController();
        controller.abort();

        const result = await runner.run({
            ...baseOptions,
            signal: controller.signal,
        });

        expect(spawnImpl).not.toHaveBeenCalled();
        expect(result.ok).toBe(false);
        if (!result.ok) {
            expect(result.reason).toBe("abort");
        }
        expect(child.kill).not.toHaveBeenCalled();
    });

    it("returns abort and terminates the child when the caller signal aborts", async () => {
        const child = createMockChild();
        const { runner, spawnImpl } = runnerWith(child);
        const controller = new AbortController();

        const resultPromise = runner.run({
            ...baseOptions,
            signal: controller.signal,
        });
        controller.abort();

        const result = await resultPromise;

        expect(spawnImpl).toHaveBeenCalledTimes(1);
        expect(result.ok).toBe(false);
        if (!result.ok) {
            expect(result.reason).toBe("abort");
            expect(result.error).toContain("aborted by caller");
        }
        expect(child.kill).toHaveBeenCalledWith("SIGTERM");
        expect(child.killSignals).toEqual(["SIGTERM"]);
    });

    it("returns abort when the caller aborts from inside the spawned progress callback", async () => {
        const child = createMockChild();
        const { runner } = runnerWith(child);
        const controller = new AbortController();

        const resultPromise = runner.run({
            ...baseOptions,
            signal: controller.signal,
            onProgress: (event) => {
                if (event.type === "spawned") controller.abort();
            },
        });
        // A child that answers anyway must not turn the aborted run into a success.
        child.writeStdoutLine(
            agentEnd([
                {
                    role: "assistant",
                    content: [{ type: "text", text: "too late" }],
                    stopReason: "stop",
                },
            ]),
        );
        child.emitClose(0);

        const result = await resultPromise;
        expect(result.ok).toBe(false);
        if (!result.ok) {
            expect(result.reason).toBe("abort");
        }
        expect(child.kill).toHaveBeenCalledWith("SIGTERM");
    });

    it("does not send SIGKILL when child exits after SIGTERM before escalation timeout", async () => {
        const child = createMockChild();

        __test.terminateChild(child as never);
        child.emitExit(0, null);
        await new Promise((resolve) => setTimeout(resolve, 2100));

        expect(child.killSignals).toEqual(["SIGTERM"]);
    });

    it("sends SIGKILL when child remains alive past escalation timeout", async () => {
        const child = createMockChild();

        __test.terminateChild(child as never);
        await new Promise((resolve) => setTimeout(resolve, 2100));

        expect(child.killSignals).toEqual(["SIGTERM", "SIGKILL"]);
    });
});

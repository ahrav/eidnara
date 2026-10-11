import { spawn } from "node:child_process";
import {
    existsSync,
    mkdirSync,
    readdirSync,
    readFileSync,
    readlinkSync,
    realpathSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
    connectionFilePath,
    managedSubtreePath,
} from "@eidnara/opencode/shared/host-lifecycle/paths";
import {
    detectHarnessRuntimeSources,
    ensurePiInstall,
    writeHarnessRuntime,
} from "../bedrock-peer/harness-runtime";
import { type SpawnedOpencode, spawnOpencode } from "../opencode-runner/spawn";
import { type PiRpcEvent, PiRpcProcess, PiRpcProtocol } from "../pi-runner/rpc-client";
import { HermeticHostStack } from "../rust-runner/hermetic-host";
import { isSensitiveEnvKey } from "../secret-env-keys";
import { StillRunningError, settleWithin } from "./deadline";
import { BedrockGateway, type CallRecord, type TurnScope } from "./gateway";
import { running, stopOwnedTree } from "./procs";
import {
    ensureSandboxScript,
    nodeRoot,
    opencodeBinary,
    sandboxKeep,
    sandboxPath,
    shellQuote,
} from "./sandbox";

const REPO_ROOT = resolve(import.meta.dir, "../../../..");
const PI_PLUGIN_ROOT = join(REPO_ROOT, "packages/pi-plugin");

export const MODEL = "global.anthropic.claude-opus-5-5";
export const MODEL_REF = `amazon-bedrock/${MODEL}`;
export const CONTEXT_LIMIT = 200_000;

/** A timed-out prompt gets this long to stop after its abort before the arm fails. */
const DRAIN_MS = 60_000;

const DUMMY_AWS = {
    AWS_ACCESS_KEY_ID: "AKIDABEVALPLACEHOLDER",
    AWS_SECRET_ACCESS_KEY: "ab-eval/placeholder+secret",
    AWS_REGION: "us-west-2",
};

export interface ArmSpec {
    name: string;
    harness: "pi" | "opencode";
    eidnara: boolean;
    stripClosureTemperature: boolean;
}

/**
 * The `on` arms strip the temperature from every Eidnara model call at the gateway; `pi-onraw`
 * forwards those calls as the plugin builds them.
 */
const ARM_SPECS: Record<string, Omit<ArmSpec, "name">> = {
    "pi-off": { harness: "pi", eidnara: false, stripClosureTemperature: false },
    "pi-on": { harness: "pi", eidnara: true, stripClosureTemperature: true },
    "pi-onraw": { harness: "pi", eidnara: true, stripClosureTemperature: false },
    "oc-off": { harness: "opencode", eidnara: false, stripClosureTemperature: false },
    "oc-on": { harness: "opencode", eidnara: true, stripClosureTemperature: true },
};

export function armSpec(name: string): ArmSpec {
    const spec = ARM_SPECS[name];
    if (!spec) {
        throw new Error(
            `unknown arm ${JSON.stringify(name)}; arms are ${Object.keys(ARM_SPECS).join(", ")}`,
        );
    }
    return { name, ...spec };
}

export interface PromptResult {
    answer: string;
    ms: number;
    error?: string;
}

export interface ArmContext {
    root: string;
    resultsDir: string;
    workdir: string;
    /** Directory holding the sandbox script; empty when arms run unsandboxed. */
    sandboxDir: string;
    /** Refuse main-harness requests past `CONTEXT_LIMIT`, as a model with that window does. */
    enforceWindow: boolean;
    onCall: (record: CallRecord) => void;
    fixtureBin: string;
}

function eidnaraUserConfig(connectionFile: string): Record<string, unknown> {
    return {
        enabled: true,
        host: { connection_file: connectionFile },
        execute_threshold_percentage: 65,
        protected_tags: 20,
        history_budget_percentage: 0.15,
        history_summarizer: { model: MODEL_REF },
        context_researcher: { model: MODEL_REF },
        memory: {
            enabled: true,
            auto_promote: true,
            auto_capture: true,
            auto_search: { enabled: true },
            git_commit_indexing: { enabled: false },
        },
    };
}

function treeBytes(dir: string): number {
    if (!existsSync(dir)) return 0;
    let total = 0;
    const walk = (d: string) => {
        for (const entry of readdirSync(d, { withFileTypes: true })) {
            const p = join(d, entry.name);
            if (entry.isDirectory()) walk(p);
            else if (entry.isFile()) {
                try {
                    total += Bun.file(p).size;
                } catch {
                    total += 0;
                }
            }
        }
    };
    walk(dir);
    return total;
}

export abstract class Arm {
    readonly main: BedrockGateway;
    readonly closure: BedrockGateway;
    host: HermeticHostStack | null = null;
    hostDataDir: string;

    constructor(
        readonly spec: ArmSpec,
        readonly ctx: ArmContext,
    ) {
        const common = {
            arm: spec.name,
            workdir: ctx.workdir,
            dumpDir: ctx.resultsDir,
            onCall: ctx.onCall,
            ...(ctx.enforceWindow ? { windowTokens: CONTEXT_LIMIT } : {}),
        };
        this.main = new BedrockGateway({ ...common, role: "main", stripTemperature: false });
        this.closure = new BedrockGateway({
            ...common,
            role: "closure",
            stripTemperature: spec.stripClosureTemperature,
        });
        this.hostDataDir = join(ctx.root, "host-data");
    }

    async start(): Promise<void> {
        await this.main.start();
        await this.closure.start();
        if (!this.spec.eidnara) return;
        const sources = detectHarnessRuntimeSources();
        if (!sources.ok) throw new Error(`harness runtime sources: ${sources.reason}`);
        const runtimeDir = join(this.ctx.root, "runtime");
        mkdirSync(runtimeDir, { recursive: true, mode: 0o700 });
        mkdirSync(this.hostDataDir, { recursive: true, mode: 0o700 });
        const harnessRuntime = writeHarnessRuntime({
            dir: runtimeDir,
            sources: sources.sources,
            piInstall: ensurePiInstall(),
            opencodeBaseUrl: this.closure.http1Url,
            piBaseUrl: this.closure.h2Url,
            anthropicSentinelUrl: this.closure.http1Url,
            credentials: {
                accessKeyId: DUMMY_AWS.AWS_ACCESS_KEY_ID,
                secretAccessKey: DUMMY_AWS.AWS_SECRET_ACCESS_KEY,
                region: DUMMY_AWS.AWS_REGION,
            },
        });
        this.host = await HermeticHostStack.start({
            dataDir: this.hostDataDir,
            fixtureBin: this.ctx.fixtureBin,
            startTimeoutMs: 300_000,
            harnessRuntime,
            credentialSource: DUMMY_AWS,
            daemonConfig: { history_summarizer: { module_model: MODEL_REF } },
        });
    }

    get connectionFile(): string {
        return connectionFilePath(this.hostDataDir);
    }

    /** The daemon's PID from its record, once the process at that PID runs the recorded executable; a PID can be reused. */
    hostPid(): number | undefined {
        try {
            const record = JSON.parse(
                readFileSync(
                    join(managedSubtreePath(this.hostDataDir), "rust-e2e-pids.json"),
                    "utf8",
                ),
            ) as { pids: Array<{ pid: number; executable: string }> };
            const entry = record.pids[0];
            if (!entry) return undefined;
            const exe = readlinkSync(`/proc/${entry.pid}/exe`).replace(/ \(deleted\)$/, "");
            return exe === entry.executable ? entry.pid : undefined;
        } catch {
            return undefined;
        }
    }

    /**
     * A harness that exited would turn every later turn into an error row, and an Eidnara arm
     * whose daemon exited would fail open for the rest of the run; either ends the arm.
     */
    assertAlive(key: string): void {
        const alive = (pid: number | undefined) => pid !== undefined && running(pid);
        if (!alive(this.harnessPid())) {
            throw new Error(`the harness is not running after turn ${key}`);
        }
        if (this.spec.eidnara && !alive(this.hostPid())) {
            throw new Error(`the host is not running after turn ${key}`);
        }
    }

    setTurn(scope: TurnScope | null): void {
        this.main.scope = scope;
        this.closure.scope = scope && {
            ...scope,
            turn: null,
            probe: null,
            deadlineAt: Number.POSITIVE_INFINITY,
        };
    }

    abstract openSession(index: number): Promise<void>;
    abstract prompt(text: string, timeoutMs: number): Promise<PromptResult>;
    abstract closeSession(): Promise<void>;
    abstract harnessPid(): number | undefined;
    abstract harnessDataDirs(): string[];
    abstract sessionId(): string | null;

    diskBytes(): { harness: number; eidnara: number; closures: number } {
        return {
            harness: this.harnessDataDirs().reduce((sum, dir) => sum + treeBytes(dir), 0),
            eidnara: this.spec.eidnara ? treeBytes(join(this.hostDataDir, "eidnara")) : 0,
            closures: this.spec.eidnara ? treeBytes(join(this.hostDataDir, "harness-closures")) : 0,
        };
    }

    /** Every teardown step runs; a harness or host that would not stop fails the arm afterwards. */
    async stop(): Promise<void> {
        const failures: unknown[] = [];
        await this.closeSession().catch((error) => failures.push(error));
        await this.host?.stop().catch((error) => failures.push(error));
        await this.main.stop();
        await this.closure.stop();
        if (failures.length > 0) throw failures[0];
    }
}

/** The answer and failure of a Pi turn, read from the `agent_end` messages. */
export function piOutcome(messages: Array<Record<string, unknown>>): Omit<PromptResult, "ms"> {
    const textOf = (m: Record<string, unknown> | undefined): string =>
        ((m?.content as Array<Record<string, unknown>> | undefined) ?? [])
            .filter((b) => b.type === "text")
            .map((b) => String(b.text))
            .join("\n")
            .trim();
    // The final assistant message is the turn's result; an earlier message's text would stand
    // in for a response that was aborted or failed.
    const assistants = messages.filter((m) => m.role === "assistant");
    const last = assistants[assistants.length - 1];
    const answer = textOf(last);
    const stop =
        last?.stopReason === "error"
            ? String(last.errorMessage ?? "error")
            : last?.stopReason === "aborted"
              ? "aborted"
              : undefined;
    return { answer, ...(stop ? { error: stop } : {}) };
}

export function inheritedHarnessEnv(source: NodeJS.ProcessEnv): Record<string, string> {
    const env: Record<string, string> = {};
    for (const [k, v] of Object.entries(source)) {
        if (v === undefined || isSensitiveEnvKey(k) || k.startsWith("EIDNARA_") || k === "NODE_ENV")
            continue;
        env[k] = v;
    }
    return env;
}

/** Pi's stdout carried a line outside the RPC protocol, so the arm's result channel is corrupt. */
class RpcOutputError extends Error {}

export class PiArm extends Arm {
    private rpc: PiRpcProcess | null = null;
    private sid: string | null = null;
    readonly agentDir: string;
    readonly configDir: string;
    readonly dataDir: string;

    constructor(spec: ArmSpec, ctx: ArmContext) {
        super(spec, ctx);
        this.agentDir = join(ctx.root, "home/.pi/agent");
        this.configDir = join(ctx.root, "home/config");
        this.dataDir = join(ctx.root, "home/data");
        for (const d of [
            this.agentDir,
            this.configDir,
            this.dataDir,
            join(ctx.root, "home/cache"),
        ]) {
            mkdirSync(d, { recursive: true });
        }
    }

    private writeConfigs(): void {
        const settings = {
            packages: [],
            defaultProvider: "amazon-bedrock",
            defaultModel: MODEL,
            enabledModels: [MODEL_REF],
            compaction: { enabled: true },
            retry: { enabled: true },
            defaultThinkingLevel: "off",
            quietStartup: true,
            enableInstallTelemetry: false,
        };
        writeFileSync(join(this.agentDir, "settings.json"), JSON.stringify(settings, null, 2));
        const models = {
            providers: {
                "amazon-bedrock": {
                    baseUrl: this.main.h2Url,
                    models: [
                        {
                            id: MODEL,
                            name: "Claude Opus 5.5 (eval)",
                            api: "bedrock-converse-stream",
                            reasoning: false,
                            input: ["text"],
                            contextWindow: CONTEXT_LIMIT,
                            maxTokens: 8192,
                            cost: { input: 5, output: 25, cacheRead: 0.5, cacheWrite: 6.25 },
                        },
                    ],
                },
            },
        };
        writeFileSync(join(this.agentDir, "models.json"), JSON.stringify(models, null, 2));
        if (this.spec.eidnara) {
            const path = join(this.configDir, "eidnara", "eidnara.jsonc");
            mkdirSync(join(this.configDir, "eidnara"), { recursive: true });
            writeFileSync(path, JSON.stringify(eidnaraUserConfig(this.connectionFile), null, 2));
        }
    }

    async openSession(_index: number): Promise<void> {
        this.writeConfigs();
        const cli = join(ensurePiCli(), "node_modules/@earendil-works/pi-coding-agent/dist/cli.js");
        const args = [
            cli,
            "--mode",
            "rpc",
            "--no-extensions",
            "--no-skills",
            "--no-prompt-templates",
            "--no-themes",
        ];
        if (this.spec.eidnara) args.push("--extension", PI_PLUGIN_ROOT);
        args.push("--model", MODEL_REF);
        const env = inheritedHarnessEnv(process.env);
        Object.assign(env, DUMMY_AWS, {
            HOME: join(this.ctx.root, "home"),
            PI_CODING_AGENT_DIR: this.agentDir,
            XDG_CONFIG_HOME: this.configDir,
            XDG_DATA_HOME: this.dataDir,
            XDG_CACHE_HOME: join(this.ctx.root, "home/cache"),
            EIDNARA_LOG_PATH: join(this.ctx.root, "eidnara.log"),
            PI_OFFLINE: "1",
            PI_SKIP_VERSION_CHECK: "1",
        });
        env.PATH = sandboxPath(this.spec);
        const node = join(nodeRoot(), "bin/node");
        const [command, argv] = this.ctx.sandboxDir
            ? [
                  ensureSandboxScript(this.ctx.sandboxDir),
                  [...sandboxKeep(this.spec, this.ctx.root), "--", node, ...args],
              ]
            : [node, args];
        const child = spawn(command, argv, {
            cwd: this.ctx.workdir,
            env,
            stdio: ["pipe", "pipe", "pipe"],
        });
        this.rpc = new PiRpcProcess(child, new PiRpcProtocol(), { stderrLimit: 20_000 });
        await Bun.sleep(300);
        const state = await this.command<{ sessionId?: string }>("get_state", {}, 120_000);
        this.sid = state.sessionId ?? null;
    }

    private async command<T>(
        method: string,
        params: Record<string, unknown>,
        timeoutMs: number,
    ): Promise<T> {
        const rpc = this.rpc;
        if (!rpc) throw new Error("pi not running");
        const response = await rpc.sendCommand<T>(method, params, { timeoutMs });
        if (response.success === false)
            throw new Error(`${method} failed: ${JSON.stringify(response)}`);
        return response.data as T;
    }

    async prompt(text: string, timeoutMs: number): Promise<PromptResult> {
        const rpc = this.rpc as PiRpcProcess;
        const started = performance.now();
        const cancel = new AbortController();
        const end = rpc.waitForEvent((event: PiRpcEvent) => event.type === "agent_end", {
            timeoutMs: timeoutMs + 2 * DRAIN_MS,
            label: "agent_end",
            signal: cancel.signal,
        });
        end.catch(() => undefined);
        // The command's own deadline sits past the turn's, so a Pi that stops answering the
        // prompt command is aborted and drained by settleWithin rather than reported as a row
        // error while its run may continue.
        const work = this.command("prompt", { message: text }, timeoutMs + 2 * DRAIN_MS).then(
            () => end,
        );
        try {
            const outcome = await settleWithin(work, timeoutMs, {
                abort: () => this.command("abort", {}, DRAIN_MS),
                drainMs: DRAIN_MS,
            });
            const ms = performance.now() - started;
            // A line outside the RPC protocol on Pi's stdout, during startup or this turn, means
            // the plugin or the CLI broke the channel the arm reads its results through.
            const malformed = rpc.getMalformedLines();
            if (malformed.length > 0) {
                throw new RpcOutputError(
                    `Pi wrote ${malformed.length} line(s) outside the RPC protocol: ${malformed[0]?.slice(0, 200)}`,
                );
            }
            if (outcome.timedOut) return { answer: "", ms, error: "timeout" };
            const messages =
                (outcome.value.messages as Array<Record<string, unknown>> | undefined) ?? [];
            return { ms, ...piOutcome(messages) };
        } catch (error) {
            if (error instanceof StillRunningError || error instanceof RpcOutputError) throw error;
            return {
                answer: "",
                ms: performance.now() - started,
                error: String(error).slice(0, 2000),
            };
        } finally {
            cancel.abort();
        }
    }

    async closeSession(): Promise<void> {
        const rpc = this.rpc;
        if (!rpc) return;
        this.rpc = null;
        const { child } = rpc;
        if (child.exitCode !== null || child.signalCode !== null) return;
        child.stdin?.end();
        if (child.pid !== undefined) await stopOwnedTree(child.pid, 10_000);
        // A Pi process that survives its SIGKILL fails the arm; stop() reports it.
        await rpc.shutdown(5_000);
    }

    harnessPid(): number | undefined {
        return this.rpc?.child.pid;
    }

    harnessDataDirs(): string[] {
        return [join(this.agentDir, "sessions")];
    }

    sessionId(): string | null {
        return this.sid;
    }

    stderrTail(): string {
        return (this.rpc?.getStderr() ?? "").slice(-4000);
    }
}

function ensurePiCli(): string {
    return ensurePiInstall();
}

interface OpencodePromptResponse {
    error?: unknown;
    data?: { parts?: Array<Record<string, unknown>>; info?: Record<string, unknown> };
}

export class OpencodeArm extends Arm {
    private oc: SpawnedOpencode | null = null;
    private servePid: number | undefined;
    private client: any = null;
    private sid: string | null = null;
    private env: { configDir: string; dataDir: string; cacheDir: string; workdir: string };

    constructor(spec: ArmSpec, ctx: ArmContext) {
        super(spec, ctx);
        const base = join(ctx.root, "home");
        this.env = {
            configDir: join(base, "config"),
            dataDir: join(base, "data"),
            cacheDir: join(base, "cache"),
            workdir: ctx.workdir,
        };
        for (const d of [this.env.configDir, this.env.dataDir, this.env.cacheDir])
            mkdirSync(d, { recursive: true });
    }

    async openSession(_index: number): Promise<void> {
        const extra: Record<string, unknown> = {
            model: MODEL_REF,
            small_model: MODEL_REF,
            share: "disabled",
        };
        if (!this.spec.eidnara) {
            extra.plugin = [];
            extra.compaction = { auto: true, prune: true };
        }
        const sbin = join(this.ctx.root, "sbin");
        mkdirSync(sbin, { recursive: true });
        const binary = opencodeBinary();
        const launch = this.ctx.sandboxDir
            ? `exec ${ensureSandboxScript(this.ctx.sandboxDir)} ${sandboxKeep(this.spec, this.ctx.root).join(" ")} -- ${binary} "$@"`
            : `exec ${shellQuote(binary)} "$@"`;
        writeFileSync(
            join(sbin, "opencode"),
            `#!/bin/sh\nexport PATH=${sandboxPath(this.spec)}\n${launch}\n`,
            {
                mode: 0o755,
            },
        );
        this.oc = await spawnOpencode({
            mockProviderURL: this.main.http1Url,
            existingEnv: this.env,
            // An Eidnara arm brings its own host and an off arm has none.
            provisionHost: false,
            modelContextLimit: CONTEXT_LIMIT,
            modelOutputLimit: 8192,
            modelId: MODEL,
            bedrock: {
                baseURL: this.main.http1Url,
                region: DUMMY_AWS.AWS_REGION,
                model: MODEL,
                anthropicSentinelURL: this.main.http1Url,
            },
            openCodeConfigExtra: extra,
            ...(this.spec.eidnara
                ? {
                      eidnaraConfig: eidnaraUserConfig(this.connectionFile),
                      userHostConnectionFile: this.connectionFile,
                  }
                : {}),
            extraEnv: {
                ...DUMMY_AWS,
                HOME: join(this.ctx.root, "home"),
                XDG_STATE_HOME: join(this.ctx.root, "home/state"),
                EIDNARA_LOG_PATH: join(this.ctx.root, "eidnara.log"),
                PATH: `${sbin}:${process.env.PATH ?? ""}`,
            },
        });
        const sdk = await import("@opencode-ai/sdk");
        this.client = sdk.createOpencodeClient({ baseUrl: this.oc.url });
        const created = await this.client.session.create({
            query: { directory: this.ctx.workdir },
        });
        if (!created.data)
            throw new Error(`session.create failed: ${JSON.stringify(created.error)}`);
        this.sid = created.data.id;
    }

    async prompt(text: string, timeoutMs: number): Promise<PromptResult> {
        const started = performance.now();
        const client = this.client;
        const sid = this.sid;
        try {
            const outcome = await settleWithin<OpencodePromptResponse>(
                client.session.prompt({
                    path: { id: sid },
                    body: {
                        model: { providerID: "amazon-bedrock", modelID: MODEL },
                        parts: [{ type: "text", text }],
                    },
                }),
                timeoutMs,
                { abort: () => client.session.abort({ path: { id: sid } }), drainMs: DRAIN_MS },
            );
            const ms = performance.now() - started;
            if (outcome.timedOut) return { answer: "", ms, error: "timeout" };
            const result = outcome.value;
            if (result.error)
                return { answer: "", ms, error: JSON.stringify(result.error).slice(0, 2000) };
            const parts = (result.data?.parts ?? []) as Array<Record<string, unknown>>;
            const answer = parts
                .filter((p) => p.type === "text" && !p.synthetic)
                .map((p) => String(p.text))
                .join("\n");
            const info = result.data?.info as Record<string, unknown> | undefined;
            const error = info?.error ? JSON.stringify(info.error).slice(0, 2000) : undefined;
            return { answer, ms, ...(error ? { error } : {}) };
        } catch (error) {
            if (error instanceof StillRunningError) throw error;
            return {
                answer: "",
                ms: performance.now() - started,
                error: String(error).slice(0, 2000),
            };
        }
    }

    async closeSession(): Promise<void> {
        const oc = this.oc;
        const pid = this.harnessPid();
        this.oc = null;
        this.client = null;
        this.servePid = undefined;
        if (!oc) return;
        if (pid !== undefined) await stopOwnedTree(pid, 10_000);
        await oc.kill();
    }

    /** The `opencode serve` process for this arm's port; a cached PID is re-read by command line, since a PID can be reused. */
    harnessPid(): number | undefined {
        if (!this.oc) return undefined;
        const port = String(this.oc.port);
        const serves = (pid: number): boolean => {
            try {
                const cmd = readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0");
                return cmd.includes("serve") && cmd.includes(port);
            } catch {
                return false;
            }
        };
        if (this.servePid !== undefined && serves(this.servePid)) return this.servePid;
        this.servePid = undefined;
        for (const entry of readdirSync("/proc")) {
            if (!/^\d+$/.test(entry)) continue;
            if (serves(Number(entry))) {
                this.servePid = Number(entry);
                break;
            }
        }
        return this.servePid;
    }

    harnessDataDirs(): string[] {
        return [join(this.env.dataDir, "opencode")];
    }

    sessionId(): string | null {
        return this.sid;
    }
}

export function makeArm(spec: ArmSpec, ctx: ArmContext): Arm {
    return spec.harness === "pi" ? new PiArm(spec, ctx) : new OpencodeArm(spec, ctx);
}

export function armRoot(base: string, name: string): string {
    const dir = join(base, name);
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    return realpathSync(dir);
}

export const DEFAULT_ROOT = join(tmpdir(), "ab-eval");

import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import {
    existsSync,
    mkdirSync,
    readdirSync,
    readFileSync,
    realpathSync,
    writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
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
import { attachStrictJsonlReader, type PiRpcEvent, PiRpcProtocol } from "../pi-runner/rpc-client";
import { HermeticHostStack } from "../rust-runner/hermetic-host";
import { BedrockGateway, type CallRecord } from "./gateway";
import type { Turn } from "./world";

const REPO_ROOT = resolve(import.meta.dir, "../../../..");
const PI_PLUGIN_ROOT = join(REPO_ROOT, "packages/pi-plugin");

export const MODEL = "global.anthropic.claude-opus-5-5";
export const MODEL_REF = `amazon-bedrock/${MODEL}`;
export const CONTEXT_LIMIT = 200_000;

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

export function procStats(pid: number | undefined): { rss: number; cpuMs: number } | null {
    if (!pid) return null;
    try {
        const status = readFileSync(`/proc/${pid}/status`, "utf8");
        const rss = Number(/VmRSS:\s+(\d+)/.exec(status)?.[1] ?? 0) * 1024;
        const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
        const fields = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
        const ticks = Number(fields[11]) + Number(fields[12]);
        return { rss, cpuMs: (ticks * 1000) / 100 };
    } catch {
        return null;
    }
}

function descendants(pid: number): number[] {
    const out: number[] = [];
    try {
        for (const entry of readdirSync("/proc")) {
            if (!/^\d+$/.test(entry)) continue;
            try {
                const stat = readFileSync(`/proc/${entry}/stat`, "utf8");
                const ppid = Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
                if (ppid === pid) out.push(Number(entry), ...descendants(Number(entry)));
            } catch {}
        }
    } catch {
        return out;
    }
    return out;
}

export function treeStats(pid: number | undefined): { rss: number; cpuMs: number } | null {
    if (!pid) return null;
    const own = procStats(pid);
    if (!own) return null;
    for (const child of descendants(pid)) {
        const s = procStats(child);
        if (s) {
            own.rss += s.rss;
            own.cpuMs += s.cpuMs;
        }
    }
    return own;
}

/**
 * The script enters a private mount namespace as root, hides `/tmp` and the invoking user's home
 * behind empty tmpfs mounts, binds the kept paths back, and drops to the invoking user. Agents in
 * an arm then see only their own arm, the harness binaries, and the repository.
 */
function sandboxScript(hidden: readonly string[]): string {
    const mounts = hidden.map((dir) => `mount -t tmpfs -o mode=1777 tmpfs ${dir}`).join("\n");
    return `#!/bin/sh
AB_PATH="$PATH" exec sudo -n --preserve-env unshare --mount --propagation private /bin/sh -c '
set -e
keep=""
while [ "$1" != "--" ]; do keep="$keep $1"; shift; done
shift
st=$(mktemp -d /dev/shm/abst.XXXXXX)
for p in $keep; do mkdir -p "$st$p"; mount --rbind "$p" "$st$p"; done
${mounts}
for p in $keep; do mkdir -p "$p"; mount --rbind "$st$p" "$p"; done
export PATH="$AB_PATH"
exec setpriv --reuid=${process.getuid?.() ?? 0} --regid=${process.getgid?.() ?? 0} --init-groups -- "$@"' sh "$@"
`;
}

export function nodeRoot(): string {
    return resolve(realpathSync(Bun.which("node") as string), "../..");
}

/** The pinned OpenCode binary: `opencode` on PATH resolved to its real file. */
export function opencodeBinary(): string {
    const found = Bun.which("opencode");
    if (!found) throw new Error("opencode is not on PATH");
    return realpathSync(found);
}

export function sandboxKeep(armRoot: string): string[] {
    return [
        REPO_ROOT,
        nodeRoot(),
        ensurePiInstall(),
        resolve(opencodeBinary(), "../../.."),
        armRoot,
    ];
}

export function sandboxPath(): string {
    return [join(nodeRoot(), "bin"), "/usr/local/bin", "/usr/bin", "/bin"].join(":");
}

/** Writes the sandbox script under `dir` and returns its path. */
export function ensureSandboxScript(dir: string): string {
    const path = join(dir, "sandbox.sh");
    const hidden = [...new Set(["/tmp", realpathSync(homedir())])];
    writeFileSync(path, sandboxScript(hidden), { mode: 0o755 });
    return path;
}

/** Whether passwordless `sudo` can run the sandbox on this host. */
export function sandboxAvailable(): boolean {
    return spawnSync("sudo", ["-n", "true"]).status === 0 && Bun.which("unshare") !== null;
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

    hostPid(): number | undefined {
        try {
            const record = JSON.parse(
                readFileSync(
                    join(managedSubtreePath(this.hostDataDir), "rust-e2e-pids.json"),
                    "utf8",
                ),
            ) as { pids: Array<{ pid: number }> };
            return record.pids[0]?.pid;
        } catch {
            return undefined;
        }
    }

    setTurn(
        turn: Turn | null,
        key: string | null,
        probeTokens: { answer: string; stale?: string } | null = null,
    ): void {
        this.main.probeTokens = probeTokens;
        this.main.turn = turn;
        this.main.turnKey = key;
        this.closure.turnKey = key;
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

    async stop(): Promise<void> {
        await this.closeSession().catch(() => undefined);
        await this.host?.stop().catch(() => undefined);
        await this.main.stop();
        await this.closure.stop();
    }
}

export class PiArm extends Arm {
    private child: ChildProcess | null = null;
    private protocol: PiRpcProtocol | null = null;
    private stderr = "";
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
        const env: Record<string, string> = {};
        for (const [k, v] of Object.entries(process.env)) {
            if (
                v === undefined ||
                k.startsWith("AWS_") ||
                k.startsWith("EIDNARA_") ||
                k === "NODE_ENV"
            )
                continue;
            env[k] = v;
        }
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
        env.PATH = sandboxPath();
        const node = join(nodeRoot(), "bin/node");
        const [command, argv] = this.ctx.sandboxDir
            ? [
                  ensureSandboxScript(this.ctx.sandboxDir),
                  [...sandboxKeep(this.ctx.root), "--", node, ...args],
              ]
            : [node, args];
        const child = spawn(command, argv, {
            cwd: this.ctx.workdir,
            env,
            stdio: ["pipe", "pipe", "pipe"],
        });
        this.child = child;
        this.stderr = "";
        child.stderr?.on("data", (chunk: Buffer) => {
            this.stderr = (this.stderr + chunk.toString()).slice(-20_000);
        });
        const protocol = new PiRpcProtocol();
        this.protocol = protocol;
        attachStrictJsonlReader(child.stdout as NonNullable<typeof child.stdout>, (line) =>
            protocol.dispatchLine(line),
        );
        child.once("close", (code, signal) =>
            protocol.rejectPending(
                new Error(`pi exited ${code} ${signal}\n${this.stderr.slice(-3000)}`),
            ),
        );
        await Bun.sleep(300);
        const state = await this.command<{ sessionId?: string }>("get_state", {}, 120_000);
        this.sid = state.sessionId ?? null;
    }

    private async command<T>(
        method: string,
        params: Record<string, unknown>,
        timeoutMs: number,
    ): Promise<T> {
        const protocol = this.protocol;
        const stdin = this.child?.stdin;
        if (!protocol || !stdin) throw new Error("pi not running");
        const response = await protocol.sendCommand<T>(
            (line) => stdin.write(line),
            method,
            params,
            { timeoutMs },
        );
        if (response.success === false)
            throw new Error(`${method} failed: ${JSON.stringify(response)}`);
        return response.data as T;
    }

    async prompt(text: string, timeoutMs: number): Promise<PromptResult> {
        const protocol = this.protocol as PiRpcProtocol;
        const started = performance.now();
        const end = protocol.waitForEvent((event: PiRpcEvent) => event.type === "agent_end", {
            timeoutMs,
            label: "agent_end",
        });
        try {
            await this.command("prompt", { message: text }, timeoutMs);
            const event = await end;
            const ms = performance.now() - started;
            const messages = (event.messages as Array<Record<string, unknown>> | undefined) ?? [];
            const textOf = (m: Record<string, unknown> | undefined): string =>
                ((m?.content as Array<Record<string, unknown>> | undefined) ?? [])
                    .filter((b) => b.type === "text")
                    .map((b) => String(b.text))
                    .join("\n")
                    .trim();
            const assistants = messages.filter((m) => m.role === "assistant");
            const last = assistants[assistants.length - 1];
            const answer = textOf([...assistants].reverse().find((m) => textOf(m).length > 0));
            const stop =
                last?.stopReason === "error" ? String(last.errorMessage ?? "error") : undefined;
            return { answer, ms, ...(stop ? { error: stop } : {}) };
        } catch (error) {
            end.catch(() => undefined);
            return {
                answer: "",
                ms: performance.now() - started,
                error: String(error).slice(0, 2000),
            };
        }
    }

    async closeSession(): Promise<void> {
        const child = this.child;
        if (!child) return;
        this.child = null;
        this.protocol = null;
        if (child.exitCode !== null) return;
        child.stdin?.end();
        child.kill("SIGTERM");
        const exited = await Promise.race([
            new Promise<boolean>((done) => child.once("exit", () => done(true))),
            Bun.sleep(10_000).then(() => false),
        ]);
        if (!exited) child.kill("SIGKILL");
    }

    harnessPid(): number | undefined {
        return this.child?.pid;
    }

    harnessDataDirs(): string[] {
        return [join(this.agentDir, "sessions")];
    }

    sessionId(): string | null {
        return this.sid;
    }

    stderrTail(): string {
        return this.stderr.slice(-4000);
    }
}

function ensurePiCli(): string {
    return ensurePiInstall();
}

export class OpencodeArm extends Arm {
    private oc: SpawnedOpencode | null = null;
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
            ? `exec ${ensureSandboxScript(this.ctx.sandboxDir)} ${sandboxKeep(this.ctx.root).join(" ")} -- ${binary} "$@"`
            : `exec ${binary} "$@"`;
        writeFileSync(
            join(sbin, "opencode"),
            `#!/bin/sh\nexport PATH=${sandboxPath()}\n${launch}\n`,
            {
                mode: 0o755,
            },
        );
        this.oc = await spawnOpencode({
            mockProviderURL: this.main.http1Url,
            existingEnv: this.env,
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
        try {
            const result = await Promise.race([
                this.client.session.prompt({
                    path: { id: this.sid },
                    body: {
                        model: { providerID: "amazon-bedrock", modelID: MODEL },
                        parts: [{ type: "text", text }],
                    },
                }),
                Bun.sleep(timeoutMs).then(() => null),
            ]);
            const ms = performance.now() - started;
            if (result === null) return { answer: "", ms, error: "timeout" };
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
            return {
                answer: "",
                ms: performance.now() - started,
                error: String(error).slice(0, 2000),
            };
        }
    }

    async closeSession(): Promise<void> {
        const oc = this.oc;
        this.oc = null;
        this.client = null;
        if (oc) await oc.kill();
    }

    harnessPid(): number | undefined {
        if (!this.oc) return undefined;
        const port = String(this.oc.port);
        for (const entry of readdirSync("/proc")) {
            if (!/^\d+$/.test(entry)) continue;
            try {
                const cmd = readFileSync(`/proc/${entry}/cmdline`, "utf8").split("\0");
                if (cmd.includes("serve") && cmd.includes(port)) return Number(entry);
            } catch {}
        }
        return undefined;
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

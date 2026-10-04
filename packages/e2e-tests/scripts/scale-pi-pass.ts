#!/usr/bin/env bun

import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";

import { HermeticHostStack } from "../src/rust-runner/hermetic-host";
import {
    command,
    exchangedBytes,
    hostManifest,
    passOutcome,
    seedCoverage,
} from "../src/scale-report/driver-common";
import { retryPosition, writePiTier } from "../src/scale-report/pi-tier";
import {
    type BoundaryState,
    PassRowWriter,
    type ScaleTier,
    TIER_MESSAGES,
} from "../src/scale-report/rows";

const { values: flags } = parseArgs({
    options: {
        "out-dir": { type: "string" },
        "plugin-root": { type: "string" },
        "fixture-bin": { type: "string" },
        "eval-runner-bin": { type: "string" },
        "daemon-build": { type: "string" },
        commit: { type: "string" },
        tiers: { type: "string", default: "10k,s3_100k,s4_1m" },
        sessions: { type: "string", default: "3" },
        samples: { type: "string", default: "330" },
        steady: { type: "string", default: "0" },
        window: { type: "string", default: "300" },
        "context-window": { type: "string", default: "700000" },
        "prompt-kib": { type: "string", default: "20" },
        seed: { type: "string", default: "850" },
    },
});

function need(name: keyof typeof flags): string {
    const value = flags[name];
    if (typeof value !== "string" || value.length === 0) throw new Error(`--${name} is required`);
    return value;
}

const budgetSeconds = Number(process.env.EIDNARA_SCALE_BUDGET_SECONDS);
if (!Number.isSafeInteger(budgetSeconds) || budgetSeconds <= 0)
    throw new Error("EIDNARA_SCALE_BUDGET_SECONDS must name a positive whole-second budget");
const deadline = performance.now() + budgetSeconds * 1_000;
const outDir = resolve(need("out-dir"));
const pluginRoot = resolve(need("plugin-root"));
const fixtureBin = resolve(need("fixture-bin"));
const evalRunnerBin = resolve(need("eval-runner-bin"));
const sessions = Number(flags.sessions);
const samples = Number(flags.samples);
const minSteady = Number(flags.steady);
const windowSize = Number(flags.window);
const contextWindow = Number(flags["context-window"]);
const promptBytes = Number(flags["prompt-kib"]) * 1024;
const baseSeed = Number(flags.seed);
const tiers = (flags.tiers as string).split(",") as ScaleTier[];
for (const tier of tiers) if (!(tier in TIER_MESSAGES)) throw new Error(`unknown tier ${tier}`);
if (!Number.isSafeInteger(sessions) || sessions < 1) throw new Error("--sessions must be positive");
if (!Number.isSafeInteger(samples) || samples < 2) throw new Error("--samples must be at least 2");
if (!Number.isSafeInteger(minSteady) || minSteady < 0 || minSteady > samples)
    throw new Error("--steady must be a count of at most --samples");
if (!Number.isSafeInteger(windowSize) || windowSize % 2 !== 0)
    throw new Error("--window must be an even count");
if (!Number.isSafeInteger(contextWindow) || contextWindow < 16_000)
    throw new Error("--context-window must be a token count of at least 16000");
if (!Number.isSafeInteger(promptBytes) || promptBytes <= 0)
    throw new Error("--prompt-kib must be a positive whole count");
const PROMPT_TEXT = "window fold segment cache budget tier "
    .repeat(Math.ceil(promptBytes / 38))
    .slice(0, promptBytes);

type Json = Record<string, unknown>;

type Handler = (event: Json, ctx: unknown) => unknown;
interface ExtensionApi {
    on(event: string, handler: Handler): void;
}

async function loadRuntime() {
    const resolveFrom = (specifier: string) => Bun.resolveSync(specifier, pluginRoot);
    const shared = join(pluginRoot, "../opencode-plugin/src");
    const [pi, ai, plugin, transport] = await Promise.all([
        import(resolveFrom("@earendil-works/pi-coding-agent")),
        import(resolveFrom("@earendil-works/pi-ai/compat")),
        import(join(pluginRoot, "src/index.ts")),
        import(join(shared, "hooks/context/module-transport.ts")),
    ]);
    return { pi, ai, plugin, transport };
}

const runtime = await loadRuntime();
// The plugin returns a replacement array only for a pass it applied, so an arm whose plugin has a
// transform writes a pass row for every call.
const transformArm = runtime.plugin.PI_TRANSFORM_AVAILABLE === true;

interface CallMarks {
    contextStart?: number;
    beforePlugin?: number;
    afterPlugin?: number;
    contextEnd?: number;
    lengthIn?: number;
    lengthOut?: number;
    replaced?: boolean;
    agentEndStart?: number;
    agentEndEnd?: number;
}

interface PassObservation {
    status?: unknown;
    action?: unknown;
    boundary?: unknown;
    totalMs?: number;
    exchanged: unknown[];
    error?: unknown;
}

let marks: CallMarks = {};
let pass: PassObservation = { exchanged: [] };

const transportCall = runtime.transport.HostModuleTransport.prototype.call;
runtime.transport.HostModuleTransport.prototype.call = async function measured(
    this: unknown,
    args: { method: string; body: unknown },
) {
    const measuring = args.method === "transform" || args.method === "transform.boundary";
    if (measuring) pass.exchanged.push(args.body);
    try {
        const response = await transportCall.call(this, args);
        if (measuring) pass.exchanged.push(response);
        if (args.method === "transform") {
            const record = response as Json;
            const value = (record.result ?? record) as Json & { timings?: { total?: unknown } };
            pass.status = value.status;
            pass.action = value.action;
            pass.boundary = value.boundary;
            if (typeof value.timings?.total === "number") pass.totalMs = value.timings.total;
        }
        return response;
    } catch (error) {
        if (measuring) pass.error = error;
        throw error;
    }
};

function bracket(position: "before" | "after") {
    return (pi: ExtensionApi) => {
        let seen: unknown;
        pi.on("context", (event) => {
            const messages = event.messages as unknown[];
            // The timed span starts on a collected heap, as the OpenCode driver's does; Pi's
            // clone of the array leaves the previous call's copy for this collection.
            if (position === "before") Bun.gc(true);
            const now = performance.now();
            if (position === "before") {
                marks.beforePlugin = now;
                marks.lengthIn = messages.length;
                seen = messages;
            } else {
                marks.afterPlugin = now;
                marks.lengthOut = messages.length;
                marks.replaced = messages !== seen;
            }
            return undefined;
        });
        pi.on("agent_end", () => {
            if (position === "before") marks.agentEndStart = performance.now();
            else marks.agentEndEnd = performance.now();
        });
    };
}

/**
 * The fixture's summarizer command: one segment per five presented messages, titled by its
 * range, so a fold's rows stay small whatever the messages hold.
 */
const SUMMARIZER = `#!${process.execPath}
const request = JSON.parse(await Bun.stdin.text());
const body = request.prompt.split("<new_messages>")[1]?.split("</new_messages>")[0] ?? "";
const ranges = [];
for (const line of body.split("\\n")) {
    const match = /^\\[(\\d+)(?:-(\\d+))?\\] /.exec(line);
    if (!match) continue;
    const start = Number(match[1]);
    const end = Number(match[2] ?? match[1]);
    const last = ranges.at(-1);
    if (last === undefined || start === last[1] + 1) ranges.push([start, end]);
}
let segments = "";
for (let index = 0; index < ranges.length; index += 5) {
    const group = ranges.slice(index, index + 5);
    const start = group[0][0];
    const end = group.at(-1)[1];
    const text = "messages " + start + " to " + end;
    segments += '<history_segment start="' + start + '" end="' + end + '" title="' + text +
        '" episode_type="feature" importance="50"><p1>' + text + "</p1><p2>" + text +
        "</p2><p3>" + text + "</p3><p4 /></history_segment>";
}
const next = (ranges.at(-1)?.[1] ?? 0) + 1;
process.stdout.write("<output><history_segments>" + segments +
    "</history_segments><meta><unprocessed_from>" + next + "</unprocessed_from></meta></output>");
`;

mkdirSync(outDir, { recursive: true });
const us = (from: number | undefined, to: number | undefined) =>
    from === undefined || to === undefined ? null : Math.max(0, Math.round((to - from) * 1_000));

const rowsPath = join(outDir, "rows.jsonl");
const callsPath = join(outDir, "calls.jsonl");
writeFileSync(rowsPath, "");
writeFileSync(callsPath, "");
const writer = new PassRowWriter(rowsPath);
const incomplete: string[] = [];
const loads: Json[] = [];

/** Runs one session and returns its call count and its completed steady passes. */
async function measureSession(
    tier: ScaleTier,
    index: number,
): Promise<{ calls: number; steady: number }> {
    const messages = TIER_MESSAGES[tier];
    const root = mkdtempSync(join(os.tmpdir(), `eidnara-scale-pi-${tier}-`));
    const dataDir = join(root, "data");
    const project = join(root, "project");
    const configHome = join(root, "config");
    for (const dir of [dataDir, project, join(configHome, "eidnara")])
        mkdirSync(dir, { recursive: true });
    const seed = baseSeed + index * 7 + tiers.indexOf(tier);
    const file = join(root, "session.jsonl");
    const sessionId = writePiTier(file, { messages, window: windowSize, seed, cwd: project });
    seedCoverage(evalRunnerBin, dataDir, sessionId, (messages - windowSize) / 2);
    const summarizer = join(root, "summarizer.ts");
    writeFileSync(summarizer, SUMMARIZER, { mode: 0o700 });
    const stack = await HermeticHostStack.start({
        dataDir,
        fixtureBin,
        startTimeoutMs: 300_000,
        daemonEnv: { EIDNARA_FIXTURE_SUMMARIZER_COMMAND: summarizer },
    });
    writeFileSync(
        join(configHome, "eidnara", "eidnara.jsonc"),
        JSON.stringify({
            history_summarizer: { model: "fixture/deterministic" },
            host: { connection_file: stack.connectionFile },
            memory: { auto_search: { enabled: false } },
        }),
    );
    process.env.XDG_CONFIG_HOME = configHome;
    const { pi, ai, plugin } = runtime;
    const faux = ai.registerFauxProvider({
        provider: "faux",
        models: [{ id: "faux-model", contextWindow, maxTokens: 8_192 }],
    });
    const model = faux.getModel();
    faux.setResponses(Array.from({ length: samples + 8 }, () => ai.fauxAssistantMessage("ok")));
    const loadStartedAt = performance.now();
    const sessionManager = pi.SessionManager.open(file, undefined, project);
    const loadMs = performance.now() - loadStartedAt;
    const settingsManager = pi.SettingsManager.inMemory({ compaction: { enabled: false } });
    const authStorage = pi.AuthStorage.inMemory();
    authStorage.setRuntimeApiKey("faux", "scale-key");
    plugin.__test?.clearPiEidnaraActive?.();
    const resourceLoader = new pi.DefaultResourceLoader({
        cwd: project,
        agentDir: root,
        settingsManager,
        extensionFactories: [bracket("before"), plugin.default, bracket("after")],
        noSkills: true,
        noPromptTemplates: true,
        noThemes: true,
        noContextFiles: true,
    });
    await resourceLoader.reload();
    const { session } = await pi.createAgentSession({
        cwd: project,
        agentDir: root,
        authStorage,
        modelRegistry: pi.ModelRegistry.inMemory(authStorage),
        model,
        settingsManager,
        sessionManager,
        resourceLoader,
        noTools: "all",
    });
    await session.bindExtensions({});
    const loaded = session.messages as Json[];
    const retry = retryPosition(messages, windowSize);
    loads.push({
        tier,
        session: sessionId,
        entries: sessionManager.getEntries().length,
        messages: loaded.length,
        load_ms: Math.round(loadMs),
        bash_executions: loaded.filter((message) => message.role === "bashExecution").length,
        retry_follows_failure:
            loaded[retry - 1]?.stopReason === "error" && loaded[retry]?.role === "assistant",
    });
    const runner = session.extensionRunner;
    const emitContext = runner.emitContext.bind(runner);
    runner.emitContext = async (current: unknown[]) => {
        marks.contextStart = performance.now();
        try {
            return await emitContext(current);
        } finally {
            marks.contextEnd = performance.now();
        }
    };
    // The boundary a published pass acknowledged; a declined pass leaves it.
    let boundary: string | undefined;
    let boundaryMoves = 0;
    let folded = false;
    let steadyPasses = 0;
    let completed = 0;
    try {
        for (let turn = 0; turn < samples; turn += 1) {
            if (performance.now() >= deadline) break;
            if (minSteady > 0 && steadyPasses >= minSteady) break;
            Bun.gc(true);
            marks = {};
            pass = { exchanged: [] };
            await session.prompt(`scale turn ${turn} ${PROMPT_TEXT}`);
            const rss = process.memoryUsage.rss();
            const call = {
                tier,
                session: sessionId,
                turn,
                context_event_us: us(marks.contextStart, marks.contextEnd),
                pi_clone_us: us(marks.contextStart, marks.beforePlugin),
                plugin_context_us: us(marks.beforePlugin, marks.afterPlugin),
                agent_end_us: us(marks.agentEndStart, marks.agentEndEnd),
                length_in: marks.lengthIn ?? null,
                length_out: marks.lengthOut ?? null,
                rss_bytes: rss,
            };
            writeFileSync(callsPath, `${JSON.stringify(call)}\n`, { flag: "a" });
            completed += 1;
            if (!transformArm) continue;
            const published = pass.status === "ok" && marks.replaced === true;
            const acknowledged = JSON.stringify(pass.boundary ?? null);
            const moved = published && boundary !== undefined && acknowledged !== boundary;
            if (published) {
                if (moved) boundaryMoves += 1;
                if (pass.action === "HARD") folded = true;
            }
            const state: BoundaryState =
                turn === 0
                    ? "cold"
                    : folded && boundaryMoves >= 3
                      ? "steady"
                      : published && !moved && pass.action !== "HARD"
                        ? "replay"
                        : "warming";
            const { outcome, refusal } = passOutcome({
                published,
                status: pass.status,
                error: pass.error,
            });
            if (state === "steady" && outcome === "completed") steadyPasses += 1;
            if (published) boundary = acknowledged;
            writer.write({
                harness: "pi",
                tier,
                session: sessionId,
                turn,
                boundary_state: state,
                outcome,
                refusal,
                response_us: us(marks.beforePlugin, marks.afterPlugin) ?? 0,
                service_us: pass.totalMs === undefined ? null : Math.round(pass.totalMs * 1_000),
                rss_bytes: rss,
                ipc_bytes: exchangedBytes(pass.exchanged),
            });
        }
    } finally {
        await runner.emit({ type: "session_shutdown", reason: "quit" }).catch(() => undefined);
        session.dispose();
        faux.unregister();
        await stack.stop();
        rmSync(root, { recursive: true, force: true });
    }
    return { calls: completed, steady: steadyPasses };
}

process.env.XDG_DATA_HOME = mkdtempSync(join(os.tmpdir(), "eidnara-scale-xdg-"));
for (let index = 0; index < sessions; index += 1) {
    for (const tier of tiers) {
        const label = `${tier}#${index}`;
        if (performance.now() >= deadline) {
            incomplete.push(label);
            continue;
        }
        try {
            const { calls, steady } = await measureSession(tier, index);
            if (minSteady > 0 ? steady < minSteady : calls < samples) incomplete.push(label);
            console.log(`${label}: ${calls} calls, ${steady} completed steady passes`);
        } catch (error) {
            incomplete.push(label);
            console.error(`${label}: inconclusive:`, error);
        }
    }
}
writer.close();

function baseline(): Json {
    const calls = readFileSync(callsPath, "utf8")
        .split("\n")
        .filter((line) => line.length > 0)
        .map((line) => JSON.parse(line) as Json);
    const out: Json = {};
    for (const tier of tiers) {
        const rows = calls.filter((call) => call.tier === tier);
        const figure = (key: string) => {
            const values = rows
                .map((row) => row[key])
                .filter((value): value is number => typeof value === "number")
                .sort((a, b) => a - b);
            const at = (p: number) => values[Math.max(0, Math.ceil((p / 100) * values.length) - 1)];
            return values.length === 0
                ? null
                : { n: values.length, p50: at(50), p95: at(95), p99: at(99), max: values.at(-1) };
        };
        out[tier] = {
            calls: rows.length,
            context_event_us: figure("context_event_us"),
            pi_clone_us: figure("pi_clone_us"),
            plugin_context_us: figure("plugin_context_us"),
            agent_end_us: figure("agent_end_us"),
            length_in: figure("length_in"),
            length_out: figure("length_out"),
            rss_bytes: figure("rss_bytes"),
        };
    }
    return out;
}

const commit = flags.commit ?? command("git", ["-C", pluginRoot, "rev-parse", "HEAD"]);
writeFileSync(
    join(outDir, "manifest.json"),
    `${JSON.stringify(
        {
            driver: { name: "pi-pass", budget_seconds: budgetSeconds },
            artifact: {
                commit,
                bun_version: Bun.version,
                daemon_build: flags["daemon-build"] ?? "unnamed",
            },
            host: hostManifest(),
            open_loop: null,
            seed: baseSeed,
        },
        null,
        2,
    )}\n`,
);
writeFileSync(
    join(outDir, "baseline.json"),
    `${JSON.stringify({ commit, loads, tiers: baseline() }, null, 2)}\n`,
);
writeFileSync(join(outDir, "incomplete.json"), `${JSON.stringify(incomplete)}\n`);
if (incomplete.length > 0) console.log(`inconclusive: ${incomplete.join(", ")}`);

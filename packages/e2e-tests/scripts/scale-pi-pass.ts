#!/usr/bin/env bun

import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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
    gcStart?: number;
    gcEnd?: number;
    lengthIn?: number;
    lengthOut?: number;
    seen?: unknown;
    replaced?: boolean;
    firstRole?: unknown;
    /** SHA-256 of the first sent message's text: m0 on a published pass, Pi's summary otherwise. */
    firstTextSha?: string;
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
        pi.on("context", (event) => {
            const messages = event.messages as unknown[];
            // The timed span starts on a collected heap, as the OpenCode driver's does. The
            // collection frees the previous call's garbage, Pi's clone and the plugin's alike,
            // and its time is recorded apart from Pi's and the plugin's.
            if (position === "before") {
                marks.gcStart = performance.now();
                Bun.gc(true);
                marks.gcEnd = performance.now();
            }
            const now = performance.now();
            if (position === "before") {
                marks.beforePlugin = now;
                marks.lengthIn = messages.length;
                marks.seen = messages;
            } else {
                marks.afterPlugin = now;
                marks.lengthOut = messages.length;
                marks.replaced = messages !== marks.seen;
                const first = messages[0] as
                    | { role?: unknown; summary?: unknown; content?: unknown }
                    | undefined;
                marks.firstRole = first?.role;
                const text =
                    typeof first?.summary === "string"
                        ? first.summary
                        : Array.isArray(first?.content)
                          ? first.content
                                .map((part: { text?: unknown }) =>
                                    typeof part.text === "string" ? part.text : "",
                                )
                                .join("")
                          : String(first?.content ?? "");
                marks.firstTextSha = createHash("sha256").update(text).digest("hex");
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
const minus = (whole: number | null, part: number | null) =>
    whole === null ? null : Math.max(0, whole - (part ?? 0));

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
    // Pi's compaction settings stay at their defaults, as a Pi user's do.
    const settingsManager = pi.SettingsManager.inMemory({});
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
    let evictions = 0;
    // The m0 texts the published passes rendered; a declined pass after an eviction shows one.
    const m0Seen = new Set<string>();
    const showsM0 = () =>
        evictions === 0 || marks.replaced || marks.firstRole !== "compactionSummary"
            ? null
            : m0Seen.has(marks.firstTextSha ?? "");
    session.subscribe((event: { type: string; result?: unknown }) => {
        if (event.type === "compaction_end" && event.result) evictions += 1;
    });
    try {
        for (let turn = 0; turn < samples; turn += 1) {
            if (performance.now() >= deadline) break;
            if (minSteady > 0 && steadyPasses >= minSteady) break;
            Bun.gc(true);
            marks = {};
            pass = { exchanged: [] };
            await session.prompt(`scale turn ${turn} ${PROMPT_TEXT}`);
            // An eviction the plugin starts once the agent is idle finishes before the next
            // sample, as it would before a user's next prompt.
            await Bun.sleep(0);
            while (session.isCompacting) await Bun.sleep(2);
            const rss = process.memoryUsage.rss();
            const call: Json = {
                tier,
                session: sessionId,
                turn,
                context_event_us: minus(
                    us(marks.contextStart, marks.contextEnd),
                    us(marks.gcStart, marks.gcEnd),
                ),
                pi_clone_us: us(marks.contextStart, marks.gcStart),
                forced_gc_us: us(marks.gcStart, marks.gcEnd),
                plugin_context_us: us(marks.beforePlugin, marks.afterPlugin),
                agent_end_us: us(marks.agentEndStart, marks.agentEndEnd),
                length_in: marks.lengthIn ?? null,
                length_out: marks.lengthOut ?? null,
                rss_bytes: rss,
                evictions,
                // After an eviction, Pi sends its own array on a pass the plugin declined.
                first_role_sent: marks.replaced ? null : (marks.firstRole ?? null),
                shows_m0: showsM0(),
                boundary_state: null,
            };
            completed += 1;
            if (!transformArm) {
                writeFileSync(callsPath, `${JSON.stringify(call)}\n`, { flag: "a" });
                continue;
            }
            const published = pass.status === "ok" && marks.replaced === true;
            if (published && marks.firstTextSha) m0Seen.add(marks.firstTextSha);
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
            call.boundary_state = state;
            writeFileSync(callsPath, `${JSON.stringify(call)}\n`, { flag: "a" });
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
        if (transformArm) {
            // With the daemon down, the plugin declines and Pi sends its own array, which
            // leads with the m0 text of its latest eviction.
            const outage: Json = { tier, session: sessionId, evictions };
            if (evictions === 0) outage.skipped = "no_eviction";
            else {
                try {
                    await stack.stop();
                    marks = {};
                    pass = { exchanged: [] };
                    await session.prompt(`scale outage ${PROMPT_TEXT}`);
                    outage.first_role_sent = marks.replaced ? null : (marks.firstRole ?? null);
                    outage.shows_m0 = showsM0();
                    outage.length_in = marks.lengthIn ?? null;
                } catch (error) {
                    outage.error = String(error);
                }
            }
            writeFileSync(join(outDir, "outages.jsonl"), `${JSON.stringify(outage)}\n`, {
                flag: "a",
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

/**
 * Per session, the least-squares growth of Pi's `context` array length across the steady span,
 * read as the report reads RSS flatness: growth across the span at most a tenth of the first
 * point's length. Each point is the longest array a steady call saw between two evictions. The
 * trailing interval, cut short by the end of the run, counts once it has run as many turns as
 * the longest completed interval or peaked above the point before it, so a session whose
 * evictions stop shows its growth. Fewer than two points does not pass.
 */
function lengthFlatness(rows: Json[]): Json[] {
    const sessions = new Map<string, Json[]>();
    for (const row of rows) {
        const calls = sessions.get(String(row.session)) ?? [];
        calls.push(row);
        sessions.set(String(row.session), calls);
    }
    return [...sessions].map(([session, calls]) => {
        // A call's `evictions` includes the eviction at its own `agent_end`, so its pass ran in
        // the interval the previous call's count names.
        const intervals = new Map<number, { turn: number; length: number }>();
        let entry: number | undefined;
        let first: number | undefined;
        let last: number | undefined;
        let lastInterval: number | undefined;
        const evictedAt: number[] = [];
        calls.forEach((row, index) => {
            const before = Number(calls[index - 1]?.evictions ?? 0);
            if (Number(row.evictions) > before) evictedAt.push(Number(row.turn));
            if (row.boundary_state !== "steady" || typeof row.length_in !== "number") return;
            entry ??= row.length_in;
            first ??= Number(row.turn);
            last = Number(row.turn);
            lastInterval = before;
            const point = intervals.get(before);
            if (!point || row.length_in > point.length)
                intervals.set(before, { turn: Number(row.turn), length: row.length_in });
        });
        // Completed intervals end at an eviction at or before the last steady call; the trailing
        // one, which the last steady call ran in, is open when that call did not evict.
        const closed = evictedAt.filter((turn) => last !== undefined && turn <= last);
        let completedGap = 0;
        for (let index = 1; index < closed.length; index += 1)
            completedGap = Math.max(completedGap, (closed[index] ?? 0) - (closed[index - 1] ?? 0));
        const lastClosed = closed.at(-1);
        const trailingOpen = last !== undefined && lastClosed !== last;
        const trailingGap =
            trailingOpen && lastClosed !== undefined && last !== undefined ? last - lastClosed : 0;
        const points = [...intervals.values()];
        const trailing =
            trailingOpen && lastInterval !== undefined ? intervals.get(lastInterval) : undefined;
        const previous = points.at(-2);
        const trailingIncluded =
            trailing === undefined ||
            trailingGap >= completedGap ||
            (previous !== undefined && trailing.length > previous.length);
        if (!trailingIncluded) points.pop();
        const n = points.length;
        // The reference is the first peak, on the same phase of the sawtooth as every point.
        const reference = points[0]?.length;
        const summary = {
            session,
            intervals: n,
            steady_entry_length: entry ?? null,
            reference_length: reference ?? null,
            longest_turns_between_evictions: Math.max(completedGap, trailingGap),
            trailing_included: trailingIncluded,
        };
        if (n < 2 || reference === undefined) return { ...summary, growth: null, passes: false };
        const mx = points.reduce((sum, point) => sum + point.turn, 0) / n;
        const my = points.reduce((sum, point) => sum + point.length, 0) / n;
        let sxy = 0;
        let sxx = 0;
        for (const point of points) {
            sxy += (point.turn - mx) * (point.length - my);
            sxx += (point.turn - mx) ** 2;
        }
        const span = (last ?? 0) - (first ?? 0);
        const growth = sxx > 0 ? (sxy / sxx) * span : 0;
        return { ...summary, growth, passes: growth <= reference / 10 };
    });
}

/** Per tier, the outage passes: how many ran, how many were skipped, and how many showed m0. */
function outageSummary(tier: string): Json {
    const path = join(outDir, "outages.jsonl");
    const rows = existsSync(path)
        ? readFileSync(path, "utf8")
              .split("\n")
              .filter((line) => line.length > 0)
              .map((line) => JSON.parse(line) as Json)
              .filter((row) => row.tier === tier)
        : [];
    return {
        sessions: rows.length,
        skipped: rows.filter((row) => row.skipped !== undefined).length,
        failed: rows.filter((row) => row.error !== undefined).length,
        showing_m0: rows.filter((row) => row.shows_m0 === true).length,
    };
}

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
            max_session_evictions: Math.max(0, ...rows.map((row) => Number(row.evictions ?? 0))),
            length_flatness: transformArm ? lengthFlatness(rows) : null,
            declined_after_eviction: rows.filter((row, index) => {
                const before = Number(
                    rows[index - 1]?.session === row.session ? rows[index - 1]?.evictions : 0,
                );
                return before > 0 && row.first_role_sent !== null;
            }).length,
            declined_after_eviction_showing_m0: rows.filter((row) => row.shows_m0 === true).length,
            outages: outageSummary(tier),
            context_event_us: figure("context_event_us"),
            pi_clone_us: figure("pi_clone_us"),
            forced_gc_us: figure("forced_gc_us"),
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

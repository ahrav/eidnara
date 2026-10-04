#!/usr/bin/env bun

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import os from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";

import { HermeticHostStack } from "../src/rust-runner/hermetic-host";
import {
    type PassRow,
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
        samples: { type: "string", default: "300" },
        window: { type: "string", default: "300" },
        seed: { type: "string", default: "847" },
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
const samples = Number(flags.samples);
const windowSize = Number(flags.window);
const tiers = (flags.tiers as string).split(",") as ScaleTier[];
for (const tier of tiers) if (!(tier in TIER_MESSAGES)) throw new Error(`unknown tier ${tier}`);
if (!Number.isSafeInteger(samples) || samples < 2) throw new Error("--samples must be at least 2");
if (!Number.isSafeInteger(windowSize) || windowSize < 4 || windowSize % 2 !== 0)
    throw new Error("--window must be an even count of at least 4");

interface PluginUnderTest {
    createRustModeTransform: (
        deps: unknown,
        options: unknown,
    ) => {
        run(sessionId: string, output: { messages: unknown[] }): Promise<void>;
        getState(sessionId: string): { boundary?: unknown; failureCount: number };
    };
    createHostModuleClient: (connectionFile: string) => {
        call(args: Record<string, unknown>): Promise<unknown>;
        disconnect(): void;
    };
    BoundedSessionMap: new (capacity: number) => unknown;
}

async function loadPlugin(): Promise<PluginUnderTest> {
    const context = join(pluginRoot, "src/hooks/context");
    const adapter = existsSync(join(context, "opencode-transform-adapter.ts"))
        ? join(context, "opencode-transform-adapter.ts")
        : join(context, "rust-mode-transform.ts");
    const [transform, transport, maps] = await Promise.all([
        import(adapter),
        import(join(context, "module-transport.ts")),
        import(join(pluginRoot, "src/shared/bounded-session-map.ts")),
    ]);
    return {
        createRustModeTransform: transform.createRustModeTransform,
        createHostModuleClient: transport.createHostModuleClient,
        BoundedSessionMap: maps.BoundedSessionMap,
    };
}

const WINDOW_TEXT = "x".repeat(5 * 1024);

function hostArray(sessionId: string, covered: number, window: number): unknown[] {
    const messages: unknown[] = new Array(covered + window);
    for (let index = 0; index < covered; index += 1) {
        messages[index] = {
            info: {
                id: `m${index + 1}`,
                sessionID: sessionId,
                role: index % 2 === 0 ? "user" : "assistant",
            },
            parts:
                index >= covered - 2
                    ? [{ id: `p${index + 1}`, type: "text", text: `covered ${index + 1}` }]
                    : [],
        };
    }
    for (let offset = 0; offset < window; offset += 1) {
        const ordinal = covered + offset + 1;
        const user = ordinal % 2 === 1;
        messages[covered + offset] = {
            info: {
                id: `m${ordinal}`,
                sessionID: sessionId,
                role: user ? "user" : "assistant",
                time: { created: 1_700_000_000_000 + ordinal },
                ...(user ? {} : { providerID: "anthropic", modelID: "claude-sonnet-4-5" }),
            },
            parts: [{ id: `p${ordinal}`, type: "text", text: `${ordinal} ${WINDOW_TEXT}` }],
        };
    }
    return messages;
}

function seed(dataDir: string, sessionId: string, segments: number): void {
    const result = spawnSync(
        evalRunnerBin,
        [
            "scale-seed",
            "--state-root",
            dataDir,
            "--session",
            sessionId,
            "--segments",
            String(segments),
        ],
        { encoding: "utf8" },
    );
    if (result.status !== 0) throw new Error(`scale-seed failed: ${result.stderr}`);
}

function command(cmd: string, args: string[]): string {
    const result = spawnSync(cmd, args, { encoding: "utf8" });
    return result.status === 0 ? result.stdout.trim() : "";
}

function hostManifest(): Record<string, unknown> {
    const cpuModel =
        /model name\s*:\s*(.+)/.exec(readFileSync("/proc/cpuinfo", "utf8"))?.[1]?.trim() ??
        os.cpus()[0]?.model ??
        "unknown";
    const device = command("df", ["--output=source", os.tmpdir()]).split("\n").at(-1) ?? "";
    const rotational = command("lsblk", ["-ndo", "ROTA", device]);
    return {
        cpu_model: cpuModel,
        core_count: os.availableParallelism(),
        memory_bytes: os.totalmem(),
        kernel: os.release(),
        glibc: command("getconf", ["GNU_LIBC_VERSION"]) || "unknown",
        disk: rotational === "0" ? "ssd" : rotational === "1" ? "hdd" : "unknown",
    };
}

interface PassObservation {
    status?: unknown;
    totalMs?: number;
    bytes: number;
    error?: unknown;
}

const plugin = await loadPlugin();
mkdirSync(outDir, { recursive: true });
const rowsPath = join(outDir, "rows.jsonl");
writeFileSync(rowsPath, "");
const writer = new PassRowWriter(rowsPath);
const incomplete: string[] = [];
process.env.XDG_DATA_HOME = mkdtempSync(join(os.tmpdir(), "eidnara-scale-xdg-"));

for (const tier of tiers) {
    if (performance.now() >= deadline) {
        incomplete.push(tier);
        continue;
    }
    const covered = TIER_MESSAGES[tier] - windowSize;
    const sessionId = `scale-opencode-${tier}`;
    const dataDir = mkdtempSync(join(os.tmpdir(), `eidnara-scale-${tier}-`));
    const projectRoot = mkdtempSync(join(os.tmpdir(), "eidnara-scale-project-"));
    seed(dataDir, sessionId, covered / 2);
    const stack = await HermeticHostStack.start({ dataDir, fixtureBin, startTimeoutMs: 120_000 });
    const client = plugin.createHostModuleClient(stack.connectionFile);
    let pass: PassObservation = { bytes: 0 };
    const measured = {
        call: async (args: Record<string, unknown>) => {
            const sent = Buffer.byteLength(JSON.stringify(args.body));
            try {
                const response = await client.call(args);
                pass.bytes += sent;
                if (args.method === "transform") {
                    const record = response as Record<string, unknown>;
                    const value = (record.result ?? record) as {
                        status?: unknown;
                        timings?: { total?: unknown };
                    };
                    pass.status = value.status;
                    if (typeof value.timings?.total === "number")
                        pass.totalMs = value.timings.total;
                    pass.bytes += Buffer.byteLength(JSON.stringify(response));
                }
                return response;
            } catch (error) {
                if (args.method === "transform") pass.error = error;
                throw error;
            }
        },
    };
    const transform = plugin.createRustModeTransform(
        {
            client: {
                app: { agents: async () => ({ data: [] }) },
                session: { get: async () => ({ data: { directory: projectRoot } }) },
            },
            contextUsageMap: new plugin.BoundedSessionMap(8),
            clearReasoningAge: 50,
            cacheTtl: "5m",
            directory: projectRoot,
            sessionDirectoryBySession: new Map(),
            isSubagentSession: () => false,
            systemPromptHashFor: () => "",
        },
        { moduleClient: measured, projectRoot },
    );
    let previousBoundary = "";
    let completed = 0;
    try {
        for (let turn = 0; turn < samples; turn += 1) {
            if (performance.now() >= deadline) break;
            const host = hostArray(sessionId, covered, windowSize);
            const output = { messages: turn === 0 ? host.slice(covered - 2) : host };
            const failuresBefore = transform.getState(sessionId).failureCount;
            Bun.gc(true);
            pass = { bytes: 0 };
            const startedAt = performance.now();
            await transform.run(sessionId, output);
            const responseUs = Math.round((performance.now() - startedAt) * 1_000);
            const state = transform.getState(sessionId);
            const boundary = JSON.stringify(state.boundary ?? null);
            const ok = pass.status === "ok" && state.failureCount === failuresBefore;
            const refusal: PassRow["refusal"] = ok
                ? null
                : pass.error !== undefined
                  ? "transport_error"
                  : pass.status === undefined
                    ? "declined"
                    : "daemon_error";
            writer.write({
                harness: "opencode",
                tier,
                session: sessionId,
                turn,
                boundary_state:
                    turn === 0 ? "cold" : boundary === previousBoundary ? "steady" : "warming",
                outcome: ok ? "completed" : "refused",
                refusal,
                response_us: responseUs,
                service_us: pass.totalMs === undefined ? null : Math.round(pass.totalMs * 1_000),
                rss_bytes: process.memoryUsage.rss(),
                ipc_bytes: pass.bytes,
            });
            previousBoundary = boundary;
            completed += 1;
        }
    } finally {
        client.disconnect();
        await stack.stop();
    }
    if (completed < samples) incomplete.push(tier);
    console.log(`${tier}: ${completed} of ${samples} passes`);
}
writer.close();

writeFileSync(
    join(outDir, "manifest.json"),
    `${JSON.stringify(
        {
            driver: { name: "opencode-pass", budget_seconds: budgetSeconds },
            artifact: {
                commit: flags.commit ?? command("git", ["-C", pluginRoot, "rev-parse", "HEAD"]),
                bun_version: Bun.version,
                daemon_build: flags["daemon-build"] ?? "unnamed",
            },
            host: hostManifest(),
            open_loop: null,
            seed: Number(flags.seed),
        },
        null,
        2,
    )}\n`,
);
writeFileSync(join(outDir, "incomplete.json"), `${JSON.stringify(incomplete)}\n`);
if (incomplete.length > 0) console.log(`inconclusive tiers: ${incomplete.join(", ")}`);

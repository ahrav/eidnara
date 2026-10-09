import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { ballastProse } from "../src/ballast";
import { readCompressionFidelityCorpus } from "../src/compression-fidelity/corpus";
import {
    emitObservation,
    leaksOutside,
    publishedOf,
    reviewedTiers,
    servedTier,
    stageOf,
} from "../src/compression-fidelity/delivery";
import { waitFor } from "../src/harness-primitives";
import {
    BASE_MS,
    type Binaries,
    body,
    buildBinaries,
    pluginModules,
    startFixture,
    textOf,
} from "../src/pi-runner/rust-fixture";
import { detectPiPrereqs } from "../src/pi-runner/spawn";
import type { HermeticHostStack, ScriptMessage } from "../src/rust-runner/hermetic-host";
import { detectRustModePrereqs } from "../src/rust-runner/hermetic-host";

const rust = detectRustModePrereqs();
const pi = detectPiPrereqs();
const PI_REQUIRED = process.env.EIDNARA_E2E_REQUIRE_PI === "1";
const active = rust.ok && pi.ok;

const SESSION = "pi-fidelity-c1";
const SCENARIO = "C1.S1";
const CONTEXT_WINDOW = 30_000;
/** Seeded baseline segments, two messages each; few enough that the m1 window stays served. */
const BASELINE_SEGMENTS = 6;
const BASELINE_MESSAGES = BASELINE_SEGMENTS * 2 + 4;
const TRAILING_PAIRS = 6;
const MAX_PROMPTS = 8;
const WAIT_MS = 120_000;

function piEntry(id: string, parentId: string | null, role: string, text: string, at: number) {
    const message =
        role === "user"
            ? { role, content: [{ type: "text", text }], timestamp: at }
            : {
                  role,
                  content: [{ type: "text", text }],
                  api: "anthropic-messages",
                  provider: "anthropic",
                  model: "claude-sonnet-4-5",
                  usage: {
                      input: 0,
                      output: 0,
                      cacheRead: 0,
                      cacheWrite: 0,
                      totalTokens: 0,
                      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
                  },
                  stopReason: "stop",
                  timestamp: at,
              };
    return { type: "message", id, parentId, timestamp: new Date(at).toISOString(), message };
}

function piText(native: ScriptMessage): string {
    return native.parts
        .map((part) =>
            part.type === "tool"
                ? `${String(part.tool)} output: ${String((part.state as { output?: string }).output ?? "")}`
                : String(part.text ?? ""),
        )
        .join("\n");
}

/**
 * The first entries match the seeded baseline segments; the trailing turns move the case source
 * out of the protected tail while usage stays below the forced-fold band.
 */
function writeSession(path: string, cwd: string, source: ScriptMessage[]): void {
    const lines: unknown[] = [
        {
            type: "session",
            version: 3,
            id: SESSION,
            timestamp: new Date(BASE_MS).toISOString(),
            cwd,
        },
    ];
    let parent: string | null = null;
    let at = BASE_MS;
    const push = (id: string, role: string, text: string) => {
        at += 1_000;
        lines.push(piEntry(id, parent, role, text, at));
        parent = id;
    };
    for (let n = 1; n <= BASELINE_MESSAGES; n += 1) {
        push(`m${n}`, n % 2 === 1 ? "user" : "assistant", body(n));
    }
    for (const [k, native] of source.entries()) push(`c${k + 1}`, native.info.role, piText(native));
    if (source.at(-1)?.info.role === "user") push("c-close", "assistant", "Noted.");
    for (let k = 1; k <= TRAILING_PAIRS; k += 1) {
        push(`t${k}u`, "user", `trailing ${k}: ${ballastProse(1_500)}`);
        push(`t${k}a`, "assistant", `trailing answer ${k}`);
    }
    writeFileSync(path, `${lines.map((line) => JSON.stringify(line)).join("\n")}\n`);
}

function writeConfig(configHome: string, connectionFile: string, surface: string): void {
    mkdirSync(join(configHome, "eidnara"), { recursive: true });
    writeFileSync(
        join(configHome, "eidnara", "eidnara.jsonc"),
        JSON.stringify({
            history_summarizer: { model: "fixture/deterministic" },
            host: { connection_file: connectionFile },
            memory: { auto_search: { enabled: false } },
            execute_threshold_percentage: 25,
            history_budget_percentage: 0.5,
            protected_tags: 1,
            prompt_surface: { default: surface },
        }),
    );
}

async function published(stack: HermeticHostStack, project: string): Promise<number> {
    const status = await stack.contextRequest(
        { project_root: project, harness: "pi", session: SESSION },
        { method: "session.status", v: 1, session_id: SESSION },
    );
    return publishedOf(status);
}

describe("compression fidelity pi delivery prerequisites", () => {
    it.skipIf(!PI_REQUIRED)("are present when EIDNARA_E2E_REQUIRE_PI=1", () => {
        expect({ rust: rust.skipReason ?? "ok", pi: pi.skipReason ?? "ok" }).toEqual({
            rust: "ok",
            pi: "ok",
        });
    });
});

describe.skipIf(!active)("compression fidelity delivery through the Pi context handler", () => {
    let binaries: Binaries;
    let root = "";

    beforeAll(async () => {
        binaries = await buildBinaries();
    }, 600_000);

    afterAll(() => {
        if (root) rmSync(root, { recursive: true, force: true });
    });

    it("serves a scripted C1 publication at P1 in m1 and then in a rematerialized m0", async () => {
        const fidelityCase = readCompressionFidelityCorpus().cases.find(
            (entry) => entry.id === "C1",
        );
        const reviewed = fidelityCase?.sources.find((entry) => entry.id === "C1.V1");
        if (!reviewed) throw new Error("C1.V1 is not in the corpus");
        const { title, bodies } = reviewedTiers(reviewed.reviewedOutput);
        const f = await startFixture(binaries, [SESSION], undefined, BASELINE_SEGMENTS);
        root = f.root;
        const project = join(f.root, "project");
        mkdirSync(project, { recursive: true });
        const configHome = join(f.root, "pi-config");
        const file = join(f.root, `${SESSION}.jsonl`);
        const modules = await pluginModules();
        const saved = process.env.XDG_CONFIG_HOME;
        process.env.XDG_CONFIG_HOME = configHome;
        const open = async (surface: string) => {
            writeConfig(configHome, f.stack.connectionFile, surface);
            modules.extension.__test.clearPiEidnaraActive();
            return modules.agent.createTestAgentSession({
                cwd: project,
                extensionFactories: [modules.extension.default],
                sessionManager: modules.agent.openSessionFile(file, project),
                contextWindow: CONTEXT_WINDOW,
            });
        };
        const diagnostics = () => f.stack.hostLog().slice(-6_000);
        const record = (
            label: string,
            stage: string,
            tier: string,
            leaks: string[],
            served: number,
        ) =>
            emitObservation({
                case: "C1",
                source: "C1.V1",
                scenario: SCENARIO,
                stage: `pi-${label}`,
                terminal: leaks.length === 0 && tier === "p1" ? "served" : "unqualified",
                markers: label === "m1" ? ["cf-delivery-m1-published-input"] : [],
                detail: { harness: "pi", stage, served_tier: tier, leaks, served_messages: served },
            });
        try {
            expect(modules.extension.PI_TRANSFORM_AVAILABLE).toBe(true);
            const source = await f.stack.scriptSource(SCENARIO);
            writeSession(file, project, source.messages);
            await f.stack.scriptCases([SCENARIO, ...Array(7).fill("filler:1")]);

            let harness = await open("full");
            try {
                const settle = () =>
                    waitFor(
                        "the summarizer to settle",
                        async () => ((await f.stack.historySummarizerLive()) ? undefined : true),
                        WAIT_MS,
                        diagnostics,
                    );
                // A pass during a live firing races its publication, so each prompt waits for
                // the firing it started.
                for (let i = 1; i <= MAX_PROMPTS; i += 1) {
                    await harness.session.prompt(`pi turn ${i}`);
                    await settle();
                    if ((await published(f.stack, project)) > 0) break;
                }
                expect(await f.stack.scriptStatus()).toMatchObject({ bound: 1, mismatched: 0 });
                expect(await published(f.stack, project)).toBeGreaterThan(0);
                const before = harness.requests.length;
                await harness.session.prompt("m1: should I implement pooling first?");
                expect(harness.requests.length).toBeGreaterThan(before);
                const texts = (harness.requests.at(-1)?.messages ?? []).map(textOf);
                const m1 = {
                    stage: stageOf(texts, title),
                    tier: servedTier(texts, title, bodies),
                    leaks: leaksOutside(texts, source.leakProbes),
                };
                record("m1", m1.stage, m1.tier, m1.leaks, texts.length);
                expect(m1).toEqual({ stage: "m1", tier: "p1", leaks: [] });
            } finally {
                await harness.dispose();
            }

            harness = await open("light");
            try {
                const before = harness.requests.length;
                await harness.session.prompt("cold: should I implement pooling first?");
                expect(harness.requests.length).toBeGreaterThan(before);
                const texts = (harness.requests.at(-1)?.messages ?? []).map(textOf);
                const cold = {
                    stage: stageOf(texts, title),
                    tier: servedTier(texts, title, bodies),
                    leaks: leaksOutside(texts, source.leakProbes),
                };
                record("cold-m0", cold.stage, cold.tier, cold.leaks, texts.length);
                expect(cold).toEqual({ stage: "m0", tier: "p1", leaks: [] });
            } finally {
                await harness.dispose();
            }
        } finally {
            if (saved === undefined) delete process.env.XDG_CONFIG_HOME;
            else process.env.XDG_CONFIG_HOME = saved;
            await f.stack.stop();
        }
    }, 540_000);
});

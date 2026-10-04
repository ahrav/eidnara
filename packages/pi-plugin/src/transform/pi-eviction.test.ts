import { afterEach, beforeEach, describe, expect, it, spyOn } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai/compat";
import type { CompactionEntry } from "@earendil-works/pi-coding-agent";
import { HostModuleTransport } from "@eidnara/opencode/hooks/context/module-transport";
import { Type } from "typebox";

import { createTestAgentSession, type TestAgentSession } from "../__tests__/agent-session";
import { textOf } from "../__tests__/test-utils";
import eidnaraPiExtension, { __test } from "../index";
import { EIDNARA_PI_SUBAGENT_ENV } from "../subagent-runner";

type Json = Record<string, unknown>;

const saved = {
    XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
};
let root: string;
let harness: TestAgentSession | undefined;

beforeEach(() => {
    root = mkdtempSync(join(tmpdir(), "eidnara-pi-eviction-"));
    process.env.XDG_CONFIG_HOME = join(root, "config");
    process.env.XDG_DATA_HOME = join(root, "data");
    mkdirSync(join(root, "config", "eidnara"), { recursive: true });
    writeFileSync(
        join(root, "config", "eidnara", "eidnara.jsonc"),
        JSON.stringify({ history_summarizer: { model: "fixture/deterministic" } }),
    );
    delete process.env[EIDNARA_PI_SUBAGENT_ENV];
    __test.clearPiEidnaraActive();
});

afterEach(async () => {
    await harness?.dispose();
    harness = undefined;
    __test.clearPiEidnaraActive();
    for (const [key, value] of Object.entries(saved)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
    rmSync(root, { recursive: true, force: true });
});

const M0 = "<session-history>\n## 1-2 · folded\n</session-history>";

/**
 * A daemon that folds every pass through the second-newest row. A `fixed` daemon renders the
 * boundary of its first pass on every later pass, so the acknowledged boundary never moves.
 */
function foldingDaemon(fixed = false, otherCallMs = 0) {
    const transforms: Json[] = [];
    let sequence = 0;
    let pinned: string | undefined;
    const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(async (input) => {
        const body = input.body as Json;
        if (input.method === "transform.boundary") return { anchors: [] };
        if (input.method !== "transform") {
            if (otherCallMs > 0) await Bun.sleep(otherCallMs);
            return { state: "accepted" };
        }
        transforms.push(body);
        const rows = body.native_messages as { id: string }[];
        let head = Math.max(0, rows.length - 2);
        if (fixed && pinned !== undefined)
            head = Math.max(
                0,
                rows.findIndex((row) => row.id === pinned),
            );
        else sequence += 1;
        pinned ??= rows[head]?.id;
        return {
            status: "ok",
            action: "HARD",
            boundary: { mid: rows[head]?.id, sequence },
            base_revision: body.base_revision,
            output_revision: `out-${sequence}`,
            operations: [
                {
                    op: "insert",
                    values: [
                        {
                            id: "eidnara:synthetic:m0",
                            message: {
                                role: "user",
                                content: [{ type: "text", text: M0 }],
                                timestamp: 0,
                            },
                        },
                    ],
                },
                { op: "keep", source: "input", start: head, count: rows.length - head },
            ],
        };
    });
    return { call, transforms };
}

function unfoldedDaemon() {
    return spyOn(HostModuleTransport.prototype, "call").mockImplementation(async (input) => {
        const body = input.body as Json;
        if (input.method === "transform.boundary") return { anchors: [] };
        if (input.method !== "transform") return { state: "accepted" };
        return {
            status: "ok",
            action: "SOFT",
            boundary: null,
            base_revision: body.base_revision,
            output_revision: `raw-${String(body.base_revision)}`,
            operations: [
                {
                    op: "keep",
                    source: "input",
                    start: 0,
                    count: (body.native_messages as unknown[]).length,
                },
            ],
        };
    });
}

function compactions(h: TestAgentSession): CompactionEntry[] {
    return h.sessionManager
        .getEntries()
        .filter((entry): entry is CompactionEntry => entry.type === "compaction");
}

async function eventually(check: () => boolean): Promise<void> {
    const deadline = Date.now() + 5_000;
    while (!check()) {
        if (Date.now() > deadline) throw new Error("condition did not hold within 5 s");
        await Bun.sleep(5);
    }
}

const SETTINGS = { compaction: { enabled: true, keepRecentTokens: 1 } };

interface CompactionEnd {
    reason: string;
    aborted: boolean;
    errorMessage?: string;
    result?: { summary: string; firstKeptEntryId: string };
}

/** Every `compaction_end` the session emits, in order. */
function compactionEnds(h: TestAgentSession): CompactionEnd[] {
    const ends: CompactionEnd[] = [];
    h.session.subscribe((event) => {
        if (event.type === "compaction_end") ends.push(event as unknown as CompactionEnd);
    });
    return ends;
}

describe("Pi eviction", () => {
    it("evicts through the acknowledged boundary at the next idle agent_end with no LLM call", async () => {
        const { call, transforms } = foldingDaemon();
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: SETTINGS,
            });
            await harness.session.prompt(`one ${"x".repeat(400)}`);
            await harness.session.prompt(`two ${"x".repeat(400)}`);
            await eventually(() => compactions(harness as TestAgentSession).length > 0);
            const [compaction] = compactions(harness);
            const boundary = (transforms.at(-1)?.native_messages as { id: string }[]).at(-2)?.id;
            expect(compaction?.summary).toBe(M0);
            expect(compaction?.firstKeptEntryId).toBe(boundary as string);
            expect(compaction?.details).toEqual({ sequence: 2 });
            expect(compaction?.fromHook).toBe(true);
            expect(harness.requests).toHaveLength(2);

            await harness.session.prompt("three");
            const served = (harness.requests.at(-1)?.messages ?? []).map(textOf);
            expect(served[0]).toBe(M0);
            expect(served.filter((text) => text.includes(M0))).toHaveLength(1);
            const sent = (transforms.at(-1)?.native_messages as { id: string }[]).map(
                (row) => row.id,
            );
            expect(sent[0]).toBe(compaction?.firstKeptEntryId as string);
            expect(sent.some((id) => id.startsWith("eidnara:compactionSummary:"))).toBe(false);
        } finally {
            call.mockRestore();
        }
    });

    it("cancels a compaction whose acknowledged boundary left the branch", async () => {
        const { call } = foldingDaemon();
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: SETTINGS,
            });
            await harness.session.prompt(`one ${"x".repeat(400)}`);
            await harness.session.prompt(`two ${"x".repeat(400)}`);
            await harness.session.prompt(`three ${"x".repeat(400)}`);
            await eventually(() => compactions(harness as TestAgentSession).length > 0);
            const before = compactions(harness).length;
            const first = harness.sessionManager
                .getEntries()
                .find((entry) => entry.type === "message" && entry.message.role === "assistant");
            await harness.session.navigateTree(first?.id as string);
            const outcome = await harness.session.compact().then(
                () => "compacted",
                (error: Error) => error.message,
            );
            expect(outcome).toBe("Compaction cancelled");
            expect(compactions(harness)).toHaveLength(before);
        } finally {
            call.mockRestore();
        }
    });

    it("answers an overflow with the fold and completes the turn on the retry", async () => {
        const { call } = foldingDaemon();
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: SETTINGS,
            });
            await harness.session.prompt(`one ${"x".repeat(400)}`);
            await harness.session.prompt(`two ${"x".repeat(400)}`);
            await eventually(() => compactions(harness as TestAgentSession).length > 0);
            const evicted = compactions(harness).length;
            const requests = harness.requests.length;
            const ends = compactionEnds(harness);
            harness.respond([
                fauxAssistantMessage("", {
                    stopReason: "error",
                    errorMessage: "prompt is too long: 213462 tokens > 200000 maximum",
                }),
                fauxAssistantMessage("after the fold"),
            ]);
            await harness.session.prompt("overflowing");
            await eventually(() => textOf(harness?.session.messages.at(-1)) === "after the fold");
            expect(compactions(harness).length).toBeGreaterThan(evicted);
            expect(compactions(harness).every((entry) => entry.summary === M0)).toBe(true);
            const overflow = ends.find((end) => end.reason === "overflow");
            expect(overflow?.aborted).toBe(false);
            expect(overflow?.result?.summary).toBe(M0);
            // The failed call and its one retry; the fold makes no LLM call.
            expect(harness.requests.length - requests).toBe(2);
        } finally {
            call.mockRestore();
        }
    });

    it("cancels an overflow with no acknowledged boundary", async () => {
        const call = unfoldedDaemon();
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: SETTINGS,
            });
            const ends = compactionEnds(harness);
            await harness.session.prompt(`one ${"x".repeat(400)}`);
            harness.respond([
                fauxAssistantMessage("", {
                    stopReason: "error",
                    errorMessage: "prompt is too long: 213462 tokens > 200000 maximum",
                }),
            ]);
            const requests = harness.requests.length;
            await harness.session.prompt("overflowing");
            await eventually(() => ends.some((end) => end.reason === "overflow"));
            expect(ends.find((end) => end.reason === "overflow")?.aborted).toBe(true);
            expect(compactions(harness)).toHaveLength(0);
            expect(harness.requests.length - requests).toBe(1);
        } finally {
            call.mockRestore();
        }
    });

    it("sends Pi's own array, led by m0, when a pass after the eviction throws", async () => {
        const { call } = foldingDaemon();
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: SETTINGS,
            });
            await harness.session.prompt(`one ${"x".repeat(400)}`);
            await harness.session.prompt(`two ${"x".repeat(400)}`);
            await eventually(() => compactions(harness as TestAgentSession).length > 0);
            call.mockImplementation(async (input) => {
                if (input.method === "transform") throw new Error("handler failed");
                return input.method === "transform.boundary"
                    ? { anchors: [] }
                    : { state: "accepted" };
            });
            await harness.session.prompt("after the failure");
            const served = textOf(harness.requests.at(-1)?.messages[0]);
            expect(served).toStartWith("The conversation history before this point was compacted");
            expect(served).toContain(M0);
        } finally {
            call.mockRestore();
        }
    });

    it("answers a threshold compaction with the fold", async () => {
        const reasons: string[] = [];
        const recordsReasons = (pi: {
            on: (event: string, handler: (event: { reason: string }) => undefined) => void;
        }) => pi.on("session_before_compact", (event) => void reasons.push(event.reason));
        // Memory calls that take a while hold the threshold compaction's hook open while the idle
        // eviction's timer fires.
        const { call } = foldingDaemon(false, 20);
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [recordsReasons as never, eidnaraPiExtension],
                contextWindow: 20_000,
                settings: {
                    compaction: { enabled: true, keepRecentTokens: 1, reserveTokens: 16_000 },
                },
            });
            const ends = compactionEnds(harness);
            await harness.session.prompt(`big ${"x".repeat(20_000)}`);
            await eventually(() => reasons.includes("threshold"));
            await eventually(() => compactions(harness as TestAgentSession).length > 0);
            const threshold = ends.find((end) => end.reason === "threshold");
            expect(threshold?.aborted).toBe(false);
            expect(threshold?.result?.summary).toBe(M0);
            // The idle eviction's timer finds the threshold compaction already evicting.
            await Bun.sleep(20);
            expect(compactions(harness)).toHaveLength(1);
            expect(compactions(harness)[0]?.summary).toBe(M0);
            expect(compactions(harness)[0]?.fromHook).toBe(true);
            expect(harness.requests).toHaveLength(1);
        } finally {
            call.mockRestore();
        }
    });

    it("retries a refused eviction at a later agent_end", async () => {
        const { call } = foldingDaemon(true);
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: { compaction: { enabled: true, keepRecentTokens: 600 } },
            });
            const ends = compactionEnds(harness);
            await harness.session.prompt("short");
            await eventually(() => ends.length > 0);
            expect(ends[0]?.errorMessage).toContain("Nothing to compact");
            expect(compactions(harness)).toHaveLength(0);
            for (const turn of [1, 2, 3, 4])
                await harness.session.prompt(`turn ${turn} ${"x".repeat(2_000)}`);
            await eventually(() => compactions(harness as TestAgentSession).length > 0);
            const [compaction] = compactions(harness);
            expect(compaction?.details).toEqual({ sequence: 1 });
            expect(ends.filter((end) => end.result !== undefined)).toHaveLength(1);
        } finally {
            call.mockRestore();
        }
    });

    it("backs off its retries while Pi keeps refusing the eviction", async () => {
        const { call } = foldingDaemon(true);
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: { compaction: { enabled: true, keepRecentTokens: 100_000 } },
            });
            const ends = compactionEnds(harness);
            for (let turn = 1; turn <= 7; turn += 1) {
                await harness.session.prompt(`turn ${turn}`);
                await Bun.sleep(5);
            }
            // Refusals at the 1st, 2nd, and 4th completed runs; the next is due at the 8th.
            expect(ends.map((end) => end.errorMessage ?? "")).toHaveLength(3);
            expect(ends.every((end) => end.errorMessage?.includes("Nothing to compact"))).toBe(
                true,
            );
            expect(compactions(harness)).toHaveLength(0);
        } finally {
            call.mockRestore();
        }
    });

    it("commits one eviction per boundary whichever path commits it", async () => {
        const { call } = foldingDaemon(true);
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: SETTINGS,
            });
            const ends = compactionEnds(harness);
            await harness.session.prompt(`one ${"x".repeat(400)}`);
            await harness.session.prompt(`two ${"x".repeat(400)}`);
            await eventually(() => compactions(harness as TestAgentSession).length > 0);
            for (const turn of [3, 4]) await harness.session.prompt(`turn ${turn}`);
            await Bun.sleep(20);
            expect(compactions(harness)).toHaveLength(1);
            expect(ends.filter((end) => end.aborted || end.errorMessage)).toHaveLength(0);
            const manual = await harness.session.compact().then(
                () => "compacted",
                (error: Error) => error.message,
            );
            expect(manual).toBe("Compaction cancelled");
        } finally {
            call.mockRestore();
        }
    });

    it("leaves a queued follow-up to run before evicting", async () => {
        const { call } = foldingDaemon();
        let queued = false;
        const followUp = (pi: {
            on: (event: string, handler: () => undefined) => void;
            sendUserMessage: (text: string, options: { deliverAs: string }) => void;
        }) =>
            pi.on("agent_end", () => {
                if (queued) return;
                queued = true;
                pi.sendUserMessage("follow-up", { deliverAs: "followUp" });
            });
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [followUp as never, eidnaraPiExtension],
                settings: SETTINGS,
            });
            const order: string[] = [];
            harness.session.subscribe((event) => {
                if (event.type === "compaction_start") order.push("compaction");
                if (event.type === "message_end" && event.message.role === "assistant")
                    order.push(textOf(event.message));
            });
            // The follow-up's model call is still running when the eviction's timer fires.
            harness.respond([
                fauxAssistantMessage("first answer"),
                async () => {
                    await Bun.sleep(50);
                    return fauxAssistantMessage("followed");
                },
            ]);
            await harness.session.prompt(`one ${"x".repeat(400)}`);
            await eventually(() => order.includes("compaction"));
            expect(order).toEqual(["first answer", "followed", "compaction"]);
            await eventually(() => compactions(harness as TestAgentSession).length > 0);
        } finally {
            call.mockRestore();
        }
    });

    it("leaves Pi's retry of an errored run to finish before evicting", async () => {
        const { call } = foldingDaemon();
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: { ...SETTINGS, retry: { enabled: true, maxRetries: 1, baseDelayMs: 50 } },
            });
            await harness.session.prompt(`one ${"x".repeat(400)}`);
            await eventually(() => compactions(harness as TestAgentSession).length > 0);
            harness.respond([
                fauxAssistantMessage("", {
                    stopReason: "error",
                    errorMessage: "529 overloaded_error: Overloaded",
                }),
                fauxAssistantMessage("after the retry"),
            ]);
            await harness.session.prompt(`two ${"x".repeat(400)}`);
            await eventually(() => textOf(harness?.session.messages.at(-1)) === "after the retry");
        } finally {
            call.mockRestore();
        }
    });

    it("lets a tool loop run to its end before evicting", async () => {
        const { call, transforms } = foldingDaemon();
        const calls: number[] = [];
        const probe = (pi: { registerTool: (tool: unknown) => void }) =>
            pi.registerTool({
                name: "probe",
                label: "probe",
                description: "probe",
                parameters: Type.Object({ n: Type.Number() }),
                execute: async (_id: string, params: { n: number }) => {
                    calls.push(params.n);
                    return { content: [{ type: "text", text: `probed ${params.n}` }], details: {} };
                },
            });
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension, probe as never],
                tools: ["probe"],
                settings: SETTINGS,
            });
            await harness.session.prompt(`one ${"x".repeat(400)}`);
            harness.respond([
                fauxAssistantMessage(fauxToolCall("probe", { n: 1 })),
                fauxAssistantMessage(fauxToolCall("probe", { n: 2 })),
                fauxAssistantMessage(fauxToolCall("probe", { n: 3 })),
                fauxAssistantMessage("loop done"),
            ]);
            await harness.session.prompt(`loop ${"x".repeat(400)}`);
            expect(calls).toEqual([1, 2, 3]);
            expect(textOf(harness.session.messages.at(-1))).toBe("loop done");
            expect(transforms.length).toBeGreaterThanOrEqual(5);
            await eventually(() => compactions(harness as TestAgentSession).length > 0);
        } finally {
            call.mockRestore();
        }
    });

    it("reaches the hook for a model with no stored provider key", async () => {
        const { call } = foldingDaemon();
        let dropKey = false;
        // The key goes after the turn's model call and before the idle eviction's compaction.
        const dropsKey = (pi: { on: (event: string, handler: () => undefined) => void }) =>
            pi.on("agent_end", () => {
                if (dropKey) harness?.authStorage.removeRuntimeApiKey("faux");
            });
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [dropsKey as never, eidnaraPiExtension],
                settings: SETTINGS,
            });
            await harness.session.prompt(`one ${"x".repeat(400)}`);
            await harness.session.prompt(`two ${"x".repeat(400)}`);
            await eventually(() => compactions(harness as TestAgentSession).length > 0);
            const before = compactions(harness).length;
            const ends = compactionEnds(harness);
            dropKey = true;
            await harness.session.prompt(`three ${"x".repeat(400)}`);
            await eventually(() => ends.length > 0);
            expect(ends[0]?.errorMessage).toBeUndefined();
            expect(ends[0]?.result?.summary).toBe(M0);
            expect(compactions(harness)).toHaveLength(before + 1);
        } finally {
            call.mockRestore();
        }
    });

    it("cancels a threshold compaction with no acknowledged boundary", async () => {
        const reasons: string[] = [];
        const recordsReasons = (pi: {
            on: (event: string, handler: (event: { reason: string }) => undefined) => void;
        }) => pi.on("session_before_compact", (event) => void reasons.push(event.reason));
        const call = unfoldedDaemon();
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [recordsReasons as never, eidnaraPiExtension],
                contextWindow: 20_000,
                settings: {
                    compaction: { enabled: true, keepRecentTokens: 1, reserveTokens: 16_000 },
                },
            });
            const ends = compactionEnds(harness);
            await harness.session.prompt(`first ${"x".repeat(2_000)}`);
            await harness.session.prompt(`big ${"x".repeat(20_000)}`);
            await eventually(() => ends.some((end) => end.reason === "threshold"));
            expect(ends.find((end) => end.reason === "threshold")?.aborted).toBe(true);
            expect(compactions(harness)).toHaveLength(0);
            expect(harness.requests).toHaveLength(2);
        } finally {
            call.mockRestore();
        }
    });
});

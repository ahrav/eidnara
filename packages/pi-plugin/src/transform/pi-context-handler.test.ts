import { afterEach, beforeEach, describe, expect, it, spyOn } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fauxAssistantMessage } from "@earendil-works/pi-ai/compat";
import { HostModuleTransport } from "@eidnara/opencode/hooks/context/module-transport";

import { createTestAgentSession, type TestAgentSession } from "../__tests__/agent-session";
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
    root = mkdtempSync(join(tmpdir(), "eidnara-pi-context-"));
    process.env.XDG_CONFIG_HOME = join(root, "config");
    process.env.XDG_DATA_HOME = join(root, "data");
    // A summarizer chain in user configuration makes Eidnara own compaction.
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

function textOf(message: unknown): string {
    const content = (message as { content?: unknown }).content;
    if (typeof content === "string") return content;
    return (content as { text?: string }[]).map((part) => part.text ?? "").join("");
}

function keepWindow(body: Json): Json {
    return {
        status: "ok",
        action: "SOFT",
        boundary: null,
        base_revision: body.base_revision,
        output_revision: `out-${String(body.base_revision)}`,
        operations: [
            {
                op: "keep",
                source: "input",
                start: 0,
                count: (body.native_messages as unknown[]).length,
            },
        ],
    };
}

function daemon(transform: (body: Json) => unknown) {
    const transforms: Json[] = [];
    const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(async (input) => {
        const body = input.body as Json;
        if (input.method === "transform.boundary") return { anchors: [] };
        if (input.method === "transform") {
            transforms.push(body);
            return transform(body);
        }
        return { state: "accepted" };
    });
    return { call, transforms };
}

describe("Pi context handler under createAgentSession", () => {
    it("serves the applied values to the provider and names rows by entry ids", async () => {
        const { call, transforms } = daemon((body) => ({
            status: "ok",
            action: "HARD",
            boundary: null,
            base_revision: body.base_revision,
            output_revision: "out-1",
            operations: [
                {
                    op: "insert",
                    values: [
                        {
                            id: "eidnara:synthetic:m0",
                            message: { role: "user", content: "folded history", timestamp: 0 },
                        },
                    ],
                },
                {
                    op: "keep",
                    source: "input",
                    start: 0,
                    count: (body.native_messages as unknown[]).length,
                },
            ],
        }));
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
            });
            await harness.session.prompt("first question");
            const served = harness.requests[0]?.messages ?? [];
            expect(served.map(textOf)).toEqual(["folded history", "first question"]);
            const sent = transforms[0] ?? {};
            expect(sent.serializer_profile).toBe("pi");
            const userEntry = harness.sessionManager
                .getEntries()
                .find((entry) => entry.type === "message");
            expect((sent.native_messages as { id: string }[]).map((row) => row.id)).toEqual([
                userEntry?.id as string,
            ]);
        } finally {
            call.mockRestore();
        }
    });

    it("leaves Pi's array unchanged when the daemon declines", async () => {
        const { call } = daemon(() => {
            throw new Error("daemon unavailable");
        });
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
            });
            await harness.session.prompt("plain question");
            expect((harness.requests[0]?.messages ?? []).map(textOf)).toEqual(["plain question"]);
        } finally {
            call.mockRestore();
        }
    });

    it("names the retry's rows by entry ids after Pi drops the failed response", async () => {
        const { call, transforms } = daemon(keepWindow);
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: { retry: { enabled: true, maxRetries: 1, baseDelayMs: 1 } },
            });
            harness.respond([
                fauxAssistantMessage("", { stopReason: "error", errorMessage: "overloaded" }),
            ]);
            await harness.session.prompt("retried question");
            const entries = harness.sessionManager.getEntries() as {
                id: string;
                message?: { role?: string; stopReason?: string };
            }[];
            const userId = entries.find((entry) => entry.message?.role === "user")?.id;
            expect(entries.some((entry) => entry.message?.stopReason === "error")).toBe(true);
            expect(transforms).toHaveLength(2);
            for (const sent of transforms)
                expect((sent.native_messages as { id: string }[]).map((row) => row.id)).toEqual([
                    userId as string,
                ]);
        } finally {
            call.mockRestore();
        }
    });

    it("cancels Pi's compaction while Eidnara owns compaction", async () => {
        const { call } = daemon(keepWindow);
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: { compaction: { enabled: false, keepRecentTokens: 1 } },
            });
            for (const turn of [1, 2, 3])
                await harness.session.prompt(`turn ${turn} ${"x".repeat(400)}`);
            const outcome = await harness.session.compact().then(
                () => "compacted",
                (error: Error) => error.message,
            );
            expect(outcome).toBe("Compaction cancelled");
            expect(
                harness.sessionManager.getEntries().some((entry) => entry.type === "compaction"),
            ).toBe(false);
        } finally {
            call.mockRestore();
        }
    });

    it("leaves compaction to Pi while every pass declines", async () => {
        const { call, transforms } = daemon(() => {
            throw new Error("daemon unavailable");
        });
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: { compaction: { enabled: false, keepRecentTokens: 1 } },
            });
            for (const turn of [1, 2, 3])
                await harness.session.prompt(`turn ${turn} ${"x".repeat(400)}`);
            expect(transforms.length).toBeGreaterThan(0);
            const outcome = await harness.session.compact().then(
                () => "compacted",
                (error: Error) => error.message,
            );
            expect(outcome).toBe("compacted");
        } finally {
            call.mockRestore();
        }
    });

    it("cancels Pi's compaction again once a pass applies", async () => {
        let available = false;
        const { call } = daemon((body) => {
            if (!available) throw new Error("daemon unavailable");
            return keepWindow(body);
        });
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
                settings: { compaction: { enabled: false, keepRecentTokens: 1 } },
            });
            await harness.session.prompt(`turn 1 ${"x".repeat(400)}`);
            available = true;
            for (const turn of [2, 3])
                await harness.session.prompt(`turn ${turn} ${"x".repeat(400)}`);
            const outcome = await harness.session.compact().then(
                () => "compacted",
                (error: Error) => error.message,
            );
            expect(outcome).toBe("Compaction cancelled");
        } finally {
            call.mockRestore();
        }
    });

    it("leaves folding and compaction to Pi in a project whose configuration is unresolved", async () => {
        const { call, transforms } = daemon(keepWindow);
        try {
            const project = join(root, "unresolved");
            mkdirSync(join(project, ".eidnara"), { recursive: true });
            writeFileSync(join(project, ".eidnara", "eidnara.jsonc"), "{ not json");
            harness = await createTestAgentSession({
                cwd: project,
                extensionFactories: [eidnaraPiExtension],
                settings: { compaction: { enabled: false, keepRecentTokens: 1 } },
            });
            for (const turn of [1, 2, 3])
                await harness.session.prompt(`turn ${turn} ${"x".repeat(400)}`);
            expect(transforms).toHaveLength(0);
            const outcome = await harness.session.compact().then(
                () => "compacted",
                (error: Error) => error.message,
            );
            expect(outcome).toBe("compacted");
        } finally {
            call.mockRestore();
        }
    });

    it("sends the previous response's completion time, not the time its stream started", async () => {
        const { call, transforms } = daemon(keepWindow);
        try {
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [eidnaraPiExtension],
            });
            const streamStartedAt = Date.now() - 10 * 60_000;
            harness.respond([fauxAssistantMessage("long answer", { timestamp: streamStartedAt })]);
            await harness.session.prompt("first");
            await harness.session.prompt("second");
            const entry = harness.sessionManager
                .getEntries()
                .find(
                    (candidate) =>
                        candidate.type === "message" && candidate.message.role === "assistant",
                );
            const completedAt = Date.parse(entry?.timestamp ?? "");
            expect(completedAt).toBeGreaterThan(streamStartedAt);
            expect(transforms.at(-1)?.prev_response_completed_at_ms).toBe(completedAt);
        } finally {
            call.mockRestore();
        }
    });

    it("names a live custom message an extension adds before the turn", async () => {
        const { call, transforms } = daemon(keepWindow);
        try {
            const addsNote = (pi: { on: (event: string, handler: () => unknown) => void }) => {
                pi.on("before_agent_start", () => ({
                    message: { customType: "note", content: "a hint", display: false },
                }));
            };
            harness = await createTestAgentSession({
                cwd: root,
                extensionFactories: [addsNote as never, eidnaraPiExtension],
            });
            await harness.session.prompt("first");
            await harness.session.prompt("second");
            const custom = harness.sessionManager
                .getEntries()
                .filter((entry) => entry.type === "custom_message")
                .map((entry) => `eidnara:custom:${entry.id}`);
            expect(custom).toHaveLength(2);
            const sent = transforms.at(-1) ?? {};
            const ids = (sent.native_messages as { id: string }[]).map((row) => row.id);
            expect(ids).toEqual(expect.arrayContaining(custom));
            expect(transforms).toHaveLength(2);
        } finally {
            call.mockRestore();
        }
    });
});

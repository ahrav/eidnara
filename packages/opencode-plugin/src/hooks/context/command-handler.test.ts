/// <reference types="bun-types" />

import { afterEach, describe, expect, it, mock } from "bun:test";
import {
    __resetNotificationStateForTests,
    type RpcNotification,
    registerNotificationSink,
} from "../../shared/rpc-notifications";
import { createEidnaraCommandHandler } from "./command-handler";
import { MAX_WRAPUP_REQUEST_BUDGET_MS } from "./module-transport";
import type { RustModeModuleClient } from "./rust-mode-transform";
import { __ignoredNotificationTest } from "./send-session-notification";

interface RecordedCall {
    method: string;
    projectRoot: string;
    body: Record<string, unknown>;
    timeoutMs?: number;
}

type Deps = Parameters<typeof createEidnaraCommandHandler>[0];

/** Builds a handler over a recording fake daemon client that answers from `respond`. */
function setup(
    respond: (call: RecordedCall) => unknown = () => ({ ok: true }),
    extra: Partial<Omit<Deps, "moduleClient" | "sendNotification">> = {},
) {
    const calls: RecordedCall[] = [];
    const moduleClient: RustModeModuleClient = {
        call: async (request) => {
            const call: RecordedCall = {
                method: request.method,
                projectRoot: request.projectRoot,
                body: request.body as Record<string, unknown>,
                timeoutMs: (request as { timeoutMs?: number }).timeoutMs,
            };
            calls.push(call);
            return respond(call);
        },
    };
    const sendNotification = mock(
        async (_sessionId: string, _text: string, _params: unknown) => {},
    );
    const handler = createEidnaraCommandHandler({
        moduleClient,
        sendNotification,
        isSubagentSession: () => false,
        ...extra,
    });
    const texts = (): string[] =>
        (sendNotification.mock.calls as unknown as Array<[string, string]>).map(([, text]) => text);
    const run = (command: string, sessionID: string, args = "", params = {}): Promise<void> =>
        handler["command.execute.before"](
            { command, sessionID, arguments: args },
            { parts: [{ type: "text", text: "" }] },
            params,
        );
    return { handler, run, calls, sendNotification, texts };
}

function connectTui(sessionId: string): RpcNotification[] {
    const received: RpcNotification[] = [];
    registerNotificationSink({
        sessionId,
        protocol: 2,
        send: (notification) => received.push(notification),
    });
    return received;
}

function sentinelFor(command: string): string {
    return `__CONTEXT_MANAGEMENT_${command.toUpperCase()}_HANDLED__`;
}

async function expectSentinel(promise: Promise<unknown>, command: string): Promise<void> {
    try {
        await promise;
        throw new Error(`Expected sentinel for ${command}`);
    } catch (error) {
        expect(String(error)).toContain(sentinelFor(command));
        const e = error as Record<string, unknown>;
        expect(e["~effect/http/HttpServerResponse"]).toBe("~effect/http/HttpServerResponse");
        expect(e["~effect/ErrorReporter/ignore"]).toBe(true);
        expect(e.status).toBe(204);
        expect((e.body as { _tag?: unknown })?._tag).toBe("Empty");
        expect((e.cookies as { cookies?: unknown })?.cookies).toEqual({});
    }
}

const STATUS_RESPONSE = {
    ok: true,
    summary:
        "session ses-status (last active 3m ago): 4 compartments, coverage ordinal 17, boundary present, 2 pending drops, 1 tag, pending m1 delta false, last historian: published, publish failures: 0, surface active",
    usage: { current_total_input_tokens: 42_000, context_limit_tokens: 100_000 },
    boundary_present: true,
    coverage_ordinal: 17,
    compartment_count: 4,
    pending_drop_count: 2,
    tag_count: 1,
    pending_m1_delta: false,
    pending_m1_age_ms: null,
    wrapup_active: false,
    wrapup_rounds: null,
    historian: { consecutive_publish_failures: 0, publish_health_degraded: false },
    pass_trace: {
        receive_count: 12,
        reject_count: 0,
        last_reject_error: null,
    },
};

describe("createEidnaraCommandHandler", () => {
    afterEach(() => {
        __resetNotificationStateForTests();
    });

    for (const command of [
        "something-else",
        "ctx-dream",
        "ctx-embed",
        "ctx-approve",
        "ctx-enforce",
        "ctx-session-upgrade",
    ]) {
        it(`does not intercept /${command}`, async () => {
            const { handler, calls, sendNotification } = setup();
            const output = { parts: [{ type: "text", text: "" }] };

            await handler["command.execute.before"](
                { command, sessionID: "ses-unknown", arguments: "start" },
                output,
                {},
            );

            expect(sendNotification).not.toHaveBeenCalled();
            expect(calls).toHaveLength(0);
            expect(output).toEqual({ parts: [{ type: "text", text: "" }] });
        });
    }

    for (const command of ["ctx-wrapup", "ctx-recomp", "ctx-flush"] as const) {
        it(`refuses /${command} without a daemon call when compaction is off`, async () => {
            const onFlush = mock(() => {});
            const { run, calls, sendNotification } = setup(undefined, {
                compactionOff: true,
                onFlush,
            });

            await expectSentinel(run(command, "ses-compaction-off"), command);

            expect(sendNotification).toHaveBeenCalledWith(
                "ses-compaction-off",
                `Eidnara compaction is disabled (compaction.enabled: false) — /${command} manages compacted history and has no effect in this mode.`,
                {},
            );
            expect(calls).toHaveLength(0);
            expect(onFlush).not.toHaveBeenCalled();
        });
    }

    describe("ctx-flush", () => {
        it("sends session.flush, reports the armed wording, and calls onFlush", async () => {
            const onFlush = mock(() => {});
            const { run, calls, sendNotification } = setup(() => ({ ok: true, armed: true }), {
                onFlush,
            });

            await expectSentinel(run("ctx-flush", "ses-flush"), "ctx-flush");

            expect(calls).toEqual([
                {
                    method: "session.flush",
                    projectRoot: process.cwd(),
                    body: { method: "session.flush", v: 1, session_id: "ses-flush" },
                    timeoutMs: undefined,
                },
            ]);
            expect(onFlush).toHaveBeenCalledWith("ses-flush");
            expect(sendNotification).toHaveBeenCalledTimes(1);
            expect(sendNotification).toHaveBeenCalledWith(
                "ses-flush",
                "Flushed: Changes take effect on next message.",
                {},
            );
        });

        it("reports the not-armed wording and unwraps a `result` envelope", async () => {
            const { run, texts } = setup(() => ({ result: { armed: false } }));

            await expectSentinel(run("ctx-flush", "ses-flush-empty"), "ctx-flush");

            expect(texts()).toEqual(["No pending operations to flush."]);
        });

        it("reports a daemon failure and still calls onFlush", async () => {
            const onFlush = mock(() => {});
            const { run, texts } = setup(
                () => {
                    throw new Error("socket closed");
                },
                { onFlush },
            );

            await expectSentinel(run("ctx-flush", "ses-flush-fail"), "ctx-flush");

            expect(onFlush).toHaveBeenCalledWith("ses-flush-fail");
            expect(texts()).toEqual(["Error: Failed to flush context operations. socket closed"]);
        });

        it("pushes the flush dialog to a connected TUI instead of notifying", async () => {
            const received = connectTui("ses-flush-tui");
            const { run, sendNotification } = setup(() => ({ armed: true }));

            await expectSentinel(run("ctx-flush", "ses-flush-tui"), "ctx-flush");

            expect(sendNotification).not.toHaveBeenCalled();
            expect(received.map((n) => [n.type, n.payload])).toEqual([
                [
                    "action",
                    {
                        action: "show-flush-dialog",
                        message: "Flushed: Changes take effect on next message.",
                    },
                ],
            ]);
        });
    });

    describe("ctx-status", () => {
        it("routes the daemon call by the session's resolved directory", async () => {
            const resolveProjectRoot = mock(async (sessionId: string) => `/repos/${sessionId}`);
            const { run, calls } = setup(() => STATUS_RESPONSE, { resolveProjectRoot });

            await expectSentinel(run("ctx-status", "ses-routed"), "ctx-status");

            expect(resolveProjectRoot).toHaveBeenCalledWith("ses-routed");
            expect(calls.map((call) => [call.method, call.projectRoot])).toEqual([
                ["session.status", "/repos/ses-routed"],
            ]);
        });

        it("sends session.status and renders the daemon text", async () => {
            const { run, calls, texts } = setup(() => STATUS_RESPONSE);

            await expectSentinel(run("ctx-status", "ses-status"), "ctx-status");

            expect(calls).toEqual([
                {
                    method: "session.status",
                    projectRoot: process.cwd(),
                    body: { method: "session.status", v: 1, session_id: "ses-status" },
                    timeoutMs: undefined,
                },
            ]);
            const [text] = texts();
            expect(text).toContain("## Eidnara Status");
            expect(text).toContain("### Module Cache");
            expect(text).toContain("- Usage: 42,000 / 100,000 tokens");
            expect(text).toContain("- Boundary: present");
            expect(text).toContain("- Coverage ordinal: 17");
            expect(text).toContain("- Compartments: 4");
            expect(text).toContain("- Pending: 2 drops, 1 tag, m1 delta none");
            expect(text).toContain("- Wrapup: idle");
            expect(text).toContain(
                "- Historian publish health: ok (0 consecutive publish failures)",
            );
            expect(text).toContain("- Passes: 12 received, 0 rejected");
            expect(text).toContain(`- Daemon: ${STATUS_RESPONSE.summary}`);
            expect(text).not.toContain("### Tail Hygiene");
            expect(text).not.toContain("**Compaction:** disabled");
        });

        it("renders pending work, a running wrapup, degraded publish health, and the last reject", async () => {
            const { run, texts } = setup(() => ({
                ...STATUS_RESPONSE,
                pending_m1_delta: true,
                pending_m1_age_ms: 42_500,
                wrapup_active: true,
                wrapup_rounds: 3,
                historian: { consecutive_publish_failures: 4, publish_health_degraded: true },
                pass_trace: {
                    receive_count: 20,
                    reject_count: 2,
                    last_reject_error: `snapshot stale ${"x".repeat(200)}`,
                },
            }));

            await expectSentinel(run("ctx-status", "ses-status-degraded"), "ctx-status");

            const [text] = texts();
            expect(text).toContain("- Pending: 2 drops, 1 tag, m1 delta pending (43s)");
            expect(text).toContain("- Wrapup: running (3 rounds complete)");
            expect(text).toContain(
                "- Historian publish health: degraded (4 consecutive publish failures)",
            );
            expect(text).toContain(
                "- Passes: 20 received, 2 rejected; last reject: snapshot stale ",
            );
            const passesLine = text.split("\n").find((line) => line.startsWith("- Passes:"));
            expect(passesLine?.length).toBeLessThan(220);
        });

        it("omits the pass and summary lines when the daemon does not supply them", async () => {
            const { run, texts } = setup(() => ({
                ok: true,
                usage: { current_total_input_tokens: 10, context_limit_tokens: 100 },
                compartment_count: 0,
            }));

            await expectSentinel(run("ctx-status", "ses-status-minimal"), "ctx-status");

            const [text] = texts();
            expect(text).toContain("- Pending: 0 drops, 0 tags, m1 delta none");
            expect(text).toContain("- Wrapup: idle");
            expect(text).toContain(
                "- Historian publish health: ok (0 consecutive publish failures)",
            );
            expect(text).not.toContain("- Passes:");
            expect(text).not.toContain("- Daemon:");
        });

        it("renders tail hygiene when the daemon reports a baseline", async () => {
            const { run, texts } = setup(() => ({
                ...STATUS_RESPONSE,
                tail_hygiene: { u: 250, t: 1_000, evaluable: true },
            }));

            await expectSentinel(run("ctx-status", "ses-status-hygiene"), "ctx-status");

            const [text] = texts();
            expect(text).toContain("### Tail Hygiene");
            expect(text).toContain("- Reclaimable / eligible: 25.0% · 250 / 1,000 tok");
        });

        it("labels compaction-off mode and reports a failed daemon call", async () => {
            const { run, texts } = setup(
                () => {
                    throw new Error("daemon unavailable");
                },
                { compactionOff: true },
            );

            await expectSentinel(run("ctx-status", "ses-status-off"), "ctx-status");

            const [text] = texts();
            expect(text).toContain("**Compaction:** disabled (compaction.enabled: false)");
            expect(text).toContain("Session status is unavailable: daemon unavailable");
        });

        it("pushes the status dialog to a connected TUI instead of notifying", async () => {
            const received = connectTui("ses-status-tui");
            const { run, calls, sendNotification } = setup(() => STATUS_RESPONSE);

            await expectSentinel(run("ctx-status", "ses-status-tui"), "ctx-status");

            expect(calls.map((call) => call.method)).toEqual(["session.status"]);
            expect(sendNotification).not.toHaveBeenCalled();
            expect(received.map((n) => n.payload)).toEqual([{ action: "show-status-dialog" }]);
        });

        it("strips agent and model params from context command notifications", async () => {
            const { run, sendNotification } = setup(() => STATUS_RESPONSE);

            await expectSentinel(
                run("ctx-status", "ses-stable-model", "", {
                    agent: "oracle",
                    variant: "fast",
                    providerId: "anthropic",
                    modelId: "claude-sonnet-4-6",
                }),
                "ctx-status",
            );

            expect(sendNotification).toHaveBeenCalledWith(
                "ses-stable-model",
                expect.stringContaining("## Eidnara Status"),
                {},
            );
        });
    });

    describe("ctx-recomp", () => {
        it("sends session.recomp with a minted command id and maps `started`", async () => {
            const { run, calls, texts } = setup(() => ({ ok: true, disposition: "started" }));

            await expectSentinel(run("ctx-recomp", "ses-recomp"), "ctx-recomp");

            expect(calls).toHaveLength(1);
            expect(calls[0]?.method).toBe("session.recomp");
            expect(calls[0]?.body).toMatchObject({
                method: "session.recomp",
                v: 1,
                session_id: "ses-recomp",
            });
            expect(calls[0]?.body.command_id).toMatch(/^opencode-recomp-/);
            expect(texts().join("\n")).toContain("Historian recomp started");
        });

        it("maps `already_in_progress`, `nothing_to_do`, failures, and thrown calls", async () => {
            const cases: Array<[() => unknown, string]> = [
                [() => ({ disposition: "already_in_progress" }), "## Eidnara Recomp — Skipped"],
                [() => ({ disposition: "nothing_to_do" }), "Nothing to rebuild"],
                [
                    () => ({ disposition: "failed", summary: "store locked" }),
                    "## Eidnara Recomp — Failed\n\nstore locked",
                ],
                [
                    () => {
                        throw new Error("timeout");
                    },
                    "## Eidnara Recomp — Failed\n\ntimeout",
                ],
            ];
            for (const [respond, expected] of cases) {
                const { run, texts } = setup(respond);
                await expectSentinel(run("ctx-recomp", "ses-recomp-map"), "ctx-recomp");
                expect(texts().join("\n")).toContain(expected);
            }
        });

        it("refuses a message range without a daemon call", async () => {
            const { run, calls, texts } = setup();

            await expectSentinel(run("ctx-recomp", "ses-recomp-range", "1-11322"), "ctx-recomp");

            expect(calls).toHaveLength(0);
            const [text] = texts();
            expect(text).toContain("## Eidnara Recomp — Unsupported");
            expect(text).toContain("Requested range: `1-11322`");
            expect(text).toContain("`session.recomp` accepts no message range");
            expect(text).toContain("Usage:");
        });

        it("rejects malformed arguments without a daemon call", async () => {
            const { run, calls, texts } = setup();

            await expectSentinel(run("ctx-recomp", "ses-recomp-bad", "--upgrade"), "ctx-recomp");

            expect(calls).toHaveLength(0);
            const [text] = texts();
            expect(text).toContain("## Eidnara Recomp — Invalid Arguments");
            expect(text).toContain("Invalid /ctx-recomp arguments: `--upgrade`");
        });
    });

    describe("ctx-wrapup", () => {
        it("parses messagesToKeep and sends session.wrapup under the wrapup budget", async () => {
            const { run, calls, texts } = setup(() => ({
                disposition: "completed",
                rounds: 2,
                summary: "Wrapped up.",
            }));

            await expectSentinel(run("ctx-wrapup", "ses-wrapup", "250"), "ctx-wrapup");

            expect(calls).toHaveLength(1);
            expect(calls[0]?.method).toBe("session.wrapup");
            expect(calls[0]?.timeoutMs).toBe(MAX_WRAPUP_REQUEST_BUDGET_MS);
            expect(calls[0]?.body).toMatchObject({
                method: "session.wrapup",
                v: 1,
                session_id: "ses-wrapup",
                keep: 250,
            });
            expect(calls[0]?.body.command_id).toMatch(/^opencode-wrapup-/);
            expect(texts()).toEqual([
                "## Eidnara Wrapup\n\nStarting wrapup…",
                "## Eidnara Wrapup\n\nWrapped up. (2 rounds)",
            ]);
        });

        it("defaults messagesToKeep to 20", async () => {
            const { run, calls, texts } = setup(() => ({ disposition: "nothing_to_compact" }));

            await expectSentinel(run("ctx-wrapup", "ses-wrapup-default"), "ctx-wrapup");

            expect(calls[0]?.body.keep).toBe(20);
            expect(texts().join("\n")).toContain("Nothing to compact.");
        });

        it("maps already_in_progress, retryable, failed, and thrown outcomes", async () => {
            const cases: Array<[() => unknown, string[]]> = [
                [
                    () => ({ disposition: "already_in_progress", rounds: 1 }),
                    ["## Eidnara Wrapup — Skipped", "(1 round complete)"],
                ],
                [
                    () => ({ ok: false, disposition: "retryable", summary: "budget expired" }),
                    [
                        "## Eidnara Wrapup — Partial",
                        "budget expired Run /ctx-wrapup again to continue.",
                    ],
                ],
                [
                    () => ({ disposition: "failed", rounds: 3 }),
                    ["## Eidnara Wrapup — Failed", "(3 rounds)"],
                ],
                [
                    () => {
                        throw new Error("timeout");
                    },
                    ["## Eidnara Wrapup — Failed\n\ntimeout"],
                ],
            ];
            for (const [respond, expected] of cases) {
                const { run, texts } = setup(respond);
                await expectSentinel(run("ctx-wrapup", "ses-wrapup-map"), "ctx-wrapup");
                const text = texts().join("\n");
                for (const fragment of expected) expect(text).toContain(fragment);
                if (text.includes("— Partial")) expect(text).not.toContain("— Failed");
            }
        });

        it("rejects a non-positive keep count without a daemon call", async () => {
            const { run, calls, texts } = setup();

            await expectSentinel(run("ctx-wrapup", "ses-wrapup-bad", "0"), "ctx-wrapup");

            expect(calls).toHaveLength(0);
            expect(texts()).toEqual([
                "## Eidnara Wrapup — Invalid Arguments\n\nmessages_to_keep must be a positive integer.",
            ]);
        });

        it("refuses a subagent session without a daemon call", async () => {
            const isSubagentSession = mock((sessionId: string) => sessionId === "ses-child");
            const { run, calls, texts } = setup(undefined, { isSubagentSession });

            await expectSentinel(run("ctx-wrapup", "ses-child", "50"), "ctx-wrapup");

            expect(isSubagentSession).toHaveBeenCalledWith("ses-child");
            expect(calls).toHaveLength(0);
            expect(texts()).toEqual([
                "## Eidnara Wrapup — Skipped\n\n/ctx-wrapup is only available in primary sessions.",
            ]);
        });

        it("awaits an asynchronous subagent classification before deciding", async () => {
            const isSubagentSession = mock(async (sessionId: string) => {
                await Bun.sleep(0);
                return sessionId === "ses-restored-child";
            });
            const { run, calls } = setup(undefined, { isSubagentSession });

            await expectSentinel(run("ctx-wrapup", "ses-restored-child", "50"), "ctx-wrapup");

            expect(calls).toHaveLength(0);
        });
    });

    describe("ctx-aug", () => {
        it("runs sidekick in a child session and sends the augmented prompt", async () => {
            const sidekickClient = {
                session: {
                    create: mock(async () => ({ data: { id: "sidekick-child" } })),
                    prompt: mock(async () => undefined),
                    promptAsync: mock(async () => undefined),
                    messages: mock(async () => ({
                        data: [
                            {
                                info: { role: "assistant", time: { created: Date.now() } },
                                parts: [{ type: "text", text: "Use Bun for commands" }],
                            },
                        ],
                    })),
                    delete: mock(async () => ({ data: undefined })),
                },
            };
            const { run, calls, sendNotification } = setup(undefined, {
                sidekick: {
                    config: { timeout_ms: 5_000 },
                    projectPath: "/repo/project",
                    resolveSessionDirectory: () => "/repo/project",
                    client: sidekickClient as never,
                },
            });

            await expectSentinel(
                run("ctx-aug", "ses-aug", "Implement sidekick migration", {
                    agent: "plan",
                    variant: "thinking",
                    providerId: "anthropic",
                    modelId: "claude-opus-4-8",
                }),
                "ctx-aug",
            );

            expect(calls).toHaveLength(0);
            expect(sendNotification).toHaveBeenCalledWith(
                "ses-aug",
                "🔍 Preparing augmentation… this may take 2-10s depending on your sidekick provider.",
                {},
            );
            expect(sidekickClient.session.create).toHaveBeenCalledTimes(1);
            expect(sidekickClient.session.promptAsync).toHaveBeenCalledWith({
                path: { id: "ses-aug" },
                body: {
                    agent: "plan",
                    model: { providerID: "anthropic", modelID: "claude-opus-4-8" },
                    variant: "thinking",
                    parts: [
                        {
                            type: "text",
                            text: "Implement sidekick migration\n\n<sidekick-augmentation>\nUse Bun for commands\n</sidekick-augmentation>",
                        },
                    ],
                },
            });
        });

        it("reports when sidekick is not configured", async () => {
            const { run, texts } = setup();

            await expectSentinel(run("ctx-aug", "ses-aug-missing", "Help"), "ctx-aug");

            expect(texts().join("\n")).toContain("Sidekick is not configured");
        });

        it("tells the user when the augmented prompt cannot be sent", async () => {
            const sidekickClient = {
                session: {
                    create: mock(async () => ({ data: { id: "sidekick-child" } })),
                    prompt: mock(async () => undefined),
                    promptAsync: mock(async () => {
                        throw new Error("session is busy");
                    }),
                    messages: mock(async () => ({
                        data: [
                            {
                                info: { role: "assistant", time: { created: Date.now() } },
                                parts: [{ type: "text", text: "Use Bun for commands" }],
                            },
                        ],
                    })),
                    delete: mock(async () => ({ data: undefined })),
                },
            };
            const { run, texts } = setup(undefined, {
                sidekick: {
                    config: { timeout_ms: 5_000 },
                    projectPath: "/repo/project",
                    resolveSessionDirectory: () => "/repo/project",
                    client: sidekickClient as never,
                },
            });

            await expectSentinel(run("ctx-aug", "ses-aug-lost", "Ship the migration"), "ctx-aug");

            expect(sidekickClient.session.promptAsync).toHaveBeenCalledTimes(1);
            const failure = texts().find((text) => text.startsWith("## /ctx-aug — Failed"));
            expect(failure).toBeDefined();
            expect(failure).toContain("session is busy");
            expect(failure).toContain("Ship the migration");
        });

        it("reports an unconfirmed delivery instead of a lost prompt when the send times out", async () => {
            const sidekickClient = {
                session: {
                    create: mock(async () => ({ data: { id: "sidekick-child" } })),
                    prompt: mock(async () => undefined),
                    promptAsync: mock(() => new Promise<never>(() => {})),
                    messages: mock(async () => ({
                        data: [
                            {
                                info: { role: "assistant", time: { created: Date.now() } },
                                parts: [{ type: "text", text: "Use Bun for commands" }],
                            },
                        ],
                    })),
                    delete: mock(async () => ({ data: undefined })),
                },
            };
            const { run, texts } = setup(undefined, {
                sidekick: {
                    config: { timeout_ms: 5_000 },
                    projectPath: "/repo/project",
                    resolveSessionDirectory: () => "/repo/project",
                    client: sidekickClient as never,
                },
            });
            __ignoredNotificationTest.setSendTimeoutMs(20);
            try {
                await expectSentinel(
                    run("ctx-aug", "ses-aug-slow", "Ship the migration"),
                    "ctx-aug",
                );
            } finally {
                __ignoredNotificationTest.reset();
            }

            const notice = texts().find((text) =>
                text.startsWith("## /ctx-aug — Delivery unconfirmed"),
            );
            expect(notice).toBeDefined();
            expect(notice).toContain("may still arrive");
            expect(notice).toContain("Ship the migration");
            expect(texts().some((text) => text.startsWith("## /ctx-aug — Failed"))).toBe(false);
        });
    });

    it("delivers notification text before throwing the sentinel", async () => {
        const order: string[] = [];
        const { run, sendNotification } = setup(() => ({ armed: false }));
        sendNotification.mockImplementation(async () => {
            order.push("notify");
        });

        await expectSentinel(
            run("ctx-flush", "ses-notify").catch((error) => {
                order.push("sentinel");
                throw error;
            }),
            "ctx-flush",
        );

        expect(order).toEqual(["notify", "sentinel"]);
        expect(sendNotification).toHaveBeenCalledWith(
            "ses-notify",
            "No pending operations to flush.",
            {},
        );
    });

    it("still throws the sentinel when notification delivery fails", async () => {
        const { run, sendNotification } = setup(() => ({ armed: false }));
        sendNotification.mockImplementation(async () => {
            throw new Error("TUI socket gone");
        });

        await expectSentinel(run("ctx-flush", "ses-notify-fail"), "ctx-flush");

        expect(sendNotification).toHaveBeenCalledTimes(1);
    });
});

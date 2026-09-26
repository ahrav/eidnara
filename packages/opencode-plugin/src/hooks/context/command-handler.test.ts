/// <reference types="bun-types" />

import { afterEach, describe, expect, it, mock } from "bun:test";
import { KernelClient } from "../../shared/kernel-client";
import { FakeKernel, FakeKernelTransport } from "../../shared/kernel-client-testing/fake-kernel";
import {
    __resetNotificationStateForTests,
    type RpcNotification,
    registerNotificationSink,
} from "../../shared/rpc-notifications";
import { createEidnaraCommandHandler } from "./command-handler";
import { MAX_WRAPUP_REQUEST_BUDGET_MS } from "./module-transport";
import type { RustModeModuleClient } from "./rust-mode-transform";
import { __ignoredNotificationTest, TUI_TOAST_MAX_CHARS } from "./send-session-notification";

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
        "session ses-status (last active 3m ago): 4 history_segments, coverage ordinal 17, boundary present, 2 pending drops, 1 tag, pending m1 delta false, last history_summarizer: published, publish failures: 0, surface active",
    usage: { current_total_input_tokens: 42_000, context_limit_tokens: 100_000 },
    boundary_present: true,
    coverage_ordinal: 17,
    history_segment_count: 4,
    pending_drop_count: 2,
    tag_count: 1,
    pending_m1_delta: false,
    pending_m1_age_ms: null,
    wrapup_active: false,
    wrapup_rounds: null,
    history_summarizer: { consecutive_publish_failures: 0, publish_health_degraded: false },
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

    for (const command of ["eidnara-wrapup", "eidnara-recomp", "eidnara-flush"] as const) {
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
                { forcePersist: true },
            );
            expect(calls).toHaveLength(0);
            expect(onFlush).not.toHaveBeenCalled();
        });
    }

    for (const command of [
        "eidnara-status",
        "eidnara-flush",
        "eidnara-recomp",
        "eidnara-wrapup",
    ] as const) {
        it(`does not send /${command} to the daemon when route resolution loses to deletion`, async () => {
            let deleted = false;
            const resolveProjectRoot = mock(async () => {
                deleted = true;
                return "/deleted/session";
            });
            const onFlush = mock(() => {});
            const { run, calls, sendNotification } = setup(undefined, {
                resolveProjectRoot,
                isSessionDeleted: () => deleted,
                onFlush,
            });

            await expectSentinel(run(command, "ses-deleted-during-command"), command);

            expect(calls).toHaveLength(0);
            expect(onFlush).not.toHaveBeenCalled();
            expect(sendNotification).not.toHaveBeenCalled();
        });
    }

    describe("eidnara-flush", () => {
        it("sends session.flush, reports the armed wording, and calls onFlush", async () => {
            const onFlush = mock(() => {});
            const { run, calls, sendNotification } = setup(() => ({ ok: true, armed: true }), {
                onFlush,
            });

            await expectSentinel(run("eidnara-flush", "ses-flush"), "eidnara-flush");

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
                { forcePersist: true },
            );
        });

        it("reports the not-armed wording and unwraps a `result` envelope", async () => {
            const { run, texts } = setup(() => ({ result: { armed: false } }));

            await expectSentinel(run("eidnara-flush", "ses-flush-empty"), "eidnara-flush");

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

            await expectSentinel(run("eidnara-flush", "ses-flush-fail"), "eidnara-flush");

            expect(onFlush).toHaveBeenCalledWith("ses-flush-fail");
            expect(texts()).toEqual(["Error: Failed to flush context operations. socket closed"]);
        });

        it("pushes the flush dialog to a connected TUI instead of notifying", async () => {
            const received = connectTui("ses-flush-tui");
            const { run, sendNotification } = setup(() => ({ armed: true }));

            await expectSentinel(run("eidnara-flush", "ses-flush-tui"), "eidnara-flush");

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

    describe("eidnara-status", () => {
        it("routes the daemon call by the session's resolved directory", async () => {
            const resolveProjectRoot = mock(async (sessionId: string) => `/repos/${sessionId}`);
            const { run, calls } = setup(() => STATUS_RESPONSE, { resolveProjectRoot });

            await expectSentinel(run("eidnara-status", "ses-routed"), "eidnara-status");

            expect(resolveProjectRoot).toHaveBeenCalledWith("ses-routed");
            expect(calls.map((call) => [call.method, call.projectRoot])).toEqual([
                ["session.status", "/repos/ses-routed"],
            ]);
        });

        it("sends session.status and renders the daemon text", async () => {
            const { run, calls, texts } = setup(() => STATUS_RESPONSE);

            await expectSentinel(run("eidnara-status", "ses-status"), "eidnara-status");

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
            expect(text).toContain("- HistorySegments: 4");
            expect(text).toContain("- Pending: 2 drops, 1 tag, m1 delta none");
            expect(text).toContain("- Wrapup: idle");
            expect(text).toContain(
                "- HistorySummarizer publish health: ok (0 consecutive publish failures)",
            );
            expect(text).toContain("- Passes: 12 received, 0 rejected");
            expect(text).toContain(`- Daemon: ${STATUS_RESPONSE.summary}`);
            expect(text).not.toContain("### Tail Hygiene");
            expect(text).not.toContain("**Compaction:** disabled");
            expect(text).not.toContain("### Compaction Timing");
        });

        it("renders pending work, a running wrapup, degraded publish health, and the last reject", async () => {
            const { run, texts } = setup(() => ({
                ...STATUS_RESPONSE,
                pending_m1_delta: true,
                pending_m1_age_ms: 42_500,
                wrapup_active: true,
                wrapup_rounds: 3,
                history_summarizer: {
                    consecutive_publish_failures: 4,
                    publish_health_degraded: true,
                },
                pass_trace: {
                    receive_count: 20,
                    reject_count: 2,
                    last_reject_error: `snapshot stale ${"x".repeat(200)}`,
                },
            }));

            await expectSentinel(run("eidnara-status", "ses-status-degraded"), "eidnara-status");

            const [text] = texts();
            expect(text).toContain("- Pending: 2 drops, 1 tag, m1 delta pending (43s)");
            expect(text).toContain("- Wrapup: running (3 rounds complete)");
            expect(text).toContain(
                "- HistorySummarizer publish health: degraded (4 consecutive publish failures)",
            );
            expect(text).toContain(
                "- Passes: 20 received, 2 rejected; last reject: snapshot stale ",
            );
            const passesLine = text.split("\n").find((line) => line.startsWith("- Passes:"));
            expect(passesLine?.length).toBeLessThan(220);
        });

        it("renders the compaction timing block from the summarizer timeline", async () => {
            const { run, texts } = setup(() => ({
                ...STATUS_RESPONSE,
                history_summarizer: {
                    ...STATUS_RESPONSE.history_summarizer,
                    recent_firings: [
                        {
                            firing_seq: 2,
                            source: "pressure_path",
                            clock: "daemon_wall_ms",
                            eligible_at_ms: 0,
                            fired_at_ms: 2_000,
                            published_at_ms: 5_000,
                            activated_at_ms: 9_000,
                            outcome: { kind: "published", sequence: 2 },
                        },
                    ],
                    counters: {
                        firings: 2,
                        published: 2,
                        superseded_before_activation: 0,
                        validation_rejected: 1,
                        invalidated: 0,
                        connect_failed: 0,
                    },
                },
            }));

            await expectSentinel(run("eidnara-status", "ses-status-timing"), "eidnara-status");

            const [text] = texts();
            expect(text).toContain("### Compaction Timing");
            expect(text).toContain("- Last summary #2 (pressure path): activated");
            expect(text).toContain("- Waited to start 2.0s, ran 3.0s, sat unactivated 4.0s");
            expect(text).toContain("- Firings 2, published 2, superseded before activation 0");
            expect(text).toContain(
                "- Best effort: validation rejected 1, invalidated 0, connect failed 0",
            );
            // The timing block follows every daemon status line.
            expect(text.indexOf("- Daemon:")).toBeLessThan(text.indexOf("### Compaction Timing"));
        });

        it("omits the pass and summary lines when the daemon does not supply them", async () => {
            const { run, texts } = setup(() => ({
                ok: true,
                usage: { current_total_input_tokens: 10, context_limit_tokens: 100 },
                history_segment_count: 0,
            }));

            await expectSentinel(run("eidnara-status", "ses-status-minimal"), "eidnara-status");

            const [text] = texts();
            expect(text).toContain("- Pending: 0 drops, 0 tags, m1 delta none");
            expect(text).toContain("- Wrapup: idle");
            expect(text).toContain(
                "- HistorySummarizer publish health: ok (0 consecutive publish failures)",
            );
            expect(text).not.toContain("- Passes:");
            expect(text).not.toContain("- Daemon:");
        });

        it("renders tail hygiene when the daemon reports a baseline", async () => {
            const { run, texts } = setup(() => ({
                ...STATUS_RESPONSE,
                tail_hygiene: { u: 250, t: 1_000, evaluable: true },
            }));

            await expectSentinel(run("eidnara-status", "ses-status-hygiene"), "eidnara-status");

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

            await expectSentinel(run("eidnara-status", "ses-status-off"), "eidnara-status");

            const [text] = texts();
            expect(text).toContain("**Compaction:** disabled (compaction.enabled: false)");
            expect(text).toContain("Session status is unavailable: daemon unavailable");
        });

        it("pushes the status dialog to a connected TUI instead of notifying", async () => {
            const received = connectTui("ses-status-tui");
            const { run, calls, sendNotification } = setup(() => STATUS_RESPONSE);

            await expectSentinel(run("eidnara-status", "ses-status-tui"), "eidnara-status");

            expect(calls.map((call) => call.method)).toEqual(["session.status"]);
            expect(sendNotification).not.toHaveBeenCalled();
            expect(received.map((n) => n.payload)).toEqual([{ action: "show-status-dialog" }]);
        });

        it("strips agent and model params while retaining the command result", async () => {
            const { run, sendNotification } = setup(() => STATUS_RESPONSE);

            await expectSentinel(
                run("eidnara-status", "ses-stable-model", "", {
                    agent: "oracle",
                    variant: "fast",
                    providerId: "anthropic",
                    modelId: "claude-sonnet-4-6",
                }),
                "eidnara-status",
            );

            expect(sendNotification).toHaveBeenCalledWith(
                "ses-stable-model",
                expect.stringContaining("## Eidnara Status"),
                { forcePersist: true },
            );
        });
    });

    describe("eidnara-recomp", () => {
        it("sends session.recomp with a minted command id and maps `started`", async () => {
            const { run, calls, texts } = setup(() => ({ ok: true, disposition: "started" }));

            await expectSentinel(run("eidnara-recomp", "ses-recomp"), "eidnara-recomp");

            expect(calls).toHaveLength(1);
            expect(calls[0]?.method).toBe("session.recomp");
            expect(calls[0]?.body).toMatchObject({
                method: "session.recomp",
                v: 1,
                session_id: "ses-recomp",
            });
            expect(calls[0]?.body.command_id).toMatch(/^opencode-recomp-/);
            expect(texts().join("\n")).toContain("HistorySummarizer recomp started");
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
                await expectSentinel(run("eidnara-recomp", "ses-recomp-map"), "eidnara-recomp");
                expect(texts().join("\n")).toContain(expected);
            }
        });

        it("refuses a message range without a daemon call", async () => {
            const { run, calls, texts } = setup();

            await expectSentinel(
                run("eidnara-recomp", "ses-recomp-range", "1-11322"),
                "eidnara-recomp",
            );

            expect(calls).toHaveLength(0);
            const [text] = texts();
            expect(text).toContain("## Eidnara Recomp — Unsupported");
            expect(text).toContain("Requested range: `1-11322`");
            expect(text).toContain("`session.recomp` accepts no message range");
            expect(text).toContain("Usage:");
        });

        it("rejects malformed arguments without a daemon call", async () => {
            const { run, calls, texts } = setup();

            await expectSentinel(
                run("eidnara-recomp", "ses-recomp-bad", "--upgrade"),
                "eidnara-recomp",
            );

            expect(calls).toHaveLength(0);
            const [text] = texts();
            expect(text).toContain("## Eidnara Recomp — Invalid Arguments");
            expect(text).toContain("Invalid /eidnara-recomp arguments: `--upgrade`");
        });
    });

    describe("eidnara-wrapup", () => {
        it("parses messagesToKeep and sends session.wrapup under the wrapup budget", async () => {
            const { run, calls, texts } = setup(() => ({
                disposition: "completed",
                rounds: 2,
                summary: "Wrapped up.",
            }));

            await expectSentinel(run("eidnara-wrapup", "ses-wrapup", "250"), "eidnara-wrapup");

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

        it("starts wrapup without waiting for the best-effort progress notification", async () => {
            const moduleCall = mock(async () => ({ disposition: "nothing_to_compact" }));
            const sendNotification = mock(async (_sessionId: string, text: string) => {
                if (text.includes("Starting wrapup")) await new Promise<void>(() => {});
            });
            const handler = createEidnaraCommandHandler({
                moduleClient: { call: moduleCall },
                sendNotification,
                isSubagentSession: () => false,
            });

            await expectSentinel(
                handler["command.execute.before"](
                    { command: "eidnara-wrapup", sessionID: "ses-wrapup-progress", arguments: "" },
                    { parts: [{ type: "text", text: "" }] },
                    {},
                ),
                "eidnara-wrapup",
            );

            expect(moduleCall).toHaveBeenCalledTimes(1);
        });

        it("defaults messagesToKeep to 20", async () => {
            const { run, calls, texts } = setup(() => ({ disposition: "nothing_to_compact" }));

            await expectSentinel(run("eidnara-wrapup", "ses-wrapup-default"), "eidnara-wrapup");

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
                        "budget expired Run /eidnara-wrapup again to continue.",
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
                await expectSentinel(run("eidnara-wrapup", "ses-wrapup-map"), "eidnara-wrapup");
                const text = texts().join("\n");
                for (const fragment of expected) expect(text).toContain(fragment);
                if (text.includes("— Partial")) expect(text).not.toContain("— Failed");
            }
        });

        it("rejects a non-positive keep count without a daemon call", async () => {
            const { run, calls, texts } = setup();

            await expectSentinel(run("eidnara-wrapup", "ses-wrapup-bad", "0"), "eidnara-wrapup");

            expect(calls).toHaveLength(0);
            expect(texts()).toEqual([
                "## Eidnara Wrapup — Invalid Arguments\n\nmessages_to_keep must be a positive integer.",
            ]);
        });

        it("refuses a subagent session without a daemon call", async () => {
            const isSubagentSession = mock((sessionId: string) => sessionId === "ses-child");
            const { run, calls, texts } = setup(undefined, { isSubagentSession });

            await expectSentinel(run("eidnara-wrapup", "ses-child", "50"), "eidnara-wrapup");

            expect(isSubagentSession).toHaveBeenCalledWith("ses-child");
            expect(calls).toHaveLength(0);
            expect(texts()).toEqual([
                "## Eidnara Wrapup — Skipped\n\n/eidnara-wrapup is only available in primary sessions.",
            ]);
        });

        it("awaits an asynchronous subagent classification before deciding", async () => {
            const isSubagentSession = mock(async (sessionId: string) => {
                await Bun.sleep(0);
                return sessionId === "ses-restored-child";
            });
            const { run, calls } = setup(undefined, { isSubagentSession });

            await expectSentinel(
                run("eidnara-wrapup", "ses-restored-child", "50"),
                "eidnara-wrapup",
            );

            expect(calls).toHaveLength(0);
        });

        it("keeps TUI delivery enabled for a connected session", async () => {
            connectTui("ses-wrapup-tui");
            const { run, sendNotification } = setup(() => ({
                disposition: "completed",
                rounds: 1,
                summary: "Wrapped up.",
            }));

            await expectSentinel(run("eidnara-wrapup", "ses-wrapup-tui"), "eidnara-wrapup");

            expect(sendNotification.mock.calls.at(-1)?.[2]).toEqual({ forcePersist: false });
        });
    });

    describe("eidnara-memory-mark", () => {
        function kernelResolver(kernel: FakeKernel) {
            const transport = new FakeKernelTransport(kernel);
            return ({ sessionId, projectRoot }: { sessionId: string; projectRoot: string }) =>
                new KernelClient({ transport, enabled: true, sessionId, projectRoot });
        }

        it("applies a tightening on a labeled memory without asking and reports the receipt", async () => {
            const kernel = new FakeKernel();
            kernel.seedDecision({
                object_id: "mem_rule",
                decision_kind: "PROJECT_RULES",
                summary: "rule",
            });
            const { run, texts } = setup(undefined, {
                kernelClient: kernelResolver(kernel),
                resolveProjectRoot: () => "/repo/project",
            });

            await expectSentinel(
                run("eidnara-memory-mark", "ses-mark", "mark_stale mem_rule"),
                "eidnara-memory-mark",
            );

            expect(texts().join("\n")).toContain("Applied");
            expect(texts().join("\n")).toContain("active -> stale");
            expect(kernel.objects.get("mem_rule")?.disposition).toBe("stale");
        });

        it("asks for the confirm flag when a served surface would change, and writes nothing", async () => {
            const kernel = new FakeKernel();
            kernel.seedDecision({
                object_id: "mem_verified",
                decision_kind: "PROJECT_RULES",
                summary: "verified",
                labeled: false,
            });
            const { run, texts } = setup(undefined, {
                kernelClient: kernelResolver(kernel),
                resolveProjectRoot: () => "/repo/project",
            });

            await expectSentinel(
                run("eidnara-memory-mark", "ses-mark", "quarantine mem_verified"),
                "eidnara-memory-mark",
            );
            expect(texts().join("\n")).toContain("Confirmation Needed");
            expect(texts().join("\n")).toContain(
                "/eidnara-memory-mark quarantine mem_verified --yes",
            );
            expect(kernel.receipts.size).toBe(0);

            await expectSentinel(
                run("eidnara-memory-mark", "ses-mark", "quarantine mem_verified --yes"),
                "eidnara-memory-mark",
            );
            expect(texts().at(-1)).toContain("Applied");
            expect(kernel.objects.get("mem_verified")?.disposition).toBe("quarantined");
        });

        it("keeps the re-run line inside the toast cut for the longest event and a derived object id", async () => {
            // With a TUI connected the reply is a toast that keeps only its opening characters, so the line the user must act on cannot follow the surface list.
            const objectId = `mem_${"f".repeat(32)}`;
            const kernel = new FakeKernel();
            kernel.seedDecision({
                object_id: objectId,
                decision_kind: "PROJECT_RULES",
                summary: "verified",
                labeled: false,
            });
            const { run, texts } = setup(undefined, {
                kernelClient: kernelResolver(kernel),
                resolveProjectRoot: () => "/repo/project",
            });

            await expectSentinel(
                run("eidnara-memory-mark", "ses-mark", `explicit_reject ${objectId}`),
                "eidnara-memory-mark",
            );
            const reply = texts().at(-1) ?? "";
            expect(reply).toContain("explicit_search: visible -> hidden");
            expect(reply.slice(0, TUI_TOAST_MAX_CHARS)).toContain(
                `/eidnara-memory-mark explicit_reject ${objectId} --yes`,
            );
            expect(kernel.receipts.size).toBe(0);
        });

        it("reports usage for malformed arguments and disabled when no kernel client is wired", async () => {
            const { run, texts } = setup();
            await expectSentinel(
                run("eidnara-memory-mark", "ses-mark", "stale"),
                "eidnara-memory-mark",
            );
            expect(texts().at(-1)).toContain("Invalid Arguments");

            await expectSentinel(
                run("eidnara-memory-mark", "ses-mark", "mark_stale mem_rule"),
                "eidnara-memory-mark",
            );
            expect(texts().at(-1)).toContain("disabled");
        });
    });

    describe("eidnara-aug", () => {
        it("runs context_researcher in a child session and sends the augmented prompt", async () => {
            const context_researcherClient = {
                session: {
                    create: mock(async () => ({ data: { id: "context_researcher-child" } })),
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
                context_researcher: {
                    config: { timeout_ms: 5_000 },
                    projectPath: "/repo/project",
                    resolveSessionDirectory: () => "/repo/project",
                    client: context_researcherClient as never,
                },
            });

            await expectSentinel(
                run("eidnara-aug", "ses-aug", "Implement context_researcher migration", {
                    agent: "plan",
                    variant: "thinking",
                    providerId: "anthropic",
                    modelId: "claude-opus-4-8",
                }),
                "eidnara-aug",
            );

            expect(calls).toHaveLength(0);
            expect(sendNotification).toHaveBeenCalledWith(
                "ses-aug",
                "🔍 Preparing augmentation… this may take 2-10s depending on your context_researcher provider.",
                {},
            );
            expect(context_researcherClient.session.create).toHaveBeenCalledTimes(1);
            expect(context_researcherClient.session.promptAsync).toHaveBeenCalledWith({
                path: { id: "ses-aug" },
                signal: expect.any(AbortSignal),
                body: {
                    agent: "plan",
                    model: { providerID: "anthropic", modelID: "claude-opus-4-8" },
                    variant: "thinking",
                    parts: [
                        {
                            type: "text",
                            text: "Implement context_researcher migration\n\n<context_researcher-augmentation>\nUse Bun for commands\n</context_researcher-augmentation>",
                        },
                    ],
                },
            });
        });

        it("reports when context_researcher is not configured", async () => {
            const { run, texts } = setup();

            await expectSentinel(run("eidnara-aug", "ses-aug-missing", "Help"), "eidnara-aug");

            expect(texts().join("\n")).toContain("ContextResearcher is not configured");
        });

        it("tells the user when the augmented prompt cannot be sent", async () => {
            const context_researcherClient = {
                session: {
                    create: mock(async () => ({ data: { id: "context_researcher-child" } })),
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
            const { run, texts, sendNotification } = setup(undefined, {
                context_researcher: {
                    config: { timeout_ms: 5_000 },
                    projectPath: "/repo/project",
                    resolveSessionDirectory: () => "/repo/project",
                    client: context_researcherClient as never,
                },
            });

            await expectSentinel(
                run("eidnara-aug", "ses-aug-lost", "Ship the migration"),
                "eidnara-aug",
            );

            expect(context_researcherClient.session.promptAsync).toHaveBeenCalledTimes(1);
            const failure = texts().find((text) => text.startsWith("## /eidnara-aug — Failed"));
            expect(failure).toBeDefined();
            expect(failure).toContain("session is busy");
            expect(failure).toContain("Ship the migration");
            expect(sendNotification.mock.calls.at(-1)?.[2]).toEqual({ forcePersist: true });
        });

        it("reports an unconfirmed delivery instead of a lost prompt when the send times out", async () => {
            const context_researcherClient = {
                session: {
                    create: mock(async () => ({ data: { id: "context_researcher-child" } })),
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
            const { run, texts, sendNotification } = setup(undefined, {
                context_researcher: {
                    config: { timeout_ms: 5_000 },
                    projectPath: "/repo/project",
                    resolveSessionDirectory: () => "/repo/project",
                    client: context_researcherClient as never,
                },
            });
            __ignoredNotificationTest.setSendTimeoutMs(20);
            try {
                await expectSentinel(
                    run("eidnara-aug", "ses-aug-slow", "Ship the migration"),
                    "eidnara-aug",
                );
            } finally {
                __ignoredNotificationTest.reset();
            }

            const notice = texts().find((text) =>
                text.startsWith("## /eidnara-aug — Delivery unconfirmed"),
            );
            expect(notice).toBeDefined();
            expect(notice).toContain("may still arrive");
            expect(notice).toContain("Ship the migration");
            expect(texts().some((text) => text.startsWith("## /eidnara-aug — Failed"))).toBe(false);
            expect(sendNotification.mock.calls.at(-1)?.[2]).toEqual({ forcePersist: true });
        });
    });

    it("delivers notification text before throwing the sentinel", async () => {
        const order: string[] = [];
        const { run, sendNotification } = setup(() => ({ armed: false }));
        sendNotification.mockImplementation(async () => {
            order.push("notify");
        });

        await expectSentinel(
            run("eidnara-flush", "ses-notify").catch((error) => {
                order.push("sentinel");
                throw error;
            }),
            "eidnara-flush",
        );

        expect(order).toEqual(["notify", "sentinel"]);
        expect(sendNotification).toHaveBeenCalledWith(
            "ses-notify",
            "No pending operations to flush.",
            { forcePersist: true },
        );
    });

    it("still throws the sentinel when notification delivery fails", async () => {
        const { run, sendNotification } = setup(() => ({ armed: false }));
        sendNotification.mockImplementation(async () => {
            throw new Error("TUI socket gone");
        });

        await expectSentinel(run("eidnara-flush", "ses-notify-fail"), "eidnara-flush");

        expect(sendNotification).toHaveBeenCalledTimes(1);
    });
});

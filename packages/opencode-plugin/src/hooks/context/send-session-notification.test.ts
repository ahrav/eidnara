import { afterEach, describe, expect, it, mock } from "bun:test";
import {
    __resetNotificationStateForTests,
    type RpcNotification,
    registerNotificationSink,
} from "../../shared/rpc-notifications";
import {
    __ignoredNotificationTest,
    flushIgnoredMessages,
    MAX_QUEUED_IGNORED_NOTIFICATIONS,
    sendIgnoredMessage,
    sendUserPrompt,
} from "./send-session-notification";

const DEFAULT_TITLE = "New session - 2026-06-11T12:00:00.000Z";

describe("sendIgnoredMessage", () => {
    afterEach(() => {
        __ignoredNotificationTest.reset();
    });

    it("returns skipped and does not post when the session never gets a real title", async () => {
        const originalSetTimeout = globalThis.setTimeout;
        globalThis.setTimeout = ((
            handler: Parameters<typeof setTimeout>[0],
            _timeout?: number,
            ...args: unknown[]
        ) => {
            if (typeof handler === "function") handler(...args);
            return 0 as never;
        }) as typeof setTimeout;

        try {
            const prompt = mock(async () => ({}));
            const get = mock(async () => ({ title: DEFAULT_TITLE }));
            const result = await sendIgnoredMessage(
                { session: { get, prompt } },
                "ses-never-titled",
                "persistent notification",
                {},
            );

            expect(result).toBe("skipped");
            expect(get).toHaveBeenCalledTimes(4);
            expect(prompt).not.toHaveBeenCalled();
        } finally {
            globalThis.setTimeout = originalSetTimeout;
        }
    });

    // `messages` supplies the last assistant turn to `resolvePromptContext`.
    // `get` supplies a title so `sendIgnoredMessage` does not skip the session.
    function titledClientWithLastTurn() {
        const prompt = mock(async () => ({}));
        const get = mock(async () => ({ title: "Real title" }));
        const messages = mock(async () => ({
            data: [
                {
                    info: {
                        role: "assistant",
                        agent: "build",
                        providerID: "anthropic",
                        modelID: "claude-opus-4-8",
                        variant: "thinking",
                    },
                },
            ],
        }));
        return { prompt, get, messages };
    }

    function lastPromptBody(prompt: ReturnType<typeof mock>): Record<string, unknown> {
        const call = prompt.mock.calls.at(-1)?.[0] as { body?: Record<string, unknown> };
        return call?.body ?? {};
    }

    it("queues without creating a user row while the session is active", async () => {
        const session = titledClientWithLastTurn();
        __ignoredNotificationTest.setMidTurnDetector(() => true);

        const result = await sendIgnoredMessage({ session }, "ses-active", "background status", {});

        expect(result).toBe("queued");
        expect(session.prompt).not.toHaveBeenCalled();
        expect(__ignoredNotificationTest.pendingTexts("ses-active")).toEqual(["background status"]);
    });

    it("reports failed and releases the flush when the prompt endpoint never settles", async () => {
        const session = titledClientWithLastTurn();
        session.prompt = mock(() => new Promise<never>(() => {}));
        __ignoredNotificationTest.setMidTurnDetector(() => false);
        __ignoredNotificationTest.setSendTimeoutMs(20);

        const result = await sendIgnoredMessage({ session }, "ses-hung-prompt", "status", {});

        expect(result).toBe("failed");
        expect(session.prompt).toHaveBeenCalledTimes(1);
        expect(__ignoredNotificationTest.pendingTexts("ses-hung-prompt")).toEqual([]);
    });

    it("flushes queued notices in order after the session becomes idle", async () => {
        const session = titledClientWithLastTurn();
        let active = true;
        __ignoredNotificationTest.setMidTurnDetector(() => active);

        await sendIgnoredMessage({ session }, "ses-idle-flush", "first status", {});
        await sendIgnoredMessage({ session }, "ses-idle-flush", "second status", {});
        expect(session.prompt).not.toHaveBeenCalled();

        active = false;
        await flushIgnoredMessages("ses-idle-flush");

        expect(
            session.prompt.mock.calls.map((call) => {
                const input = call[0] as { body?: { parts?: Array<{ text?: string }> } };
                return input.body?.parts?.[0]?.text;
            }),
        ).toEqual(["first status", "second status"]);
        expect(__ignoredNotificationTest.pendingTexts("ses-idle-flush")).toEqual([]);
    });

    it("keeps only the newest notices when the active queue is full", async () => {
        const session = titledClientWithLastTurn();
        __ignoredNotificationTest.setMidTurnDetector(() => true);

        for (let index = 0; index < MAX_QUEUED_IGNORED_NOTIFICATIONS + 3; index += 1) {
            await sendIgnoredMessage({ session }, "ses-bounded", `status ${index}`, {});
        }

        expect(__ignoredNotificationTest.pendingTexts("ses-bounded")).toEqual(
            Array.from(
                { length: MAX_QUEUED_IGNORED_NOTIFICATIONS },
                (_, index) => `status ${index + 3}`,
            ),
        );
        expect(session.prompt).not.toHaveBeenCalled();
    });

    it("pins the last assistant turn's agent+model+variant by default (mid-session)", async () => {
        const session = titledClientWithLastTurn();
        const result = await sendIgnoredMessage({ session }, "ses-titled", "historian failed", {});
        expect(result).toBe("sent");
        const body = lastPromptBody(session.prompt);
        expect(body.agent).toBe("build");
        expect(body.model).toEqual({ providerID: "anthropic", modelID: "claude-opus-4-8" });
        expect(body.variant).toBe("thinking");
        expect(body.noReply).toBe(true);
    });

    it("passes noReply to promptAsync as well as prompt", async () => {
        const promptAsync = mock(async () => ({}));
        const get = mock(async () => ({ title: "Real title" }));
        const messages = mock(async () => ({
            data: [
                {
                    info: {
                        role: "assistant",
                        agent: "build",
                        providerID: "anthropic",
                        modelID: "claude-opus-4-8",
                    },
                },
            ],
        }));
        await sendIgnoredMessage(
            { session: { get, messages, promptAsync } },
            "ses-prompt-async",
            "async notification",
            {},
        );

        const input = promptAsync.mock.calls[0]?.[0] as { body?: Record<string, unknown> };
        expect(input.body?.noReply).toBe(true);
    });

    it("pins the session's last turn for a startup config warning too (no pinContext opt-out)", async () => {
        // notification.
        const session = titledClientWithLastTurn();
        const result = await sendIgnoredMessage({ session }, "ses-titled", "config warning", {});
        expect(result).toBe("sent");
        const body = lastPromptBody(session.prompt);
        expect(body.agent).toBe("build");
        expect(body.model).toEqual({ providerID: "anthropic", modelID: "claude-opus-4-8" });
        expect(body.variant).toBe("thinking");
    });

    it("caller-supplied model/agent win over resolution", async () => {
        const session = titledClientWithLastTurn();
        await sendIgnoredMessage({ session }, "ses-titled", "explicit", {
            agent: "plan",
            providerId: "openai",
            modelId: "gpt-5.5",
            variant: "high",
        });
        const body = lastPromptBody(session.prompt);
        expect(body.agent).toBe("plan");
        expect(body.model).toEqual({ providerID: "openai", modelID: "gpt-5.5" });
        expect(body.variant).toBe("high");
        expect(session.messages).not.toHaveBeenCalled();
    });
});

describe("TUI toast delivery", () => {
    afterEach(() => {
        __ignoredNotificationTest.reset();
        __resetNotificationStateForTests();
    });

    async function toastPayloadFor(params: Parameters<typeof sendIgnoredMessage>[3]) {
        const sent: RpcNotification[] = [];
        const unregister = registerNotificationSink({
            sessionId: "ses-tui",
            protocol: 2,
            send: (notification) => sent.push(notification),
        });
        try {
            const prompt = mock(async () => ({}));
            const result = await sendIgnoredMessage(
                { session: { prompt } },
                "ses-tui",
                "hello",
                params,
            );
            expect(result).toBe("sent");
            expect(prompt).not.toHaveBeenCalled();
            expect(sent).toHaveLength(1);
            expect(sent[0]?.type).toBe("toast");
            return sent[0]?.payload ?? {};
        } finally {
            unregister();
        }
    }

    it("omits duration when the caller supplies none so the TUI applies toast_duration_ms", async () => {
        const payload = await toastPayloadFor({});
        expect(payload).not.toHaveProperty("duration");
        expect(payload.message).toBe("hello");
    });

    it("carries an explicit toastDurationMs as the per-call override", async () => {
        const payload = await toastPayloadFor({ toastDurationMs: 10_000 });
        expect(payload.duration).toBe(10_000);
    });
});

describe("sendUserPrompt", () => {
    it("prefers promptAsync and sends the text as a single user part", async () => {
        const prompt = mock(async () => ({}));
        const promptAsync = mock(async () => ({}));

        await sendUserPrompt({ session: { prompt, promptAsync } }, "ses-user", "hello");

        expect(prompt).not.toHaveBeenCalled();
        expect(promptAsync).toHaveBeenCalledWith({
            path: { id: "ses-user" },
            body: { parts: [{ type: "text", text: "hello" }] },
        });
    });

    it("falls back to prompt when promptAsync is absent", async () => {
        const prompt = mock(() => ({}));

        await sendUserPrompt({ session: { prompt } }, "ses-user-sync", "hello");

        expect(prompt).toHaveBeenCalledTimes(1);
    });

    it("rejects when the session prompt API is unavailable", async () => {
        await expect(sendUserPrompt(undefined, "ses-no-client", "hello")).rejects.toThrow(
            "session prompt API unavailable",
        );
        await expect(sendUserPrompt({ session: {} }, "ses-no-prompt", "hello")).rejects.toThrow(
            "session prompt API unavailable",
        );
    });

    it("propagates a rejected prompt call", async () => {
        const promptAsync = mock(async () => {
            throw new Error("session is busy");
        });

        await expect(
            sendUserPrompt({ session: { promptAsync } }, "ses-busy", "hello"),
        ).rejects.toThrow("session is busy");
    });
});

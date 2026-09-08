import { afterEach, describe, expect, it, mock } from "bun:test";
import {
    __ignoredNotificationTest,
    flushIgnoredMessages,
    MAX_QUEUED_IGNORED_NOTIFICATIONS,
    MAX_QUEUED_NOTIFICATION_DELIVERY_ATTEMPTS,
    sendIgnoredMessage,
    sendUserPrompt,
} from "./send-session-notification";

const DEFAULT_TITLE = "New session - 2026-06-11T12:00:00.000Z";

describe("sendIgnoredMessage", () => {
    afterEach(() => {
        __ignoredNotificationTest.reset();
    });

    it("returns skipped and does not post when the session never gets a real title", async () => {
        __ignoredNotificationTest.setSafeTargetOptions({ attempts: 4, delayMs: 1 });
        const prompt = mock(async () => ({}));
        const get = mock(async () => {
            await null;
            return { title: DEFAULT_TITLE };
        });
        const result = await sendIgnoredMessage(
            { session: { get, prompt } },
            "ses-never-titled",
            "persistent notification",
            {},
        );

        expect(result).toBe("skipped");
        expect(get).toHaveBeenCalledTimes(4);
        expect(prompt).not.toHaveBeenCalled();
    });

    it("retains a forced command result until the session gets a real title", async () => {
        __ignoredNotificationTest.setSafeTargetOptions({ attempts: 4, delayMs: 1 });
        let title = DEFAULT_TITLE;
        const prompt = mock(async () => ({}));
        const get = mock(async () => {
            await null;
            return { title };
        });
        const client = { session: { get, prompt } };

        const result = await sendIgnoredMessage(
            client,
            "ses-command-result",
            "full command result",
            {},
            true,
        );
        expect(result).toBe("queued");
        expect(get).toHaveBeenCalledTimes(1);
        expect(prompt).not.toHaveBeenCalled();
        expect(__ignoredNotificationTest.pendingTexts("ses-command-result")).toEqual([
            "full command result",
        ]);

        title = "Real title";
        await flushIgnoredMessages("ses-command-result");
        expect(prompt).toHaveBeenCalledTimes(1);
        expect(__ignoredNotificationTest.pendingTexts("ses-command-result")).toEqual([]);
    });

    it("uses one title read per retry for a forced result that remains untitled", async () => {
        const get = mock(async () => ({ title: DEFAULT_TITLE }));
        const prompt = mock(async () => ({}));
        const client = { session: { get, prompt } };

        expect(
            await sendIgnoredMessage(client, "ses-still-untitled", "command result", {}, true),
        ).toBe("queued");
        await flushIgnoredMessages("ses-still-untitled");
        await flushIgnoredMessages("ses-still-untitled");

        expect(get).toHaveBeenCalledTimes(3);
        expect(prompt).not.toHaveBeenCalled();
        expect(__ignoredNotificationTest.pendingTexts("ses-still-untitled")).toEqual([
            "command result",
        ]);
    });

    it("retains forced results and skips ordinary notices when the title read times out", async () => {
        __ignoredNotificationTest.setSafeTargetOptions({
            attempts: 4,
            delayMs: 1,
            readTimeoutMs: 10,
        });
        const client = {
            session: {
                get: () => new Promise<never>(() => {}),
                prompt: mock(async () => ({})),
            },
        };

        expect(
            await sendIgnoredMessage(client, "ses-timeout-forced", "command result", {}, true),
        ).toBe("queued");
        expect(__ignoredNotificationTest.pendingTexts("ses-timeout-forced")).toEqual([
            "command result",
        ]);
        expect(
            await sendIgnoredMessage(client, "ses-timeout-ordinary", "ordinary notice", {}),
        ).toBe("skipped");
        expect(client.session.prompt).not.toHaveBeenCalled();
    });

    // `messages` supplies the last assistant turn to `resolvePromptContext`.
    // `get` supplies a title so `sendIgnoredMessage` does not skip the session.
    function titledClientWithLastTurn() {
        const prompt = mock(async (_input: unknown) => ({}));
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

    it("reports an unknown outcome and aborts when the prompt endpoint never settles", async () => {
        const session = titledClientWithLastTurn();
        let signal: AbortSignal | undefined;
        session.prompt = mock((input: unknown) => {
            signal = (input as { signal?: AbortSignal }).signal;
            return new Promise<never>(() => {});
        });
        __ignoredNotificationTest.setMidTurnDetector(() => false);
        __ignoredNotificationTest.setSendTimeoutMs(20);

        const result = await sendIgnoredMessage({ session }, "ses-hung-prompt", "status", {});

        expect(result).toBe("unknown");
        expect(session.prompt).toHaveBeenCalledTimes(1);
        expect(signal).toBeInstanceOf(AbortSignal);
        expect(signal?.aborted).toBe(true);
        expect(__ignoredNotificationTest.pendingTexts("ses-hung-prompt")).toEqual([]);
    });

    it("does not retry or requeue a deferred notification whose send times out", async () => {
        const session = titledClientWithLastTurn();
        let active = true;
        let signal: AbortSignal | undefined;
        session.prompt = mock((input: unknown) => {
            signal = (input as { signal?: AbortSignal }).signal;
            return new Promise<never>(() => {});
        });
        __ignoredNotificationTest.setMidTurnDetector(() => active);
        __ignoredNotificationTest.setSendTimeoutMs(20);

        await sendIgnoredMessage({ session }, "ses-timeout-flush", "status", {});
        active = false;
        await flushIgnoredMessages("ses-timeout-flush");
        await flushIgnoredMessages("ses-timeout-flush");

        expect(session.prompt).toHaveBeenCalledTimes(1);
        expect(signal?.aborted).toBe(true);
        expect(__ignoredNotificationTest.pendingTexts("ses-timeout-flush")).toEqual([]);
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

    it("retains a queued notice whose deferred delivery fails and drops it after the attempt cap", async () => {
        const session = titledClientWithLastTurn();
        session.prompt.mockImplementation(async () => {
            throw new Error("transient prompt failure");
        });
        let active = true;
        __ignoredNotificationTest.setMidTurnDetector(() => active);

        await sendIgnoredMessage({ session }, "ses-retry", "flaky status", {});
        active = false;

        for (let attempt = 1; attempt < MAX_QUEUED_NOTIFICATION_DELIVERY_ATTEMPTS; attempt += 1) {
            await flushIgnoredMessages("ses-retry");
            expect(session.prompt).toHaveBeenCalledTimes(attempt);
            expect(__ignoredNotificationTest.pendingTexts("ses-retry")).toEqual(["flaky status"]);
        }

        await flushIgnoredMessages("ses-retry");
        expect(session.prompt).toHaveBeenCalledTimes(MAX_QUEUED_NOTIFICATION_DELIVERY_ATTEMPTS);
        expect(__ignoredNotificationTest.pendingTexts("ses-retry")).toEqual([]);
    });

    it("stops the flush at a retained failure so later notices are not delivered first", async () => {
        const session = titledClientWithLastTurn();
        let failNext = true;
        session.prompt.mockImplementation(async () => {
            if (failNext) {
                failNext = false;
                throw new Error("transient prompt failure");
            }
            return {};
        });
        let active = true;
        __ignoredNotificationTest.setMidTurnDetector(() => active);

        await sendIgnoredMessage({ session }, "ses-order", "older", {});
        await sendIgnoredMessage({ session }, "ses-order", "newer", {});
        active = false;

        await flushIgnoredMessages("ses-order");
        expect(session.prompt).toHaveBeenCalledTimes(1);
        expect(__ignoredNotificationTest.pendingTexts("ses-order")).toEqual(["older", "newer"]);

        await flushIgnoredMessages("ses-order");
        expect(
            session.prompt.mock.calls.map((call) => {
                const input = call[0] as { body?: { parts?: Array<{ text?: string }> } };
                return input.body?.parts?.[0]?.text;
            }),
        ).toEqual(["older", "older", "newer"]);
        expect(__ignoredNotificationTest.pendingTexts("ses-order")).toEqual([]);
    });

    it("retains a queued notice whose deferred flush is skipped for a default title", async () => {
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
            const session = titledClientWithLastTurn();
            let title = DEFAULT_TITLE;
            session.get.mockImplementation(async () => ({ title }));
            let active = true;
            __ignoredNotificationTest.setMidTurnDetector(() => active);

            await sendIgnoredMessage({ session }, "ses-untitled", "queued status", {});
            await sendIgnoredMessage({ session }, "ses-untitled", "later status", {});
            active = false;

            await flushIgnoredMessages("ses-untitled");
            expect(session.prompt).not.toHaveBeenCalled();
            expect(__ignoredNotificationTest.pendingTexts("ses-untitled")).toEqual([
                "queued status",
                "later status",
            ]);

            title = "Real title";
            await flushIgnoredMessages("ses-untitled");
            expect(
                session.prompt.mock.calls.map((call) => {
                    const input = call[0] as { body?: { parts?: Array<{ text?: string }> } };
                    return input.body?.parts?.[0]?.text;
                }),
            ).toEqual(["queued status", "later status"]);
            expect(__ignoredNotificationTest.pendingTexts("ses-untitled")).toEqual([]);
        } finally {
            globalThis.setTimeout = originalSetTimeout;
        }
    });

    it("drops a notice at the attempt cap and delivers the tail in the same flush", async () => {
        const session = titledClientWithLastTurn();
        session.prompt.mockImplementation(async (input: unknown) => {
            const text = (input as { body?: { parts?: Array<{ text?: string }> } }).body?.parts?.[0]
                ?.text;
            if (text === "poison") throw new Error("permanent prompt failure");
            return {};
        });
        let active = true;
        __ignoredNotificationTest.setMidTurnDetector(() => active);

        await sendIgnoredMessage({ session }, "ses-poison", "poison", {});
        await sendIgnoredMessage({ session }, "ses-poison", "healthy", {});
        active = false;

        for (let attempt = 1; attempt < MAX_QUEUED_NOTIFICATION_DELIVERY_ATTEMPTS; attempt += 1) {
            await flushIgnoredMessages("ses-poison");
            expect(__ignoredNotificationTest.pendingTexts("ses-poison")).toEqual([
                "poison",
                "healthy",
            ]);
        }

        await flushIgnoredMessages("ses-poison");
        expect(__ignoredNotificationTest.pendingTexts("ses-poison")).toEqual([]);
        expect(
            session.prompt.mock.calls.map((call) => {
                const input = call[0] as { body?: { parts?: Array<{ text?: string }> } };
                return input.body?.parts?.[0]?.text;
            }),
        ).toEqual(["poison", "poison", "poison", "healthy"]);
    });

    it("re-inserts an interrupted flush batch ahead of notices queued during the flush", async () => {
        const session = titledClientWithLastTurn();
        let active = true;
        __ignoredNotificationTest.setMidTurnDetector(() => active);

        await sendIgnoredMessage({ session }, "ses-reorder", "old-1", {});
        await sendIgnoredMessage({ session }, "ses-reorder", "old-2", {});
        active = false;

        // The title lookup is the flush's first await; use it to interleave a new notice and a
        // new active run before the first delivery calls `session.prompt`.
        session.get.mockImplementation(async () => {
            active = true;
            await sendIgnoredMessage({ session }, "ses-reorder", "new", {});
            return { title: "Real title" };
        });

        await flushIgnoredMessages("ses-reorder");

        expect(session.prompt).not.toHaveBeenCalled();
        expect(__ignoredNotificationTest.pendingTexts("ses-reorder")).toEqual([
            "old-1",
            "old-2",
            "new",
        ]);
    });

    it("evicts the oldest entries when an interrupted flush batch overflows the cap", async () => {
        const session = titledClientWithLastTurn();
        let active = true;
        __ignoredNotificationTest.setMidTurnDetector(() => active);

        for (let index = 0; index < MAX_QUEUED_IGNORED_NOTIFICATIONS; index += 1) {
            await sendIgnoredMessage({ session }, "ses-cap", `old-${index}`, {});
        }
        active = false;

        session.get.mockImplementation(async () => {
            active = true;
            await sendIgnoredMessage({ session }, "ses-cap", "new", {});
            return { title: "Real title" };
        });

        await flushIgnoredMessages("ses-cap");

        const pending = __ignoredNotificationTest.pendingTexts("ses-cap");
        expect(pending).toHaveLength(MAX_QUEUED_IGNORED_NOTIFICATIONS);
        expect(pending[0]).toBe("old-1");
        expect(pending.at(-1)).toBe("new");
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

    it("queues when prompt context cannot be read instead of sending unset context", async () => {
        const session = titledClientWithLastTurn();
        session.messages.mockImplementation(async () => {
            throw new Error("message read failed");
        });

        const result = await sendIgnoredMessage({ session }, "ses-context-error", "status", {});

        expect(result).toBe("queued");
        expect(session.prompt).not.toHaveBeenCalled();
        expect(__ignoredNotificationTest.pendingTexts("ses-context-error")).toEqual(["status"]);
    });

    it("drops a queued notification after repeated unavailable context reads", async () => {
        const session = titledClientWithLastTurn();
        session.messages.mockImplementation(async () => {
            throw new Error("message read failed");
        });

        await sendIgnoredMessage({ session }, "ses-context-unavailable", "status", {});
        for (let attempt = 0; attempt < MAX_QUEUED_NOTIFICATION_DELIVERY_ATTEMPTS; attempt++) {
            await flushIgnoredMessages("ses-context-unavailable");
        }

        expect(session.prompt).not.toHaveBeenCalled();
        expect(session.messages).toHaveBeenCalledTimes(
            MAX_QUEUED_NOTIFICATION_DELIVERY_ATTEMPTS + 1,
        );
        expect(__ignoredNotificationTest.pendingTexts("ses-context-unavailable")).toEqual([]);
    });

    it("sends unset context only after a successful empty context read", async () => {
        const session = titledClientWithLastTurn();
        session.messages.mockImplementation(async () => ({ data: [] }));

        const result = await sendIgnoredMessage({ session }, "ses-empty-context", "status", {});

        expect(result).toBe("sent");
        const body = lastPromptBody(session.prompt);
        expect(body.agent).toBeUndefined();
        expect(body.model).toBeUndefined();
        expect(body.variant).toBeUndefined();
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
        expect((input as { signal?: AbortSignal }).signal).toBeInstanceOf(AbortSignal);
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

    it("does not inherit the resolved variant when the caller overrides the model", async () => {
        const session = titledClientWithLastTurn();
        await sendIgnoredMessage({ session }, "ses-titled", "explicit model", {
            providerId: "openai",
            modelId: "gpt-5.5",
        });
        const body = lastPromptBody(session.prompt);
        expect(body.agent).toBe("build");
        expect(body.model).toEqual({ providerID: "openai", modelID: "gpt-5.5" });
        expect(body.variant).toBeUndefined();
    });

    it("inherits the resolved variant when the caller names the same model", async () => {
        const session = titledClientWithLastTurn();
        await sendIgnoredMessage({ session }, "ses-titled", "same model", {
            providerId: "anthropic",
            modelId: "claude-opus-4-8",
        });
        const body = lastPromptBody(session.prompt);
        expect(body.model).toEqual({ providerID: "anthropic", modelID: "claude-opus-4-8" });
        expect(body.variant).toBe("thinking");
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

    it("carries the caller's agent, model, and variant and omits absent ones", async () => {
        const promptAsync = mock(async () => ({}));
        await sendUserPrompt({ session: { promptAsync } }, "ses-user-ctx", "hello", {
            agent: "plan",
            providerId: "anthropic",
            modelId: "claude-opus-4-8",
        });
        expect(promptAsync).toHaveBeenCalledWith({
            path: { id: "ses-user-ctx" },
            body: {
                agent: "plan",
                model: { providerID: "anthropic", modelID: "claude-opus-4-8" },
                parts: [{ type: "text", text: "hello" }],
            },
        });
    });

    it("falls back to prompt when promptAsync is absent", async () => {
        const prompt = mock(() => ({}));

        await sendUserPrompt({ session: { prompt } }, "ses-user-sync", "hello");

        expect(prompt).toHaveBeenCalledTimes(1);
    });

    it("rejects when promptAsync never settles so the caller can report the lost prompt", async () => {
        const promptAsync = mock(() => new Promise<never>(() => {}));
        __ignoredNotificationTest.setSendTimeoutMs(20);
        try {
            await expect(
                sendUserPrompt({ session: { promptAsync } }, "ses-user-hung", "hello"),
            ).rejects.toThrow("user prompt delivery timed out");
        } finally {
            __ignoredNotificationTest.reset();
        }
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

import { describe, expect, test } from "bun:test";
import { BoundedSessionMap } from "../../shared/bounded-session-map";
import { invalidateToolPermissionDenied } from "./ctx-reduce-availability";
import type { ContextUsageEntry } from "./event-handler";
import {
    createChatMessageHook,
    createEventHook,
    createToolExecuteAfterHook,
} from "./hook-handlers";

type TodoStateCall = { sessionId: string; stateJson: string; ownerMessageId: string };

function createForwardingHook(options?: {
    subagentSessions?: ReadonlySet<string>;
    client?: Parameters<typeof createToolExecuteAfterHook>[0]["client"];
}): { hook: ReturnType<typeof createToolExecuteAfterHook>; calls: TodoStateCall[] } {
    const calls: TodoStateCall[] = [];
    const hook = createToolExecuteAfterHook({
        subagentSessions: options?.subagentSessions ?? new Set(),
        client:
            options?.client ??
            ({
                app: { agents: async () => ({ data: [] }) },
                session: { get: async () => ({ data: {} }) },
            } as never),
        transformMode: "rust",
        todoStateSet: async (input) => {
            calls.push(input);
        },
    });
    return { hook, calls };
}

describe("createToolExecuteAfterHook todo snapshots", () => {
    test("ts mode does not forward todo state", async () => {
        const calls: TodoStateCall[] = [];
        const hook = createToolExecuteAfterHook({
            subagentSessions: new Set(),
            transformMode: "ts",
            todoStateSet: async (input) => {
                calls.push(input);
            },
        });
        await hook({
            tool: "todowrite",
            sessionID: "ses-ts-todo",
            args: { todos: [{ status: "pending", priority: "high", content: "Stay local" }] },
        });
        expect(calls).toEqual([]);
    });

    test("permission-denied todowrite capture is refused, including lookalike calls", async () => {
        let denied = true;
        const client = {
            app: {
                agents: async () => ({
                    data: [
                        {
                            name: "build",
                            permission: { todowrite: denied ? "deny" : "allow" },
                        },
                    ],
                }),
            },
            session: {
                get: async () => ({ data: {} }),
            },
        } as never;
        const { hook, calls } = createForwardingHook({ client });

        // The hook forwards the input's agent; the SDK session payload carries none.
        await hook({
            tool: "todowrite",
            sessionID: "ses-denied-capture",
            agent: "build",
            args: {
                todos: [{ status: "pending", priority: "high", content: "Must not capture" }],
            },
        });
        expect(calls).toEqual([]);

        // A third-party lookalike is never accepted as a native todowrite capture.
        denied = false;
        invalidateToolPermissionDenied("ses-denied-capture");
        await hook({
            tool: "mcp_Todowrite",
            sessionID: "ses-denied-capture",
            agent: "build",
            args: {
                todos: [{ status: "pending", priority: "high", content: "Still refuse" }],
            },
        });
        expect(calls).toEqual([]);

        await hook({
            tool: "todowrite",
            sessionID: "ses-denied-capture",
            agent: "build",
            args: {
                todos: [{ status: "pending", priority: "high", content: "Capture now" }],
            },
        });
        expect(calls).toHaveLength(1);
        expect(calls[0]?.stateJson).toContain("Capture now");
    });

    test("a permission read that never settles suppresses capture within the deadline", async () => {
        const client = {
            app: { agents: () => new Promise<never>(() => {}) },
            session: { get: async () => ({ data: {} }) },
        } as never;
        const { hook, calls } = createForwardingHook({ client });

        const startedAt = performance.now();
        await hook({
            tool: "todowrite",
            sessionID: "ses-permission-hung",
            agent: "build",
            args: { todos: [{ status: "pending", priority: "high", content: "Must not capture" }] },
        });
        const elapsedMs = performance.now() - startedAt;

        expect(calls).toHaveLength(0);
        expect(elapsedMs).toBeGreaterThanOrEqual(1_500);
        expect(elapsedMs).toBeLessThan(10_000);
    });

    test("multiple todowrite calls forward each snapshot in order", async () => {
        const { hook, calls } = createForwardingHook();

        await hook({
            tool: "todowrite",
            sessionID: "ses-todo",
            args: { todos: [{ content: "First", status: "pending", priority: "low" }] },
        });
        await hook({
            tool: "todowrite",
            sessionID: "ses-todo",
            args: { todos: [{ content: "Second", status: "in_progress", priority: "high" }] },
        });

        expect(calls.map((call) => call.stateJson)).toEqual([
            '[{"content":"First","status":"pending","priority":"low"}]',
            '[{"content":"Second","status":"in_progress","priority":"high"}]',
        ]);
    });

    test("non-todowrite tools, subagent sessions, and foreign or malformed todo payloads are not forwarded", async () => {
        const refused: Array<{
            label: string;
            subagentSessions?: ReadonlySet<string>;
            input: Parameters<ReturnType<typeof createToolExecuteAfterHook>>[0];
        }> = [
            {
                label: "non-todowrite tool",
                input: {
                    tool: "read",
                    sessionID: "ses-other",
                    args: { todos: [{ content: "Nope", status: "pending", priority: "high" }] },
                },
            },
            {
                label: "subagent session",
                subagentSessions: new Set(["ses-sub"]),
                input: {
                    tool: "todowrite",
                    sessionID: "ses-sub",
                    args: { todos: [{ content: "Sub work", status: "pending", priority: "high" }] },
                },
            },
            {
                label: "foreign status",
                input: {
                    tool: "todowrite",
                    sessionID: "ses-foreign",
                    args: { todos: [{ content: "Third-party", status: "done" }] },
                },
            },
            {
                label: "missing todos",
                input: { tool: "todowrite", sessionID: "ses-malformed", args: {} },
            },
            {
                label: "non-array todos",
                input: {
                    tool: "todowrite",
                    sessionID: "ses-malformed",
                    args: { todos: { content: "Not an array", status: "pending" } },
                },
            },
            {
                label: "todo without status",
                input: {
                    tool: "todowrite",
                    sessionID: "ses-malformed",
                    args: { todos: [{ content: "Missing status" }] },
                },
            },
        ];

        for (const { label, subagentSessions, input } of refused) {
            const { hook, calls } = createForwardingHook({ subagentSessions });
            await hook(input);
            expect(calls, label).toEqual([]);
        }
    });
});

describe("createEventHook live model tracking", () => {
    function makeAssistantEvent(
        sessionID: string,
        providerID: string,
        modelID: string,
        id = `msg-${Math.random().toString(36).slice(2)}`,
    ) {
        return {
            event: {
                type: "message.updated",
                properties: {
                    info: {
                        role: "assistant",
                        sessionID,
                        id,
                        providerID,
                        modelID,
                        finish: "stop",
                        tokens: { input: 1000, cache: { read: 0, write: 0 } },
                    },
                },
            },
        };
    }

    function makeHook(
        liveModelBySession: Map<string, { providerID: string; modelID: string }>,
        contextUsageMap = new BoundedSessionMap<ContextUsageEntry>(8),
    ) {
        const sessionDirectoryBySession = new Map<string, string>();
        const hook = createEventHook({
            eventHandler: async () => {},
            contextUsageMap,
            liveModelBySession,
            variantBySession: new Map(),
            agentBySession: new Map(),
            sessionDirectoryBySession,
            historyRefreshSessions: new Set(),
            deferredHistoryRefreshSessions: new Set(),
            systemPromptRefreshSessions: new Set(),
            pendingMaterializationSessions: new Set(),
            deferredMaterializationSessions: new Set(),
            lastHeuristicsTurnId: new Map(),
            client: undefined as never,
            protectedTags: 5,
        });
        return { hook, sessionDirectoryBySession };
    }

    test("records the latest assistant model per session across a mid-session switch", async () => {
        const sessionId = "ses-model-switch";
        const liveModelBySession = new Map<string, { providerID: string; modelID: string }>();
        const { hook } = makeHook(liveModelBySession);

        await hook(makeAssistantEvent(sessionId, "anthropic", "claude-small"));
        expect(liveModelBySession.get(sessionId)).toEqual({
            providerID: "anthropic",
            modelID: "claude-small",
        });

        await hook(makeAssistantEvent(sessionId, "anthropic", "claude-large"));
        expect(liveModelBySession.get(sessionId)).toEqual({
            providerID: "anthropic",
            modelID: "claude-large",
        });
    });

    test("an update to an older response does not move the live model off the newest response", async () => {
        const sessionId = "ses-model-older-edit";
        const liveModelBySession = new Map<string, { providerID: string; modelID: string }>();
        const contextUsageMap = new BoundedSessionMap<ContextUsageEntry>(8);
        contextUsageMap.set(sessionId, {
            usage: { percentage: 10, inputTokens: 10_000 },
            updatedAt: Date.now(),
            hasUsageTokens: true,
            messageID: "msg-9",
            model: { providerID: "anthropic", modelID: "claude-large" },
        });
        const { hook } = makeHook(liveModelBySession, contextUsageMap);

        await hook(makeAssistantEvent(sessionId, "anthropic", "claude-large", "msg-9"));
        await hook(makeAssistantEvent(sessionId, "anthropic", "claude-small", "msg-3"));
        expect(liveModelBySession.get(sessionId)).toEqual({
            providerID: "anthropic",
            modelID: "claude-large",
        });

        await hook(makeAssistantEvent(sessionId, "openai", "gpt", "msg-9z"));
        expect(liveModelBySession.get(sessionId)).toEqual({ providerID: "openai", modelID: "gpt" });
    });

    test("session.deleted clears the session's live state", async () => {
        const sessionId = "ses-deleted";
        const liveModelBySession = new Map<string, { providerID: string; modelID: string }>();
        const { hook, sessionDirectoryBySession } = makeHook(liveModelBySession);
        sessionDirectoryBySession.set(sessionId, "/tmp/project");

        await hook(makeAssistantEvent(sessionId, "anthropic", "claude-small"));
        await hook({
            event: { type: "session.deleted", properties: { info: { id: sessionId } } },
        });

        expect(liveModelBySession.has(sessionId)).toBe(false);
        expect(sessionDirectoryBySession.has(sessionId)).toBe(false);
    });
});

describe("createChatMessageHook live session tracking", () => {
    test("tracks model, variant, and agent per session", async () => {
        const liveModelBySession = new Map<string, { providerID: string; modelID: string }>();
        const variantBySession = new Map<string, string | undefined>();
        const agentBySession = new Map<string, string>();
        const hook = createChatMessageHook({
            liveModelBySession,
            variantBySession,
            agentBySession,
        });

        await hook({
            sessionID: "ses",
            variant: "low",
            agent: "build",
            model: { providerID: "anthropic", modelID: "claude" },
        });
        await hook({ sessionID: "ses", variant: "high" });

        expect(liveModelBySession.get("ses")).toEqual({
            providerID: "anthropic",
            modelID: "claude",
        });
        expect(variantBySession.get("ses")).toBe("high");
        expect(agentBySession.get("ses")).toBe("build");
    });
});

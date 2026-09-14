/// <reference types="bun-types" />

import { afterAll, afterEach, describe, expect, it, mock } from "bun:test";
import type { ContextResearcherConfig } from "../../../config/schema/eidnara";
import type { PluginContext } from "../../../plugin/types";
import { runContextResearcher } from "./agent";

const baseConfig: ContextResearcherConfig = {
    enabled: true,
    timeout_ms: 5_000,
};

function createContextResearcherClient(
    args: { createSessionId?: string | null; messages?: unknown[] } = {},
): PluginContext["client"] {
    return {
        session: {
            create: mock(async () =>
                args.createSessionId === null
                    ? { data: {} }
                    : { data: { id: args.createSessionId ?? "context_researcher-child" } },
            ),
            prompt: mock(async () => undefined),
            messages: mock(async () => ({
                data: args.messages ?? [
                    {
                        info: { role: "assistant", time: { created: Date.now() } },
                        parts: [{ type: "text", text: "Relevant memory briefing" }],
                    },
                ],
            })),
            delete: mock(async () => ({ data: undefined })),
        },
    } as unknown as PluginContext["client"];
}

afterEach(() => {
    mock.restore();
});

afterAll(() => {
    mock.restore();
});

describe("runContextResearcher", () => {
    it("creates a child session, prompts the context_researcher agent, and deletes the child session", async () => {
        const client = createContextResearcherClient();

        const result = await runContextResearcher({
            client,
            sessionId: "ses-parent",
            projectPath: "/repo/project",
            sessionDirectory: "/repo/project",
            userMessage: "Implement context_researcher and keep Bun workflow rules.",
            config: baseConfig,
        });

        expect(result).toBe("Relevant memory briefing");
        expect(client.session.create).toHaveBeenCalledWith({
            body: { parentID: "ses-parent", title: "eidnara-context_researcher" },
            query: { directory: "/repo/project" },
        });
        expect(client.session.prompt).toHaveBeenCalledWith(
            expect.objectContaining({
                path: { id: "context_researcher-child" },
                query: { directory: "/repo/project" },
                body: {
                    agent: "context-researcher",
                    system: expect.stringContaining('eidnara_search(query="'),
                    parts: [
                        {
                            type: "text",
                            text: "Implement context_researcher and keep Bun workflow rules.",
                            synthetic: true,
                        },
                    ],
                },
            }),
        );
        expect(client.session.messages).toHaveBeenCalledWith({
            path: { id: "context_researcher-child" },
            query: { directory: "/repo/project", limit: 50 },
        });
        expect(client.session.delete).toHaveBeenCalledWith({
            path: { id: "context_researcher-child" },
        });
    });

    it("strips closed and trailing unterminated thinking blocks and rejects reasoning-only output", async () => {
        for (const [text, expected] of [
            ["<think>hidden</think>Focused result", "Focused result"],
            ["<think>hidden</think>Focused result<think>more reasoning", "Focused result"],
            ["<think>reasoning cut off mid-", null],
        ] as const) {
            const client = createContextResearcherClient({
                messages: [
                    {
                        info: { role: "assistant", time: { created: Date.now() } },
                        parts: [{ type: "text", text }],
                    },
                ],
            });

            const result = await runContextResearcher({
                client,
                projectPath: "/repo/project",
                userMessage: "Implement context_researcher.",
                config: baseConfig,
            });

            expect(result).toBe(expected);
        }
    });

    it("returns null for the no-result sentinel instead of an augmentation", async () => {
        const client = createContextResearcherClient({
            messages: [
                {
                    info: { role: "assistant", time: { created: Date.now() } },
                    parts: [
                        {
                            type: "text",
                            text: "<think>searched</think>No relevant memories found.",
                        },
                    ],
                },
            ],
        });

        const result = await runContextResearcher({
            client,
            projectPath: "/repo/project",
            userMessage: "Implement context_researcher.",
            config: baseConfig,
        });

        expect(result).toBeNull();
        // The sentinel is a completed run, so it must not trigger a fallback-model retry.
        expect(client.session.prompt).toHaveBeenCalledTimes(1);
    });

    it("returns null when the child session cannot be created", async () => {
        const client = createContextResearcherClient({ createSessionId: null });

        const result = await runContextResearcher({
            client,
            projectPath: "/repo/project",
            userMessage: "Implement context_researcher.",
            config: baseConfig,
        });

        expect(result).toBeNull();
        expect(client.session.delete).not.toHaveBeenCalled();
    });

    it("returns null when prompting fails and still deletes the child session", async () => {
        const client = createContextResearcherClient();
        (client.session.prompt as ReturnType<typeof mock>).mockRejectedValue(
            new Error("prompt timed out after 5000ms"),
        );

        const result = await runContextResearcher({
            client,
            projectPath: "/repo/project",
            userMessage: "Implement context_researcher.",
            config: baseConfig,
        });

        expect(result).toBeNull();
        expect(client.session.delete).toHaveBeenCalledTimes(1);
    });

    it("uses config system_prompt when provided", async () => {
        const client = createContextResearcherClient();

        await runContextResearcher({
            client,
            projectPath: "/repo/project",
            userMessage: "Implement context_researcher.",
            config: {
                ...baseConfig,
                system_prompt: "Custom context_researcher system prompt",
            },
        });

        expect((client.session.prompt as ReturnType<typeof mock>).mock.calls[0]?.[0]).toEqual(
            expect.objectContaining({
                body: expect.objectContaining({
                    system: "Custom context_researcher system prompt",
                }),
            }),
        );
    });
});

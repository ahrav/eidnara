import { describe, expect, it, mock } from "bun:test";
import { resolvePromptContext } from "./prompt-context";

function clientWithMessages(messages: Array<{ info: Record<string, unknown> }>) {
    const list = mock(async (_input: unknown) => ({ data: messages }));
    return { client: { session: { messages: list } }, list };
}

describe("resolvePromptContext", () => {
    it("returns the newest message's context when it is complete", async () => {
        const { client } = clientWithMessages([
            {
                info: {
                    role: "assistant",
                    agent: "plan",
                    providerID: "openai",
                    modelID: "gpt-5.5",
                    variant: "high",
                },
            },
            {
                info: {
                    role: "assistant",
                    agent: "build",
                    providerID: "anthropic",
                    modelID: "claude-opus-4-8",
                    variant: "thinking",
                },
            },
        ]);

        expect(await resolvePromptContext(client, "ses-1")).toEqual({
            agent: "build",
            model: { providerID: "anthropic", modelID: "claude-opus-4-8" },
            variant: "thinking",
        });
    });

    it("fills a missing variant from an older message with the same model", async () => {
        const { client } = clientWithMessages([
            {
                info: {
                    role: "assistant",
                    agent: "build",
                    providerID: "anthropic",
                    modelID: "claude-opus-4-8",
                    variant: "thinking",
                },
            },
            {
                info: {
                    role: "assistant",
                    agent: "build",
                    providerID: "anthropic",
                    modelID: "claude-opus-4-8",
                },
            },
        ]);

        expect(await resolvePromptContext(client, "ses-1")).toEqual({
            agent: "build",
            model: { providerID: "anthropic", modelID: "claude-opus-4-8" },
            variant: "thinking",
        });
    });

    it("leaves the variant unset when the only older variant belongs to a different model", async () => {
        const { client } = clientWithMessages([
            {
                info: {
                    role: "assistant",
                    agent: "build",
                    providerID: "anthropic",
                    modelID: "claude-opus-4-8",
                    variant: "thinking",
                },
            },
            {
                info: {
                    role: "assistant",
                    agent: "build",
                    providerID: "openai",
                    modelID: "gpt-5.5",
                },
            },
        ]);

        expect(await resolvePromptContext(client, "ses-1")).toEqual({
            agent: "build",
            model: { providerID: "openai", modelID: "gpt-5.5" },
            variant: undefined,
        });
    });

    it("reads the user message model shape and scopes its variant the same way", async () => {
        const { client } = clientWithMessages([
            {
                info: {
                    role: "user",
                    agent: "build",
                    model: { providerID: "anthropic", modelID: "claude-opus-4-8", variant: "max" },
                },
            },
            {
                info: {
                    role: "user",
                    agent: "build",
                    model: { providerID: "openai", modelID: "gpt-5.5" },
                },
            },
        ]);

        expect(await resolvePromptContext(client, "ses-1")).toEqual({
            agent: "build",
            model: { providerID: "openai", modelID: "gpt-5.5" },
            variant: undefined,
        });
    });

    it("returns null when the session has no messages", async () => {
        const { client } = clientWithMessages([]);

        expect(await resolvePromptContext(client, "ses-1")).toBeNull();
    });
});

import { describe, expect, it } from "bun:test";
import { finalAssistantText } from "./pi-harness";

describe("finalAssistantText", () => {
    it("returns the text of the last assistant message, joining its text parts", () => {
        expect(
            finalAssistantText({
                type: "agent_end",
                messages: [
                    { role: "user", content: [{ type: "text", text: "hello" }] },
                    { role: "assistant", content: [{ type: "text", text: "first" }] },
                    { role: "toolResult", content: [{ type: "text", text: "ignored" }] },
                    {
                        role: "assistant",
                        content: [
                            { type: "text", text: "pi " },
                            { type: "toolCall", name: "ctx_search" },
                            { type: "text", text: "smoke ok" },
                        ],
                    },
                ],
            }),
        ).toBe("pi smoke ok");
    });

    it("returns an empty string for an assistant message without text parts", () => {
        expect(
            finalAssistantText({
                type: "agent_end",
                messages: [{ role: "assistant", content: [{ type: "toolCall", name: "x" }] }],
            }),
        ).toBe("");
        expect(finalAssistantText({ type: "agent_end", messages: [{ role: "assistant" }] })).toBe(
            "",
        );
    });

    it("returns null when agent_end carries no assistant message", () => {
        expect(finalAssistantText({ type: "agent_end", messages: [] })).toBeNull();
        expect(
            finalAssistantText({ type: "agent_end", messages: [{ role: "user", content: [] }] }),
        ).toBeNull();
        expect(finalAssistantText({ type: "agent_end" })).toBeNull();
    });
});

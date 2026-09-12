import { describe, expect, it } from "bun:test";
import { stripTagPrefixFromAssistantMessage } from "./strip-tag-prefix";

describe("stripTagPrefixFromAssistantMessage", () => {
    describe("leading tag prefix (canonical Eidnara tagger mimicry)", () => {
        // Each row is one single-text-part assistant message: the leading-tag
        // scrub must report a mutation and leave exactly the expected text.
        it.each([
            [
                "strips a single §N§ prefix from assistant text",
                "§4§ Yes. I can see the project memory.",
                "Yes. I can see the project memory.",
            ],
            [
                "strips consecutive §N§ prefixes (model-mimicked sequence)",
                "§3§ §4§ §5§ Hello world",
                "Hello world",
            ],
            ["strips trailing whitespace after the prefix", "§4§   \n\nYes", "Yes"],
            ["strips multi-digit tag IDs", "§38773§ Found it.", "Found it."],
        ] as Array<[string, string, string]>)("%s", (_title, input, expected) => {
            const msg = {
                role: "assistant",
                content: [{ type: "text", text: input }],
            };
            expect(stripTagPrefixFromAssistantMessage(msg)).toBe(true);
            expect((msg.content[0] as { type: string; text: string }).text).toBe(expected);
        });
    });

    describe("cargo-culted § mid-text (models mimicking Eidnara notation)", () => {
        // Cargo-cult defense rows: § notation appearing mid-text (not as a
        // leading tag) must be scrubbed wherever it appears.
        it.each([
            [
                "removes mid-text §N§ pair entirely (cargo-cult defense)",
                "Looking at §5§ which references the earlier discussion",
                "Looking at  which references the earlier discussion",
            ],
            [
                'removes malformed §N"> hybrid mid-text',
                'Hello §40827">Oracle confirmed',
                "Hello Oracle confirmed",
            ],
            [
                "strips stray § character anywhere",
                "See § marker for details",
                "See  marker for details",
            ],
            [
                "strips both leading prefix and mid-text § in same message",
                "§42§ The pattern §40827§ appeared.",
                "The pattern  appeared.",
            ],
        ] as Array<[string, string, string]>)("%s", (_title, input, expected) => {
            const msg = {
                role: "assistant",
                content: [{ type: "text", text: input }],
            };
            expect(stripTagPrefixFromAssistantMessage(msg)).toBe(true);
            expect((msg.content[0] as { type: string; text: string }).text).toBe(expected);
        });
    });

    describe("multi-part messages", () => {
        // Each row is one assistant message spread over several text parts: the scrub must
        // report a mutation, strip a well-formed leading tag only from the first part, and
        // keep the whitespace around removed interior tags so joined words stay separated.
        it.each([
            [
                "keeps the separator that follows a tag at the start of a later part",
                ["§4§ Hello", "§5§ world"],
                ["Hello", " world"],
            ],
            [
                "keeps interior boundary whitespace while scrubbing tags from a middle part",
                ["§4§ Hello ", " §5§ big ", " world §6§ "],
                ["Hello ", "  big ", " world"],
            ],
            ["empties an edge part that held only tag notation", ["§4§ ", "Hello"], ["", "Hello"]],
            [
                "keeps the separator when an interior part held only tag notation",
                ["Hello", " §4§ ", "world"],
                ["Hello", "  ", "world"],
            ],
        ] as Array<[string, string[], string[]]>)("%s", (_title, inputs, expected) => {
            const msg = {
                role: "assistant",
                content: inputs.map((text) => ({ type: "text", text })),
            };
            expect(stripTagPrefixFromAssistantMessage(msg)).toBe(true);
            const texts = (msg.content as Array<{ text: string }>).map((part) => part.text);
            expect(texts).toEqual(expected);
            expect(texts.join("")).toMatch(/^\S.*\S$/);
            expect(texts.join("")).not.toContain("§");
        });

        it("ignores non-text parts (thinking, toolCall, image)", () => {
            const msg = {
                role: "assistant",
                content: [
                    {
                        type: "thinking",
                        thinking: "§4§ pretend reasoning, not stripped",
                    },
                    { type: "text", text: "§4§ Real assistant text" },
                    {
                        type: "toolCall",
                        id: "t1",
                        name: "ctx_search",
                        arguments: {},
                    },
                ],
            };
            expect(stripTagPrefixFromAssistantMessage(msg)).toBe(true);
            expect((msg.content[1] as { type: string; text: string }).text).toBe(
                "Real assistant text",
            );
            // stripTagPrefixFromAssistantMessage scrubs only text parts.
            expect((msg.content[0] as { type: string; thinking: string }).thinking).toBe(
                "§4§ pretend reasoning, not stripped",
            );
        });
    });

    it("returns false and leaves the message untouched when nothing is in scope", () => {
        // Non-assistant roles, non-array or empty content, and text without any `§` are
        // all outside the scrub's scope: no mutation, and the message is byte-identical.
        const messages: Array<{ role: string; content: unknown }> = [
            { role: "user", content: [{ type: "text", text: "§4§ Hello from user" }] },
            { role: "toolResult", content: [{ type: "text", text: "§7§ tool output" }] },
            { role: "assistant", content: "§4§ legacy string" },
            { role: "assistant", content: [] },
            {
                role: "assistant",
                content: [{ type: "text", text: "Plain response without any prefix" }],
            },
            {
                role: "assistant",
                content: [
                    { type: "text", text: "Hello " },
                    { type: "text", text: "world" },
                ],
            },
        ];
        for (const msg of messages) {
            const original = structuredClone(msg);
            expect(stripTagPrefixFromAssistantMessage(msg), JSON.stringify(original)).toBe(false);
            expect(msg, JSON.stringify(original)).toEqual(original);
        }
    });
});

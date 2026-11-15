import { describe, expect, it } from "bun:test";
import { stripStreamedTags } from "./stream-tag-strip";
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
            ["keeps a tag's neighbour that only resembles a mangled tag", '§1§2">x', '2">x'],
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
        // Cargo-cult defense rows: the scrub removes tag notation embedded in prose.
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
                "strips both leading prefix and mid-text § in same message",
                "§42§ The pattern §40827§ appeared.",
                "The pattern  appeared.",
            ],
            ["keeps edge whitespace that no opening tag owns", "  a §5§ b  ", "  a  b  "],
            ["keeps the whitespace before a trailing tag", "Tail tag §99§", "Tail tag "],
        ] as Array<[string, string, string]>)("%s", (_title, input, expected) => {
            const msg = {
                role: "assistant",
                content: [{ type: "text", text: input }],
            };
            expect(stripTagPrefixFromAssistantMessage(msg)).toBe(true);
            expect((msg.content[0] as { type: string; text: string }).text).toBe(expected);
        });
    });

    it("keeps a section sign that is no tag, as the stream shows it", () => {
        for (const text of [
            "See § 4 of the spec.",
            "Under §42 of the Act the fee applies.",
            "§3.5 and §12a and price§§",
        ]) {
            const msg = { role: "assistant", content: [{ type: "text", text: `§9§ ${text}` }] };
            expect(stripTagPrefixFromAssistantMessage(msg)).toBe(true);
            expect((msg.content[0] as { text: string }).text).toBe(text);
        }
    });

    it("leaves text that holds no tag untouched, edge whitespace included", () => {
        const messages = [
            ["  Under §42 of the Act  "],
            ["\n\nSee § 4 of the spec.\n"],
            [" price§§ ", " and §12a "],
        ].map((texts) => ({
            role: "assistant",
            content: texts.map((text) => ({ type: "text", text })),
        }));
        for (const msg of messages) {
            const original = structuredClone(msg);
            expect(stripTagPrefixFromAssistantMessage(msg), JSON.stringify(original)).toBe(false);
            expect(msg, JSON.stringify(original)).toEqual(original);
        }
    });

    describe("multi-part messages", () => {
        // Each row is one assistant message spread over several text parts: the scrub must
        // report a mutation, strip a well-formed leading tag only from the first part, and
        // keep the whitespace around every other removed tag, as the stream shows it.
        it.each([
            [
                "keeps the separator that follows a tag at the start of a later part",
                ["§4§ Hello", "§5§ world"],
                ["Hello", " world"],
            ],
            [
                "keeps interior boundary whitespace while scrubbing tags from a middle part",
                ["§4§ Hello ", " §5§ big ", " world §6§ "],
                ["Hello ", "  big ", " world  "],
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
                        name: "eidnara_search",
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

    describe("after message_start showed the message", () => {
        // Pi hands message_start and message_end the same object for a whole message, or a
        // copy that shares the content array; either way the saved text is one complete strip.
        it.each([
            ["a trailing tag prefix", "§1§ see §§1§2§ and §4", "see §2§ and ", "see §2§ and §4"],
            ["trailing whitespace", "§1§ hi \n", "hi \n", "hi \n"],
            ["leading whitespace alone", " \n", "", " \n"],
        ])("holds %s at message_start and saves the complete strip", (_title, text, start, end) => {
            const same = { role: "assistant", content: [{ type: "text", text }] };
            stripStreamedTags({ message: same });
            expect(same.content[0]?.text).toBe(start);
            stripTagPrefixFromAssistantMessage(same);
            expect(same.content[0]?.text).toBe(end);

            const final = { role: "assistant", content: [{ type: "text", text }] };
            const copy = { ...final };
            stripStreamedTags({ message: copy });
            expect(copy.content[0]?.text).toBe(start);
            expect(final.content[0]?.text).toBe(text);
            stripTagPrefixFromAssistantMessage(final);
            expect(final.content[0]?.text).toBe(end);
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

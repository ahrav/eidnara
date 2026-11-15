import { describe, expect, it } from "bun:test";
import { StreamTagStripper, stripStreamedTags } from "./stream-tag-strip";

const S = "\u00a7";
const TAG_SOURCE = String.raw`\u00a7\d{1,19}(?:\u00a7|">(?:\u00a7(?:\d{1,19}\u00a7)?)?)`;
const TAG = new RegExp(TAG_SOURCE, "gu");
const OPENING = new RegExp(String.raw`^\s*(?:${TAG_SOURCE}\s*)+`, "u");

function reference(text: string, leading: boolean): string {
    return (leading ? text.replace(OPENING, "") : text).replace(TAG, "");
}

/** Everything released so far must be a prefix of the final text, so a tag fragment that reached the stream would fail here. */
function stream(chunks: string[], leading: boolean): { released: string[]; text: string } {
    const stripper = new StreamTagStripper(leading);
    const expected = reference(chunks.join(""), leading);
    const released: string[] = [];
    for (const chunk of chunks) {
        released.push(stripper.push(chunk));
        expect(expected.startsWith(stripper.released)).toBe(true);
    }
    released.push(stripper.flush());
    expect(released.join("")).toBe(stripper.released);
    return { released, text: stripper.released };
}

function chunkings(text: string): string[][] {
    const cuts: string[][] = [[text], [...text]];
    for (let first = 1; first < text.length; first += 1) {
        cuts.push([text.slice(0, first), text.slice(first)]);
        for (let second = first + 1; second < text.length; second += 1) {
            cuts.push([text.slice(0, first), text.slice(first, second), text.slice(second)]);
        }
    }
    return cuts;
}

const TAGGED: [string, boolean, string][] = [
    [`${S}12${S} The parser accepts the header.`, true, "The parser accepts the header."],
    [`  ${S}3${S}\n\n${S}40${S} Done.`, true, "Done."],
    [`Fixed in ${S}7${S} parser.rs and ${S}8${S}.`, false, "Fixed in  parser.rs and ."],
    [`Tail tag ${S}99${S}`, true, "Tail tag "],
    [`${S}1${S}${S}2${S}x`, true, "x"],
    [`${S}${"9".repeat(19)}${S} max`, true, "max"],
    [`Line one\n${S}5${S} line two`, true, "Line one\n line two"],
    [`${S}1${S} `, true, ""],
    [`Hello ${S}40827">Oracle confirmed`, false, "Hello Oracle confirmed"],
    [`${S}3">${S}4${S} Done ${S}5">${S}x`, true, "Done x"],
    [`${S}6">  tail`, true, "tail"],
];

const PLAIN = [
    "See \u00a7 4 of the spec.",
    "Under \u00a742 of the Act the fee applies.",
    `price ${S}${S} and ${S}12a and ${S}3.5`,
    `${S}${"1".repeat(20)}${S} is too long to be a tag`,
    "  indented answer with no tags",
    "trailing reference \u00a77",
    "",
];

describe("StreamTagStripper", () => {
    it("removes every tag however the deltas split it", () => {
        for (const [text, leading, expected] of TAGGED) {
            expect(reference(text, leading)).toBe(expected);
            for (const chunks of chunkings(text)) {
                const { released, text: shown } = stream(chunks, leading);
                expect(shown, JSON.stringify(chunks)).toBe(expected);
                for (const piece of released) expect(piece).not.toMatch(/\u00a7\d/u);
            }
        }
    });

    it("streams text holding a section sign that is no tag unchanged", () => {
        for (const text of PLAIN) {
            for (const leading of [true, false]) {
                for (const chunks of chunkings(text)) {
                    expect(stream(chunks, leading).text, JSON.stringify(chunks)).toBe(text);
                }
            }
        }
    });

    it("matches the reference over random text and random cuts", () => {
        let seed = 0x5eed;
        const next = (bound: number) => {
            seed = (seed * 48_271) % 0x7fffffff;
            return seed % bound;
        };
        const alphabet = [S, S, S, "1", "2", "0", "9", "a", " ", "\n", ".", '"', ">"];
        for (let round = 0; round < 3_000; round += 1) {
            const length = next(24);
            let text = "";
            for (let i = 0; i < length; i += 1) text += alphabet[next(alphabet.length)];
            const chunks: string[] = [];
            let at = 0;
            while (at < text.length) {
                const size = 1 + next(4);
                chunks.push(text.slice(at, at + size));
                at += size;
            }
            const leading = next(2) === 0;
            expect(stream(chunks, leading).text, JSON.stringify({ text, leading })).toBe(
                reference(text, leading),
            );
        }
    });
});

type Part = { type: string; text?: string; thinking?: string };
type Event = {
    type: "message_update";
    message: { role: string; content: Part[] };
    assistantMessageEvent: {
        type: string;
        contentIndex: number;
        delta?: string;
        content?: string;
        partial: { role: string; content: Part[] };
    };
};

function replay(
    text: string,
    chunk: number,
): { events: Event[]; partial: { role: string; content: Part[] } } {
    const partial = { role: "assistant", content: [] as Part[] };
    const events: Event[] = [];
    const emit = (update: Omit<Event["assistantMessageEvent"], "partial">) => {
        const event: Event = {
            type: "message_update",
            message: { ...partial },
            assistantMessageEvent: { ...update, partial: { ...partial } },
        };
        stripStreamedTags(event);
        events.push(structuredClone(event));
    };
    partial.content = [{ type: "thinking", thinking: `plan ${S}4${S}` }];
    emit({ type: "thinking_end", contentIndex: 0, content: `plan ${S}4${S}` });
    partial.content = [...partial.content, { type: "text", text: "" }];
    emit({ type: "text_start", contentIndex: 1 });
    for (let at = 0; at < text.length; at += chunk) {
        const delta = text.slice(at, at + chunk);
        (partial.content[1] as { text: string }).text += delta;
        emit({ type: "text_delta", contentIndex: 1, delta });
    }
    emit({
        type: "text_end",
        contentIndex: 1,
        content: (partial.content[1] as { text: string }).text,
    });
    return { events, partial };
}

function deltaSender(part: Part): (delta: string) => [string | undefined, string] {
    const partial = { role: "assistant", content: [part] };
    return (delta) => {
        const event = {
            message: { ...partial },
            assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta },
        };
        stripStreamedTags(event);
        return [(event.message.content[0] as Part).text, event.assistantMessageEvent.delta];
    };
}

describe("stripStreamedTags", () => {
    const answer = `${S}230${S} I reran the suite: ${S}12${S} 1610 pass, see \u00a7 4.`;
    const shown = "I reran the suite:  1610 pass, see \u00a7 4.";

    it("keeps every tag out of the rendered message, the deltas, and the partial", () => {
        for (const chunk of [1, 2, 3, 5]) {
            const { events, partial } = replay(answer, chunk);
            let deltas = "";
            for (const event of events) {
                const text = event.message.content[1]?.text ?? "";
                expect(shown.startsWith(text)).toBe(true);
                expect(event.assistantMessageEvent.partial.content[1]?.text ?? "").toBe(text);
                if (event.assistantMessageEvent.type === "text_delta")
                    deltas += event.assistantMessageEvent.delta ?? "";
            }
            const last = events.at(-1);
            expect(last?.message.content[1]?.text).toBe(shown);
            expect(last?.assistantMessageEvent.content).toBe(shown);
            expect(deltas).toBe(shown);
            expect(last?.message.content[0]).toEqual({
                type: "thinking",
                thinking: `plan ${S}4${S}`,
            });
            expect(partial.content[1]?.text).toBe(answer);
        }
    });

    it("leaves an answer with no tag as the provider streamed it", () => {
        const plain = "Under \u00a742 of the Act the fee applies.";
        for (const chunk of [1, 4]) {
            const { events } = replay(plain, chunk);
            let deltas = "";
            for (const event of events) {
                if (event.assistantMessageEvent.type === "text_delta")
                    deltas += event.assistantMessageEvent.delta ?? "";
            }
            expect(deltas).toBe(plain);
            expect(events.at(-1)?.message.content[1]?.text).toBe(plain);
        }
    });

    it("releases a trailing section sign and its digits when the part ends", () => {
        const text = `The fee applies under ${S}42`;
        const { events } = replay(text, 3);
        const end = events.at(-1);
        expect(end?.assistantMessageEvent.type).toBe("text_end");
        expect(end?.assistantMessageEvent.content).toBe(text);
        expect(end?.message.content[1]?.text).toBe(text);
        const deltas = events
            .filter((event) => event.assistantMessageEvent.type === "text_delta")
            .map((event) => event.assistantMessageEvent.delta ?? "")
            .join("");
        expect(deltas).toBe("The fee applies under ");
    });

    it("keeps each delta in step when the part's text runs ahead of its update", () => {
        const part = { type: "text", text: "" };
        const partial = { role: "assistant", content: [part] as Part[] };
        const chunks = [`${S}3`, `${S} The`, ` fix ${S}1`, `2${S} here`];
        part.text = chunks.join("");
        const deltas: string[] = [];
        for (const delta of chunks) {
            const event = {
                message: { ...partial },
                assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta },
            };
            stripStreamedTags(event);
            expect((event.message.content[0] as Part).text).toBe("The fix  here");
            deltas.push(event.assistantMessageEvent.delta);
        }
        expect(deltas).toEqual(["", "The", " fix ", " here"]);
    });

    it("keeps each part's deltas when the provider has already started the next part", () => {
        const first = { type: "text", text: `${S}1${S} A` };
        const second = { type: "text", text: "C" };
        const partial = { role: "assistant", content: [first, second] as Part[] };
        const deltas: string[] = [];
        for (const [contentIndex, delta] of [
            [0, `${S}1${S} `],
            [0, "A"],
            [1, "C"],
        ] as const) {
            const event = {
                message: { ...partial },
                assistantMessageEvent: { type: "text_delta", contentIndex, delta },
            };
            stripStreamedTags(event);
            deltas.push(event.assistantMessageEvent.delta);
        }
        expect(deltas).toEqual(["", "A", "C"]);
    });

    it("restarts a part whose text stops extending what it showed", () => {
        const part = { type: "text", text: `${S}1${S} abc` };
        const partial = { role: "assistant", content: [part] as Part[] };
        const shown = () => {
            const event = {
                message: { ...partial },
                assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: "" },
            };
            stripStreamedTags(event);
            return (event.message.content[0] as Part).text;
        };
        expect(shown()).toBe("abc");
        part.text = `xy ${S}2${S} z`;
        expect(shown()).toBe("xy  z");
    });

    it("starts the deltas over when a part restarts, dropping a held prefix", () => {
        const part = { type: "text", text: `ok ${S}12` };
        const send = deltaSender(part);
        expect(send(`ok ${S}12`)).toEqual(["ok ", "ok "]);
        part.text = "new";
        expect(send("new")).toEqual(["new", "new"]);
    });

    it("hands listeners snapshots, so text the provider appends later stays out of them", () => {
        const part = { type: "text", text: "Hello there" };
        const partial = { role: "assistant", content: [part] as Part[] };
        const update = {
            type: "text_delta",
            contentIndex: 0,
            delta: "Hello there",
            partial: { ...partial },
        };
        const event = { message: { ...partial }, assistantMessageEvent: update };
        stripStreamedTags(event);
        part.text += ` ${S}12${S} more`;
        expect((event.message.content[0] as Part).text).toBe("Hello there");
        expect((update.partial.content[0] as Part).text).toBe("Hello there");
    });

    it("leaves the provider's own partial message holding its raw parts", () => {
        const part = { type: "text", text: `${S}1${S} hi` };
        const provider = { role: "assistant", content: [part] as Part[] };
        const update = { type: "text_delta", contentIndex: 0, delta: part.text, partial: provider };
        const event = { message: { ...provider }, assistantMessageEvent: update };
        stripStreamedTags(event);
        expect(provider.content[0]).toBe(part);
        expect(part.text).toBe(`${S}1${S} hi`);
        expect((update.partial.content[0] as Part).text).toBe("hi");
    });

    it("strips a whole message an event carries without a stream update", () => {
        const event = {
            message: {
                role: "assistant",
                content: [{ type: "text", text: `${S}1${S} see ${S}4` }],
            },
        };
        stripStreamedTags(event);
        expect(event.message.content[0]?.text).toBe(`see ${S}4`);
    });

    it("treats only the first text part as leading", () => {
        const first = { type: "text", text: `${S}1${S} First.` };
        const second = { type: "text", text: "" };
        const partial = { role: "assistant", content: [first, second] as Part[] };
        let shown = "";
        for (const delta of ["\n\n", S, "5", S, " more"]) {
            second.text += delta;
            const event = {
                message: { ...partial },
                assistantMessageEvent: { type: "text_delta", contentIndex: 1, delta },
            };
            stripStreamedTags(event);
            shown = (event.message.content[1] as Part).text ?? "";
            expect((event.message.content[0] as Part).text).toBe("First.");
        }
        expect(shown).toBe("\n\n more");
    });

    it("ignores messages other than an assistant's", () => {
        const event = {
            message: { role: "user", content: [{ type: "text", text: `${S}1${S} hi` }] },
        };
        stripStreamedTags(event);
        expect(event.message.content[0]?.text).toBe(`${S}1${S} hi`);
    });

    it("strips each delta as its update carries it when the part grew by other text of that length", () => {
        const part = { type: "text", text: "" };
        const partial = { role: "assistant", content: [part] as Part[] };
        const send = (grown: string, delta: string) => {
            part.text += grown;
            const event = {
                message: { ...partial },
                assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta },
            };
            stripStreamedTags(event);
            return [(event.message.content[0] as Part).text, event.assistantMessageEvent.delta];
        };
        expect(send("abc ", "abc ")).toEqual(["abc ", "abc "]);
        expect(send(`${S}1${S}de`, `fg${S}2${S}`)).toEqual(["abc de", "fg"]);
    });

    it("starts deltas that run on their own over when the part restarts", () => {
        const part = { type: "text", text: "" };
        const send = deltaSender(part);
        part.text = `ok ${S}12x`;
        expect(send(`ok ${S}12`)).toEqual([`ok ${S}12x`, "ok "]);
        part.text = "new";
        expect(send("new")).toEqual(["new", "new"]);
    });

    it("shows a restarted part whose text keeps an earlier prefix of what it showed", () => {
        const part = { type: "text", text: "" };
        const send = deltaSender(part);
        for (const delta of ["ab", "c"]) {
            part.text += delta;
            expect(send(delta)).toEqual([part.text, delta]);
        }
        part.text = "abX";
        expect(send("abX")).toEqual(["abX", "abX"]);
    });

    it("keeps the whitespace after an opening tag out of deltas that run behind the part", () => {
        const part = { type: "text", text: "" };
        const send = deltaSender(part);
        part.text = `${S}1${S}`;
        expect(send(`${S}1${S}`)).toEqual(["", ""]);
        part.text += " ab cd";
        expect(send(" ab")).toEqual(["ab cd", "ab"]);
        expect(send(" cd")).toEqual(["ab cd", " cd"]);
    });

    it("treats text a part gains after its end as interior text", () => {
        const part = { type: "text", text: "" };
        const partial = { role: "assistant", content: [part] as Part[] };
        const send = (type: string, delta?: string) => {
            const update: { type: string; contentIndex: number; delta?: string } = {
                type,
                contentIndex: 0,
            };
            if (delta !== undefined) update.delta = delta;
            const event = { message: { ...partial }, assistantMessageEvent: update };
            stripStreamedTags(event);
            return [(event.message.content[0] as Part).text, event.assistantMessageEvent.delta];
        };
        part.text = `${S}1${S}`;
        expect(send("text_delta", `${S}1${S}`)).toEqual(["", ""]);
        expect(send("text_end")).toEqual(["", undefined]);
        part.text += " hi";
        expect(send("text_delta", " hi")).toEqual([" hi", "hi"]);
    });

    it("releases a delta for a part whose text has not grown", () => {
        const part = { type: "text", text: "" };
        const event = {
            message: { role: "assistant", content: [part] as Part[] },
            assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: "x" },
        };
        stripStreamedTags(event);
        expect((event.message.content[0] as Part).text).toBe("");
        expect(event.assistantMessageEvent.delta).toBe("x");
    });

    it("strips the message of an update that is null before the update read fails", () => {
        const message = { role: "assistant", content: [{ type: "text", text: `${S}1${S} hi` }] };
        expect(() => stripStreamedTags({ message, assistantMessageEvent: null })).toThrow(
            TypeError,
        );
        expect(message.content[0]?.text).toBe("hi");
    });

    it("keeps an update's delta when a part's accessor strips another update meanwhile", () => {
        const inner = {
            message: { role: "assistant", content: [{ type: "text", text: "q" }] },
            assistantMessageEvent: { type: "text_delta", contentIndex: 3, delta: "zz" },
        };
        const reentrant = {
            get type() {
                stripStreamedTags(inner);
                return "text";
            },
            text: "x",
        };
        const event = {
            message: {
                role: "assistant",
                content: [{ type: "text", text: `hi${S}1${S}` }, reentrant],
            },
            assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: `hi${S}1${S}` },
        };
        stripStreamedTags(event);
        expect(event.assistantMessageEvent.delta).toBe("hi");
        expect(inner.assistantMessageEvent.delta).toBe("zz");
    });

    it("copies a streamed part after streaming it, and a whole message's part before stripping it", () => {
        const growing = () => {
            const part = { type: "text", text: "ab" };
            Object.defineProperty(part, "extra", {
                enumerable: true,
                get() {
                    part.text += `${S}1${S}cd`;
                    return 1;
                },
            });
            return part;
        };
        const streamed = {
            message: { role: "assistant", content: [growing()] },
            assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: "ab" },
        };
        stripStreamedTags(streamed);
        expect(streamed.message.content[0]?.text).toBe("ab");
        const whole = { message: { role: "assistant", content: [growing()] } };
        stripStreamedTags(whole);
        expect(whole.message.content[0]?.text).toBe("abcd");
    });

    it("shows what a strip of the same part from its accessor released", () => {
        let reads = 0;
        const part = {
            get type() {
                reads += 1;
                if (reads === 2) {
                    part.text += " more";
                    stripStreamedTags({
                        message: { role: "assistant", content: [part] },
                        assistantMessageEvent: {
                            type: "text_delta",
                            contentIndex: 0,
                            delta: " more",
                        },
                    });
                }
                return "text";
            },
            text: "hi",
        };
        const event = {
            message: { role: "assistant", content: [part] as Part[] },
            assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: "hi" },
        };
        stripStreamedTags(event);
        expect((event.message.content[0] as Part).text).toBe("hi more");
        expect(event.assistantMessageEvent.delta).toBe("hi");
    });

    it("shows the stream it copied when a strip of the same part from its accessor restarts it", () => {
        let reads = 0;
        const part = {
            get type() {
                reads += 1;
                if (reads === 2) {
                    part.text = "zzz";
                    stripStreamedTags({
                        message: { role: "assistant", content: [part] },
                        assistantMessageEvent: {
                            type: "text_delta",
                            contentIndex: 0,
                            delta: "zzz",
                        },
                    });
                }
                return "text";
            },
            text: "hi",
        };
        const event = {
            message: { role: "assistant", content: [part] as Part[] },
            assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: "hi" },
        };
        stripStreamedTags(event);
        expect((event.message.content[0] as Part).text).toBe("hi");
    });

    it("keeps an update's delta when a reentrant strip fails inside a part's accessor", () => {
        const failing = {
            get type(): string {
                throw new Error("accessor failed");
            },
        };
        const caught: unknown[] = [];
        const reentrant = {
            get type() {
                try {
                    stripStreamedTags({
                        message: { role: "assistant", content: [failing] },
                        assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: "q" },
                    });
                } catch (error) {
                    caught.push(error);
                }
                return "text";
            },
            text: "x",
        };
        const event = {
            message: {
                role: "assistant",
                content: [{ type: "text", text: `hi${S}1${S}` }, reentrant],
            },
            assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: `hi${S}1${S}` },
        };
        stripStreamedTags(event);
        expect(caught.length).toBeGreaterThan(0);
        expect(event.assistantMessageEvent.delta).toBe("hi");
    });

    it("streams random answers, in step, run ahead, or restarted, as prefixes of the whole-text strip", () => {
        let seed = 0x7a11;
        const next = (bound: number) => {
            seed = (seed * 48_271) % 0x7fffffff;
            return seed % bound;
        };
        const alphabet = [
            S,
            S,
            S,
            "1",
            "2",
            "0",
            "9",
            "a",
            " ",
            " ",
            "\n",
            ".",
            '"',
            ">",
            "\u2014",
        ];
        const randomText = (max: number) => {
            let text = "";
            for (let i = next(max); i > 0; i -= 1) text += alphabet[next(alphabet.length)];
            return text;
        };
        for (let round = 0; round < 2_000; round += 1) {
            const part = { type: "text", text: "" };
            const partial = {
                role: "assistant",
                content: [{ type: "thinking", thinking: "" }, part] as Part[],
            };
            const emit = (update: { type: string; delta?: string; content?: string }) => {
                const event = {
                    message: { ...partial },
                    assistantMessageEvent: { ...update, contentIndex: 1, partial },
                };
                stripStreamedTags(event);
                const shownPart = event.message.content[1] as Part;
                expect(event.assistantMessageEvent.partial.content[1]).toBe(shownPart);
                expect(shownPart).not.toBe(part);
                return { shown: shownPart.text ?? "", update: event.assistantMessageEvent };
            };
            const restartAt = next(3) === 0 ? next(16) : -1;
            const pending: string[] = [];
            let seen = "";
            let shown = "";
            let sent = "";
            let deltas = "";
            const deliver = (lag: number) => {
                while (pending.length > lag) {
                    const piece = pending.shift() ?? "";
                    if (!part.text.startsWith(seen)) {
                        shown = "";
                        sent = "";
                        deltas = "";
                    }
                    seen = part.text;
                    const event = emit({ type: "text_delta", delta: piece });
                    const whole = reference(part.text, true);
                    const context = JSON.stringify({ round, text: part.text, shown: event.shown });
                    expect(whole.startsWith(event.shown), context).toBe(true);
                    expect(event.shown.startsWith(shown), context).toBe(true);
                    shown = event.shown;
                    sent += piece;
                    deltas += event.update.delta;
                    expect(reference(sent, true).startsWith(deltas), context).toBe(true);
                }
            };
            const steps = 2 + next(20);
            for (let step = 0; step < steps; step += 1) {
                if (step === restartAt) {
                    deliver(0);
                    part.text = randomText(10);
                }
                const piece = randomText(7);
                part.text += piece;
                pending.push(piece);
                deliver(next(3));
            }
            deliver(0);
            const end = emit({ type: "text_end", content: part.text });
            const whole = reference(part.text, true);
            expect(end.shown, JSON.stringify({ round, text: part.text })).toBe(whole);
            expect(end.update.content).toBe(whole);
        }
    });
});

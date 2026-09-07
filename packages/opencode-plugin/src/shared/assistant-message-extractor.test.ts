import { describe, expect, it } from "bun:test";
import { extractLatestAssistantText, hasLengthCappedOutput } from "./assistant-message-extractor";

function assistant(text: string, created?: number) {
    return {
        info: { role: "assistant", ...(created === undefined ? {} : { time: { created } }) },
        parts: [{ type: "text", text }],
    };
}

describe("extractLatestAssistantText", () => {
    it("returns null for non-arrays, empty arrays, and arrays without assistant messages", () => {
        expect(extractLatestAssistantText(undefined)).toBeNull();
        expect(extractLatestAssistantText([])).toBeNull();
        expect(
            extractLatestAssistantText([
                { info: { role: "user" }, parts: [{ type: "text", text: "hi" }] },
            ]),
        ).toBeNull();
    });

    it("picks the newest assistant message by time.created regardless of array order", () => {
        const messages = [assistant("T9", 9), assistant("T1", 1), assistant("T5", 5)];
        expect(extractLatestAssistantText(messages)).toBe("T9");
    });

    it("breaks timestamp ties toward the later array position", () => {
        expect(
            extractLatestAssistantText([
                assistant("OLDEST", 1000),
                assistant("MIDDLE", 1000),
                assistant("NEWEST", 1000),
            ]),
        ).toBe("NEWEST");
    });

    it("falls back to array order when no message carries a timestamp", () => {
        expect(
            extractLatestAssistantText([
                assistant("OLDEST"),
                assistant("MIDDLE"),
                assistant("NEWEST"),
            ]),
        ).toBe("NEWEST");
    });

    it("ranks a timestamped message above an untimestamped one", () => {
        expect(extractLatestAssistantText([assistant("T5", 5), assistant("NOTIME")])).toBe("T5");
    });

    it("ranks a zero timestamp above an absent one", () => {
        expect(extractLatestAssistantText([assistant("T0", 0), assistant("NOTIME")])).toBe("T0");
    });

    it("treats NaN and infinite timestamps as absent", () => {
        expect(extractLatestAssistantText([assistant("NAN", Number.NaN)])).toBe("NAN");
        expect(extractLatestAssistantText([assistant("T5", 5), assistant("NAN", Number.NaN)])).toBe(
            "T5",
        );
        expect(
            extractLatestAssistantText([
                assistant("INF", Number.POSITIVE_INFINITY),
                assistant("T5", 5),
            ]),
        ).toBe("T5");
    });

    it("reads a created getter once so validation and ranking see the same value", () => {
        let reads = 0;
        const flaky = {
            info: {
                role: "assistant",
                time: {
                    get created(): number {
                        reads += 1;
                        return reads === 1 ? 5 : Number.POSITIVE_INFINITY;
                    },
                },
            },
            parts: [{ type: "text", text: "FLAKY" }],
        };
        expect(extractLatestAssistantText([flaky, assistant("SOLID", 9)])).toBe("SOLID");
        expect(reads).toBe(1);
    });

    it("joins text parts and ignores non-text parts", () => {
        const message = {
            info: { role: "assistant" },
            parts: [
                { type: "text", text: "a" },
                { type: "tool", text: "ignored" },
                { type: "text", text: "b" },
                "not a record",
            ],
        };
        expect(extractLatestAssistantText([message])).toBe("a\nb");
    });

    it("returns null when the latest assistant message has no text parts", () => {
        expect(extractLatestAssistantText([{ info: { role: "assistant" }, parts: [] }])).toBeNull();
    });

    it("skips a message whose fields trap on read and keeps the readable ones", () => {
        const trapping = {
            get info(): never {
                throw new Error("info getter");
            },
        };
        expect(extractLatestAssistantText([trapping, assistant("OK")])).toBe("OK");
        expect(extractLatestAssistantText([trapping])).toBeNull();
    });

    it("returns null when the latest message's parts trap on read", () => {
        const trappingParts = {
            info: { role: "assistant", time: { created: 9 } },
            parts: [
                {
                    get type(): never {
                        throw new Error("type getter");
                    },
                },
            ],
        };
        expect(extractLatestAssistantText([assistant("OLDER", 1), trappingParts])).toBeNull();
    });

    it("skips an array index whose accessor throws and keeps scanning", () => {
        const messages: unknown[] = [assistant("FIRST", 1), undefined, assistant("LAST", 3)];
        Object.defineProperty(messages, 1, {
            get(): never {
                throw new Error("index getter");
            },
        });
        expect(extractLatestAssistantText(messages)).toBe("LAST");
    });

    it("ignores a trapping length and still reads the present elements", () => {
        const lengthTrap = new Proxy([assistant("HIDDEN")], {
            get(target, key, receiver) {
                if (key === "length") throw new Error("length trap");
                return Reflect.get(target, key, receiver);
            },
        });
        expect(extractLatestAssistantText(lengthTrap)).toBe("HIDDEN");
    });

    it("visits only present indices of a sparse or length-inflated array", () => {
        const sparse: unknown[] = [];
        sparse[2] = assistant("SECOND");
        sparse[1_000_000_000] = assistant("LAST");
        expect(extractLatestAssistantText(sparse)).toBe("LAST");

        const inflated = new Proxy([assistant("ONLY")], {
            get(target, key, receiver) {
                if (key === "length") return 1_000_000_000;
                return Reflect.get(target, key, receiver);
            },
        });
        expect(extractLatestAssistantText(inflated)).toBe("ONLY");
    });
});

describe("hasLengthCappedOutput", () => {
    it("detects the boolean flags and the finish-reason strings", () => {
        expect(hasLengthCappedOutput({ length_capped: true })).toBe(true);
        expect(hasLengthCappedOutput({ lengthCapped: true })).toBe(true);
        expect(hasLengthCappedOutput({ finish_reason: "LENGTH" })).toBe(true);
        expect(hasLengthCappedOutput({ finishReason: "max_tokens" })).toBe(true);
        expect(hasLengthCappedOutput({ finishReason: "max_output_tokens" })).toBe(true);
        expect(hasLengthCappedOutput({ finish_reason: "stop" })).toBe(false);
        expect(hasLengthCappedOutput({ length_capped: "true" })).toBe(false);
    });

    it("checks both finish-reason aliases independently", () => {
        expect(hasLengthCappedOutput({ finish_reason: "stop", finishReason: "length" })).toBe(true);
        expect(hasLengthCappedOutput({ finish_reason: "length", finishReason: "stop" })).toBe(true);
        expect(hasLengthCappedOutput({ finish_reason: "stop", finishReason: "stop" })).toBe(false);
    });

    it("recognizes OpenCode's native info.finish field", () => {
        const capped = { info: { role: "assistant", finish: "length" }, parts: [] };
        const toolCalls = { info: { role: "assistant", finish: "tool-calls" }, parts: [] };
        const stopped = { info: { role: "assistant", finish: "stop" }, parts: [] };
        expect(hasLengthCappedOutput([stopped, capped])).toBe(true);
        expect(hasLengthCappedOutput([stopped, toolCalls])).toBe(false);
    });

    it("walks nested objects and arrays", () => {
        expect(hasLengthCappedOutput({ a: [{ b: { finish_reason: "length" } }] })).toBe(true);
        expect(hasLengthCappedOutput([[{ lengthCapped: true }]])).toBe(true);
        expect(hasLengthCappedOutput({ a: [{ b: 1 }], c: "x" })).toBe(false);
    });

    it("returns false for primitives", () => {
        expect(hasLengthCappedOutput(null)).toBe(false);
        expect(hasLengthCappedOutput("length")).toBe(false);
        expect(hasLengthCappedOutput(42)).toBe(false);
    });

    it("terminates on cyclic object graphs", () => {
        const cyclic: Record<string, unknown> = { a: 1 };
        cyclic.self = cyclic;
        expect(hasLengthCappedOutput(cyclic)).toBe(false);

        const cyclicArray: unknown[] = [1];
        cyclicArray.push(cyclicArray);
        expect(hasLengthCappedOutput(cyclicArray)).toBe(false);

        const capped: Record<string, unknown> = { finish_reason: "length" };
        const wrapper: Record<string, unknown> = { inner: capped };
        capped.back = wrapper;
        expect(hasLengthCappedOutput(wrapper)).toBe(true);
    });

    it("visits a shared reference once without changing the verdict", () => {
        const shared = { finish_reason: "stop" };
        expect(hasLengthCappedOutput({ a: shared, b: shared })).toBe(false);
        const cappedShared = { finish_reason: "length" };
        expect(hasLengthCappedOutput({ a: { x: 1 }, b: cappedShared, c: cappedShared })).toBe(true);
    });

    it("returns false instead of propagating a throwing accessor or proxy trap", () => {
        const throwingAccessor = {
            get boom(): never {
                throw new Error("accessor");
            },
        };
        expect(hasLengthCappedOutput({ nested: throwingAccessor })).toBe(false);

        const trappingProxy = new Proxy(
            {},
            {
                ownKeys() {
                    throw new Error("trap");
                },
            },
        );
        expect(hasLengthCappedOutput({ nested: trappingProxy })).toBe(false);
    });

    it("still finds a capping marker on a sibling of a trapping property", () => {
        const throwingAccessor = {
            get boom(): never {
                throw new Error("accessor");
            },
        };
        expect(hasLengthCappedOutput({ a: throwingAccessor, b: { finish_reason: "length" } })).toBe(
            true,
        );

        const withTrappingMarker = {
            get length_capped(): never {
                throw new Error("marker getter");
            },
            nested: { finishReason: "max_tokens" },
        };
        expect(hasLengthCappedOutput(withTrappingMarker)).toBe(true);

        const array: unknown[] = [undefined, { lengthCapped: true }];
        Object.defineProperty(array, 0, {
            get(): never {
                throw new Error("index getter");
            },
        });
        expect(hasLengthCappedOutput(array)).toBe(true);

        const lengthTrap = new Proxy([{ finish_reason: "length" }], {
            get(target, key, receiver) {
                if (key === "length") throw new Error("length trap");
                return Reflect.get(target, key, receiver);
            },
        });
        expect(hasLengthCappedOutput({ a: lengthTrap, b: { finish_reason: "stop" } })).toBe(true);
        expect(hasLengthCappedOutput({ a: lengthTrap })).toBe(true);
    });

    it("visits only present indices of a sparse or length-inflated array", () => {
        const sparse: unknown[] = [];
        sparse[1_000_000_000] = { finish_reason: "length" };
        expect(hasLengthCappedOutput(sparse)).toBe(true);

        const inflated = new Proxy([{ finish_reason: "stop" }], {
            get(target, key, receiver) {
                if (key === "length") return 1_000_000_000;
                return Reflect.get(target, key, receiver);
            },
        });
        expect(hasLengthCappedOutput(inflated)).toBe(false);
    });
});

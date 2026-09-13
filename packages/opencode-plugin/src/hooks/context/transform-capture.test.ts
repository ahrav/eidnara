import { describe, expect, it, mock, spyOn } from "bun:test";
import {
    assertCapturedMessagesUnchanged,
    assertReferenceableMessages,
    type CapturedMessages,
    captureMessages,
    readOwnDataProperty,
    rootArrayRejection,
    SourceRejected,
    SourceWalkLimitExceeded,
    snapshotFieldsEqual,
} from "./transform-capture";

function referenceableMessages(messages: unknown): boolean {
    try {
        assertReferenceableMessages(messages);
        return true;
    } catch (error) {
        if (error instanceof SourceRejected) return false;
        throw error;
    }
}

function capturedMessagesUnchanged(live: unknown, captured: CapturedMessages): boolean {
    try {
        assertCapturedMessagesUnchanged(live, captured);
        return true;
    } catch (error) {
        if (error instanceof SourceRejected) return false;
        throw error;
    }
}

describe("referenceable source guard", () => {
    it("does not read an inherited tape slot when a live array grows", () => {
        const member = [0];
        const source = [member];
        const captured = captureMessages(source);
        Object.assign(member, { extra: "added" });
        const key = String(captured.snapshots[0].fields.length);
        const saved = Object.getOwnPropertyDescriptor(Array.prototype, key);
        let calls = 0;
        let unchanged = true;
        try {
            Object.defineProperty(Array.prototype, key, {
                configurable: true,
                get: () => {
                    calls += 1;
                    return "added";
                },
            });
            unchanged = capturedMessagesUnchanged(source, captured);
        } finally {
            if (saved) Object.defineProperty(Array.prototype, key, saved);
            else Reflect.deleteProperty(Array.prototype, key);
        }
        expect(unchanged).toBe(false);
        expect(calls).toBe(0);
    });

    it("compares root bookkeeping values, keys, order and descriptors", () => {
        const member = { text: "same" };
        const source = Object.assign([member], { foo: 1, bar: "two" });
        const captured = captureMessages(source);
        const variants = [
            Object.assign([member], { foo: 2, bar: "two" }),
            Object.assign([member], { foo: "1", bar: "two" }),
            Object.assign([member], { other: 1, bar: "two" }),
            Object.assign([member], { bar: "two", foo: 1 }),
            Object.defineProperty(Object.assign([member], { foo: 1, bar: "two" }), "foo", {
                enumerable: false,
            }),
            Object.defineProperty(Object.assign([member], { foo: 1, bar: "two" }), "foo", {
                writable: false,
            }),
            Object.defineProperty(Object.assign([member], { foo: 1, bar: "two" }), "foo", {
                configurable: false,
            }),
        ];
        expect(capturedMessagesUnchanged(source, captured)).toBe(true);
        for (const changed of variants) {
            expect(changed[0]).toBe(member);
            expect(capturedMessagesUnchanged(changed, captured)).toBe(false);
        }
    });

    it("distinguishes array extra names and the array that owns them", () => {
        const left = Object.assign([], { foo: 1 });
        const renamed = Object.assign([], { bar: 1 });
        const outer = Object.assign([[]], { extra: 1 });
        const inner = [Object.assign([], { extra: 1 })];
        expect(Object.keys(left)).toEqual(["foo"]);
        expect(Object.keys(renamed)).toEqual(["bar"]);
        expect(Object.keys(outer)).toEqual(["0", "extra"]);
        expect(Object.keys(inner[0])).toEqual(["extra"]);
        for (const [before, after] of [
            [left, renamed],
            [outer, inner],
        ]) {
            expect(
                snapshotFieldsEqual(
                    captureMessages([before]).snapshots[0],
                    captureMessages([after]).snapshots[0],
                ),
            ).toBe(false);
        }
        const source = [Object.assign([], { foo: 1 })];
        const captured = captureMessages(source);
        Reflect.deleteProperty(source[0], "foo");
        Object.assign(source[0], { bar: 1 });
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
    });

    it("rejects proxies and revoked roots without reflective traps", () => {
        const trap = mock(() => {
            throw new Error("source trap ran");
        });
        const live = [{ parts: [{ text: "hello" }] }];
        const captured = captureMessages(live);
        const handlers = {
            get: trap,
            ownKeys: trap,
            getOwnPropertyDescriptor: trap,
            getPrototypeOf: trap,
        };
        const revoked = Proxy.revocable(live, {});
        revoked.revoke();
        for (const source of [
            new Proxy(live, handlers),
            revoked.proxy,
            [new Proxy(live[0], handlers)],
        ]) {
            expect(referenceableMessages(source)).toBe(false);
            expect(() => captureMessages(source)).toThrow();
            expect(capturedMessagesUnchanged(source, captured)).toBe(false);
        }
        expect(trap).not.toHaveBeenCalled();
    });

    it.each([true, false])("rejects accessors with enumerable=%s", (enumerable) => {
        for (const key of ["info", "parts", "unknown", "toJSON"]) {
            const source = [{ ordinary: "value" }];
            const captured = captureMessages(source);
            const trap = mock(() => "value");
            Object.defineProperty(source[0], key, { get: trap, enumerable });
            expect(referenceableMessages(source)).toBe(false);
            expect(() => captureMessages(source)).toThrow();
            expect(capturedMessagesUnchanged(source, captured)).toBe(false);
            expect(trap).not.toHaveBeenCalled();
        }
    });

    it.each([
        "0",
        "map",
        "filter",
        "slice",
        "constructor",
        "then",
        Symbol.iterator,
    ])("rejects array read overrides: %s", (key) => {
        const source = ["value"];
        const captured = captureMessages(source);
        const trap = mock(() => "value");
        Object.defineProperty(source, key, { get: trap });
        expect(referenceableMessages(source)).toBe(false);
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
        expect(trap).not.toHaveBeenCalled();
    });

    it("rejects an inherited then on the root array without calling it", () => {
        const key = "then";
        const trap = mock(() => undefined);
        const source = [{ text: "hello" }];
        const captured = captureMessages(source);
        const saved = Object.getOwnPropertyDescriptor(Array.prototype, key);
        let valid = true;
        let unchanged = true;
        let reason: string | undefined;
        try {
            Object.defineProperty(Array.prototype, key, { get: trap, configurable: true });
            valid = referenceableMessages(source);
            unchanged = capturedMessagesUnchanged(source, captured);
            reason = rootArrayRejection(source);
        } finally {
            if (saved) Object.defineProperty(Array.prototype, key, saved);
            else Reflect.deleteProperty(Array.prototype, key);
        }
        expect(valid).toBe(false);
        expect(unchanged).toBe(false);
        expect(reason).toBe("then property on root array");
        expect(rootArrayRejection(source)).toBeUndefined();
        expect(rootArrayRejection(new Proxy(source, {}))).toBe("proxy root array");
        expect(rootArrayRejection({ length: 0 })).toBe("root is not an array");
        expect(rootArrayRejection(Object.defineProperty([], key, { value: 1 }))).toBe(
            "then property on root array",
        );
        expect(trap).not.toHaveBeenCalled();
    });

    it("rejects own and inherited serialization hooks without calling them", () => {
        const trap = mock(() => "serialized");
        for (const prototype of [Object.prototype, Array.prototype]) {
            const source = [{ text: "hello" }];
            const captured = captureMessages(source);
            const saved = Object.getOwnPropertyDescriptor(prototype, "toJSON");
            let valid = true;
            let unchanged = true;
            try {
                Object.defineProperty(prototype, "toJSON", { get: trap, configurable: true });
                valid = referenceableMessages(source);
                unchanged = capturedMessagesUnchanged(source, captured);
            } finally {
                if (saved) Object.defineProperty(prototype, "toJSON", saved);
                else Reflect.deleteProperty(prototype, "toJSON");
            }
            expect(valid).toBe(false);
            expect(unchanged).toBe(false);
        }
        for (const enumerable of [true, false]) {
            for (const target of [{}, []]) {
                Object.defineProperty(target, "toJSON", { value: trap, enumerable });
                expect(referenceableMessages([target])).toBe(false);
            }
        }
        expect(trap).not.toHaveBeenCalled();
    });

    it("rejects symbol-keyed accessors and coercion callbacks without invocation", () => {
        const trap = mock(() => "value");
        for (const key of [Symbol("hidden"), Symbol.toPrimitive, Symbol.iterator]) {
            const source = [{ visible: "data" }];
            const captured = captureMessages(source);
            Object.defineProperty(source[0], key, { get: trap });
            expect(referenceableMessages(source)).toBe(false);
            expect(capturedMessagesUnchanged(source, captured)).toBe(false);
        }
        expect(referenceableMessages([{ [Symbol.toPrimitive]: trap }])).toBe(false);
        expect(trap).not.toHaveBeenCalled();
    });

    it("rejects unsupported values, sparse arrays, cycles and excessive depth", () => {
        const cyclic: unknown[] = [];
        cyclic.push(cyclic);
        let deep: unknown = null;
        for (let index = 0; index < 300; index += 1) deep = [deep];
        class Unsupported {}
        for (const value of [
            undefined,
            NaN,
            Infinity,
            -Infinity,
            1n,
            Symbol("value"),
            () => "value",
            new Unsupported(),
            new Date(),
            Object.create({ inherited: "value" }),
            new Array(2),
            cyclic,
            deep,
        ]) {
            expect(referenceableMessages([value])).toBe(false);
            expect(() => captureMessages([value])).toThrow();
        }
    });

    it("accepts readonly dense input, shared subtrees, hidden data and omitted fields", () => {
        const shared = { text: "hello", optional: undefined, nested: [null, 1, true] };
        const object = Object.assign(Object.create(null), { left: shared, right: shared });
        Object.defineProperty(object, "hidden", { value: { count: 1 }, configurable: true });
        const source = [object, shared];
        Object.assign(source, { bookkeeping: "harmless" });
        Object.freeze(source);
        const captured = captureMessages(source);
        expect(referenceableMessages(source)).toBe(true);
        expect(captured.members[0]).toBe(object);
        expect(captured.members[1]).toBe(shared);
        expect(capturedMessagesUnchanged(source, captured)).toBe(true);
        expect(Object.isFrozen(shared)).toBe(false);
        expect(JSON.stringify(source)).not.toContain("hidden");
        Object.defineProperty(object, "hidden", { value: { count: 2 } });
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
        expect(
            snapshotFieldsEqual(
                captureMessages([{ a: undefined }]).snapshots[0],
                captureMessages([{}]).snapshots[0],
            ),
        ).toBe(true);
    });

    it("distinguishes types, keys, order and counts with exact fields", () => {
        const values = [
            null,
            true,
            false,
            0,
            -0,
            1,
            "",
            "null",
            [],
            {},
            [null],
            { a: 1 },
            { b: 1 },
        ];
        for (const left of values) {
            const snapshot = captureMessages([left]).snapshots[0];
            for (const right of values) {
                expect(snapshotFieldsEqual(snapshot, captureMessages([right]).snapshots[0])).toBe(
                    Object.is(left, right),
                );
            }
        }
        const source = [{ a: 1, b: "x" }];
        const captured = captureMessages(source);
        for (const next of [
            { b: "x", a: 1 },
            { a: 1, b: "x", c: null },
            { a: "1", b: "x" },
        ]) {
            expect(
                snapshotFieldsEqual(captured.snapshots[0], captureMessages([next]).snapshots[0]),
            ).toBe(false);
        }
        source[0] = { a: 1, b: "x" };
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
    });

    it("detects nested edits and removal without serializing source objects", () => {
        const source = [{ nested: { list: ["a", "b"], value: null } }];
        const captured = captureMessages(source);
        source[0].nested.list[0] = "changed";
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
        source[0].nested.list[0] = "a";
        expect(capturedMessagesUnchanged(source, captured)).toBe(true);
        source[0].nested.list.pop();
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
        source.pop();
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
    });

    it("bounds cumulative work, large sparse roots and shared-reference amplification", () => {
        const source = [{ text: "x".repeat(20 * 1024 * 1024) }];
        const captured = captureMessages(source);
        for (let index = 0; index < 6; index += 1) source.push(source[0]);
        expect(() => captureMessages(source)).toThrow(SourceWalkLimitExceeded);
        expect(referenceableMessages(source)).toBe(false);
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
        let shared: unknown = null;
        for (let index = 0; index < 25; index += 1) shared = { left: shared, right: shared };
        expect(() => captureMessages([shared])).toThrow(SourceWalkLimitExceeded);
        expect(() => captureMessages(new Array(2 ** 32 - 1))).toThrow(SourceWalkLimitExceeded);
    });

    it("accepts a metadata-heavy history of short messages within the encoded-output limit", () => {
        // Persisted OpenCode rows carry many short identifier, timestamp and token fields per
        // message, so descriptor charges dominate string bytes; this shape must not trip the walk.
        const sessionID = "ses_0123456789abcdefghijkl";
        const message = (index: number) => ({
            info: {
                id: `msg_${String(index).padStart(24, "0")}`,
                sessionID,
                role: index % 2 ? "assistant" : "user",
                time: { created: 1_700_000_000_000 + index, completed: 1_700_000_000_500 + index },
                ...(index % 2
                    ? {
                          providerID: "anthropic",
                          modelID: "claude-sonnet-4",
                          mode: "build",
                          path: { cwd: "/home/user/project", root: "/home/user/project" },
                          cost: 0.0123,
                          tokens: {
                              input: 1200,
                              output: 300,
                              reasoning: 0,
                              cache: { read: 1000, write: 0 },
                          },
                          system: [],
                      }
                    : {}),
                agent: "build",
            },
            parts: [
                {
                    id: `prt_${index}a`,
                    sessionID,
                    messageID: `msg_${index}`,
                    type: "text",
                    text: "y".repeat(200),
                    time: { start: 1, end: 2 },
                },
                ...(index % 2
                    ? [
                          {
                              id: `prt_${index}b`,
                              sessionID,
                              messageID: `msg_${index}`,
                              type: "tool",
                              callID: `call_${index}`,
                              tool: "read",
                              state: {
                                  status: "completed",
                                  input: { filePath: "/home/user/project/src/a.ts" },
                                  output: "z".repeat(200),
                                  title: "a.ts",
                                  metadata: { preview: "p".repeat(64), truncated: false },
                                  time: { start: 1, end: 2 },
                              },
                          },
                      ]
                    : []),
            ],
        });
        const source = Array.from({ length: 12_000 }, (_, index) => message(index));
        const jsonBytes = Buffer.byteLength(JSON.stringify(source));
        expect(jsonBytes).toBeGreaterThan(8 * 1024 * 1024);
        expect(jsonBytes).toBeLessThan(16 * 1024 * 1024);
        const captured = captureMessages(source);
        expect(captured.members).toHaveLength(12_000);
        expect(capturedMessagesUnchanged(source, captured)).toBe(true);
    });

    it("visits source leaves once during capture and once during recheck", () => {
        const leaf = { ordinary: "value" };
        const source = [{ branch: [leaf] }];
        const descriptor = Object.getOwnPropertyDescriptor;
        let reads = 0;
        const spy = spyOn(Object, "getOwnPropertyDescriptor").mockImplementation((object, key) => {
            if (object === leaf && key === "ordinary") reads += 1;
            return descriptor(object, key);
        });
        try {
            const captured = captureMessages(source);
            expect(reads).toBe(1);
            reads = 0;
            expect(capturedMessagesUnchanged(source, captured)).toBe(true);
            expect(reads).toBe(1);
        } finally {
            spy.mockRestore();
        }
    });

    it("reads roots and indexes through own data descriptors", () => {
        const trap = mock(() => "read");
        const value = {};
        Object.defineProperty(value, "messages", { get: trap, enumerable: true });
        expect(readOwnDataProperty(value, "messages")).toBeUndefined();
        expect(readOwnDataProperty(new Proxy({}, { get: trap }), "messages")).toBeUndefined();
        expect(readOwnDataProperty(Object.create({ messages: [] }), "messages")).toBeUndefined();
        expect(readOwnDataProperty(["ok"], "0")).toBe("ok");
        expect(readOwnDataProperty(["ok"], "length")).toBe(1);
        expect(
            readOwnDataProperty(Object.defineProperty({}, "info", { value: "hidden" }), "info"),
        ).toBe("hidden");
        expect(trap).not.toHaveBeenCalled();
    });
});

import { afterEach, describe, expect, it, spyOn } from "bun:test";
import {
    CaptureBudgetExceeded,
    type CapturedMessages,
    type CaptureLease,
    capturedMessagesUnchanged,
    captureMessages as captureWithLease,
    defaultTransformCaptureAdmission,
    hostArrayReplacementRejection,
    inspectReferenceableMessages,
    type MessageContentSnapshot,
    readOwnDataProperty,
    replaceHostArrayContents,
    rootArrayRejection,
    snapshotFieldsEqual,
    TransformCaptureAdmission,
} from "./transform-capture";

let captureLease: CaptureLease | undefined;
afterEach(() => {
    captureLease?.release();
    captureLease = undefined;
    expect(defaultTransformCaptureAdmission.activePasses).toBe(0);
    expect(defaultTransformCaptureAdmission.chargedBytes).toBe(0);
});

function reserveCapture(messages: unknown): CaptureLease {
    if (!captureLease) {
        const admitted = new TransformCaptureAdmission().admit("fixture");
        if (!("lease" in admitted)) throw new Error("fixture admission refused");
        captureLease = admitted.lease;
    }
    const inspection = inspectReferenceableMessages(messages, captureLease.remainingBytes);
    if (!inspection.ok) throw new Error(`unsupported fixture: ${inspection.rejection.reason}`);
    if (!captureLease.reserve(inspection.estimatedBytes))
        throw new CaptureBudgetExceeded("fixture");
    return captureLease;
}

function captureMessages(messages: unknown): CapturedMessages {
    return captureWithLease(messages, reserveCapture(messages));
}

function messageContentSnapshot(message: unknown): MessageContentSnapshot {
    return captureMessages([message]).snapshots[0];
}

function messageMatchesContentSnapshot(
    message: unknown,
    snapshot: MessageContentSnapshot,
): boolean {
    return capturedMessagesUnchanged([message], {
        members: [message],
        snapshots: [snapshot],
        rootSnapshot: captureMessages([null]).rootSnapshot,
    });
}

function message(id: string, text = `text ${id}`): Record<string, unknown> {
    return { info: { id, role: "user", sessionID: "ses" }, parts: [{ type: "text", text }] };
}

/** Records every invocation so a test can prove that the guard ran none of them. */
function trapCounter(): { count: number; trap: () => never } {
    const counter = {
        count: 0,
        trap: (): never => {
            counter.count += 1;
            throw new Error("user code ran during a guard");
        },
    };
    return counter;
}

describe("referenceable JSON domain guard", () => {
    it("rejects symbol-keyed accessors and coercion callbacks without invocation", () => {
        const counter = trapCounter();
        for (const key of [Symbol("hidden"), Symbol.toPrimitive, Symbol.iterator]) {
            const source = [{ visible: "data" }];
            const captured = captureMessages(source);
            Object.defineProperty(source[0], key, { get: counter.trap });
            expect(inspectReferenceableMessages(source).ok).toBe(false);
            expect(capturedMessagesUnchanged(source, captured)).toBe(false);
        }
        expect(inspectReferenceableMessages([{ [Symbol.toPrimitive]: counter.trap }]).ok).toBe(
            false,
        );
        expect(counter.count).toBe(0);
    });

    it("accepts readonly dense input, shared subtrees, hidden data and omitted fields", () => {
        const shared = { text: "hello", optional: undefined, nested: [null, 1, true] };
        const object = Object.assign(Object.create(null), { left: shared, right: shared });
        Object.defineProperty(object, "hidden", { value: { count: 1 }, configurable: true });
        const source = Object.assign([object, shared], { bookkeeping: "harmless" });
        Object.freeze(source);
        const captured = captureMessages(source);
        expect(inspectReferenceableMessages(source).ok).toBe(true);
        expect(captured.members[0]).toBe(object);
        expect(captured.members[1]).toBe(shared);
        expect(capturedMessagesUnchanged(source, captured)).toBe(true);
        expect(Object.isFrozen(shared)).toBe(false);
        expect(JSON.stringify(source)).not.toContain("hidden");
        Object.defineProperty(object, "hidden", { value: { count: 2 } });
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
    });

    it("rejects proxy roots, including revoked arrays, before reflective traps", () => {
        const counter = trapCounter();
        const live = [message("m1")];
        const captured = captureMessages(live);
        const proxy = new Proxy(live, {
            get: counter.trap,
            ownKeys: counter.trap,
            getOwnPropertyDescriptor: counter.trap,
            getPrototypeOf: counter.trap,
            isExtensible: counter.trap,
        });
        const revoked = Proxy.revocable(live, {});
        revoked.revoke();
        for (const source of [proxy, revoked.proxy]) {
            expect(inspectReferenceableMessages(source)).toEqual({
                ok: false,
                rejection: { reason: "proxy", path: "" },
            });
            expect(() => captureMessages(source)).toThrow();
            expect(capturedMessagesUnchanged(source, captured)).toBe(false);
            expect(hostArrayReplacementRejection(source)).toBe("proxy");
        }
        expect(counter.count).toBe(0);
    });

    it.each([true, false])("rejects own array toJSON with enumerable=%s", (enumerable) => {
        for (const nested of [false, true]) {
            const counter = trapCounter();
            const array: unknown[] = ["text"];
            const live = nested ? [{ parts: array }] : array;
            const captured = captureMessages(live);
            Object.defineProperty(array, "toJSON", { value: counter.trap, enumerable });
            expect(inspectReferenceableMessages(live).ok).toBe(false);
            expect(() => captureMessages(live)).toThrow();
            expect(capturedMessagesUnchanged(live, captured)).toBe(false);
            expect(counter.count).toBe(0);
        }
    });

    it.each([
        "map",
        "slice",
        "values",
        "constructor",
        "toJSON",
        "then",
        Symbol.iterator,
    ])("rejects hidden array operation overrides: %s", (key) => {
        const counter = trapCounter();
        const live = [message("m1")];
        const captured = captureMessages(live);
        Object.defineProperty(live, key, { get: counter.trap });
        expect(inspectReferenceableMessages(live).ok).toBe(false);
        expect(() => captureMessages(live)).toThrow();
        expect(capturedMessagesUnchanged(live, captured)).toBe(false);
        expect(counter.count).toBe(0);
    });

    it.each([
        true,
        false,
    ])("rejects source membership accessors with enumerable=%s", (enumerable) => {
        const counter = trapCounter();
        const live = [message("m1")];
        const captured = captureMessages(live);
        Object.defineProperty(live, "0", { get: counter.trap, enumerable });
        expect(inspectReferenceableMessages(live)).toEqual({
            ok: false,
            rejection: { reason: "accessor", path: "/0" },
        });
        expect(() => captureMessages(live)).toThrow();
        expect(capturedMessagesUnchanged(live, captured)).toBe(false);
        expect(counter.count).toBe(0);
    });

    it.each([
        { prototype: Object.prototype },
        { prototype: Array.prototype },
    ])("rejects inherited toJSON without invoking it", ({ prototype }) => {
        const counter = trapCounter();
        const live = [message("m1")];
        const captured = captureMessages(live);
        const saved = Object.getOwnPropertyDescriptor(prototype, "toJSON");
        let rejected = false;
        let unchanged = true;
        let captureThrew = false;
        try {
            Object.defineProperty(prototype, "toJSON", { get: counter.trap, configurable: true });
            rejected = !inspectReferenceableMessages(live).ok;
            unchanged = capturedMessagesUnchanged(live, captured);
            try {
                captureMessages(live);
            } catch {
                captureThrew = true;
            }
        } finally {
            if (saved) Object.defineProperty(prototype, "toJSON", saved);
            else Reflect.deleteProperty(prototype, "toJSON");
        }
        expect(rejected).toBe(true);
        expect(unchanged).toBe(false);
        expect(captureThrew).toBe(true);
        expect(counter.count).toBe(0);
    });

    it("rejects hidden accessors without depending on consumer field names", () => {
        const counter = trapCounter();
        const entry = { visible: "value" };
        const snapshot = messageContentSnapshot(entry);
        Object.defineProperty(entry, "hidden", { get: counter.trap });
        expect(inspectReferenceableMessages([entry])).toEqual({
            ok: false,
            rejection: { reason: "accessor", path: "/0/hidden" },
        });
        expect(messageMatchesContentSnapshot(entry, snapshot)).toBe(false);
        Object.defineProperty(entry, "toJSON", { get: counter.trap });
        expect(inspectReferenceableMessages([entry]).ok).toBe(false);
        expect(counter.count).toBe(0);
    });

    it("accepts hidden own data and array bookkeeping without skipping hidden value guards", () => {
        const entry = { visible: "value" };
        Object.defineProperty(entry, "cached", { value: { count: 1 }, configurable: true });
        const source = [entry];
        Object.assign(source, { bookkeeping: "ignored by JSON" });
        const captured = captureMessages(source);
        expect(inspectReferenceableMessages(source).ok).toBe(true);
        expect(capturedMessagesUnchanged(source, captured)).toBe(true);
        expect(JSON.stringify(source)).toBe('[{"visible":"value"}]');
        Object.defineProperty(entry, "cached", { value: { count: 2 } });
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
    });

    it.each([
        "info",
        "parts",
        "text",
        "id",
        "model",
        "signature",
        "input",
        "eidnara_reduce",
    ])("rejects hidden accessors on production-read field %s", (key) => {
        const counter = trapCounter();
        const value = { ordinary: "data" };
        const source = [value];
        const captured = captureMessages(source);
        const snapshot = messageContentSnapshot(value);
        Object.defineProperty(value, key, { get: counter.trap, enumerable: false });
        expect(inspectReferenceableMessages(source)).toEqual({
            ok: false,
            rejection: { reason: "accessor", path: `/0/${key}` },
        });
        expect(() => captureMessages(source)).toThrow();
        expect(() => messageContentSnapshot(value)).toThrow();
        expect(messageMatchesContentSnapshot(value, snapshot)).toBe(false);
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
        expect(counter.count).toBe(0);
    });

    it("accepts plain objects, dense arrays, and optional undefined object fields", () => {
        const optional = message("m1") as { info: { id: string; role: string; parentID?: string } };
        optional.info.parentID = undefined;
        const result = inspectReferenceableMessages([optional, message("m2")]);
        expect(result.ok).toBe(true);
        if (result.ok) {
            expect(result.messageWireBytes).toHaveLength(2);
            expect(result.estimatedBytes).toBeGreaterThan(0);
        }
        // The undefined field is omitted the way JSON.stringify omits it.
        expect(messageMatchesContentSnapshot(message("m1"), messageContentSnapshot(optional))).toBe(
            true,
        );
    });

    it("rejects an accessor without invoking it", () => {
        const counter = trapCounter();
        const hooked = message("m1");
        Object.defineProperty(hooked, "parts", { get: counter.trap, enumerable: true });
        const result = inspectReferenceableMessages([hooked]);
        expect(result).toEqual({ ok: false, rejection: { reason: "accessor", path: "/0/parts" } });
        expect(counter.count).toBe(0);
    });

    it("rejects a proxy without running its traps", () => {
        const counter = trapCounter();
        const proxied = new Proxy(message("m1"), {
            get: counter.trap,
            ownKeys: counter.trap,
            getOwnPropertyDescriptor: counter.trap,
        });
        const outer = message("m2");
        (outer.parts as unknown[]).push(proxied);
        const result = inspectReferenceableMessages([outer]);
        expect(result).toEqual({ ok: false, rejection: { reason: "proxy", path: "/0/parts/1" } });
        expect(counter.count).toBe(0);
    });

    it("rejects toJSON hooks, class instances, functions, symbols, bigints, and non-finite numbers", () => {
        const counter = trapCounter();
        const withToJson = message("m1");
        withToJson.toJSON = counter.trap;
        expect(inspectReferenceableMessages([withToJson])).toEqual({
            ok: false,
            rejection: { reason: "function", path: "/0/toJSON" },
        });
        class Part {
            type = "text";
            toJSON = counter.trap;
        }
        const withInstance = message("m2");
        (withInstance.parts as unknown[]).push(new Part());
        expect(inspectReferenceableMessages([withInstance])).toEqual({
            ok: false,
            rejection: { reason: "prototype", path: "/0/parts/1" },
        });
        expect(inspectReferenceableMessages([{ ...message("m3"), tag: Symbol("x") }])).toEqual({
            ok: false,
            rejection: { reason: "symbol", path: "/0/tag" },
        });
        expect(inspectReferenceableMessages([{ ...message("m4"), big: 1n }])).toEqual({
            ok: false,
            rejection: { reason: "bigint", path: "/0/big" },
        });
        expect(inspectReferenceableMessages([{ ...message("m5"), nan: Number.NaN }])).toEqual({
            ok: false,
            rejection: { reason: "nonfinite_number", path: "/0/nan" },
        });
        expect(counter.count).toBe(0);
    });

    it("rejects cycles, sparse arrays, undefined elements, and excessive depth", () => {
        const cyclic = message("m1");
        (cyclic.info as Record<string, unknown>).self = cyclic;
        expect(inspectReferenceableMessages([cyclic])).toEqual({
            ok: false,
            rejection: { reason: "cycle", path: "/0/info/self" },
        });
        const sparse = message("m2");
        (sparse.parts as unknown[]).length = 3;
        expect(inspectReferenceableMessages([sparse])).toEqual({
            ok: false,
            rejection: { reason: "sparse_array", path: "/0/parts/1" },
        });
        const holed = message("m3");
        (holed.parts as unknown[]).push(undefined);
        expect(inspectReferenceableMessages([holed])).toEqual({
            ok: false,
            rejection: { reason: "undefined_element", path: "/0/parts/1" },
        });
        let deep: unknown = "leaf";
        for (let level = 0; level < 300; level += 1) deep = [deep];
        const result = inspectReferenceableMessages([{ ...message("m4"), deep }]);
        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.rejection.reason).toBe("depth");
    });

    it("reads own data properties without touching accessors or proxies", () => {
        const counter = trapCounter();
        const hooked: Record<string, unknown> = {};
        Object.defineProperty(hooked, "info", { get: counter.trap, enumerable: true });
        expect(readOwnDataProperty(hooked, "info")).toBeUndefined();
        expect(readOwnDataProperty(new Proxy({}, { get: counter.trap }), "info")).toBeUndefined();
        expect(readOwnDataProperty({ info: { id: "x" } }, "info")).toEqual({ id: "x" });
        expect(readOwnDataProperty(Object.create({ info: "inherited" }), "info")).toBeUndefined();
        expect(readOwnDataProperty(["ok"], "length")).toBe(1);
        expect(
            readOwnDataProperty(Object.defineProperty({}, "info", { value: "hidden" }), "info"),
        ).toBe("hidden");
        expect(counter.count).toBe(0);
    });
});

describe("built-in prototype scan", () => {
    const referenceable = (source: unknown): boolean => inspectReferenceableMessages(source).ok;

    it("rejects an accessor inherited from Object.prototype without calling it", () => {
        const key = "agent";
        const counter = trapCounter();
        const source = [{ info: { role: "user" } }];
        const captured = captureMessages(source);
        const saved = Object.getOwnPropertyDescriptor(Object.prototype, key);
        let valid = true;
        let unchanged = true;
        let rejection: ReturnType<typeof rootArrayRejection>;
        try {
            Object.defineProperty(Object.prototype, key, { get: counter.trap, configurable: true });
            valid = referenceable(source);
            unchanged = capturedMessagesUnchanged(source, captured);
            rejection = rootArrayRejection(source);
        } finally {
            if (saved) Object.defineProperty(Object.prototype, key, saved);
            else Reflect.deleteProperty(Object.prototype, key);
        }
        expect(valid).toBe(false);
        expect(unchanged).toBe(false);
        expect(rejection).toEqual({ reason: "prototype_accessor", path: "Object.prototype/agent" });
        expect(rootArrayRejection(source)).toBeUndefined();
        expect(counter.count).toBe(0);
    });

    it("rejects an Array.prototype iterator accessor without calling it", () => {
        const key = Symbol.iterator;
        const original = Object.getOwnPropertyDescriptor(Array.prototype, key)!;
        const counter = trapCounter();
        const source = [{ text: "hello" }];
        let rejection: ReturnType<typeof rootArrayRejection>;
        try {
            Object.defineProperty(Array.prototype, key, { get: counter.trap, configurable: true });
            rejection = rootArrayRejection(source);
        } finally {
            Object.defineProperty(Array.prototype, key, original);
        }
        expect(rejection).toEqual({
            reason: "prototype_accessor",
            path: "Array.prototype/Symbol(Symbol.iterator)",
        });
        expect(counter.count).toBe(0);
    });

    it("rejects an Object.prototype value accessor alongside a source accessor without calling either", () => {
        const protoCounter = trapCounter();
        const sourceCounter = trapCounter();
        const source = [{ info: { role: "user" } }];
        Object.defineProperty(source[0], "parts", { get: sourceCounter.trap, enumerable: true });
        const saved = Object.getOwnPropertyDescriptor(Object.prototype, "value");
        let valid = true;
        let hidden: unknown = "unset";
        try {
            Object.defineProperty(Object.prototype, "value", {
                get: protoCounter.trap,
                configurable: true,
            });
            valid = referenceable(source);
            hidden = readOwnDataProperty(source[0], "parts");
        } finally {
            if (saved) Object.defineProperty(Object.prototype, "value", saved);
            else Reflect.deleteProperty(Object.prototype, "value");
        }
        expect(valid).toBe(false);
        expect(hidden).toBeUndefined();
        expect(protoCounter.count).toBe(0);
        expect(sourceCounter.count).toBe(0);
    });

    it("rejects prototype-reset boxed primitives whose tapes cannot distinguish their values", () => {
        const boxed = (value: boolean): object => Object.setPrototypeOf(new Boolean(value), null);
        const source = [{ flag: boxed(true) }];
        expect(JSON.stringify(boxed(true))).not.toBe(JSON.stringify(boxed(false)));
        expect(inspectReferenceableMessages(source)).toEqual({
            ok: false,
            rejection: { reason: "boxed_primitive", path: "/0/flag" },
        });
    });

    it("rejects a String.prototype accessor without calling it", () => {
        const key = "polluted";
        const counter = trapCounter();
        const source = [{ url: "file:///tmp/a.png" }];
        let rejection: ReturnType<typeof rootArrayRejection>;
        try {
            Object.defineProperty(String.prototype, key, { get: counter.trap, configurable: true });
            rejection = rootArrayRejection(source);
        } finally {
            Reflect.deleteProperty(String.prototype, key);
        }
        expect(rejection).toEqual({
            reason: "prototype_accessor",
            path: "String.prototype/polluted",
        });
        expect(counter.count).toBe(0);
    });

    it("records whether a nested object has a null prototype", () => {
        const info = { role: "user" };
        const source = [{ info }];
        const captured = captureMessages(source);
        Object.setPrototypeOf(info, null);
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
        Object.setPrototypeOf(info, Object.prototype);
        expect(capturedMessagesUnchanged(source, captured)).toBe(true);
    });

    it("rejects an inherited then on the root array without calling it", () => {
        const key = "then";
        const counter = trapCounter();
        const source = [{ text: "hello" }];
        const captured = captureMessages(source);
        const saved = Object.getOwnPropertyDescriptor(Array.prototype, key);
        let valid = true;
        let unchanged = true;
        let rejection: ReturnType<typeof rootArrayRejection>;
        try {
            Object.defineProperty(Array.prototype, key, { get: counter.trap, configurable: true });
            valid = referenceable(source);
            unchanged = capturedMessagesUnchanged(source, captured);
            rejection = rootArrayRejection(source);
        } finally {
            if (saved) Object.defineProperty(Array.prototype, key, saved);
            else Reflect.deleteProperty(Array.prototype, key);
        }
        expect(valid).toBe(false);
        expect(unchanged).toBe(false);
        expect(rejection).toEqual({ reason: "extra_property", path: "/then" });
        expect(rootArrayRejection(source)).toBeUndefined();
        expect(rootArrayRejection(new Proxy(source, {}))).toEqual({ reason: "proxy", path: "" });
        expect(rootArrayRejection({ length: 0 })).toEqual({ reason: "not_array", path: "" });
        expect(counter.count).toBe(0);
    });

    it("accepts a metadata-heavy history of short messages within the walk budget", () => {
        // Persisted OpenCode rows carry many short identifier, timestamp and token fields per
        // message, so per-slot charges dominate string bytes; this shape must not trip the walk.
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
        const inspection = inspectReferenceableMessages(source);
        if (!inspection.ok) throw new Error("valid history rejected");
        // Short fields make slot charges dominate; the charge still stays within a small multiple of the JSON size.
        expect(inspection.estimatedBytes).toBeLessThan(5 * jsonBytes);
        const captured = captureMessages(source);
        expect(captured.members).toHaveLength(12_000);
        expect(capturedMessagesUnchanged(source, captured)).toBe(true);
    });
});

describe("capture snapshots and rechecks", () => {
    it("does not read an inherited tape slot when a live array grows", () => {
        const member = [0];
        const source = [member];
        const captured = captureMessages(source);
        Object.assign(member, { extra: "added" });
        const key = String(captured.snapshots[0].fields.length);
        const saved = Object.getOwnPropertyDescriptor(Array.prototype, key);
        const counter = trapCounter();
        let unchanged = true;
        try {
            Object.defineProperty(Array.prototype, key, { configurable: true, get: counter.trap });
            unchanged = capturedMessagesUnchanged(source, captured);
        } finally {
            if (saved) Object.defineProperty(Array.prototype, key, saved);
            else Reflect.deleteProperty(Array.prototype, key);
        }
        expect(unchanged).toBe(false);
        expect(counter.count).toBe(0);
    });

    it.each(["root", "message"])("checks %s tape bounds before indexing", (which) => {
        const source = [[0]];
        const captured = captureMessages(source);
        const fields = (which === "root" ? captured.rootSnapshot : captured.snapshots[0]).fields;
        fields.pop();
        const key = String(fields.length);
        const saved = Object.getOwnPropertyDescriptor(Array.prototype, key);
        const counter = trapCounter();
        let unchanged = true;
        try {
            Object.defineProperty(Array.prototype, key, { configurable: true, get: counter.trap });
            unchanged = capturedMessagesUnchanged(source, captured);
        } finally {
            if (saved) Object.defineProperty(Array.prototype, key, saved);
            else Reflect.deleteProperty(Array.prototype, key);
        }
        expect(unchanged).toBe(false);
        expect(counter.count).toBe(0);
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
        for (const [before, after] of [
            [left, renamed],
            [outer, inner],
        ]) {
            expect(
                snapshotFieldsEqual(messageContentSnapshot(before), messageContentSnapshot(after)),
            ).toBe(false);
        }
        const source = [Object.assign([], { foo: 1 })];
        const captured = captureMessages(source);
        Reflect.deleteProperty(source[0], "foo");
        Object.assign(source[0], { bar: 1 });
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
    });

    it("visits each source leaf once in capture and once in recheck", () => {
        const leaf = { ordinary: "data" };
        const source = [{ branch: [leaf] }];
        const lease = reserveCapture(source);
        const descriptor = Object.getOwnPropertyDescriptor;
        let reads = 0;
        const spy = spyOn(Object, "getOwnPropertyDescriptor").mockImplementation((object, key) => {
            if (object === leaf && key === "ordinary") reads += 1;
            return descriptor(object, key);
        });
        try {
            const captured = captureWithLease(source, lease);
            expect(reads).toBe(1);
            reads = 0;
            expect(capturedMessagesUnchanged(source, captured)).toBe(true);
            expect(reads).toBe(1);
            expect(Object.getPrototypeOf(captured.snapshots[0].fields)).toBe(Array.prototype);
        } finally {
            spy.mockRestore();
        }
    });

    it("accepts shared acyclic references and retains source identity without freezing", () => {
        const shared = { data: ["shared", null, false] };
        const live = [{ left: shared, right: shared }, shared];
        const captured = captureMessages(live);
        expect(captured.members[0]).toBe(live[0]);
        expect(captured.members[1]).toBe(shared);
        expect(Object.isFrozen(live)).toBe(false);
        expect(Object.isFrozen(shared)).toBe(false);
        expect(capturedMessagesUnchanged(live, captured)).toBe(true);
        shared.data[0] = "changed";
        expect(capturedMessagesUnchanged(live, captured)).toBe(false);
    });

    it("distinguishes generated scalar, container, key, and ordering cases", () => {
        const values: unknown[] = [
            null,
            true,
            false,
            0,
            -0,
            1,
            "",
            "null",
            "object",
            [],
            {},
            [null],
            { a: 1 },
            { b: 1 },
            { a: undefined },
        ];
        for (const left of values) {
            const snapshot = messageContentSnapshot(left);
            for (const right of values) {
                const same =
                    Object.is(left, right) ||
                    (JSON.stringify(left) === "{}" && JSON.stringify(right) === "{}");
                expect(messageMatchesContentSnapshot(right, snapshot)).toBe(same);
                expect(snapshotFieldsEqual(snapshot, messageContentSnapshot(right))).toBe(same);
            }
        }
    });

    it("detects membership changes and in-place content edits after capture", () => {
        const first = message("m1");
        const second = message("m2");
        const live = [first, second];
        const captured = captureMessages(live);
        expect(capturedMessagesUnchanged(live, captured)).toBe(true);
        (second.parts as Array<{ text: string }>)[0].text = "edited";
        expect(capturedMessagesUnchanged(live, captured)).toBe(false);
        (second.parts as Array<{ text: string }>)[0].text = "text m2";
        expect(capturedMessagesUnchanged(live, captured)).toBe(true);
        live.push(message("m3"));
        expect(capturedMessagesUnchanged(live, captured)).toBe(false);
        live.pop();
        live[0] = message("m1");
        expect(capturedMessagesUnchanged(live, captured)).toBe(false);
    });

    it("fails the recheck without running an accessor installed after capture", () => {
        const counter = trapCounter();
        const entry = message("m1");
        const live = [entry];
        const captured = captureMessages(live);
        Object.defineProperty(entry, "parts", { get: counter.trap, enumerable: true });
        expect(capturedMessagesUnchanged(live, captured)).toBe(false);
        expect(counter.count).toBe(0);
    });

    it("distinguishes key order, entry counts, and value types", () => {
        const base = messageContentSnapshot({ a: 1, b: "x" });
        expect(messageMatchesContentSnapshot({ b: "x", a: 1 }, base)).toBe(false);
        expect(messageMatchesContentSnapshot({ a: 1, b: "x", c: null }, base)).toBe(false);
        expect(messageMatchesContentSnapshot({ a: "1", b: "x" }, base)).toBe(false);
        expect(messageMatchesContentSnapshot({ a: 1, b: "x" }, base)).toBe(true);
    });
});

describe("host array replacement contract", () => {
    it.each([
        { prototype: Array.prototype },
        { prototype: Object.prototype },
    ])("refuses a numeric accessor on a built-in prototype and defines slots without invoking it", ({
        prototype,
    }) => {
        const counter = trapCounter();
        const target: unknown[] = [];
        const next = [message("m1"), message("m2"), message("m3")];
        const saved = Object.getOwnPropertyDescriptor(prototype, "0");
        let inspection: ReturnType<typeof inspectReferenceableMessages> | undefined;
        let hostRejection: ReturnType<typeof hostArrayReplacementRejection> | undefined;
        try {
            Object.defineProperty(prototype, "0", {
                set: counter.trap,
                get: counter.trap,
                configurable: true,
            });
            inspection = inspectReferenceableMessages(next);
            hostRejection = hostArrayReplacementRejection(target);
            // Own-slot definitions never consult inherited accessors.
            replaceHostArrayContents(target, next);
        } finally {
            if (saved) Object.defineProperty(prototype, "0", saved);
            else Reflect.deleteProperty(prototype, "0");
        }
        const name = prototype === Array.prototype ? "Array" : "Object";
        expect(inspection).toEqual({
            ok: false,
            rejection: { reason: "prototype_accessor", path: `${name}.prototype/0` },
        });
        expect(hostRejection).toBe("prototype_accessor");
        expect(target).toEqual(next);
        expect(target[0]).toBe(next[0]);
        expect(counter.count).toBe(0);
    });

    it("reports a destination slot that stopped accepting writes and reads no candidate getter", () => {
        const counter = trapCounter();
        const target: unknown[] = ["old0", "old1", "old2"];
        expect(hostArrayReplacementRejection(target)).toBeNull();
        Object.defineProperty(target, "2", { writable: false, configurable: false });
        expect(hostArrayReplacementRejection(target)).toBe("element_not_writable");
        const source = ["new0", "new1"];
        Object.defineProperty(source, "1", { get: counter.trap, enumerable: true });
        expect(inspectReferenceableMessages(source)).toEqual({
            ok: false,
            rejection: { reason: "accessor", path: "/1" },
        });
        expect(counter.count).toBe(0);
    });

    it("replaces every slot and the length of an accepted destination in place", () => {
        const target: unknown[] = ["old0", "old1", "old2"];
        const kept = { id: "kept" };
        replaceHostArrayContents(target, [kept, "new1"]);
        expect(target).toEqual([kept, "new1"]);
        expect(target[0]).toBe(kept);
        replaceHostArrayContents(target, []);
        expect(target).toEqual([]);
        expect(Object.getOwnPropertyDescriptor(target, "length")?.writable).toBe(true);
    });

    it("rejects inherited membership and does not consult its getter", () => {
        const counter = trapCounter();
        const live = [message("m1")];
        const captured = captureMessages(live);
        Reflect.deleteProperty(live, "0");
        const saved = Object.getOwnPropertyDescriptor(Array.prototype, "0");
        let unchanged = true;
        let rejected = false;
        try {
            Object.defineProperty(Array.prototype, "0", { get: counter.trap, configurable: true });
            unchanged = capturedMessagesUnchanged(live, captured);
            rejected = !inspectReferenceableMessages(live).ok;
        } finally {
            if (saved) Object.defineProperty(Array.prototype, "0", saved);
            else Reflect.deleteProperty(Array.prototype, "0");
        }
        expect(unchanged).toBe(false);
        expect(rejected).toBe(true);
        expect(counter.count).toBe(0);
    });

    it("accepts a plain extensible array and replaces its contents in place", () => {
        const target: unknown[] = [1, 2, 3];
        expect(hostArrayReplacementRejection(target)).toBeNull();
        replaceHostArrayContents(target, ["a", "b"]);
        expect(target).toEqual(["a", "b"]);
        replaceHostArrayContents(target, ["a", "b", "c", "d"]);
        expect(target).toEqual(["a", "b", "c", "d"]);
    });

    it("rejects containers whose element or length assignment could throw", () => {
        expect(hostArrayReplacementRejection({ length: 0 })).toBe("not_array");
        expect(hostArrayReplacementRejection(new Proxy([], {}))).toBe("proxy");
        expect(hostArrayReplacementRejection(Object.freeze([1]))).toBe("not_extensible");
        const sealedLength: unknown[] = [1];
        Object.defineProperty(sealedLength, "length", { writable: false });
        expect(hostArrayReplacementRejection(sealedLength)).toBe("length_not_writable");
        const readOnlyElement: unknown[] = [1, 2];
        Object.defineProperty(readOnlyElement, 0, { writable: false });
        expect(hostArrayReplacementRejection(readOnlyElement)).toBe("element_not_writable");
        class Sub extends Array {}
        expect(hostArrayReplacementRejection(new Sub())).toBe("prototype");
    });
});

describe("capture admission", () => {
    it("keeps counts and charges equal to a reference model across generated lease sequences", () => {
        // Seeded LCG so a failure names a reproducible sequence.
        const seed = Date.now() % 2 ** 31;
        let state = seed;
        const next = (bound: number): number => {
            state = (state * 1_103_515_245 + 12_345) % 2 ** 31;
            return state % bound;
        };
        const sessions = ["a", "b", "c", "d"];
        const maxPasses = 3;
        const maxBytes = 1000;
        for (let sequence = 0; sequence < 200; sequence += 1) {
            const owner = new TransformCaptureAdmission({ maxPasses, maxBytes });
            const model = new Map<string, number>();
            const live = new Map<string, CaptureLease>();
            const stale: CaptureLease[] = [];
            const trace: string[] = [];
            const check = (): void => {
                const total = [...model.values()].reduce((sum, bytes) => sum + bytes, 0);
                const detail = `seed=${seed} sequence=${sequence} trace=${trace.join(" ")}`;
                expect([owner.activePasses, owner.chargedBytes, detail]).toEqual([
                    model.size,
                    total,
                    detail,
                ]);
                expect(owner.chargedBytes).toBeLessThanOrEqual(maxBytes);
                expect(owner.activePasses).toBeLessThanOrEqual(maxPasses);
            };
            for (let step = 0; step < 40; step += 1) {
                const session = sessions[next(sessions.length)];
                const op = next(5);
                if (op === 0) {
                    trace.push(`admit(${session})`);
                    const admitted = owner.admit(session);
                    if (model.has(session)) {
                        expect(admitted).toEqual({ declined: "session_busy" });
                        expect(live.get(session)?.signal.aborted).toBe(true);
                    } else if (model.size >= maxPasses) {
                        expect(admitted).toEqual({ declined: "pass_count" });
                    } else {
                        if (!("lease" in admitted))
                            throw new Error(`admit refused: ${trace.join(" ")}`);
                        live.set(session, admitted.lease);
                        model.set(session, 0);
                    }
                } else if (op === 1) {
                    const bytes = next(maxBytes + 2);
                    trace.push(`reserve(${session},${bytes})`);
                    const lease = live.get(session);
                    if (!lease) continue;
                    const total = [...model.values()].reduce((sum, held) => sum + held, 0);
                    const granted = lease.reserve(bytes);
                    expect(granted).toBe(bytes <= maxBytes - total);
                    if (granted) model.set(session, (model.get(session) ?? 0) + bytes);
                } else if (op === 2) {
                    trace.push(`release(${session})`);
                    const lease = live.get(session);
                    if (!lease) continue;
                    lease.release();
                    live.delete(session);
                    model.delete(session);
                    stale.push(lease);
                } else if (op === 3) {
                    trace.push(`stale(${session})`);
                    const lease = stale[next(stale.length + 1)];
                    if (!lease) continue;
                    lease.release();
                    expect(lease.reserve(1)).toBe(false);
                    expect(lease.remainingBytes).toBe(0);
                } else {
                    trace.push(`cancel(${session})`);
                    owner.requestCancel(session, "generated");
                    expect(live.get(session)?.signal.aborted ?? true).toBe(true);
                }
                check();
            }
            for (const lease of live.values()) lease.release();
            expect(owner.activePasses).toBe(0);
            expect(owner.chargedBytes).toBe(0);
        }
    });

    it("shares the default 64-pass and 64 MiB owner across independent borrowers", () => {
        const owner = defaultTransformCaptureAdmission;
        const leases: CaptureLease[] = [];
        try {
            for (let index = 0; index < 64; index += 1) {
                const admitted = owner.admit(`capture-default-${index}`);
                if (!("lease" in admitted)) throw new Error("unexpected default refusal");
                leases.push(admitted.lease);
            }
            expect(owner.admit("capture-default-overflow")).toEqual({ declined: "pass_count" });
            expect(leases[0].remainingBytes).toBe(64 * 1024 * 1024);
            expect(leases[0].reserve(64 * 1024 * 1024)).toBe(true);
            expect(leases[1].remainingBytes).toBe(0);
            expect(leases[1].reserve(1)).toBe(false);
            owner.requestCancel("capture-default-0", "cancelled");
            expect(leases[0].signal.aborted).toBe(true);
            expect(owner.chargedBytes).toBe(64 * 1024 * 1024);
            leases[0].release();
            expect(leases[0].remainingBytes).toBe(0);
            expect(leases[1].remainingBytes).toBe(64 * 1024 * 1024);
        } finally {
            for (const lease of leases) lease.release();
        }
        expect(owner.activePasses).toBe(0);
        expect(owner.chargedBytes).toBe(0);
    });

    it("rejects invalid charges, isolates accounting, and cannot release a replacement lease", () => {
        const owner = new TransformCaptureAdmission({ maxPasses: 2, maxBytes: 1000 });
        const first = owner.admit("s");
        if (!("lease" in first)) throw new Error("unexpected refusal");
        for (const bytes of [-1, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1, 1001]) {
            expect(first.lease.reserve(bytes)).toBe(false);
            expect(owner.chargedBytes).toBe(0);
        }
        expect(first.lease.reserve(1000)).toBe(true);
        first.lease.release();
        const next = owner.admit("s");
        if (!("lease" in next)) throw new Error("unexpected refusal");
        expect(next.lease.reserve(1000)).toBe(true);
        first.lease.release();
        expect(owner.chargedBytes).toBe(1000);
        expect(owner.activePasses).toBe(1);
        const separate = new TransformCaptureAdmission({ maxPasses: 1, maxBytes: 1000 });
        separate.requestCancel("s", "not owned");
        expect(next.lease.signal.aborted).toBe(false);
        next.lease.release();
    });

    it("holds one lease per session, declines a newer call, and cancels the holder", () => {
        const admission = new TransformCaptureAdmission({ maxPasses: 2, maxBytes: 1024 });
        const first = admission.admit("s1");
        expect("lease" in first).toBe(true);
        if (!("lease" in first)) throw new Error("unreachable");
        expect(first.lease.signal.aborted).toBe(false);
        expect(admission.admit("s1")).toEqual({ declined: "session_busy" });
        expect(first.lease.signal.aborted).toBe(true);
        expect(admission.activePasses).toBe(1);
        // The declined call did not take the slot; the older owner still holds it until release.
        expect(admission.admit("s1")).toEqual({ declined: "session_busy" });
        first.lease.release();
        expect(admission.activePasses).toBe(0);
        expect("lease" in admission.admit("s1")).toBe(true);
    });

    it("declines beyond the global pass count without queueing", () => {
        const admission = new TransformCaptureAdmission({ maxPasses: 2, maxBytes: 1024 });
        const a = admission.admit("a");
        const b = admission.admit("b");
        expect(admission.admit("c")).toEqual({ declined: "pass_count" });
        if ("lease" in a) a.lease.release();
        expect("lease" in admission.admit("c")).toBe(true);
        if ("lease" in b) b.lease.release();
    });

    it("charges bytes against one aggregate budget and releases them exactly once", () => {
        const admission = new TransformCaptureAdmission({ maxPasses: 4, maxBytes: 100 });
        const a = admission.admit("a");
        const b = admission.admit("b");
        if (!("lease" in a) || !("lease" in b)) throw new Error("unreachable");
        expect(a.lease.reserve(60)).toBe(true);
        expect(b.lease.reserve(50)).toBe(false);
        expect(admission.chargedBytes).toBe(60);
        expect(b.lease.reserve(40)).toBe(true);
        expect(a.lease.reserve(1)).toBe(false);
        // Cancellation never releases a charge; only the holder's release does.
        a.lease.requestCancel("test");
        expect(admission.chargedBytes).toBe(100);
        a.lease.release();
        a.lease.release();
        expect(admission.chargedBytes).toBe(40);
        expect(a.lease.reserve(10)).toBe(false);
        b.lease.release();
        expect(admission.chargedBytes).toBe(0);
        expect(admission.activePasses).toBe(0);
    });
});

describe("bounded capture sizing", () => {
    it.each([
        "plain",
        '"\\\b\t\n\f\r',
        "\u0000\u001f",
        "\ud800",
        "\udfff",
        "\ud800\ud800\udc00\udfff",
        "\u4e2d\ud83d\ude00",
    ])("bounds escaped JSON string values and keys for %j", (fragment) => {
        const text = fragment.repeat(1000);
        for (const source of [[text], [{ [text]: null }]]) {
            const inspection = inspectReferenceableMessages(source);
            if (!inspection.ok) throw new Error("valid source rejected");
            expect(inspection.messageWireBytes[0]).toBeGreaterThanOrEqual(
                JSON.stringify(text).length * 2,
            );
        }
    });

    it("requires reservation before capture and refuses released or cancelled leases", () => {
        const owner = new TransformCaptureAdmission();
        const admitted = owner.admit("s");
        if (!("lease" in admitted)) throw new Error("unexpected refusal");
        const { lease } = admitted;
        const source = [message("m1")];
        const inspection = inspectReferenceableMessages(source);
        if (!inspection.ok) throw new Error("valid source rejected");
        try {
            expect(() => captureWithLease(source, lease)).toThrow(CaptureBudgetExceeded);
            expect(lease.reserve(inspection.estimatedBytes - 1)).toBe(true);
            expect(() => captureWithLease(source, lease)).toThrow(CaptureBudgetExceeded);
            expect(lease.reserve(1)).toBe(true);
            const captured = captureWithLease(source, lease);
            expect(capturedMessagesUnchanged(source, captured)).toBe(true);
            lease.requestCancel("cancelled");
            expect(() => captureWithLease(source, lease)).toThrow(CaptureBudgetExceeded);
            expect(owner.chargedBytes).toBe(inspection.estimatedBytes);
        } finally {
            lease.release();
        }
        expect(() => captureWithLease(source, lease)).toThrow(CaptureBudgetExceeded);
    });

    it("revalidates hooks installed between inspection and reserved capture", () => {
        const source = [message("m1")];
        const lease = reserveCapture(source);
        const counter = trapCounter();
        Object.defineProperty(source[0], "parts", { get: counter.trap });
        expect(() => captureWithLease(source, lease)).toThrow("accessor");
        expect(counter.count).toBe(0);
    });

    it("charges root metadata and every root and message tape slot", () => {
        const source = Object.assign([message("m1")], { bookkeeping: { text: "x".repeat(1024) } });
        const base = inspectReferenceableMessages([source[0]]);
        const inspection = inspectReferenceableMessages(source);
        if (!base.ok || !inspection.ok) throw new Error("valid source rejected");
        expect(inspection.estimatedBytes - base.estimatedBytes).toBeGreaterThan(2048);
        const captured = captureMessages(source);
        const slots = captured.rootSnapshot.fields.length + captured.snapshots[0].fields.length;
        expect(inspection.estimatedBytes).toBeGreaterThan(slots * 16);
        expect(inspectReferenceableMessages(source, inspection.estimatedBytes)).toEqual(inspection);
        expect(() => inspectReferenceableMessages(source, inspection.estimatedBytes - 1)).toThrow(
            CaptureBudgetExceeded,
        );
        source.bookkeeping.text = "changed";
        expect(capturedMessagesUnchanged(source, captured)).toBe(false);
    });

    it("charges descriptions of retained symbol keys in root and nested metadata", () => {
        const key = Symbol("x".repeat(1024));
        for (const source of [Object.assign([null], { [key]: 1 }), [{ [key]: 1 }]]) {
            const inspection = inspectReferenceableMessages(source);
            if (!inspection.ok) throw new Error("valid source rejected");
            expect(inspection.estimatedBytes).toBeGreaterThan(2048);
            expect(() => inspectReferenceableMessages(source, 2048)).toThrow(CaptureBudgetExceeded);
        }
    });

    it("uses one cumulative budget for capture and recheck rather than one per message", () => {
        const text = "x".repeat(20 * 1024 * 1024);
        const source = [{ text }, { text }];
        const snapshot = messageContentSnapshot(source[0]);
        const rootSnapshot = captureMessages([null, null]).rootSnapshot;
        for (const member of source) expect(inspectReferenceableMessages([member]).ok).toBe(true);
        expect(() => inspectReferenceableMessages(source)).toThrow(CaptureBudgetExceeded);
        expect(() => captureMessages(source)).toThrow(CaptureBudgetExceeded);
        expect(
            capturedMessagesUnchanged(source, {
                members: source,
                snapshots: [snapshot, snapshot],
                rootSnapshot,
            }),
        ).toBe(false);
    });

    it("accepts the exact byte charge and rejects one byte less", () => {
        const live = [message("m1"), { a: undefined, b: "\ud800😀" }];
        const result = inspectReferenceableMessages(live);
        if (!result.ok) throw new Error("valid source rejected");
        expect(inspectReferenceableMessages(live, result.estimatedBytes)).toEqual(result);
        expect(() => inspectReferenceableMessages(live, result.estimatedBytes - 1)).toThrow(
            CaptureBudgetExceeded,
        );
        const fields = captureMessages(live).snapshots.reduce(
            (sum, snapshot) => sum + snapshot.fields.length,
            0,
        );
        expect(result.estimatedBytes).toBeGreaterThan(fields * 8);
        for (const bytes of [-1, 0.5]) {
            expect(() => inspectReferenceableMessages(live, bytes)).toThrow(CaptureBudgetExceeded);
        }
    });

    it("bounds descriptor traversal for huge sparse lengths, omitted fields, long strings, and wide objects", () => {
        const omitted: Record<string, unknown> = {};
        const wide: Record<string, unknown> = {};
        for (let index = 0; index < 10_000; index += 1) {
            omitted[`key${index}`] = undefined;
            wide[`key${index}`] = index;
        }
        const cases = [[omitted], [wide], ["x".repeat(10_000)], new Array(2 ** 32 - 1)];
        const descriptors = spyOn(Object, "getOwnPropertyDescriptor");
        let failures = 0;
        try {
            for (const source of cases) {
                try {
                    inspectReferenceableMessages(source, 2048);
                } catch (error) {
                    if (error instanceof CaptureBudgetExceeded) failures += 1;
                    else throw error;
                }
            }
            expect(descriptors.mock.calls.length).toBeLessThan(1000);
        } finally {
            descriptors.mockRestore();
        }
        expect(failures).toBe(cases.length);
    });

    it("bounds acyclic reference amplification and rejects an oversized recheck", () => {
        let shared: unknown = null;
        for (let index = 0; index < 25; index += 1) shared = { left: shared, right: shared };
        expect(() => inspectReferenceableMessages([shared])).toThrow(CaptureBudgetExceeded);
        expect(() => captureMessages([shared])).toThrow(CaptureBudgetExceeded);
        const live: unknown[] = [null];
        const captured = captureMessages(live);
        live[0] = shared;
        expect(capturedMessagesUnchanged(live, captured)).toBe(false);
        expect(messageMatchesContentSnapshot(shared, messageContentSnapshot(null))).toBe(false);
    });
});

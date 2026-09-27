import { afterEach, describe, expect, it, spyOn } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { BoundedSessionMap } from "../../shared/bounded-session-map";
import * as logger from "../../shared/logger";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";
import { closeReadOnlySessionDb } from "./read-session-db";
import {
    createRustModeTransform,
    type RustModeModuleClient,
    type RustModeTransformDeps,
} from "./rust-mode-transform";
import type { MessageLike } from "./tag-content-primitives";
import {
    defaultTransformCaptureAdmission,
    filterMayHold,
    fnv1a32,
    messageIdFilter,
    scanMessageIds,
    TransformCaptureAdmission,
} from "./transform-capture";

function makeDeps(): RustModeTransformDeps {
    return {
        client: {
            app: { agents: async () => ({ data: [] }) },
            session: { get: async () => ({ data: { directory: "/tmp/project" } }) },
        } as never,
        contextUsageMap: new BoundedSessionMap(8),
        protectedTags: 4,
        clearReasoningAge: 50,
        cacheTtl: "5m",
        directory: "/tmp/project",
        sessionDirectoryBySession: new Map(),
        isSubagentSession: () => false,
        systemPromptHashFor: () => "",
    };
}

const message = (sessionId: string, id: string): MessageLike => ({
    info: { id, role: "user", sessionID: sessionId },
    parts: [{ type: "text", text: `message ${id}` }],
});

const hostArray = (sessionId: string, count: number): MessageLike[] =>
    Array.from({ length: count }, (_, index) => message(sessionId, `m-${index}`));

type Anchor = { mid: string; sequence: number };
type Body = Record<string, unknown>;

let outputCounter = 0;

/** Keeps the whole submitted window and reports `boundary` as the rendered boundary. */
function keepAll(body: Body, boundary: Anchor | null, extra: Body = {}): Body {
    outputCounter += 1;
    const count = (body.native_messages as unknown[]).length;
    return {
        base_revision: body.base_revision,
        output_revision: `window-out-${outputCounter}`,
        boundary,
        operations: count === 0 ? [] : [{ op: "keep", source: "input", start: 0, count }],
        ...extra,
    };
}

/** A daemon fake: `pages` answers `transform.boundary`, `transform` answers each transform body. */
function fakeDaemon(args: {
    pages?: (before: number | undefined, index: number) => unknown;
    transform?: (body: Body, index: number) => unknown;
}): {
    client: RustModeModuleClient;
    bodies: Body[];
    cursors: Array<number | undefined>;
    timeouts: Array<number | undefined>;
} {
    const bodies: Body[] = [];
    const cursors: Array<number | undefined> = [];
    const timeouts: Array<number | undefined> = [];
    const client: RustModeModuleClient = {
        call: async ({ method, body, timeoutMs }) => {
            const request = body as Body;
            if (method === "transform.boundary") {
                expect(Object.keys(request).sort()).toEqual(
                    request.before_sequence === undefined
                        ? ["method", "session_id", "v"]
                        : ["before_sequence", "method", "session_id", "v"],
                );
                const before = request.before_sequence as number | undefined;
                cursors.push(before);
                timeouts.push(timeoutMs);
                return (args.pages ?? (() => ({ anchors: [] })))(before, cursors.length - 1);
            }
            if (method !== "transform") return { ok: true };
            bodies.push(request);
            return (args.transform ?? ((b) => keepAll(b, null)))(request, bodies.length - 1);
        },
    };
    return { client, bodies, cursors, timeouts };
}

function logsOf(spy: { mock: { calls: unknown[][] } }, sessionId: string): string[] {
    return spy.mock.calls.filter(([id]) => id === sessionId).map(([, text]) => String(text));
}

describe("id scan and membership filter", () => {
    it("names a message by its top-level id when info carries none, as the daemon's decoder does", () => {
        const host: unknown[] = [
            { id: "bare" },
            { info: { role: "user" }, id: "top" },
            { info: { id: "nested" }, id: "shadowed" },
        ];
        expect(scanMessageIds(host, (id) => id === "bare")).toBe(0);
        expect(scanMessageIds(host, (id) => id === "top")).toBe(1);
        expect(scanMessageIds(host, (id) => id === "nested")).toBe(2);
        expect(scanMessageIds(host, (id) => id === "shadowed")).toBe(-1);
    });

    it("crosses planted proxies, accessors, and revoked proxies without invoking any hook", () => {
        let traps = 0;
        const count = (): never => {
            traps += 1;
            throw new Error("hook ran");
        };
        const handler: ProxyHandler<object> = {
            get: count,
            has: count,
            ownKeys: count,
            getOwnPropertyDescriptor: count,
            getPrototypeOf: count,
        };
        const revoked = Proxy.revocable({ info: { id: "target" } }, handler);
        revoked.revoke();
        const revokedInfo = Proxy.revocable({ id: "target" }, handler);
        revokedInfo.revoke();
        const host: unknown[] = [
            { info: { id: "target" } },
            new Proxy({ info: { id: "target" } }, handler),
            { info: new Proxy({ id: "target" }, handler) },
            { info: Object.defineProperty({}, "id", { get: count, enumerable: true }) },
            Object.defineProperty({}, "info", { get: count, enumerable: true }),
            revoked.proxy,
            { info: revokedInfo.proxy },
            "not an object",
        ];
        Object.defineProperty(host, 7, { get: count, enumerable: true, configurable: true });
        // Enabling state: every hostile hop sits between the end and the one readable match.
        expect(scanMessageIds(host, (id) => id === "target")).toBe(0);
        expect(scanMessageIds(host, (id) => id === "absent")).toBe(-1);
        let charged = 0;
        const filter = messageIdFilter(host, (bytes) => {
            charged += bytes;
            return true;
        });
        // The falsifier reads `host[i].info.id` before checking for a proxy and trips a trap here.
        expect(traps).toBe(0);
        expect(charged).toBe(host.length * 4);
        expect(filter).toBeInstanceOf(Uint32Array);
        expect(Array.from(filter ?? [])).toEqual([fnv1a32("target")]);
    });

    it("holds every present id, refuses an unaffordable filter, and retains no id string", () => {
        const ids = Array.from({ length: 2_000 }, (_, index) => `msg_${index.toString(36)}`);
        const host = ids.map((id) => ({ info: { id } }));
        const filter = messageIdFilter(host, () => true);
        if (!filter) throw new Error("filter refused");
        for (const id of ids) expect(filterMayHold(filter, id)).toBe(true);
        let absentHits = 0;
        for (let index = 0; index < 2_000; index += 1)
            if (filterMayHold(filter, `absent_${index}`)) absentHits += 1;
        expect(absentHits).toBe(0);
        expect(Object.getPrototypeOf(filter)).toBe(Uint32Array.prototype);
        expect(messageIdFilter(host, () => false)).toBeUndefined();
    });
});

describe("window capture and publication", () => {
    it("sends the declared window in both representations and publishes the recipe at boundaryIndex + i", async () => {
        const sessionId = `window-steady-${Date.now()}`;
        const first = hostArray(sessionId, 10);
        const { client, bodies, cursors } = fakeDaemon({
            pages: () => ({
                anchors: [
                    { mid: "m-6", sequence: 3 },
                    { mid: "gone", sequence: 2 },
                ],
            }),
            transform: (body, index) =>
                index === 0
                    ? {
                          base_revision: body.base_revision,
                          output_revision: "window-first",
                          boundary: { mid: "m-7", sequence: 4 },
                          operations: [{ op: "keep", source: "input", start: 1, count: 2 }],
                      }
                    : keepAll(body, { mid: "m-7", sequence: 4 }),
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const output = { messages: [...first] as unknown[] };
        await transform.run(sessionId, output);
        const body = bodies[0] as Body;
        expect(body.v).toBe(3);
        expect(body.boundary).toEqual({ mid: "m-6", sequence: 3 });
        expect(body.native_messages).toEqual(first.slice(6));
        expect((body.messages as Array<{ mid: string }>).map((entry) => entry.mid)).toEqual([
            "m-6",
            "m-7",
            "m-8",
            "m-9",
        ]);
        expect(JSON.stringify(body)).not.toContain("ordinal");
        // Window position i is host index 6 + i.
        expect(output.messages).toEqual([first[7], first[8]]);
        expect(output.messages[0]).toBe(first[7]);
        expect(transform.getState(sessionId).boundary).toEqual({ mid: "m-7", sequence: 4 });

        const second = hostArray(sessionId, 11);
        await transform.run(sessionId, { messages: [...second] });
        expect(cursors).toEqual([undefined]);
        expect(bodies[1]?.boundary).toEqual({ mid: "m-7", sequence: 4 });
        expect(bodies[1]?.native_messages).toEqual(second.slice(7));
    });

    it("rejects a duplicate id inside the window and ignores one outside it", async () => {
        const sessionId = `window-duplicate-${Date.now()}`;
        const { client, bodies } = fakeDaemon({
            pages: () => ({ anchors: [{ mid: "m-4", sequence: 1 }] }),
            transform: (body) => keepAll(body, { mid: "m-4", sequence: 1 }),
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const outside = hostArray(sessionId, 6);
        outside[1] = message(sessionId, "m-0");
        await transform.run(sessionId, { messages: [...outside] });
        expect(bodies).toHaveLength(1);

        const inside = hostArray(sessionId, 7);
        inside[6] = message(sessionId, "m-5");
        const output = { messages: [...inside] as unknown[] };
        const debug = spyOn(logger.sessionLog, "debug");
        try {
            await transform.run(sessionId, output);
            expect(logsOf(debug, sessionId)).toContain(
                `rust session ${sessionId} pass declined: unsupported_source (duplicate id m-5)`,
            );
        } finally {
            debug.mockRestore();
        }
        expect(bodies).toHaveLength(1);
        expect(output.messages).toEqual(inside);
    });

    it("declines a matched boundary that fails the hostile walk instead of picking another", async () => {
        const sessionId = `window-hostile-boundary-${Date.now()}`;
        const { client, bodies, cursors } = fakeDaemon({
            pages: () => ({ anchors: [{ mid: "m-6", sequence: 3 }] }),
            transform: (body) => keepAll(body, { mid: "m-6", sequence: 3 }),
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        await transform.run(sessionId, { messages: hostArray(sessionId, 8) });
        expect(bodies).toHaveLength(1);

        const host = hostArray(sessionId, 9);
        let getterCalls = 0;
        Object.defineProperty(host[6], "parts", {
            get: () => {
                getterCalls += 1;
                return [];
            },
            enumerable: true,
        });
        const output = { messages: [...host] as unknown[] };
        await transform.run(sessionId, output);
        expect(getterCalls).toBe(0);
        expect(bodies).toHaveLength(1);
        expect(cursors).toHaveLength(1);
        expect(output.messages).toEqual(host);
        expect(transform.getState(sessionId).boundary).toEqual({ mid: "m-6", sequence: 3 });
    });

    const mutations: Record<string, (output: { messages: unknown[] }) => void> = {
        "prefix deletion": (output) => {
            output.messages.splice(0, 1);
        },
        "same-length reorder": (output) => {
            const [covered, windowed] = [output.messages[1], output.messages[7]];
            output.messages[1] = windowed;
            output.messages[7] = covered;
        },
        "root rebinding": (output) => {
            output.messages = [...output.messages];
        },
        "interior window omission": (output) => {
            output.messages.splice(8, 1);
            output.messages.push(message("rebuilt", "m-late"));
        },
    };
    for (const [name, mutate] of Object.entries(mutations)) {
        it(`declines ${name} during the await with no candidate write and no promotion`, async () => {
            const sessionId = `window-fixed-${name.replaceAll(" ", "-")}-${Date.now()}`;
            const pending = Promise.withResolvers<unknown>();
            const reached = Promise.withResolvers<void>();
            const candidate = { info: { id: "candidate" }, parts: [] };
            const { client, bodies } = fakeDaemon({
                pages: () => ({ anchors: [{ mid: "m-6", sequence: 3 }] }),
                transform: (body, index) => {
                    if (index === 0) return keepAll(body, { mid: "m-6", sequence: 3 });
                    reached.resolve();
                    return pending.promise;
                },
            });
            const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
            await transform.run(sessionId, { messages: hostArray(sessionId, 10) });
            const output = { messages: [...hostArray(sessionId, 10)] as unknown[] };
            const pass = transform.run(sessionId, output);
            await Promise.race([reached.promise, pass]);
            // Enabling state: the window starts past index 0 and the host changes during the await.
            expect(bodies[1]?.boundary).toEqual({ mid: "m-6", sequence: 3 });
            expect(bodies[1]?.native_messages).toHaveLength(4);
            mutate(output);
            const mutated = [...output.messages];
            pending.resolve({
                base_revision: bodies[1]?.base_revision,
                output_revision: "window-late",
                boundary: { mid: "m-9", sequence: 9 },
                operations: [{ op: "insert", values: [candidate] }],
            });
            await pass;
            expect(output.messages).toEqual(mutated);
            expect(output.messages).not.toContain(candidate);
            expect(transform.getState(sessionId).boundary).toEqual({ mid: "m-6", sequence: 3 });
            expect(defaultTransformCaptureAdmission.chargedBytes).toBe(0);
        });
    }
});

describe("boundary discovery", () => {
    it("declares a match found past page one", async () => {
        const sessionId = `discovery-page-two-${Date.now()}`;
        const host = hostArray(sessionId, 8);
        const { client, bodies, cursors, timeouts } = fakeDaemon({
            pages: (before) =>
                before === undefined
                    ? {
                          anchors: [
                              { mid: "reverted-2", sequence: 90 },
                              { mid: "reverted-1", sequence: 80 },
                          ],
                      }
                    : { anchors: [{ mid: "m-3", sequence: 12 }] },
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        await transform.run(sessionId, { messages: [...host] });
        expect(cursors).toEqual([undefined, 80]);
        // Discovery carries the transform deadline class, within its share of the pass.
        for (const timeout of timeouts) expect(timeout).toBeLessThanOrEqual(1_000);
        expect(bodies[0]?.boundary).toEqual({ mid: "m-3", sequence: 12 });
        expect(bodies[0]?.native_messages).toEqual(host.slice(3));
    });

    it("sends null with the whole array only after an empty page", async () => {
        const sessionId = `discovery-exhausted-${Date.now()}`;
        const host = hostArray(sessionId, 4);
        const { client, bodies, cursors } = fakeDaemon({
            pages: (before) =>
                before === undefined
                    ? { anchors: [{ mid: "gone", sequence: 5 }] }
                    : { anchors: [] },
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        await transform.run(sessionId, { messages: [...host] });
        expect(cursors).toEqual([undefined, 5]);
        expect(bodies[0]?.boundary).toBeNull();
        expect(bodies[0]?.native_messages).toEqual(host);
    });

    it("verifies a filter hit by id scan and rejects a colliding absent id", async () => {
        const seen = new Map<number, string>();
        let pair: [string, string] | undefined;
        for (let index = 0; !pair; index += 1) {
            const id = `c${index}`;
            const other = seen.get(fnv1a32(id));
            if (other) pair = [other, id];
            else seen.set(fnv1a32(id), id);
        }
        const [present, absent] = pair;
        const sessionId = `discovery-collision-${Date.now()}`;
        const host = [message(sessionId, present), message(sessionId, "tail")];
        const filter = messageIdFilter(host, () => true);
        // Enabling state: the absent anchor passes the filter.
        expect(filter && filterMayHold(filter, absent)).toBe(true);
        const { client, bodies } = fakeDaemon({
            pages: (before) =>
                before === undefined
                    ? { anchors: [{ mid: absent, sequence: 3 }] }
                    : { anchors: [] },
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        await transform.run(sessionId, { messages: [...host] });
        expect(bodies[0]?.boundary).toBeNull();
    });

    const declines: Record<string, Parameters<typeof fakeDaemon>[0]["pages"]> = {
        "a repeated cursor": (before) => ({
            anchors: [{ mid: "gone", sequence: before === undefined ? 10 : before }],
        }),
        "an ascending page": () => ({
            anchors: [
                { mid: "gone-1", sequence: 5 },
                { mid: "gone-2", sequence: 6 },
            ],
        }),
        "a malformed page": () => ({ anchors: [{ mid: "", sequence: 1 }] }),
        "an unsafe sequence": () => ({ anchors: [{ mid: "gone", sequence: 2 ** 53 }] }),
        "a timeout": () => {
            throw Object.assign(new Error("module transport deadline expired"), {
                code: "ETIMEDOUT",
            });
        },
        "a revision 2 daemon": () => {
            throw Object.assign(new Error("unrecognized request shape"), {
                code: "unrecognized_request_shape",
            });
        },
    };
    for (const [name, pages] of Object.entries(declines)) {
        it(`declines ${name} without sending null`, async () => {
            const sessionId = `discovery-decline-${name.replaceAll(" ", "-")}-${Date.now()}`;
            const host = hostArray(sessionId, 3);
            const { client, bodies } = fakeDaemon({ pages });
            const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
            const warn = spyOn(logger.sessionLog, "warn");
            const debug = spyOn(logger.sessionLog, "debug");
            const output = { messages: [...host] as unknown[] };
            try {
                await transform.run(sessionId, output);
                const lines = [...logsOf(warn, sessionId), ...logsOf(debug, sessionId)];
                expect(lines.some((line) => line.includes("discovery_declined"))).toBe(true);
                if (name === "a revision 2 daemon")
                    expect(logsOf(warn, sessionId).some((line) => line.includes("upgrade"))).toBe(
                        true,
                    );
            } finally {
                warn.mockRestore();
                debug.mockRestore();
            }
            expect(bodies).toHaveLength(0);
            expect(output.messages).toEqual(host);
            expect(transform.getState(sessionId).boundary).toBeUndefined();
            expect(transform.getState(sessionId).failureCount).toBe(0);
        });
    }

    it("declines when the time budget fires with no null submission", async () => {
        const sessionId = `discovery-budget-${Date.now()}`;
        let sequence = 1_000_000;
        const { client, bodies, cursors } = fakeDaemon({
            pages: async () => {
                await Bun.sleep(300);
                sequence -= 1;
                return { anchors: [{ mid: `gone-${sequence}`, sequence }] };
            },
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const output = { messages: hostArray(sessionId, 3) as unknown[] };
        await transform.run(sessionId, output);
        // Enabling state: the walk passed page one before the budget fired.
        expect(cursors.length).toBeGreaterThan(1);
        expect(bodies).toHaveLength(0);
    });

    it("rediscovers once after boundary_unknown and declines the second in one pass", async () => {
        const sessionId = `discovery-unknown-${Date.now()}`;
        const { client, bodies, cursors } = fakeDaemon({
            pages: () => ({ anchors: [{ mid: "m-2", sequence: 7 }] }),
            transform: (body, index) =>
                index === 0
                    ? keepAll(body, { mid: "m-2", sequence: 7 })
                    : { status: "boundary_unknown" },
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        await transform.run(sessionId, { messages: hostArray(sessionId, 5) });
        expect(cursors).toHaveLength(1);
        const host = hostArray(sessionId, 6);
        const output = { messages: [...host] as unknown[] };
        const debug = spyOn(logger.sessionLog, "debug");
        try {
            await transform.run(sessionId, output);
            // One pass: one pass line, with the rediscovery visible in it.
            const passes = logsOf(debug, sessionId).filter((line) => line.startsWith("rust pass:"));
            expect(passes).toHaveLength(1);
            expect(passes[0]).toContain("decision=declined:boundary_unknown");
            expect(passes[0]).toContain("rediscovered=true");
        } finally {
            debug.mockRestore();
        }
        // One rediscovery, then the second boundary_unknown declines the pass.
        expect(cursors).toHaveLength(2);
        expect(bodies).toHaveLength(3);
        expect(output.messages).toEqual(host);
        expect(transform.getState(sessionId).passCount).toBe(2);
        // The next pass starts with discovery.
        expect(transform.getState(sessionId).boundary).toBeUndefined();

        // A cold pass rediscovers once, then declines a second `boundary_unknown`.
        const cold = createRustModeTransform(makeDeps(), { moduleClient: client });
        await cold.run(`${sessionId}-cold`, { messages: hostArray(`${sessionId}-cold`, 5) });
        expect(bodies).toHaveLength(5);
        expect(cursors).toHaveLength(4);
    });

    it("rediscovers and publishes when an anchor it just discovered draws boundary_unknown", async () => {
        const sessionId = `discovery-lost-${Date.now()}`;
        // The daemon drops segment 7 between the walk and the send; the next walk lists 6.
        const { client, bodies, cursors } = fakeDaemon({
            pages: (_before, index) =>
                index < 2
                    ? { anchors: [{ mid: "m-2", sequence: 7 }] }
                    : { anchors: [{ mid: "m-1", sequence: 6 }] },
            transform: (body, index) =>
                index === 0
                    ? keepAll(body, { mid: "m-3", sequence: 8 })
                    : index === 1
                      ? { status: "boundary_unknown" }
                      : keepAll(body, { mid: "m-1", sequence: 6 }),
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        await transform.run(sessionId, { messages: hostArray(sessionId, 5) });
        expect(transform.getState(sessionId).boundary).toEqual({ mid: "m-3", sequence: 8 });
        // Enabling state: the stored anchor m-3 is gone from the host, so the scan misses it.
        const host = hostArray(sessionId, 6).filter((entry) => entry.info.id !== "m-3");
        const output = { messages: [...host] as unknown[] };
        await transform.run(sessionId, output);
        // One walk, `boundary_unknown`, one more walk, then publication at the new anchor.
        expect(cursors).toEqual([undefined, undefined, undefined]);
        expect(bodies).toHaveLength(3);
        expect(bodies[1]?.boundary).toEqual({ mid: "m-2", sequence: 7 });
        expect(bodies[2]?.boundary).toEqual({ mid: "m-1", sequence: 6 });
        expect(bodies[2]?.native_messages).toEqual(host.slice(1));
        expect(output.messages).toEqual(host.slice(1));
        expect(transform.getState(sessionId).boundary).toEqual({ mid: "m-1", sequence: 6 });
    });

    /** A pass with a known anchor `m-2` whose first send answers through `unknown`; returns its pass lines. */
    async function rediscoveryPass(
        sessionId: string,
        unknown: (
            transform: ReturnType<typeof createRustModeTransform>,
            output: { messages: unknown[] },
        ) => Promise<void> | void,
    ) {
        let transform: ReturnType<typeof createRustModeTransform> | undefined;
        const output = { messages: hostArray(sessionId, 6) as unknown[] };
        const daemon = fakeDaemon({
            pages: () => ({ anchors: [{ mid: "m-2", sequence: 7 }] }),
            transform: async (body, index) => {
                if (index !== 1) return keepAll(body, { mid: "m-2", sequence: 7 });
                if (transform) await unknown(transform, output);
                return { status: "boundary_unknown" };
            },
        });
        transform = createRustModeTransform(makeDeps(), { moduleClient: daemon.client });
        await transform.run(sessionId, { messages: hostArray(sessionId, 5) });
        const debug = spyOn(logger.sessionLog, "debug");
        try {
            await transform.run(sessionId, output);
            const lines = logsOf(debug, sessionId);
            const passes = lines.filter((line) => line.startsWith("rust pass:"));
            return { ...daemon, lines, passes, output };
        } finally {
            debug.mockRestore();
        }
    }

    it("stops at boundary_unknown for a session cleared during the send", async () => {
        const sessionId = `discovery-cleared-${Date.now()}`;
        const { cursors, bodies, passes } = await rediscoveryPass(sessionId, (transform) =>
            transform.clearSession(sessionId),
        );
        // The falsifier reruns into a fresh state entry for the cleared session and declines superseded.
        expect(passes).toHaveLength(1);
        expect(passes[0]).toContain("decision=declined:cleared");
        expect(passes[0]).toContain("rediscovered=false");
        expect(cursors).toHaveLength(1);
        expect(bodies).toHaveLength(2);
    });

    it("logs one pass line when the host locks its container before a rerun", async () => {
        const sessionId = `discovery-locked-${Date.now()}`;
        const { cursors, passes } = await rediscoveryPass(sessionId, (_, output) => {
            Object.preventExtensions(output.messages);
        });
        expect(cursors).toHaveLength(2);
        expect(passes).toHaveLength(1);
        expect(passes[0]).toContain("rediscovered=true");
        expect(passes[0]).toContain("applied=false");
    });

    it("declines cleanly when the host replaces its root array before a rerun", async () => {
        let traps = 0;
        const trapped = new Proxy([], {
            get: (...args) => {
                traps += 1;
                return Reflect.get(...args);
            },
        });
        for (const replacement of [null, trapped]) {
            const sessionId = `discovery-replaced-${Date.now()}-${traps}`;
            const { passes } = await rediscoveryPass(sessionId, (_, output) => {
                (output as { messages: unknown }).messages = replacement;
            });
            expect(passes).toHaveLength(1);
            expect(passes[0]).toContain("rediscovered=true");
            expect(passes[0]).toContain("applied=false");
        }
        expect(traps).toBe(0);
    });

    it("declines a rediscovery once a slow first send spent the pass's discovery budget", async () => {
        const sessionId = `discovery-slow-${Date.now()}`;
        const { cursors, bodies, lines, passes, output } = await rediscoveryPass(sessionId, () =>
            Bun.sleep(1_100),
        );
        expect(lines).toContain(
            `rust session ${sessionId} pass declined: discovery_declined (time budget)`,
        );
        // The first pass walked once; the rerun sent no page and no second transform.
        expect(cursors).toHaveLength(1);
        expect(bodies).toHaveLength(2);
        expect(passes[0]).toContain("rediscovered=true");
        expect(output.messages).toEqual(hostArray(sessionId, 6));
    });

    it("refunds the first attempt so a rediscovered window near the byte limit still publishes", async () => {
        const run = async (maxBytes: number, unknownAt: number | undefined) => {
            const sessionId = "discovery-refund";
            let sends = 0;
            const { client, bodies, cursors } = fakeDaemon({
                pages: () => ({ anchors: [{ mid: "m-2", sequence: 7 }] }),
                transform: (body) => {
                    sends += 1;
                    return sends === unknownAt
                        ? { status: "boundary_unknown" }
                        : keepAll(body, { mid: "m-2", sequence: 7 });
                },
            });
            const admission = new TransformCaptureAdmission({ maxPasses: 4, maxBytes });
            const transform = createRustModeTransform(makeDeps(), {
                moduleClient: client,
                captureAdmission: admission,
            });
            await transform.run(sessionId, { messages: hostArray(sessionId, 5) });
            const host = hostArray(sessionId, 6);
            const output = { messages: [...host] as unknown[] };
            const debug = spyOn(logger.sessionLog, "debug");
            try {
                await transform.run(sessionId, output);
                const line = logsOf(debug, sessionId).find((text) => text.startsWith("rust pass:"));
                return { line: line ?? "", output, bodies, cursors };
            } finally {
                debug.mockRestore();
            }
        };
        // A known-anchor pass's charge, measured with room to spare.
        const probe = await run(Number.MAX_SAFE_INTEGER, undefined);
        const charged = Number(/charged:(\d+)/.exec(probe.line)?.[1]);
        expect(charged).toBeGreaterThan(0);
        // The rerun adds a 4-byte filter slot per host message; a doubled charge does not fit.
        const limited = await run(charged + 6 * 4 + 64, 2);
        expect(limited.cursors).toHaveLength(2);
        expect(limited.bodies).toHaveLength(3);
        expect(limited.line).toContain("applied=true");
        expect(limited.line).toContain("rediscovered=true");
        expect(limited.output.messages).toEqual(hostArray("discovery-refund", 6).slice(2));
    });

    it("declines when the host moves the discovered anchor before the window is copied", async () => {
        const sessionId = `discovery-moved-${Date.now()}`;
        const page = Promise.withResolvers<unknown>();
        const reached = Promise.withResolvers<void>();
        const bodies: Body[] = [];
        const client: RustModeModuleClient = {
            // Not async: discovery awaits this promise directly, so tick order is fixed.
            call: ({ method, body }) => {
                if (method === "transform.boundary") {
                    reached.resolve();
                    return page.promise;
                }
                bodies.push(body as Body);
                return Promise.resolve(keepAll(body as Body, { mid: "m-2", sequence: 7 }));
            },
        };
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const output = { messages: hostArray(sessionId, 6) as unknown[] };
        const debug = spyOn(logger.sessionLog, "debug");
        try {
            const pass = transform.run(sessionId, output);
            await reached.promise;
            page.resolve({ anchors: [{ mid: "m-2", sequence: 7 }] });
            // Runs after discovery's scan fixes index 2 and before the capture resumes.
            queueMicrotask(() => output.messages.splice(0, 1));
            await pass;
            expect(logsOf(debug, sessionId)).toContain(
                `rust session ${sessionId} pass declined: source_changed (boundary moved)`,
            );
        } finally {
            debug.mockRestore();
        }
        expect(bodies).toHaveLength(0);
        expect(transform.getState(sessionId).boundary).toBeUndefined();
    });

    it("names a callID-less tool call by its unfiltered window position after a compaction summary", async () => {
        const sessionId = `callid-position-${Date.now()}`;
        const host: MessageLike[] = [
            message(sessionId, "m-0"),
            {
                info: {
                    id: "m-1",
                    role: "assistant",
                    sessionID: sessionId,
                    summary: true,
                    finish: "stop",
                },
                parts: [{ type: "text", text: "summary" }],
            },
            {
                info: { id: "m-2", role: "assistant", sessionID: sessionId },
                parts: [{ type: "tool", tool: "read", state: { status: "pending", input: {} } }],
            },
        ];
        const { client, bodies } = fakeDaemon({});
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        await transform.run(sessionId, { messages: [...host] });
        const ck = bodies[0]?.messages as Array<{ mid: string; ck: { content: unknown[] } }>;
        expect(ck.map((entry) => entry.mid)).toEqual(["m-0", "m-2"]);
        // The native decoder numbers the tool part's message 3, counting the summary.
        expect(JSON.stringify(ck[1]?.ck.content)).toContain('"id":"synth-tool-3-0-read-');
    });

    it("keeps a later tool call whose callID a dropped compaction summary also carries", async () => {
        const sessionId = `callid-summary-call-${Date.now()}`;
        const tool = {
            type: "tool",
            tool: "read",
            callID: "call-1",
            state: { status: "pending", input: {} },
        };
        const host: MessageLike[] = [
            {
                info: {
                    id: "m-0",
                    role: "assistant",
                    sessionID: sessionId,
                    summary: true,
                    finish: "stop",
                },
                parts: [tool],
            },
            { info: { id: "m-1", role: "assistant", sessionID: sessionId }, parts: [tool] },
        ];
        const { client, bodies } = fakeDaemon({});
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        await transform.run(sessionId, { messages: [...host] });
        const ck = bodies[0]?.messages as Array<{ mid: string; ck: { content: unknown[] } }>;
        expect(ck.map((entry) => entry.mid)).toEqual(["m-1"]);
        expect(JSON.stringify(ck[0]?.ck.content)).toContain('"type":"tool_call","id":"call-1"');
    });

    it("serves raw and logs an upgrade hint when the daemon refuses the revision", async () => {
        const sessionId = `revision-refused-${Date.now()}`;
        const { client } = fakeDaemon({
            transform: () => {
                throw Object.assign(new Error("expected transform revision 3, received 2"), {
                    code: "transform_revision_unsupported",
                });
            },
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        const host = hostArray(sessionId, 3);
        const output = { messages: [...host] as unknown[] };
        const warn = spyOn(logger.sessionLog, "warn");
        try {
            await transform.run(sessionId, output);
            expect(
                logsOf(warn, sessionId).some(
                    (line) =>
                        line.includes("daemon_revision_unsupported") && line.includes("upgrade"),
                ),
            ).toBe(true);
        } finally {
            warn.mockRestore();
        }
        expect(output.messages).toEqual(host);
    });
});

describe("work counters", () => {
    /** The steady-state pass's scanned items, lease charge, retained charge, and window bytes. */
    async function steadyWork(covered: number, sessionId: string): Promise<number[]> {
        const window = Array.from({ length: 5 }, (_, index) => message(sessionId, `w-${index}`));
        const build = (): unknown[] => {
            const host: unknown[] = Array.from({ length: covered }, (_, index) => ({
                info: { id: `c-${index}` },
            }));
            for (const entry of window) host.push(structuredClone(entry));
            return host;
        };
        const anchor = { mid: "w-0", sequence: 1 };
        const { client, bodies } = fakeDaemon({
            pages: () => ({ anchors: [anchor] }),
            transform: (body) => keepAll(body, anchor),
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        await transform.run(sessionId, { messages: build() });
        const debug = spyOn(logger.sessionLog, "debug");
        try {
            await transform.run(sessionId, { messages: build() });
            const line = logsOf(debug, sessionId).find((text) => text.startsWith("rust pass:"));
            const work = /work=scanned:(\d+) charged:(\d+) retained:(\d+)$/.exec(line ?? "");
            if (!work) throw new Error(`no work counters in ${line}`);
            const body = bodies[1] as Body;
            const windowBytes = JSON.stringify([body.messages, body.native_messages]).length;
            return [...work.slice(1).map(Number), windowBytes];
        } finally {
            debug.mockRestore();
        }
    }

    it("stays equal at a fixed window for 10k and 100k host messages", async () => {
        const small = await steadyWork(10_000, "work-small");
        const large = await steadyWork(100_000, "work-large");
        expect(small[0]).toBe(5);
        expect(large).toEqual(small);
    });
});

describe("window-scoped fail-open", () => {
    const folded = (sessionId: string): MessageLike => ({
        info: { id: "fold", role: "user", sessionID: sessionId },
        parts: [{ type: "text", text: "folded" }],
    });

    /** Pass one discovers `m-2` and folds its window; every later transform fails for real, or declines with `status`. */
    function failAfterFold(sessionId: string, rendered: Anchor, status?: string) {
        return fakeDaemon({
            pages: () => ({ anchors: [{ mid: "m-2", sequence: 5 }] }),
            transform: (body, index) => {
                if (index > 0 && status) return { status };
                if (index > 0) throw new Error("request deadline expired after a possible send");
                return {
                    base_revision: body.base_revision,
                    output_revision: "fold-out",
                    boundary: rendered,
                    operations: [{ op: "insert", values: [folded(sessionId)] }],
                };
            },
        });
    }

    it("appends exactly the unacknowledged window suffix and promotes no basis", async () => {
        const sessionId = `fail-open-window-${Date.now()}`;
        const anchor = { mid: "m-2", sequence: 5 };
        const { client, bodies } = failAfterFold(sessionId, anchor);
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        await transform.run(sessionId, { messages: hostArray(sessionId, 5) });
        const grown = hostArray(sessionId, 8);
        const output = { messages: [...grown] as unknown[] };
        await transform.run(sessionId, output);
        // The acknowledged prefix is host[2..5); only host[5..8) follows the applied output.
        expect(bodies[1]?.boundary).toEqual(anchor);
        expect(output.messages).toHaveLength(4);
        expect(output.messages[0]).toEqual(folded(sessionId));
        for (const [index, member] of grown.slice(5).entries())
            expect(output.messages[index + 1]).toBe(member);
        expect(transform.getState(sessionId).failureCount).toBe(1);
        expect(transform.getState(sessionId).boundary).toEqual(anchor);

        // The basis is still pass one's: a later failure appends from the same prefix.
        const again = hostArray(sessionId, 9);
        const againOutput = { messages: [...again] as unknown[] };
        await transform.run(sessionId, againOutput);
        expect(againOutput.messages).toHaveLength(5);
        expect(againOutput.messages.slice(1)).toEqual(again.slice(5));
    });

    for (const [status, reason] of [
        ["session_busy", "daemon_session_busy"],
        ["status_added_later", "daemon_status_unrecognized"],
    ]) {
        it(`appends the unacknowledged window suffix on a ${status} decline`, async () => {
            const sessionId = `fail-open-window-${status}-${Date.now()}`;
            const anchor = { mid: "m-2", sequence: 5 };
            const { client, bodies } = failAfterFold(sessionId, anchor, status);
            const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
            const debug = spyOn(logger.sessionLog, "debug");
            try {
                await transform.run(sessionId, { messages: hostArray(sessionId, 5) });
                const grown = hostArray(sessionId, 8);
                const output = { messages: [...grown] as unknown[] };
                await transform.run(sessionId, output);
                expect(bodies[1]?.boundary).toEqual(anchor);
                expect(output.messages).toHaveLength(4);
                expect(output.messages[0]).toEqual(folded(sessionId));
                for (const [index, member] of grown.slice(5).entries())
                    expect(output.messages[index + 1]).toBe(member);
                expect(transform.getState(sessionId).failureCount).toBe(0);
                expect(transform.getState(sessionId).boundary).toEqual(anchor);
                const passLines = logsOf(debug, sessionId).filter((line) =>
                    line.startsWith("rust pass:"),
                );
                expect(passLines[1]).toContain(
                    `decision=declined:${reason} reason=none served_from=last_applied in=6 out=4`,
                );
            } finally {
                debug.mockRestore();
            }
        });
    }

    for (const status of [undefined, "session_busy"]) {
        it(`serves raw when a rerun ${status ? `declines ${status}` : "fails"} after rediscovering the basis the daemon disowned`, async () => {
            const sessionId = `fail-open-rerun-${status}-${Date.now()}`;
            const anchor = { mid: "m-2", sequence: 5 };
            const { client, bodies } = fakeDaemon({
                pages: () => ({ anchors: [anchor] }),
                transform: (body, index) => {
                    if (index === 1) return { status: "boundary_unknown" };
                    if (index > 1 && status) return { status };
                    if (index > 1)
                        throw new Error("request deadline expired after a possible send");
                    return {
                        base_revision: body.base_revision,
                        output_revision: "fold-out",
                        boundary: anchor,
                        operations: [{ op: "insert", values: [folded(sessionId)] }],
                    };
                },
            });
            const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
            await transform.run(sessionId, { messages: hostArray(sessionId, 5) });
            const grown = hostArray(sessionId, 8);
            const output = { messages: [...grown] as unknown[] };
            await transform.run(sessionId, output);
            // Enabling state: both attempts declared the retained basis, and the rerun failed or declined.
            expect(bodies.map((body) => body.boundary)).toEqual([anchor, anchor, anchor]);
            expect(output.messages).toEqual(grown);
            expect(transform.getState(sessionId).failureCount).toBe(status ? 0 : 1);
        });
    }

    it("serves raw against a mismatched basis anchor or a changed terminal message", async () => {
        const moved = `fail-open-moved-${Date.now()}`;
        const movedDaemon = failAfterFold(moved, { mid: "m-3", sequence: 6 });
        const movedTransform = createRustModeTransform(makeDeps(), {
            moduleClient: movedDaemon.client,
        });
        await movedTransform.run(moved, { messages: hostArray(moved, 5) });
        const movedHost = hostArray(moved, 7);
        const movedOutput = { messages: [...movedHost] as unknown[] };
        await movedTransform.run(moved, movedOutput);
        // Enabling state: the failed pass declared an anchor other than the retained basis.
        expect(movedDaemon.bodies[1]?.boundary).toEqual({ mid: "m-3", sequence: 6 });
        expect(movedOutput.messages).toEqual(movedHost);

        const edited = `fail-open-terminal-${Date.now()}`;
        const editedDaemon = failAfterFold(edited, { mid: "m-2", sequence: 5 });
        const editedTransform = createRustModeTransform(makeDeps(), {
            moduleClient: editedDaemon.client,
        });
        await editedTransform.run(edited, { messages: hostArray(edited, 5) });
        const editedHost = hostArray(edited, 7);
        (editedHost[4]?.parts[0] as { text: string }).text = "terminal edited in place";
        const editedOutput = { messages: [...editedHost] as unknown[] };
        await editedTransform.run(edited, editedOutput);
        expect(editedDaemon.bodies[1]?.boundary).toEqual({ mid: "m-2", sequence: 5 });
        expect(editedOutput.messages).toEqual(editedHost);
    });
});

describe("first-user tool policy", () => {
    const dataHomes: string[] = [];
    const originalDataHome = process.env.XDG_DATA_HOME;
    afterEach(() => {
        closeReadOnlySessionDb();
        for (const home of dataHomes.splice(0)) rmSync(home, { recursive: true, force: true });
        process.env.XDG_DATA_HOME = originalDataHome;
    });

    /** An OpenCode database whose earliest user row carries `tools`, or no row when absent. */
    function installDb(sessionId: string, tools?: Record<string, boolean>): void {
        const home = mkdtempSync(join(tmpdir(), "window-first-user-"));
        dataHomes.push(home);
        const path = join(home, "opencode", "opencode.db");
        mkdirSync(dirname(path), { recursive: true });
        const db = new Database(path);
        db.exec(
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL)",
        );
        if (tools)
            db.prepare("INSERT INTO message VALUES (?, ?, 1, 1, ?)").run(
                "m-0",
                sessionId,
                JSON.stringify({ id: "m-0", role: "user", tools }),
            );
        closeQuietly(db);
        process.env.XDG_DATA_HOME = home;
    }

    /** A cold pass whose window starts past the session's first user message. */
    async function toolPresent(sessionId: string, windowTools: Record<string, boolean>) {
        const host = hostArray(sessionId, 6);
        (host[3]?.info as { tools?: Record<string, boolean> }).tools = windowTools;
        const { client, bodies } = fakeDaemon({
            pages: () => ({ anchors: [{ mid: "m-3", sequence: 2 }] }),
        });
        const transform = createRustModeTransform(makeDeps(), { moduleClient: client });
        await transform.run(sessionId, { messages: host });
        expect(bodies[0]?.boundary).toEqual({ mid: "m-3", sequence: 2 });
        return [bodies[0]?.tool_present, bodies[0]?.todo_tool_present];
    }

    it("takes the verdict from the earliest user row in both signal directions", async () => {
        const denied = `first-user-deny-${Date.now()}`;
        installDb(denied, { eidnara_reduce: false, todowrite: false });
        expect(await toolPresent(denied, { eidnara_reduce: true, todowrite: true })).toEqual([
            false,
            false,
        ]);
        const allowed = `first-user-allow-${Date.now()}`;
        installDb(allowed, {});
        expect(await toolPresent(allowed, { eidnara_reduce: false, todowrite: false })).toEqual([
            true,
            true,
        ]);
    });

    it("freezes fail-open without a database and stays fail-closed for an unpersisted session", async () => {
        const home = mkdtempSync(join(tmpdir(), "window-no-db-"));
        dataHomes.push(home);
        process.env.XDG_DATA_HOME = home;
        const missing = `first-user-no-db-${Date.now()}`;
        expect(await toolPresent(missing, { eidnara_reduce: false })).toEqual([true, true]);
        const unpersisted = `first-user-no-row-${Date.now()}`;
        installDb(unpersisted);
        expect(await toolPresent(unpersisted, { eidnara_reduce: true })).toEqual([false, false]);
    });

    /** `todo_tool_present` for a pass over a user turn run by `plan`, whose agent config denies todowrite. */
    async function todoPresentUnderPlan(
        sessionId: string,
        anchor: Anchor | null,
        assistant: Record<string, unknown>,
    ) {
        installDb(sessionId, {});
        const host: MessageLike[] = [
            { info: { id: "m-0", role: "user", sessionID: sessionId, agent: "plan" }, parts: [] },
            ...Array.from({ length: 5 }, (_, index) => ({
                info: {
                    id: `m-${index + 1}`,
                    role: "assistant",
                    sessionID: sessionId,
                    ...assistant,
                },
                parts: [{ type: "text", text: `step ${index + 1}` }],
            })),
        ];
        const deps = makeDeps();
        deps.client = {
            app: {
                agents: async () => ({
                    data: [
                        {
                            name: "plan",
                            permission: [{ permission: "todowrite", pattern: "*", action: "deny" }],
                        },
                    ],
                }),
            },
            session: { get: async () => ({ data: { directory: "/tmp/project" } }) },
        } as never;
        const { client, bodies } = fakeDaemon({
            pages: () => ({ anchors: anchor ? [anchor] : [] }),
        });
        await createRustModeTransform(deps, { moduleClient: client }).run(sessionId, {
            messages: host,
        });
        expect(bodies[0]?.boundary).toEqual(anchor);
        return bodies[0]?.todo_tool_present;
    }

    it("keeps the agent's todowrite deny for a window that starts after the newest user message", async () => {
        const stamp = Date.now();
        // Control: the whole array holds the user message that names `plan`.
        expect(await todoPresentUnderPlan(`agent-whole-${stamp}`, null, { mode: "plan" })).toBe(
            false,
        );
        const past = { mid: "m-3", sequence: 4 };
        // The window holds only assistant messages, which OpenCode stamps with the agent's name.
        expect(await todoPresentUnderPlan(`agent-mode-${stamp}`, past, { mode: "plan" })).toBe(
            false,
        );
        expect(await todoPresentUnderPlan(`agent-field-${stamp}`, past, { agent: "plan" })).toBe(
            false,
        );
        // No message in the window names an agent, so the permission evidence is missing.
        expect(await todoPresentUnderPlan(`agent-none-${stamp}`, past, {})).toBe(false);
    });
});

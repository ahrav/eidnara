import { describe, expect, it, spyOn } from "bun:test";
import { BoundedSessionMap } from "../../shared/bounded-session-map";
import * as logger from "../../shared/logger";
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
        await transform.run(sessionId, output);
        // One rediscovery, then the second boundary_unknown declines the pass.
        expect(cursors).toHaveLength(2);
        expect(bodies).toHaveLength(3);
        expect(output.messages).toEqual(host);
        expect(transform.getState(sessionId).boundary).toEqual({ mid: "m-2", sequence: 7 });

        // A pass that already discovered declines its first boundary_unknown.
        const cold = createRustModeTransform(makeDeps(), { moduleClient: client });
        await cold.run(`${sessionId}-cold`, { messages: hostArray(`${sessionId}-cold`, 5) });
        expect(bodies).toHaveLength(4);
        expect(cursors).toHaveLength(3);
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

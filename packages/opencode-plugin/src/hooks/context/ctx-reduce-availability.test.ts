/// <reference types="bun-types" />

import { afterEach, beforeEach, describe, expect, it, jest, mock, spyOn } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Database } from "../../shared/sqlite";
import { closeQuietly } from "../../shared/sqlite-helpers";
import {
    clearCtxReduceAvailability,
    clearTodowriteAvailability,
    clearToolPermissionDenied,
    invalidateToolPermissionDenied,
    peekToolPermissionDeniedForTest,
    permissionDisabled,
    resetCtxReduceRegisteredGloballyForTest,
    resolveCtxReduceAvailability,
    resolveCtxReduceAvailabilityFromMessages,
    resolveTodowriteAvailabilityFromMessages,
    resolveToolPermissionDenied,
    setCtxReduceRegisteredGlobally,
} from "./ctx-reduce-availability";
import { closeReadOnlySessionDb } from "./read-session-db";

function userMsg(tools?: Record<string, unknown>) {
    return { info: { role: "user", ...(tools !== undefined ? { tools } : {}) } };
}

describe("ctx_reduce availability (OpenCode DB)", () => {
    const originalXdgDataHome = process.env.XDG_DATA_HOME;
    let dataHome: string | undefined;

    afterEach(() => {
        closeReadOnlySessionDb();
        if (originalXdgDataHome === undefined) delete process.env.XDG_DATA_HOME;
        else process.env.XDG_DATA_HOME = originalXdgDataHome;
        if (dataHome) rmSync(dataHome, { recursive: true, force: true });
        dataHome = undefined;
    });

    function writeOpenCodeDb(
        rows: ReadonlyArray<{ id: string; sessionId: string; timeCreated: number; tools: unknown }>,
    ): void {
        if (!dataHome) throw new Error("dataHome is unset");
        const dir = join(dataHome, "opencode");
        mkdirSync(dir, { recursive: true });
        const db = new Database(join(dir, "opencode.db"));
        try {
            db.exec(
                "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT)",
            );
            const insert = db.prepare(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?, ?, ?, ?, ?)",
            );
            for (const row of rows) {
                insert.run(
                    row.id,
                    row.sessionId,
                    row.timeCreated,
                    row.timeCreated,
                    JSON.stringify({ role: "user", tools: row.tools }),
                );
            }
        } finally {
            closeQuietly(db);
        }
    }

    function writeOpenCodeDbWithFirstUserTools(sessionId: string, tools: unknown): void {
        writeOpenCodeDb([{ id: "msg-1", sessionId, timeCreated: 1, tools }]);
    }

    it("keeps the frozen fail-open verdict when the database appears after the first read", () => {
        dataHome = mkdtempSync(join(tmpdir(), "eidnara-ctx-reduce-db-"));
        process.env.XDG_DATA_HOME = dataHome;
        const sessionId = "ses-db-missing-then-present";
        clearCtxReduceAvailability(sessionId);

        const beforeDb = resolveCtxReduceAvailability(sessionId);
        expect(beforeDb).toEqual({ callable: true, frozen: true });

        writeOpenCodeDbWithFirstUserTools(sessionId, { ctx_reduce: false });

        // The frozen verdict survives even though the first user message now denies the tool.
        expect(resolveCtxReduceAvailability(sessionId)).toEqual({ callable: true, frozen: true });

        // Control: a fresh session reads the deny from the database.
        clearCtxReduceAvailability(sessionId);
        expect(resolveCtxReduceAvailability(sessionId)).toEqual({ callable: false, frozen: true });
    });

    it("breaks a time_created tie by id so the canonical first user message decides", () => {
        dataHome = mkdtempSync(join(tmpdir(), "eidnara-ctx-reduce-db-"));
        process.env.XDG_DATA_HOME = dataHome;
        const sessionId = "ses-db-tie";
        clearCtxReduceAvailability(sessionId);

        // Insertion order is reversed so a rowid-ordered scan would pick the later id.
        writeOpenCodeDb([
            { id: "msg-b", sessionId, timeCreated: 7, tools: { ctx_reduce: true } },
            { id: "msg-a", sessionId, timeCreated: 7, tools: { ctx_reduce: false } },
        ]);

        expect(resolveCtxReduceAvailability(sessionId)).toEqual({ callable: false, frozen: true });
    });

    it("treats a message row with malformed JSON as a non-match instead of failing open unfrozen", () => {
        dataHome = mkdtempSync(join(tmpdir(), "eidnara-ctx-reduce-db-"));
        process.env.XDG_DATA_HOME = dataHome;
        const sessionId = "ses-db-malformed";
        clearCtxReduceAvailability(sessionId);

        writeOpenCodeDb([{ id: "msg-2", sessionId, timeCreated: 2, tools: { ctx_reduce: false } }]);
        const db = new Database(join(dataHome, "opencode", "opencode.db"));
        try {
            db.prepare(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES ('msg-1', ?, 1, 1, '{not json')",
            ).run(sessionId);
        } finally {
            closeQuietly(db);
        }

        expect(resolveCtxReduceAvailability(sessionId)).toEqual({ callable: false, frozen: true });
    });
});

describe("ctx_reduce availability (spawn tools map)", () => {
    it("resolves false for an explicit allow-list without ctx_reduce", () => {
        clearCtxReduceAvailability("ses-allow");
        const verdict = resolveCtxReduceAvailabilityFromMessages("ses-allow", [
            userMsg({ "*": false, read: true, grep: true }),
        ]);
        expect(verdict).toEqual({ callable: false, frozen: true });
    });

    it("resolves true when ctx_reduce is explicitly allowed", () => {
        clearCtxReduceAvailability("ses-explicit");
        const verdict = resolveCtxReduceAvailabilityFromMessages("ses-explicit", [
            userMsg({ "*": false, read: true, ctx_reduce: true }),
        ]);
        expect(verdict).toEqual({ callable: true, frozen: true });
    });

    it("fails open for sessions without a tools map (normal sessions)", () => {
        clearCtxReduceAvailability("ses-plain");
        const verdict = resolveCtxReduceAvailabilityFromMessages("ses-plain", [userMsg()]);
        expect(verdict).toEqual({ callable: true, frozen: true });
    });

    it("resolves false when ctx_reduce is explicitly denied", () => {
        clearCtxReduceAvailability("ses-deny");
        const verdict = resolveCtxReduceAvailabilityFromMessages("ses-deny", [
            userMsg({ ctx_reduce: false }),
        ]);
        expect(verdict).toEqual({ callable: false, frozen: true });
    });

    it("freezes the verdict per session — later, different tool maps cannot flap it", () => {
        clearCtxReduceAvailability("ses-frozen");
        const first = resolveCtxReduceAvailabilityFromMessages("ses-frozen", [
            userMsg({ "*": false, read: true }),
        ]);
        expect(first).toEqual({ callable: false, frozen: true });
        // Same session, contradictory map on a later pass: cached verdict wins
        // (per-turn maps can differ; a flapping verdict would bust the cache).
        const second = resolveCtxReduceAvailabilityFromMessages("ses-frozen", [
            userMsg({ "*": false, ctx_reduce: true }),
        ]);
        expect(second).toEqual({ callable: false, frozen: true });
    });

    it("ignores non-user messages and falls open when the first user message carries no signal", () => {
        clearCtxReduceAvailability("ses-nosignal");
        const verdict = resolveCtxReduceAvailabilityFromMessages("ses-nosignal", [
            { info: { role: "assistant" } },
            userMsg({}),
        ]);
        expect(verdict).toEqual({ callable: true, frozen: true });
    });

    it("does not freeze a fail-open verdict from an array with no user message", () => {
        clearCtxReduceAvailability("ses-no-user-yet");
        // No-user scans remain provisional until a user message supplies the session policy.
        const provisional = resolveCtxReduceAvailabilityFromMessages("ses-no-user-yet", [
            { info: { role: "assistant" } },
        ]);
        expect(provisional).toEqual({ callable: true, frozen: false });
        // The first user tools map freezes the session verdict.
        const final = resolveCtxReduceAvailabilityFromMessages("ses-no-user-yet", [
            { info: { role: "assistant" } },
            userMsg({ "*": false, read: true }),
        ]);
        expect(final).toEqual({ callable: false, frozen: true });
    });
});

describe("todowrite availability (generalized resolver)", () => {
    it("resolves false for an explicit allow-list without todowrite", () => {
        clearTodowriteAvailability("ses-td-allow");
        const verdict = resolveTodowriteAvailabilityFromMessages("ses-td-allow", [
            userMsg({ "*": false, read: true, grep: true }),
        ]);
        expect(verdict).toEqual({ callable: false, frozen: true });
    });

    it("resolves true when todowrite is explicitly allowed", () => {
        clearTodowriteAvailability("ses-td-explicit");
        const verdict = resolveTodowriteAvailabilityFromMessages("ses-td-explicit", [
            userMsg({ "*": false, read: true, todowrite: true }),
        ]);
        expect(verdict).toEqual({ callable: true, frozen: true });
    });

    it("resolves false when todowrite is explicitly denied", () => {
        clearTodowriteAvailability("ses-td-deny");
        const verdict = resolveTodowriteAvailabilityFromMessages("ses-td-deny", [
            userMsg({ todowrite: false }),
        ]);
        expect(verdict).toEqual({ callable: false, frozen: true });
    });

    it("fails open for sessions without a tools map (normal sessions)", () => {
        clearTodowriteAvailability("ses-td-plain");
        const verdict = resolveTodowriteAvailabilityFromMessages("ses-td-plain", [userMsg()]);
        expect(verdict).toEqual({ callable: true, frozen: true });
    });

    it("resolves ctx_reduce and todowrite independently for the same session", () => {
        // A tools map can keep ctx_reduce but filter todowrite (or vice versa);
        // the two verdicts must not bleed into each other through the cache.
        clearCtxReduceAvailability("ses-td-mixed");
        clearTodowriteAvailability("ses-td-mixed");
        const map = userMsg({ "*": false, ctx_reduce: true });
        const reduce = resolveCtxReduceAvailabilityFromMessages("ses-td-mixed", [map]);
        const todo = resolveTodowriteAvailabilityFromMessages("ses-td-mixed", [map]);
        expect(reduce).toEqual({ callable: true, frozen: true });
        expect(todo).toEqual({ callable: false, frozen: true });
    });
});

describe("OpenCode todowrite permission evaluator", () => {
    it("denies a top-level agent rule for the whole tool", () => {
        expect(
            permissionDisabled("todowrite", [
                { permission: "todowrite", pattern: "*", action: "deny" },
            ]),
        ).toBe(true);
    });

    it("uses findLast semantics so a later per-agent allow overrides deny", () => {
        expect(
            permissionDisabled("todowrite", [
                { permission: "todowrite", pattern: "*", action: "deny" },
                { permission: "todowrite", pattern: "*", action: "allow" },
            ]),
        ).toBe(false);
    });

    it("applies a session overlay after the merged agent rules", () => {
        expect(
            permissionDisabled("todowrite", [
                { permission: "todowrite", pattern: "*", action: "allow" },
                { permission: "todowrite", pattern: "*", action: "deny" },
            ]),
        ).toBe(true);
    });

    it("fails open when no rule matches", () => {
        expect(permissionDisabled("todowrite", [])).toBe(false);
        expect(
            permissionDisabled("todowrite", [
                { permission: "todowrite", pattern: "src/**", action: "deny" },
            ]),
        ).toBe(false);
    });

    it("treats regex punctuation literally while preserving wildcard segments", () => {
        const deny = (permission: string, toolName: string): boolean =>
            permissionDisabled(toolName, [{ permission, pattern: "*", action: "deny" }]);

        expect(() => deny("to(do*", "to(doThing")).not.toThrow();
        expect(deny("to(do*", "to(doThing")).toBe(true);
        expect(deny("to(do*", "todoThing")).toBe(false);
        expect(deny("todo.rite*", "todo.riteLater")).toBe(true);
        expect(deny("todo.rite*", "todowriteLater")).toBe(false);
        expect(deny("to*write", "todowrite")).toBe(true);
        expect(deny("to*write", "to-something-write")).toBe(true);
        expect(deny("*", "todowrite")).toBe(true);
        expect(deny("todowrite", "todowrite")).toBe(true);
    });

    it("reads the active agent and session overlay through the SDK", async () => {
        const client = {
            app: {
                agents: async () => ({
                    data: [
                        {
                            name: "build",
                            permission: [
                                { permission: "todowrite", pattern: "*", action: "allow" },
                            ],
                        },
                    ],
                }),
            },
            session: {
                get: async () => ({
                    data: {
                        permission: {
                            todowrite: "deny",
                        },
                    },
                }),
            },
        } as never;
        await expect(
            resolveToolPermissionDenied(client, "ses-permission-overlay", "todowrite", "build"),
        ).resolves.toBe(true);
    });

    it("applies the supplied agent's whole-tool deny and skips agent rules when the agent is undefined", async () => {
        const client = {
            app: {
                agents: async () => ({
                    data: [
                        {
                            name: "plan",
                            permission: { todowrite: "deny" },
                        },
                    ],
                }),
            },
            session: {
                get: async () => ({ data: { id: "ses-agent-deny", agent: "plan" } }),
            },
        } as never;
        await expect(
            resolveToolPermissionDenied(client, "ses-agent-deny", "todowrite", "plan"),
        ).resolves.toBe(true);
        // A stray `agent` field on the session payload does not substitute for the caller's agent.
        await expect(
            resolveToolPermissionDenied(client, "ses-agent-deny", "todowrite", undefined),
        ).resolves.toBe(false);
    });
});

describe("permission cache lifetime", () => {
    const sessionId = "ses-permission-cache";
    const agents = mock<() => Promise<unknown>>(async () => ({ data: [] }));
    const get = mock<() => Promise<unknown>>(async () => ({ data: {} }));
    const client = { app: { agents }, session: { get } } as never;
    const read = (agent: string | undefined = "build", session = sessionId) =>
        resolveToolPermissionDenied(client, session, "todowrite", agent);

    beforeEach(() => {
        clearToolPermissionDenied(sessionId);
        clearToolPermissionDenied(`${sessionId}-other`);
        agents.mockReset().mockResolvedValue({ data: [{ name: "build" }] });
        get.mockReset().mockResolvedValue({ data: {} });
        jest.useFakeTimers();
    });
    afterEach(() => {
        jest.useRealTimers();
        clearToolPermissionDenied(sessionId);
        clearToolPermissionDenied(`${sessionId}-other`);
    });

    it("coalesces overlapping same-key successful reads without inventing a deny", async () => {
        const response = Promise.withResolvers<unknown>();
        get.mockReturnValue(response.promise);
        const first = read();
        const second = read();
        response.resolve({ data: {} });
        expect(await Promise.all([first, second])).toEqual([false, false]);
        expect(agents).toHaveBeenCalledTimes(1);
        expect(get).toHaveBeenCalledTimes(1);
    });

    it("shares the first fill's timeout rather than restarting it for a follower", async () => {
        const response = Promise.withResolvers<unknown>();
        get.mockReturnValueOnce(response.promise);
        const first = read();
        jest.advanceTimersByTime(1_000);
        const follower = read();
        jest.advanceTimersByTime(1_000);
        expect(await Promise.all([first, follower])).toEqual([true, true]);
        expect(get).toHaveBeenCalledTimes(1);
        await expect(read()).resolves.toBe(false);
        expect(get).toHaveBeenCalledTimes(2);
        response.resolve({ data: {} });
    });

    it("keeps session and agent identities independent, including unknown and separator strings", async () => {
        agents.mockResolvedValue({
            data: [
                { name: "build", permission: { todowrite: "allow" } },
                { name: "plan", permission: { todowrite: "deny" } },
                { name: "undefined", permission: { todowrite: "deny" } },
                { name: "", permission: { todowrite: "deny" } },
                { name: "\u0000", permission: { todowrite: "deny" } },
            ],
        });
        for (const [agent, denied] of [
            ["build", false],
            ["plan", true],
            [undefined, false],
            ["undefined", true],
            ["", true],
            ["\u0000", true],
        ] as const) {
            await expect(
                resolveToolPermissionDenied(client, sessionId, "todowrite", agent),
            ).resolves.toBe(denied);
        }
        get.mockResolvedValue({ data: { permission: { todowrite: "deny" } } });
        await expect(read("build", `${sessionId}-other`)).resolves.toBe(true);
        await expect(read("build")).resolves.toBe(false);
        await expect(read("plan")).resolves.toBe(true);
        expect(agents).toHaveBeenCalledTimes(6);
        expect(get).toHaveBeenCalledTimes(7);
    });

    it("expires exactly 30 seconds after read start, not after completion or a fresh hit", async () => {
        const response = Promise.withResolvers<unknown>();
        agents.mockReturnValueOnce(response.promise);
        const pending = read();
        jest.advanceTimersByTime(1_000);
        response.resolve({ data: [{ name: "build" }] });
        await expect(pending).resolves.toBe(false);
        jest.advanceTimersByTime(28_999);
        await expect(read()).resolves.toBe(false);
        expect(agents).toHaveBeenCalledTimes(1);
        get.mockResolvedValue({ data: { permission: { todowrite: "deny" } } });
        jest.advanceTimersByTime(1);
        await expect(read()).resolves.toBe(true);
        expect(agents).toHaveBeenCalledTimes(2);
    });

    for (const prior of [undefined, false, true]) {
        for (const failure of ["rejection", "timeout"] as const) {
            it(`fails closed with prior ${prior} after ${failure}, without caching the failure`, async () => {
                if (prior !== undefined) {
                    get.mockResolvedValue({
                        data: { permission: { todowrite: prior ? "deny" : "allow" } },
                    });
                    await expect(read()).resolves.toBe(prior);
                    invalidateToolPermissionDenied(sessionId);
                }
                const response = Promise.withResolvers<unknown>();
                agents.mockReturnValueOnce(response.promise);
                const pending = read();
                if (failure === "rejection") response.reject(new Error("injected read failure"));
                else jest.advanceTimersByTime(2_000);
                await expect(pending).resolves.toBe(true);
                expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "build")).toBe(
                    prior,
                );
                const reads = agents.mock.calls.length;
                get.mockResolvedValue({ data: {} });
                await expect(read()).resolves.toBe(false);
                expect(agents).toHaveBeenCalledTimes(reads + 1);
                response.resolve({ data: [] });
            });
        }
    }

    it("denies missing APIs and unsuccessful SDK responses even after an allow", async () => {
        await expect(read()).resolves.toBe(false);
        for (const missing of [undefined, {}, { app: {} }, { session: {} }]) {
            await expect(
                resolveToolPermissionDenied(missing as never, sessionId, "todowrite", "build"),
            ).resolves.toBe(true);
        }
        for (const response of [undefined, { error: "unavailable" }, { data: undefined }]) {
            invalidateToolPermissionDenied(sessionId);
            get.mockResolvedValueOnce(response);
            await expect(read()).resolves.toBe(true);
        }
    });

    it("preserves supported permission shapes and rule order", async () => {
        for (const [permission, denied] of [
            [undefined, false],
            [{}, false],
            [[], false],
            [{ todowrite: "deny" }, true],
            [{ todowrite: { "*": "ask" } }, false],
            [[{ permission: "todowrite", pattern: "*", action: "deny" }], true],
            [[{ tool: "todowrite", value: "deny" }], true],
            [[{ name: "todowrite", pattern: ["src/**", "*"], action: "deny" }], true],
            [
                {
                    rules: [{ permission: "todowrite", pattern: "*", action: "deny" }],
                    todowrite: "allow",
                },
                false,
            ],
        ] as const) {
            invalidateToolPermissionDenied(sessionId);
            get.mockResolvedValueOnce({ data: { permission } });
            await expect(read()).resolves.toBe(denied);
            expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "build")).toBe(denied);
        }
    });

    for (const source of ["agent", "permission", "permissions"] as const) {
        it(`rejects malformed nested ${source} payloads without caching an allow`, async () => {
            for (const permission of [
                42,
                null,
                { todowrite: 42 },
                { todowrite: "invalid" },
                { todowrite: { "*": "invalid" } },
                { rules: 42 },
                [42],
                [{ permission: "todowrite", action: "invalid" }],
                [{ permission: 42, action: "deny" }],
                [{ permission: "todowrite", pattern: 42, action: "deny" }],
                [{ permission: "todowrite", pattern: ["*", 42], action: "deny" }],
            ]) {
                clearToolPermissionDenied(sessionId);
                get.mockResolvedValueOnce({ data: { permission: { todowrite: "deny" } } });
                await expect(read()).resolves.toBe(true);
                invalidateToolPermissionDenied(sessionId);
                if (source === "agent") {
                    agents.mockResolvedValueOnce({ data: [{ name: "build", permission }] });
                } else {
                    get.mockResolvedValueOnce({ data: { [source]: permission } });
                }
                await expect(read()).resolves.toBe(true);
                expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "build")).toBe(true);
                const reads = get.mock.calls.length;
                await expect(read()).resolves.toBe(false);
                expect(get).toHaveBeenCalledTimes(reads + 1);
            }
        });
    }

    for (const failure of ["missing", "rejected", "malformed", "hung"] as const) {
        it(`uses session rules alone with no active agent and a ${failure} agent-list API`, async () => {
            const response = Promise.withResolvers<unknown>();
            if (failure === "rejected") agents.mockRejectedValue(new Error("unavailable"));
            if (failure === "malformed") agents.mockResolvedValue({ data: 42 });
            if (failure === "hung") agents.mockReturnValue(response.promise);
            const sessionOnlyClient =
                failure === "missing" ? ({ session: { get } } as never) : client;
            try {
                const pending = resolveToolPermissionDenied(
                    sessionOnlyClient,
                    sessionId,
                    "todowrite",
                    undefined,
                );
                expect(agents).not.toHaveBeenCalled();
                await expect(pending).resolves.toBe(false);
                get.mockResolvedValue({ data: { permission: { todowrite: "deny" } } });
                invalidateToolPermissionDenied(sessionId);
                await expect(
                    resolveToolPermissionDenied(
                        sessionOnlyClient,
                        sessionId,
                        "todowrite",
                        undefined,
                    ),
                ).resolves.toBe(true);
                expect(agents).not.toHaveBeenCalled();
                expect(get).toHaveBeenCalledTimes(2);
            } finally {
                response.resolve({ data: [] });
            }
        });
    }

    for (const boundary of ["invalidation", "deletion", "timeout"] as const) {
        it(`a late allow cannot publish after ${boundary} or clear a newer deny`, async () => {
            const oldResponse = Promise.withResolvers<unknown>();
            get.mockReturnValueOnce(oldResponse.promise);
            const old = read();
            const follower = read();
            if (boundary === "invalidation") invalidateToolPermissionDenied(sessionId);
            if (boundary === "deletion") clearToolPermissionDenied(sessionId);
            if (boundary === "timeout") {
                jest.advanceTimersByTime(2_000);
                await expect(old).resolves.toBe(true);
            }
            get.mockResolvedValue({ data: { permission: { todowrite: "deny" } } });
            await expect(read()).resolves.toBe(true);
            oldResponse.resolve({ data: { permission: { todowrite: "allow" } } });
            await expect(old).resolves.toBe(true);
            await expect(follower).resolves.toBe(true);
            expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "build")).toBe(true);
            const reads = get.mock.calls.length;
            await expect(read()).resolves.toBe(true);
            expect(get).toHaveBeenCalledTimes(reads);
        });
    }

    for (const boundary of ["invalidation", "deletion"] as const) {
        it(`a late completion cannot repopulate an entry after ${boundary} without a newer read`, async () => {
            get.mockResolvedValueOnce({ data: { permission: { todowrite: "deny" } } });
            await read();
            invalidateToolPermissionDenied(sessionId);
            const response = Promise.withResolvers<unknown>();
            get.mockReturnValueOnce(response.promise);
            const pending = read();
            if (boundary === "deletion") clearToolPermissionDenied(sessionId);
            else invalidateToolPermissionDenied(sessionId);
            response.resolve({ data: {} });
            await expect(pending).resolves.toBe(true);
            expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "build")).toBe(
                boundary === "deletion" ? undefined : true,
            );
        });
    }

    it("bounds retained verdict count even for oversized identifiers", async () => {
        const longAgent = "x".repeat(100_000);
        agents.mockResolvedValue({ data: [{ name: longAgent }] });
        await read(longAgent);
        for (let index = 0; index < 2_000; index += 1) {
            agents.mockResolvedValue({ data: [{ name: `agent-${index}` }] });
            await read(`agent-${index}`);
        }
        expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", longAgent)).toBeUndefined();
        expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "agent-1999")).toBe(false);
        const reads = get.mock.calls.length;
        agents.mockResolvedValue({ data: [{ name: longAgent }] });
        await read(longAgent);
        expect(get).toHaveBeenCalledTimes(reads + 1);
    });

    it("discards a successful fill whose TTL expires before settlement without running timers", async () => {
        const clock = spyOn(performance, "now").mockReturnValue(100);
        try {
            const response = Promise.withResolvers<unknown>();
            get.mockReturnValueOnce(response.promise);
            const pending = read();
            clock.mockReturnValue(30_100);
            response.resolve({ data: {} });
            await expect(pending).resolves.toBe(true);
            expect(
                peekToolPermissionDeniedForTest(sessionId, "todowrite", "build"),
            ).toBeUndefined();
            await expect(read()).resolves.toBe(false);
            expect(get).toHaveBeenCalledTimes(2);
        } finally {
            clock.mockRestore();
        }
    });

    it("does not return or publish an allow when a pending shared fill is evicted", async () => {
        const response = Promise.withResolvers<unknown>();
        get.mockReturnValueOnce(response.promise);
        const first = read();
        const follower = read();
        for (let index = 0; index < 2_000; index += 1) {
            agents.mockResolvedValue({ data: [{ name: `agent-${index}` }] });
            await read(`agent-${index}`);
        }
        expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "build")).toBeUndefined();
        response.resolve({ data: {} });
        expect(await Promise.all([first, follower])).toEqual([true, true]);
        expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "build")).toBeUndefined();
        agents.mockResolvedValue({ data: [{ name: "build" }] });
        await expect(read()).resolves.toBe(false);
        expect(get).toHaveBeenCalledTimes(2_002);
    });

    it("fails closed for a missing named agent, but unknown agent uses session rules alone", async () => {
        agents.mockResolvedValue({ data: [{ name: "other" }] });
        await expect(read()).resolves.toBe(true);
        expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "build")).toBeUndefined();
        await expect(
            resolveToolPermissionDenied(client, sessionId, "todowrite", undefined),
        ).resolves.toBe(false);
    });

    it("rejects malformed and error agents payloads without caching a successful verdict", async () => {
        for (const response of [
            undefined,
            null,
            { data: {} },
            { data: [null, { name: 42 }] },
            { error: "unavailable" },
            { data: [{ name: "build" }], error: "unavailable" },
        ]) {
            agents.mockResolvedValueOnce(response);
            await expect(read()).resolves.toBe(true);
            expect(
                peekToolPermissionDeniedForTest(sessionId, "todowrite", "build"),
            ).toBeUndefined();
        }
        agents.mockResolvedValueOnce({ data: [{ name: "build" }], error: null });
        get.mockResolvedValueOnce({ data: {}, error: null });
        await expect(read()).resolves.toBe(false);
    });
});

describe("ctx_reduce process-global registration override (compaction-off #266 S4)", () => {
    // The override applies process-wide.
    // `afterEach` resets the process-wide override to `true` so later tests cannot inherit `false`.
    // `afterEach` resets the process-wide override to `true` so later tests cannot inherit `false`.
    it("when ctx_reduce is not registered globally, every session resolves callable=false frozen=true", () => {
        setCtxReduceRegisteredGlobally(false);
        try {
            // The override must force `callable=false` so unregistration reaches guidance, nudges, and `§N§` prefix injection.
            // The override must force `callable=false` so unregistration reaches guidance, nudges, and `§N§` prefix injection.
            clearCtxReduceAvailability("ses-plain-off");
            const verdict = resolveCtxReduceAvailabilityFromMessages("ses-plain-off", [userMsg()]);
            expect(verdict).toEqual({ callable: false, frozen: true });

            // Global registration takes precedence over a per-session `ctx_reduce` allow.
            // Global registration takes precedence over a per-session `ctx_reduce` allow.
            // Global unregistration overrides the per-session map.
            clearCtxReduceAvailability("ses-allow-off");
            const verdictAllow = resolveCtxReduceAvailabilityFromMessages("ses-allow-off", [
                userMsg({ "*": false, ctx_reduce: true }),
            ]);
            expect(verdictAllow).toEqual({ callable: false, frozen: true });
        } finally {
            resetCtxReduceRegisteredGloballyForTest();
        }
    });

    it("the override is specific to ctx_reduce — todowrite is unaffected", () => {
        setCtxReduceRegisteredGlobally(false);
        try {
            clearTodowriteAvailability("ses-td-off");
            const verdict = resolveTodowriteAvailabilityFromMessages("ses-td-off", [userMsg()]);
            expect(verdict).toEqual({ callable: true, frozen: true });
        } finally {
            resetCtxReduceRegisteredGloballyForTest();
        }
    });

    it("when ctx_reduce IS registered globally (default), the per-session tools map decides as before", () => {
        // With `registered=true`, the per-session tools map determines `ctx_reduce` availability.
        clearCtxReduceAvailability("ses-plain-on");
        const verdict = resolveCtxReduceAvailabilityFromMessages("ses-plain-on", [userMsg()]);
        expect(verdict).toEqual({ callable: true, frozen: true });
    });

    // `callable=false` in compaction-off mode closes the nudge gate; no separate mode gate is needed at the nudge site.
    it("nudge gate source: compaction-off resolves callable=false (Channel-1/Channel-2 stay silent)", () => {
        setCtxReduceRegisteredGlobally(false);
        try {
            clearCtxReduceAvailability("ses-nudge-off");
            const verdict = resolveCtxReduceAvailabilityFromMessages("ses-nudge-off", [userMsg()]);
            // `callable=false` prevents the Channel-1 append and Channel-2 claim.
            // `callable=false` prevents the Channel-1 append and Channel-2 claim.
            expect(verdict.callable).toBe(false);
            expect(verdict.frozen).toBe(true);
        } finally {
            resetCtxReduceRegisteredGloballyForTest();
        }
    });
});

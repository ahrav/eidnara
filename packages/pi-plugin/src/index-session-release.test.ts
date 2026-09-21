import { afterEach, describe, expect, it, mock, spyOn } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    __setProjectIdentityTestHooks,
    resolveProjectRootDirectory,
} from "@eidnara/opencode/features/context/project-identity";
import { HostModuleTransport } from "@eidnara/opencode/hooks/context/module-transport";
import { createCountingPi } from "./__tests__/test-utils";
import eidnaraPiExtension, { __test } from "./index";
import {
    isolatePiSessionKernelTokens,
    piSessionTokenCacheForTest,
    resetPiKernelClientsForTest,
} from "./kernel-client-pi";
import { EIDNARA_PI_SUBAGENT_ENV } from "./subagent-runner";

const tempRoots: string[] = [];
const originalEnv = {
    EIDNARA_PI_SUBAGENT: process.env.EIDNARA_PI_SUBAGENT,
    XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
};

function restoreEnv() {
    for (const [key, value] of Object.entries(originalEnv)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
}

function isolateXdgEnv() {
    const root = mkdtempSync(join(tmpdir(), "eidnara-pi-release-test-"));
    tempRoots.push(root);
    process.env.XDG_CONFIG_HOME = join(root, "config");
    process.env.XDG_DATA_HOME = join(root, "data");
}

type SessionHandler = (event: { reason: string }, ctx: unknown) => unknown;

function contextFor(sessionId: string) {
    return { sessionManager: { getSessionId: () => sessionId } };
}

async function registeredHandlers() {
    isolateXdgEnv();
    delete process.env[EIDNARA_PI_SUBAGENT_ENV];
    __test.clearPiEidnaraActive();
    const registrations = createCountingPi();
    await eidnaraPiExtension(registrations.pi);
    const shutdown = registrations.handlers.get("session_shutdown") as SessionHandler | undefined;
    const beforeSwitch = registrations.handlers.get("session_before_switch") as
        | SessionHandler
        | undefined;
    if (!shutdown || !beforeSwitch) throw new Error("session handlers not registered");
    return { shutdown, beforeSwitch, registrations };
}

afterEach(() => {
    restoreEnv();
    __setProjectIdentityTestHooks({});
    __test.clearPiEidnaraActive();
    resetPiKernelClientsForTest();
    for (const root of tempRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe("Pi fork token caches across session lifecycle events", () => {
    it("survives a reload shutdown, which re-creates the extension for the same session", async () => {
        const { shutdown } = await registeredHandlers();
        isolatePiSessionKernelTokens("ses-fork");

        await shutdown({ reason: "reload" }, contextFor("ses-fork"));

        expect(piSessionTokenCacheForTest("ses-fork")).toBeDefined();
    });

    it("is released when the session ends for any other reason", async () => {
        const { shutdown } = await registeredHandlers();
        for (const reason of ["quit", "new", "resume", "fork"]) {
            isolatePiSessionKernelTokens(`ses-${reason}`);
            await shutdown({ reason }, contextFor(`ses-${reason}`));
            expect(piSessionTokenCacheForTest(`ses-${reason}`)).toBeUndefined();
        }
    });

    it("survives a switch away, so a return to the fork keeps its isolation", async () => {
        const { beforeSwitch } = await registeredHandlers();
        isolatePiSessionKernelTokens("ses-fork");

        await beforeSwitch({ reason: "resume" }, contextFor("ses-fork"));

        expect(piSessionTokenCacheForTest("ses-fork")).toBeDefined();
    });
});

describe("Pi daemon transport across runtime teardown", () => {
    const DAY_MS = 24 * 60 * 60 * 1000;

    function captureContext(args: {
        model?: { provider: string; id: string };
        branch?: unknown[];
        setStatus?: ReturnType<typeof mock>;
    }) {
        return {
            cwd: process.cwd(),
            model: args.model,
            sessionManager: {
                getSessionId: () => "capture-status",
                getBranch: () =>
                    args.branch ?? [
                        {
                            type: "message",
                            id: "source-1",
                            message: { role: "user", content: "Production uses port 4567." },
                        },
                    ],
            },
            hasUI: true,
            ui: { setStatus: args.setStatus ?? mock(() => undefined) },
        };
    }

    async function agentEndHandler() {
        const { registrations } = await registeredHandlers();
        const agentEnd = registrations.handlers.get("agent_end");
        if (typeof agentEnd !== "function") throw new Error("agent_end not registered");
        return { agentEnd, registrations };
    }

    it.each([
        true,
        false,
    ])("shows pending capture until the drain reports ready, initial model present: %s", async (hasModel) => {
        let ready = false;
        const methods: string[] = [];
        const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(
            async (input) => {
                methods.push(input.method);
                return {
                    state:
                        input.method === "memory.capture"
                            ? "accepted"
                            : ready
                              ? "ready"
                              : "pending",
                };
            },
        );
        try {
            const { agentEnd } = await agentEndHandler();
            const setStatus = mock(() => undefined);
            const ctx = captureContext({
                model: hasModel ? { provider: "openai", id: "test" } : undefined,
                setStatus,
            });
            await agentEnd({}, ctx);
            expect(methods).toEqual(["memory.capture", "memory.capture.next"]);
            await __test.settleMemoryCapture();
            expect(setStatus).toHaveBeenLastCalledWith(
                "eidnara-capture",
                "Memory capture: pending",
            );
            ready = true;
            ctx.model = { provider: "openai", id: "test" };
            await agentEnd({}, ctx);
            await __test.settleMemoryCapture();
            expect(setStatus).toHaveBeenLastCalledWith("eidnara-capture", undefined);
            expect(methods).toEqual([
                "memory.capture",
                "memory.capture.next",
                "memory.capture.next",
            ]);
        } finally {
            call.mockRestore();
        }
    });

    it("returns from agent_end while the drain is still waiting on the daemon", async () => {
        const next = Promise.withResolvers<unknown>();
        const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(
            async (input) =>
                input.method === "memory.capture.next" ? next.promise : { state: "accepted" },
        );
        try {
            const { agentEnd } = await agentEndHandler();
            const setStatus = mock(() => undefined);
            const ctx = captureContext({ model: { provider: "openai", id: "test" }, setStatus });
            await agentEnd({}, ctx);
            expect(setStatus).not.toHaveBeenCalled();
            next.resolve({ state: "ready" });
            await __test.settleMemoryCapture();
            expect(setStatus).toHaveBeenLastCalledWith("eidnara-capture", undefined);
        } finally {
            call.mockRestore();
        }
    });

    it("marks capture unconfirmed when the drain fails", async () => {
        const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(
            async (input) => ({
                state: input.method === "memory.capture" ? "accepted" : "store_failed",
            }),
        );
        try {
            const { agentEnd } = await agentEndHandler();
            const setStatus = mock(() => undefined);
            const ctx = captureContext({ model: { provider: "openai", id: "test" }, setStatus });
            await agentEnd({}, ctx);
            await __test.settleMemoryCapture();
            expect(setStatus).toHaveBeenLastCalledWith(
                "eidnara-capture",
                "Memory capture: unconfirmed",
            );
        } finally {
            call.mockRestore();
        }
    });

    it("offers only recent branch entries for capture", async () => {
        const bodies: unknown[] = [];
        const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(
            async (input) => {
                if (input.method === "memory.capture") bodies.push(input.body);
                return { state: input.method === "memory.capture" ? "accepted" : "ready" };
            },
        );
        try {
            const { agentEnd } = await agentEndHandler();
            const now = Date.now();
            const ctx = captureContext({
                model: { provider: "openai", id: "test" },
                branch: [
                    {
                        type: "message",
                        id: "stale",
                        timestamp: now - 3 * DAY_MS,
                        message: { role: "user", content: "An old port." },
                    },
                    {
                        type: "message",
                        id: "recent",
                        timestamp: now,
                        message: { role: "user", content: "Production uses port 4567." },
                    },
                ],
            });
            await agentEnd({}, ctx);
            await __test.settleMemoryCapture();
            expect(bodies).toEqual([
                expect.objectContaining({
                    messages: [{ id: "recent", role: "user", text: "Production uses port 4567." }],
                }),
            ]);
        } finally {
            call.mockRestore();
        }
    });

    it("offers only entries appended after the last stored leaf, and rescans after a branch switch", async () => {
        const bodies: Array<{ messages: Array<{ id: string }> }> = [];
        const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(
            async (input) => {
                if (input.method === "memory.capture")
                    bodies.push(input.body as { messages: Array<{ id: string }> });
                return { state: input.method === "memory.capture" ? "accepted" : "ready" };
            },
        );
        const entry = (id: string) => ({
            type: "message",
            id,
            message: { role: "user", content: `Fact ${id}.` },
        });
        try {
            const { agentEnd } = await agentEndHandler();
            const branch = [entry("one"), entry("two")];
            const ctx = captureContext({ model: { provider: "openai", id: "test" }, branch });
            await agentEnd({}, ctx);
            // A stored entry is never read again, even when its object changes in place.
            branch[0].message.content = "Rewritten.";
            branch.push(entry("three"));
            await agentEnd({}, ctx);
            // The stored leaf "three" is gone, so the whole branch is offered; "one" is a
            // repeat of the accepted digest only when its content is unchanged.
            branch.splice(0, branch.length, entry("one"), entry("four"));
            await agentEnd({}, ctx);
            await __test.settleMemoryCapture();
            expect(bodies.map((body) => body.messages.map((message) => message.id))).toEqual([
                ["one", "two"],
                ["three"],
                ["four"],
            ]);
        } finally {
            call.mockRestore();
        }
    });

    it.each([
        "fork",
        "resume",
        "reload",
        "startup",
    ])("offers a session that starts with a branch only the entries appended afterwards (%s)", async (reason) => {
        const bodies: Array<{ messages: Array<{ id: string }> }> = [];
        const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(
            async (input) => {
                if (input.method === "memory.capture")
                    bodies.push(input.body as { messages: Array<{ id: string }> });
                return { state: input.method === "memory.capture" ? "accepted" : "ready" };
            },
        );
        const entry = (id: string) => ({
            type: "message",
            id,
            message: { role: "user", content: `Fact ${id}.` },
        });
        try {
            const { agentEnd, registrations } = await agentEndHandler();
            const sessionStart = registrations.handlers.get("session_start") as SessionHandler;
            // The runtime that produced these entries already offered them, possibly under
            // another project; a fork inherits them under a new session id.
            const branch = [entry("one"), entry("two")];
            const ctx = captureContext({ model: { provider: "openai", id: "test" }, branch });
            await sessionStart({ reason }, ctx);
            branch.push(entry("three"));
            await agentEnd({}, ctx);
            await __test.settleMemoryCapture();
            expect(bodies.map((body) => body.messages.map((message) => message.id))).toEqual([
                ["three"],
            ]);
        } finally {
            call.mockRestore();
        }
    });

    it("offers the same entries again after a checkpoint the daemon did not accept", async () => {
        let accept = false;
        const bodies: Array<{ messages: Array<{ id: string }> }> = [];
        const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(
            async (input) => {
                // The drain runs after the refused checkpoint too and finds nothing pending;
                // its `ready` must not clear the warning for the entries that were never stored.
                if (input.method !== "memory.capture") return { state: "ready" };
                bodies.push(input.body as { messages: Array<{ id: string }> });
                return { state: accept ? "accepted" : "store_failed" };
            },
        );
        try {
            const { agentEnd } = await agentEndHandler();
            const setStatus = mock(() => undefined);
            const ctx = captureContext({ model: { provider: "openai", id: "test" }, setStatus });
            await agentEnd({}, ctx);
            await __test.settleMemoryCapture();
            expect(setStatus).toHaveBeenLastCalledWith(
                "eidnara-capture",
                "Memory capture: unconfirmed",
            );
            accept = true;
            await agentEnd({}, ctx);
            await __test.settleMemoryCapture();
            expect(setStatus).toHaveBeenLastCalledWith("eidnara-capture", undefined);
            expect(bodies.map((body) => body.messages.map((message) => message.id))).toEqual([
                ["source-1"],
                ["source-1"],
            ]);
        } finally {
            call.mockRestore();
        }
    });

    it("offers the same entries again after a checkpoint the daemon reported disabled", async () => {
        let enabled = false;
        const bodies: Array<{ messages: Array<{ id: string }> }> = [];
        const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(
            async (input) => {
                if (input.method !== "memory.capture") return { state: "ready" };
                bodies.push(input.body as { messages: Array<{ id: string }> });
                return { state: enabled ? "accepted" : "disabled" };
            },
        );
        try {
            const { agentEnd } = await agentEndHandler();
            const setStatus = mock(() => undefined);
            const ctx = captureContext({ model: { provider: "openai", id: "test" }, setStatus });
            await agentEnd({}, ctx);
            // A disabled daemon wrote nothing; that is not a failure to report.
            expect(setStatus).not.toHaveBeenCalledWith(
                "eidnara-capture",
                "Memory capture: unconfirmed",
            );
            enabled = true;
            await agentEnd({}, ctx);
            await __test.settleMemoryCapture();
            expect(bodies.map((body) => body.messages.map((message) => message.id))).toEqual([
                ["source-1"],
                ["source-1"],
            ]);
        } finally {
            call.mockRestore();
        }
    });

    it("keeps the unconfirmed status through a later disabled checkpoint", async () => {
        let state = "store_failed";
        const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(
            async (input) => ({ state: input.method === "memory.capture" ? state : "disabled" }),
        );
        try {
            const { agentEnd } = await agentEndHandler();
            const setStatus = mock(() => undefined);
            const ctx = captureContext({ model: { provider: "openai", id: "test" }, setStatus });
            await agentEnd({}, ctx);
            await __test.settleMemoryCapture();
            // A disabled daemon stored nothing either; the refused entries are still unconfirmed.
            state = "disabled";
            await agentEnd({}, ctx);
            await __test.settleMemoryCapture();
            expect(setStatus).toHaveBeenLastCalledWith(
                "eidnara-capture",
                "Memory capture: unconfirmed",
            );
        } finally {
            call.mockRestore();
        }
    });

    it("drains after a checkpoint the daemon refused, so a full queue can make progress", async () => {
        const methods: string[] = [];
        const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(
            async (input) => {
                methods.push(input.method);
                return { state: input.method === "memory.capture" ? "queue_full" : "ready" };
            },
        );
        try {
            const { agentEnd } = await agentEndHandler();
            const ctx = captureContext({ model: { provider: "openai", id: "test" } });
            await agentEnd({}, ctx);
            await __test.settleMemoryCapture();
            expect(methods).toEqual(["memory.capture", "memory.capture.next"]);
        } finally {
            call.mockRestore();
        }
    });

    it("captures nothing from the home directory unless allow_home_project opts in", async () => {
        // The session's checkout root is the user's home directory; the default config keeps it
        // opted out.
        const home = resolveProjectRootDirectory(process.cwd());
        __setProjectIdentityTestHooks({ homeDirectory: () => home });
        const methods: string[] = [];
        const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(
            async (input) => {
                methods.push(input.method);
                return { state: "accepted" };
            },
        );
        try {
            const { agentEnd } = await agentEndHandler();
            const setStatus = mock(() => undefined);
            const ctx = captureContext({ model: { provider: "openai", id: "test" }, setStatus });
            await agentEnd({}, ctx);
            await __test.settleMemoryCapture();
            expect(methods).toEqual([]);
            expect(setStatus).not.toHaveBeenCalled();
        } finally {
            call.mockRestore();
        }
    });

    it("disconnects the runtime's transport on session_shutdown, for a reload and for a quit", async () => {
        for (const reason of ["reload", "quit"]) {
            const disconnect = spyOn(
                HostModuleTransport.prototype,
                "disconnect",
            ).mockImplementation(() => undefined);
            try {
                const { shutdown } = await registeredHandlers();
                expect(disconnect).not.toHaveBeenCalled();
                await shutdown({ reason }, contextFor(`ses-${reason}`));
                expect(disconnect).toHaveBeenCalledTimes(1);
            } finally {
                disconnect.mockRestore();
            }
        }
    });

    it("stops an in-flight capture drain on session_shutdown so nothing dials the disconnected transport", async () => {
        const next = Promise.withResolvers<unknown>();
        const calls: Array<{ method: string; afterDisconnect: boolean; error?: unknown }> = [];
        let disconnected = false;
        const call = spyOn(HostModuleTransport.prototype, "call").mockImplementation(
            async (input) => {
                calls.push({
                    method: input.method,
                    afterDisconnect: disconnected,
                    error: (input.body as { error?: unknown } | undefined)?.error,
                });
                if (input.method === "memory.capture.next") return next.promise;
                return { state: "accepted" };
            },
        );
        const disconnect = spyOn(HostModuleTransport.prototype, "disconnect").mockImplementation(
            () => {
                disconnected = true;
            },
        );
        try {
            const { agentEnd, registrations } = await agentEndHandler();
            const shutdown = registrations.handlers.get("session_shutdown") as SessionHandler;
            const setStatus = mock(() => undefined);
            const ctx = captureContext({ model: { provider: "openai", id: "test" }, setStatus });
            await agentEnd({}, ctx);
            const closing = shutdown({ reason: "reload" }, ctx);
            // Shutdown waits for the drain to stop, so the daemon's late reply lands while the
            // transport is still connected; the returned lease is released, never executed.
            await new Promise((resolve) => setTimeout(resolve, 0));
            expect(disconnect).not.toHaveBeenCalled();
            next.resolve({
                state: "work",
                lease: "a".repeat(32),
                model: "openai/test",
                system: "Extract facts.",
                prompt: "A fact.",
                max_output_tokens: 8192,
                max_output_bytes: 131072,
                max_duration_ms: 90000,
            });
            await closing;
            expect(disconnect).toHaveBeenCalledTimes(1);
            await __test.settleMemoryCapture();
            expect(calls.filter((entry) => entry.afterDisconnect)).toEqual([]);
            expect(
                calls
                    .filter((entry) => entry.method === "memory.capture.submit")
                    .map((e) => e.error),
            ).toEqual(["cancelled"]);
            expect(setStatus).not.toHaveBeenCalledWith(
                "eidnara-capture",
                "Memory capture: unconfirmed",
            );
        } finally {
            call.mockRestore();
            disconnect.mockRestore();
        }
    });

    it("keeps the transport connected across a session switch", async () => {
        const disconnect = spyOn(HostModuleTransport.prototype, "disconnect").mockImplementation(
            () => undefined,
        );
        try {
            const { beforeSwitch } = await registeredHandlers();
            await beforeSwitch({ reason: "resume" }, contextFor("ses-switch"));
            expect(disconnect).not.toHaveBeenCalled();
        } finally {
            disconnect.mockRestore();
        }
    });
});

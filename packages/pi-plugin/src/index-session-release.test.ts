import { afterEach, describe, expect, it, mock, spyOn } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
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
    it.each([
        true,
        false,
    ])("keeps capture unconfirmed until flushed, initial model present: %s", async (hasModel) => {
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
            const { registrations } = await registeredHandlers();
            const agentEnd = registrations.handlers.get("agent_end");
            if (typeof agentEnd !== "function") throw new Error("agent_end not registered");
            const setStatus = mock(() => undefined);
            const ctx = {
                cwd: process.cwd(),
                model: hasModel ? { provider: "openai", id: "test" } : undefined,
                sessionManager: {
                    getSessionId: () => "capture-status",
                    getBranch: () => [
                        {
                            type: "message",
                            id: "source-1",
                            message: { role: "user", content: "Production uses port 4567." },
                        },
                    ],
                },
                hasUI: true,
                ui: { setStatus },
            };
            await agentEnd({}, ctx);
            expect(setStatus).toHaveBeenLastCalledWith(
                "eidnara-capture",
                "Memory capture: unconfirmed",
            );
            ready = true;
            ctx.model = { provider: "openai", id: "test" };
            await agentEnd({}, ctx);
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

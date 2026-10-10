import { afterEach, describe, expect, it, mock } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { __setProjectIdentityTestHooks } from "@eidnara/opencode/features/context/project-identity";
import type { default as IndexExtension, __test as IndexTest } from "./index";
import { resetPiKernelClientsForTest } from "./kernel-client-pi";
import { EIDNARA_PI_SUBAGENT_ENV } from "./subagent-runner";

const tempRoots: string[] = [];
const originalEnv = {
    EIDNARA_PI_SUBAGENT: process.env.EIDNARA_PI_SUBAGENT,
    XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
};

type Handler = (event: unknown, ctx: unknown) => unknown;

function orderedPi() {
    const handlers = new Map<string, Handler[]>();
    const pi = {
        on: (event: string, handler: Handler) => {
            handlers.set(event, [...(handlers.get(event) ?? []), handler]);
        },
        registerTool: () => undefined,
        registerFlag: () => undefined,
        registerCommand: () => undefined,
        registerEntryRenderer: () => undefined,
        appendEntry: () => undefined,
        sendMessage: () => undefined,
        sendUserMessage: () => undefined,
    } as unknown as ExtensionAPI;
    return { pi, handlers };
}

function branchReader(branch: Array<{ id: string; [key: string]: unknown }>) {
    return {
        getBranch: () => {
            throw new Error("memory capture called getBranch");
        },
        getLeafId: () => branch.at(-1)?.id ?? null,
        getEntry: (id: string) => {
            const index = branch.findIndex((entry) => entry.id === id);
            if (index < 0) return undefined;
            return { ...branch[index], parentId: index > 0 ? branch[index - 1]?.id : null };
        },
    };
}

/** Each run announces every refusal reason once, so the test loads its own instance of the extension module. */
async function freshExtension(): Promise<{
    extension: typeof IndexExtension;
    __test: typeof IndexTest;
}> {
    const module = (await import("./index.ts?start-notice-order")) as {
        default: typeof IndexExtension;
        __test: typeof IndexTest;
    };
    return { extension: module.default, __test: module.__test };
}

let activeTest: typeof IndexTest | undefined;

afterEach(() => {
    for (const [key, value] of Object.entries(originalEnv)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
    __setProjectIdentityTestHooks({});
    activeTest?.clearPiEidnaraActive();
    resetPiKernelClientsForTest();
    for (const root of tempRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe("Pi start notice delivery order", () => {
    it("shows a refusal raised by this turn's memory capture before agent_end returns", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-pi-start-notice-"));
        tempRoots.push(root);
        process.env.XDG_CONFIG_HOME = join(root, "config");
        process.env.XDG_DATA_HOME = join(root, "data");
        delete process.env[EIDNARA_PI_SUBAGENT_ENV];
        const { extension, __test } = await freshExtension();
        activeTest = __test;
        __test.clearPiEidnaraActive();
        const { pi, handlers } = orderedPi();
        await extension(pi);
        const agentEnds = handlers.get("agent_end") ?? [];
        expect(agentEnds.length).toBeGreaterThan(1);

        const notify = mock((_text: string, _level: string) => undefined);
        const ctx = {
            cwd: process.cwd(),
            model: { provider: "openai", id: "test" },
            sessionManager: {
                getSessionId: () => "start-notice-order",
                ...branchReader([
                    {
                        id: "source-1",
                        type: "message",
                        message: { role: "user", content: "Production uses port 4567." },
                    },
                ]),
            },
            hasUI: true,
            ui: { setStatus: mock(() => undefined), notify },
        };
        for (const handler of agentEnds) await handler({}, ctx);
        await __test.settleMemoryCapture();

        expect(notify).toHaveBeenCalledTimes(1);
        expect(notify.mock.calls[0]?.[0]).toContain("unsupported_install_layout");
        expect(notify.mock.calls[0]?.[1]).toBe("warning");
    });
});

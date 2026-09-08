import { afterEach, beforeEach, describe, expect, spyOn, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    __resetProjectIdentityForTests,
    __setProjectIdentityTestHooks,
} from "./features/context/project-identity";
import * as rpcServer from "./shared/rpc-server";

let importCounter = 0;

async function freshPluginServer() {
    const module = (await import(`./index.ts?rpc-project-gate=${importCounter++}`)) as {
        default: { server: (ctx: unknown) => Promise<Record<string, unknown>> };
    };
    return module.default.server;
}

function fakeClient() {
    return {
        config: {
            get: async () => ({ data: { compaction: { auto: false, prune: false } } }),
            providers: async () => ({ data: { providers: [] } }),
        },
        session: {
            list: async () => [],
            messages: async () => ({ data: [] }),
        },
    };
}

async function dispose(hooks: Record<string, unknown>, directory: string): Promise<void> {
    const event = hooks.event as (input: { event: unknown }) => Promise<void>;
    await event({ event: { type: "server.instance.disposed", properties: { directory } } });
}

describe("RPC server project gate in the plugin entry", () => {
    let rpcStartSpy: ReturnType<typeof spyOn>;
    let directory: string;
    let configHome: string;
    let dataHome: string;
    const savedEnv: Record<string, string | undefined> = {};

    beforeEach(() => {
        directory = mkdtempSync(join(tmpdir(), "eidnara-rpc-gate-project-"));
        configHome = mkdtempSync(join(tmpdir(), "eidnara-rpc-gate-config-"));
        dataHome = mkdtempSync(join(tmpdir(), "eidnara-rpc-gate-data-"));
        for (const key of ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "EIDNARA_BROCA_CHILD"]) {
            savedEnv[key] = process.env[key];
        }
        process.env.XDG_CONFIG_HOME = configHome;
        process.env.XDG_DATA_HOME = dataHome;
        delete process.env.EIDNARA_BROCA_CHILD;
        rpcStartSpy = spyOn(rpcServer.EidnaraRpcServer.prototype, "start").mockImplementation(
            async () => {},
        );
    });

    afterEach(() => {
        rpcStartSpy.mockRestore();
        __setProjectIdentityTestHooks({});
        __resetProjectIdentityForTests();
        for (const [key, value] of Object.entries(savedEnv)) {
            if (value === undefined) delete process.env[key];
            else process.env[key] = value;
        }
        for (const dir of [directory, configHome, dataHome]) {
            rmSync(dir, { recursive: true, force: true });
        }
    });

    test("a directory with no project identity gets hooks that refuse it and no RPC server", async () => {
        // The default `allow_home_project: false` refuses the user's home directory as a project.
        __setProjectIdentityTestHooks({ homeDirectory: () => directory });
        const server = await freshPluginServer();
        const hooks = await server({ directory, client: fakeClient() });
        try {
            expect(rpcStartSpy).not.toHaveBeenCalled();
        } finally {
            await dispose(hooks, directory);
        }
    });

    test("a bound project starts the RPC server", async () => {
        __setProjectIdentityTestHooks({ homeDirectory: () => join(tmpdir(), "not-the-project") });
        const server = await freshPluginServer();
        const hooks = await server({ directory, client: fakeClient() });
        try {
            expect(rpcStartSpy).toHaveBeenCalledTimes(1);
        } finally {
            await dispose(hooks, directory);
        }
    });
});

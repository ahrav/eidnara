import { afterEach, beforeEach, describe, expect, spyOn, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HostModuleTransport } from "./hooks/context/module-transport";
import * as rpcServer from "./shared/rpc-server";

let importCounter = 0;

async function freshPluginServer() {
    const module = (await import(`./index.ts?instance-dispose=${importCounter++}`)) as {
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
    // The disposal event settles only after capture has stopped and the transport is gone.
    await event({ event: { type: "server.instance.disposed", properties: { directory } } });
}

describe("daemon transport teardown on instance disposal", () => {
    let rpcStartSpy: ReturnType<typeof spyOn>;
    let disconnectSpy: ReturnType<typeof spyOn>;
    let directory: string;
    let otherDirectory: string;
    let configHome: string;
    let dataHome: string;
    const savedEnv: Record<string, string | undefined> = {};

    beforeEach(() => {
        directory = mkdtempSync(join(tmpdir(), "eidnara-dispose-project-"));
        otherDirectory = mkdtempSync(join(tmpdir(), "eidnara-dispose-other-project-"));
        configHome = mkdtempSync(join(tmpdir(), "eidnara-dispose-config-"));
        dataHome = mkdtempSync(join(tmpdir(), "eidnara-dispose-data-"));
        for (const key of ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "EIDNARA_MODEL_EXECUTION_CHILD"]) {
            savedEnv[key] = process.env[key];
        }
        process.env.XDG_CONFIG_HOME = configHome;
        process.env.XDG_DATA_HOME = dataHome;
        delete process.env.EIDNARA_MODEL_EXECUTION_CHILD;
        rpcStartSpy = spyOn(rpcServer.EidnaraRpcServer.prototype, "start").mockImplementation(
            async () => {},
        );
        // The plugin's client wraps a `HostModuleTransport`, so its `disconnect` reaches the prototype method.
        disconnectSpy = spyOn(HostModuleTransport.prototype, "disconnect");
    });

    afterEach(() => {
        disconnectSpy.mockRestore();
        rpcStartSpy.mockRestore();
        for (const [key, value] of Object.entries(savedEnv)) {
            if (value === undefined) delete process.env[key];
            else process.env[key] = value;
        }
        for (const dir of [directory, otherDirectory, configHome, dataHome]) {
            rmSync(dir, { recursive: true, force: true });
        }
    });

    test("disposing the instance's own directory disconnects its transport, another directory does not", async () => {
        {
            const server = await freshPluginServer();
            const hooks = await server({ directory, client: fakeClient() });
            expect(rpcStartSpy).toHaveBeenCalledTimes(1);

            await dispose(hooks, otherDirectory);
            expect(disconnectSpy).not.toHaveBeenCalled();

            await dispose(hooks, directory);
            expect(disconnectSpy).toHaveBeenCalledTimes(1);
        }
    });
});

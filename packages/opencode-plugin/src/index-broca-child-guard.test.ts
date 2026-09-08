import { afterEach, beforeEach, describe, expect, spyOn, test } from "bun:test";
import * as config from "./config";
import * as rpcServer from "./shared/rpc-server";

// EIDNARA_BROCA_CHILD="1" returns before config load.
const CHILD_GUARD_SENTINEL = "CHILD-GUARD-SIDE-EFFECT";

let importCounter = 0;

async function freshPluginServer() {
    const module = (await import(`./index.ts?broca-guard=${importCounter++}`)) as {
        default: { id: string; server: (ctx: unknown) => Promise<unknown> };
    };
    return module.default.server;
}

function minimalCtx() {
    return { directory: "/tmp/broca-guard-test", client: {} };
}

describe("Broca-child guard in the plugin entry", () => {
    let configLoadSpy: ReturnType<typeof spyOn>;
    let rpcStartSpy: ReturnType<typeof spyOn>;
    let previousGuard: string | undefined;

    beforeEach(() => {
        previousGuard = process.env.EIDNARA_BROCA_CHILD;
        // The spy replaces the live ESM binding, so `index.ts` sees the sentinel; `mockRestore` keeps the real loader for other test files in the same process.
        configLoadSpy = spyOn(config, "loadPluginConfigDetailed").mockImplementation(() => {
            throw new Error(CHILD_GUARD_SENTINEL);
        });
        rpcStartSpy = spyOn(rpcServer.EidnaraRpcServer.prototype, "start");
    });

    afterEach(() => {
        if (previousGuard === undefined) delete process.env.EIDNARA_BROCA_CHILD;
        else process.env.EIDNARA_BROCA_CHILD = previousGuard;
        configLoadSpy.mockRestore();
        rpcStartSpy.mockRestore();
    });

    test("EIDNARA_BROCA_CHILD=1 returns before config load, hooks, and RPC", async () => {
        process.env.EIDNARA_BROCA_CHILD = "1";
        const server = await freshPluginServer();

        const timerSpy = spyOn(globalThis, "setTimeout");
        try {
            const hooks = await server(minimalCtx());
            expect(hooks).toEqual({});
            expect(configLoadSpy).not.toHaveBeenCalled();
            expect(rpcStartSpy).not.toHaveBeenCalled();
            expect(timerSpy).not.toHaveBeenCalled();
        } finally {
            timerSpy.mockRestore();
        }
    });

    test('guard values other than "1" do not trip the guard', async () => {
        for (const value of ["0", "true", ""]) {
            process.env.EIDNARA_BROCA_CHILD = value;
            configLoadSpy.mockClear();
            const server = await freshPluginServer();
            await expect(server(minimalCtx())).rejects.toThrow(CHILD_GUARD_SENTINEL);
            expect(configLoadSpy).toHaveBeenCalledTimes(1);
        }
    });

    test("without the guard, ordinary startup proceeds into config load", async () => {
        delete process.env.EIDNARA_BROCA_CHILD;
        const server = await freshPluginServer();

        // Unguarded startup reaches config load rather than returning at the child guard.
        await expect(server(minimalCtx())).rejects.toThrow(CHILD_GUARD_SENTINEL);
        expect(configLoadSpy).toHaveBeenCalledTimes(1);
        expect(rpcStartSpy).not.toHaveBeenCalled();
    });
});

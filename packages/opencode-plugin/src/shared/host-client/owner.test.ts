import { afterEach, describe, expect, spyOn, test } from "bun:test";
import { HostClient } from "./client";
import { processHostClient, resetProcessHostClientsForTest } from "./owner";

afterEach(() => {
    resetProcessHostClientsForTest();
});

describe("processHostClient", () => {
    test("shares one connection attempt per publication path", async () => {
        const connectionFile = `/tmp/missing-eidnara-host-${crypto.randomUUID()}.json`;
        const first = processHostClient({ connectionFile });
        const second = processHostClient({ connectionFile });

        expect(first).toBe(second);
        await expect(first).rejects.toThrow();
    });

    test("callers with different construction options do not share a client", async () => {
        const connectionFile = `/tmp/missing-eidnara-host-${crypto.randomUUID()}.json`;
        // A probe that presents no credentials must not hand its client to a
        // caller that depends on credential fingerprints reaching route.open.
        const probe = processHostClient({ connectionFile, requestTimeoutMs: 2_000 });
        const withCredentials = processHostClient({
            connectionFile,
            credentialSource: { EXAMPLE_API_KEY: "value" },
        });
        const differentTimeout = processHostClient({
            connectionFile,
            requestTimeoutMs: 30_000,
        });

        expect(withCredentials).not.toBe(probe);
        expect(differentTimeout).not.toBe(probe);
        await expect(probe).rejects.toThrow();
        await expect(withCredentials).rejects.toThrow();
        await expect(differentTimeout).rejects.toThrow();
    });

    test("owners keyed by handshake budget keep a generous lane off a tight one", async () => {
        // Clients with different handshake budgets must not share an owner:
        // whichever dialed first would fix the budget for the other.
        const connectionFile = `/tmp/missing-eidnara-host-${crypto.randomUUID()}.json`;
        const provider = processHostClient({ connectionFile, handshakeTimeoutMs: 10_000 });
        const hook = processHostClient({ connectionFile, handshakeTimeoutMs: 2_000 });

        expect(provider).not.toBe(hook);
        await expect(provider).rejects.toThrow();
        await expect(hook).rejects.toThrow();
    });

    test("an identical option set still shares one attempt", async () => {
        const connectionFile = `/tmp/missing-eidnara-host-${crypto.randomUUID()}.json`;
        const credentialSource = { EXAMPLE_API_KEY: "value" };
        const first = processHostClient({
            connectionFile,
            credentialSource,
            requestTimeoutMs: 2_000,
        });
        const second = processHostClient({
            connectionFile,
            credentialSource,
            requestTimeoutMs: 2_000,
        });

        expect(first).toBe(second);
        await expect(first).rejects.toThrow();
    });

    test("keeps owners for distinct publication paths separate", async () => {
        const firstClient = { isClosed: false, closeAsync: async () => {} } as HostClient;
        const secondClient = { isClosed: false, closeAsync: async () => {} } as HostClient;
        const connect = spyOn(HostClient, "connect")
            .mockResolvedValueOnce(firstClient)
            .mockResolvedValueOnce(secondClient);
        const firstPath = `/tmp/eidnara-host-${crypto.randomUUID()}.json`;
        const secondPath = `/tmp/eidnara-host-${crypto.randomUUID()}.json`;

        const first = processHostClient({ connectionFile: firstPath });
        const second = processHostClient({ connectionFile: secondPath });

        expect(first).not.toBe(second);
        expect(await first).toBe(firstClient);
        expect(await second).toBe(secondClient);
        expect(connect).toHaveBeenNthCalledWith(
            1,
            expect.objectContaining({ connectionFile: firstPath }),
        );
        expect(connect).toHaveBeenNthCalledWith(
            2,
            expect.objectContaining({ connectionFile: secondPath }),
        );
        connect.mockRestore();
    });

    test("forgets a connection attempt that fails before publication", async () => {
        const connectionFile = `/tmp/missing-eidnara-host-${crypto.randomUUID()}.json`;
        const first = processHostClient({ connectionFile });
        await expect(first).rejects.toThrow();

        const second = processHostClient({ connectionFile });
        expect(second).not.toBe(first);
        await expect(second).rejects.toThrow();
    });

    test("evicts an irreversibly closed owner without splitting live owners", async () => {
        const firstClient = { isClosed: false, closeAsync: async () => {} } as HostClient;
        const secondClient = { isClosed: false, closeAsync: async () => {} } as HostClient;
        const connect = spyOn(HostClient, "connect")
            .mockResolvedValueOnce(firstClient)
            .mockResolvedValueOnce(secondClient);
        const connectionFile = `/tmp/eidnara-host-${crypto.randomUUID()}.json`;

        const first = processHostClient({ connectionFile });
        expect(processHostClient({ connectionFile })).toBe(first);
        await first;
        Object.defineProperty(firstClient, "isClosed", { value: true });

        expect(await processHostClient({ connectionFile })).toBe(secondClient);
        expect(connect).toHaveBeenCalledTimes(2);
        connect.mockRestore();
    });
});

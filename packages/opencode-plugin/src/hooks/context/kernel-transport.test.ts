import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    isAvailable,
    KernelClient,
    type KernelTransport,
    type KernelTransportCall,
    TokenCache,
} from "../../shared/kernel-client";
import {
    closeKernelSession,
    createKernelClient,
    createKernelTransport,
    MAX_CONNECTION_FILE_STATES,
    MAX_TOKEN_CACHE_PROJECTS,
    resetKernelClientsForTest,
    sharedConnectionFilesForTest,
    sharedModuleForTest,
} from "./kernel-transport";
import { HostModuleTransport, type ManagedDemandStart } from "./module-transport";

const PROJECT = "/repo/project";
const SESSION = "session-a";
const MISSING_CONNECTION_FILE = "/tmp/kernel-transport-test-missing-connection.json";

const demandStartNeverInvoked: ManagedDemandStart = () => {
    throw new Error("demand start must not run during gating");
};

function managedTransport(): HostModuleTransport {
    return new HostModuleTransport({ demandStart: demandStartNeverInvoked });
}

function explicitTransport(): HostModuleTransport {
    return new HostModuleTransport({
        connectionFile: MISSING_CONNECTION_FILE,
        demandStart: demandStartNeverInvoked,
    });
}

/** Reuses the created transport's gate while scripting `call`, so the client exercises the real reachability answer. */
function recordingTransport(kernelTransport: KernelTransport): {
    transport: KernelTransport;
    calls: KernelTransportCall[];
} {
    const calls: KernelTransportCall[] = [];
    return {
        calls,
        transport: {
            ...kernelTransport,
            async call(args: KernelTransportCall): Promise<unknown> {
                calls.push(args);
                return {
                    state: { kind: "available" },
                    known_as_of: 1,
                    tip: 1,
                    gated: false,
                    rows: [],
                };
            },
        },
    };
}

function client(transport: KernelTransport): KernelClient {
    return new KernelClient({ transport, enabled: true, sessionId: SESSION, projectRoot: PROJECT });
}

describe("createKernelTransport reachability gate", () => {
    test("a managed transport with demand start is reachable before its connection file exists", () => {
        const transport = managedTransport();
        expect(transport.canDemandStart()).toBe(true);
        expect(createKernelTransport(transport).connectionFileExists()).toBe(true);
    });

    test("an explicit connection file never demand-starts, so a missing file is unreachable", () => {
        const transport = explicitTransport();
        expect(transport.canDemandStart()).toBe(false);
        expect(createKernelTransport(transport).connectionFileExists()).toBe(false);
    });

    test("a kernel call on a managed demand-start transport reaches the transport", async () => {
        const { transport, calls } = recordingTransport(createKernelTransport(managedTransport()));
        const result = await client(transport).read({ surface: "auto_inject" });
        expect(isAvailable(result)).toBe(true);
        expect(calls.map((call) => call.method)).toEqual(["kernel.read"]);
    });

    test("a kernel call on an explicit origin with a missing file is daemon_absent without a dial", async () => {
        const { transport, calls } = recordingTransport(createKernelTransport(explicitTransport()));
        const result = await client(transport).read({ surface: "auto_inject" });
        expect(result.state).toEqual({ kind: "unavailable", reason: "daemon_absent" });
        expect(calls).toEqual([]);
    });
});

describe("createKernelTransport method guard", () => {
    test("a body whose encoded method differs from the checked method is refused before any dial", async () => {
        const transport = createKernelTransport(managedTransport());
        await expect(
            transport.call({
                sessionId: SESSION,
                projectRoot: PROJECT,
                method: "kernel.read",
                body: { method: "session.delete", v: 1, session_id: SESSION },
            }),
        ).rejects.toThrow(/encoded method is not kernel\.read/);
        await expect(
            transport.call({
                sessionId: SESSION,
                projectRoot: PROJECT,
                method: "kernel.read",
                body: "not a record",
            }),
        ).rejects.toThrow(/encoded method is not kernel\.read/);
    });

    test("a non-kernel method is refused before any dial", async () => {
        const transport = createKernelTransport(managedTransport());
        await expect(
            transport.call({
                sessionId: SESSION,
                projectRoot: PROJECT,
                method: "session.delete",
                body: { method: "session.delete", v: 1, session_id: SESSION },
            }),
        ).rejects.toThrow(/refuses non-kernel method session\.delete/);
    });
});

describe("createKernelTransport store lifecycle translation", () => {
    function storageTransport(storage: "starting" | "unavailable"): HostModuleTransport {
        return new HostModuleTransport({
            demandStart: async () => ({ ok: true, reason: "ready", storage }),
        });
    }

    test("a managed daemon whose store is starting reads as unavailable:store_starting", async () => {
        const result = await client(createKernelTransport(storageTransport("starting"))).read({
            surface: "auto_inject",
        });
        expect(result.state).toEqual({ kind: "unavailable", reason: "store_starting" });
    });

    test("a managed daemon whose store is unavailable reads as unavailable:store_unavailable", async () => {
        const result = await client(createKernelTransport(storageTransport("unavailable"))).read({
            surface: "auto_inject",
        });
        expect(result.state).toEqual({ kind: "unavailable", reason: "store_unavailable" });
    });
});

describe("shared transport eviction", () => {
    afterEach(() => {
        resetKernelClientsForTest();
    });

    const files = Array.from(
        { length: MAX_CONNECTION_FILE_STATES + 1 },
        (_, index) => `/tmp/kernel-transport-test-missing-${index}.json`,
    );
    const config = (file: string) => ({ subc: { connection_file: file } });
    const keyOf = (file: string) => `explicit:${file}`;

    test("a client that outlives its shared state's eviction re-resolves through the map instead of redialing outside the cap", async () => {
        const stale = createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: config(files[0] as string),
        });
        for (const file of files.slice(1)) {
            createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config: config(file) });
        }
        expect(sharedConnectionFilesForTest()).toEqual(files.slice(1).map(keyOf));

        const result = await stale.read({ surface: "auto_inject" });
        expect(result.state).toEqual({ kind: "unavailable", reason: "daemon_absent" });
        // The stale client's call recreated its connection file's state through the map, so the cap evicted the next-oldest entry rather than a ninth transport living on outside it.
        expect(sharedConnectionFilesForTest()).toEqual(
            [...files.slice(2), files[0] as string].map(keyOf),
        );
    });

    test("a retained client's token view follows every replacement state, so no token crosses from one daemon to the next", async () => {
        const stale = createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: config(files[0] as string),
        });
        stale.tokens.rememberTokens(PROJECT, [{ object_id: "mem_a", known_as_of: 1 }], 1);
        expect(stale.tokens.get(PROJECT, "mem_a")).toBeDefined();

        // First eviction: the retained client sees the replacement state's empty cache.
        for (const file of files.slice(1)) {
            createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config: config(file) });
        }
        expect(stale.tokens.get(PROJECT, "mem_a")).toBeUndefined();

        // The retained client's use recreates its state; a fresh client for the same file shares that cache.
        stale.tokens.rememberTokens(PROJECT, [{ object_id: "mem_b", known_as_of: 2 }], 2);
        const fresh = createKernelClient({
            sessionId: "session-b",
            projectRoot: PROJECT,
            config: config(files[0] as string),
        });
        expect(fresh.tokens.get(PROJECT, "mem_b")).toEqual({ object_id: "mem_b", known_as_of: 2 });

        // Second eviction of the same connection file: the retained client still holds nothing from the evicted daemon.
        for (const file of files.slice(1)) {
            createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config: config(file) });
        }
        expect(sharedConnectionFilesForTest()).not.toContain(keyOf(files[0] as string));
        expect(stale.tokens.get(PROJECT, "mem_b")).toBeUndefined();
        expect(fresh.tokens.get(PROJECT, "mem_b")).toBeUndefined();
        const result = await stale.read({ surface: "auto_inject" });
        expect(result.state).toEqual({ kind: "unavailable", reason: "daemon_absent" });
    });

    test("the managed default and an explicit empty path never share a state", () => {
        createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config: {} });
        createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: { subc: { connection_file: "" } },
        });
        expect(sharedConnectionFilesForTest()).toEqual(["managed-default", "explicit:"]);
    });

    test("tokens minted before the transport reconnects are discarded, since the daemon behind the connection file may have changed", () => {
        const cfg = config(files[0] as string);
        const first = createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config: cfg });
        first.tokens.rememberTokens(PROJECT, [{ object_id: "mem_a", known_as_of: 7 }], 7);
        expect(first.tokens.knownAsOfFor(PROJECT)).toBe(7);

        const shared = sharedModuleForTest(cfg);
        if (!shared) throw new Error("the resolved client must have a shared transport");
        // The same invalidation a daemon restart triggers inside the transport.
        shared.disconnect();

        expect(first.tokens.get(PROJECT, "mem_a")).toBeUndefined();
        expect(first.tokens.knownAsOfFor(PROJECT)).toBeUndefined();
        // A lower position from the new daemon is accepted rather than being shadowed by the stale higher one.
        first.tokens.rememberTokens(PROJECT, [{ object_id: "mem_a", known_as_of: 2 }], 2);
        expect(first.tokens.get(PROJECT, "mem_a")).toEqual({ object_id: "mem_a", known_as_of: 2 });
        const second = createKernelClient({
            sessionId: "session-b",
            projectRoot: PROJECT,
            config: cfg,
        });
        expect(second.tokens.get(PROJECT, "mem_a")).toEqual({ object_id: "mem_a", known_as_of: 2 });
    });

    test("disabled clients occupy no shared-state slot and answer disabled", async () => {
        const live = createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: config(files[0] as string),
        });
        for (const file of files.slice(1)) {
            createKernelClient({
                sessionId: SESSION,
                projectRoot: PROJECT,
                config: { ...config(file), memory: { enabled: false } },
            });
        }
        expect(sharedConnectionFilesForTest()).toEqual([keyOf(files[0] as string)]);
        expect(live.tokens).toBe(
            createKernelClient({
                sessionId: "session-b",
                projectRoot: PROJECT,
                config: config(files[0] as string),
            }).tokens,
        );

        const disabled = createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: { ...config(files[1] as string), memory: { enabled: false } },
        });
        const result = await disabled.read({ surface: "auto_inject" });
        expect(result.state).toEqual({ kind: "disabled" });
    });

    test("buckets a retained client fills in a replacement state stay under the project cap", () => {
        const roots = Array.from(
            { length: MAX_TOKEN_CACHE_PROJECTS + 1 },
            (_, index) => `/repo/retained-${index}`,
        );
        const retained = roots.map((root) =>
            createKernelClient({
                sessionId: SESSION,
                projectRoot: root,
                config: config(files[0] as string),
            }),
        );
        // Evict the state every retained client was resolved against.
        for (const file of files.slice(1)) {
            createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config: config(file) });
        }
        // Each retained client's write recreates and fills the replacement state through its view, never through `createKernelClient`.
        retained.forEach((client, index) => {
            client.tokens.rememberTokens(
                roots[index] as string,
                [{ object_id: "mem", known_as_of: index + 1 }],
                index + 1,
            );
        });
        // The oldest bucket was tracked and evicted when the cap was exceeded; the newest survives.
        expect(retained[0]?.tokens.get(roots[0] as string, "mem")).toBeUndefined();
        expect(
            retained[MAX_TOKEN_CACHE_PROJECTS]?.tokens.get(
                roots[MAX_TOKEN_CACHE_PROJECTS] as string,
                "mem",
            ),
        ).toEqual({ object_id: "mem", known_as_of: MAX_TOKEN_CACHE_PROJECTS + 1 });
    });
});

describe("closeKernelSession", () => {
    afterEach(() => {
        resetKernelClientsForTest();
    });

    test("releases the shared transport's routes for the session and is a no-op for an unresolved connection file", () => {
        const config = { subc: { connection_file: MISSING_CONNECTION_FILE } };
        createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config });
        const shared = sharedModuleForTest(config);
        if (!shared) throw new Error("the resolved client must have a shared transport");
        const closed: string[] = [];
        shared.closeSession = (sessionId: string) => {
            closed.push(sessionId);
        };

        closeKernelSession(config, SESSION);
        closeKernelSession({ subc: { connection_file: "/tmp/never-resolved.json" } }, SESSION);

        expect(closed).toEqual([SESSION]);
    });
});

describe("shared-path project root canonicalization", () => {
    let dir = "";

    beforeEach(() => {
        dir = mkdtempSync(join(tmpdir(), "kernel-transport-canonical-"));
        mkdirSync(join(dir, "real"));
        symlinkSync(join(dir, "real"), join(dir, "link"));
    });

    afterEach(() => {
        resetKernelClientsForTest();
        rmSync(dir, { recursive: true, force: true });
    });

    test("a symlinked spelling resolves to the same token bucket as the resolved spelling", () => {
        const resolved = realpathSync.native(join(dir, "real"));
        const link = join(dir, "link");
        const config = { subc: { connection_file: MISSING_CONNECTION_FILE } };
        const tokens = createKernelClient({
            sessionId: SESSION,
            projectRoot: resolved,
            config,
        }).tokens;
        tokens.rememberTokens(resolved, [{ object_id: "mem_a", known_as_of: 1 }], 1);
        for (let index = 1; index < MAX_TOKEN_CACHE_PROJECTS; index += 1) {
            createKernelClient({ sessionId: SESSION, projectRoot: `/repo/other-${index}`, config });
        }
        // Resolving through the symlink must touch the resolved root's bucket rather than open a new one; a new one would push the cache past the cap and evict the resolved root.
        createKernelClient({ sessionId: SESSION, projectRoot: link, config });
        expect(tokens.get(resolved, "mem_a")).toEqual({ object_id: "mem_a", known_as_of: 1 });
    });
});

/** A transport that never dials; token-cache scoping is decided before any call. */
function inertTransport(): KernelTransport {
    return {
        connectionFileExists: () => true,
        call: () => Promise.reject(new Error("no call expected")),
        ensureRoute: async () => {},
    };
}

describe("createKernelClient token-cache scoping", () => {
    afterEach(() => {
        resetKernelClientsForTest();
    });

    test("a custom transport without explicit tokens gets a fresh cache per client", () => {
        const transport = inertTransport();
        const first = createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: {},
            transport,
        });
        const second = createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: {},
            transport,
        });
        expect(first.tokens).not.toBe(second.tokens);
    });

    test("explicit tokens keep continuity across clients on one custom transport", () => {
        const transport = inertTransport();
        const tokens = new TokenCache();
        const first = createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: {},
            transport,
            tokens,
        });
        const second = createKernelClient({
            sessionId: "session-b",
            projectRoot: PROJECT,
            config: {},
            transport,
            tokens,
        });
        expect(first.tokens).toBe(tokens);
        expect(second.tokens).toBe(tokens);
    });

    test("shared-path clients for one connection file share one token cache", () => {
        const first = createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config: {} });
        const second = createKernelClient({
            sessionId: "session-b",
            projectRoot: "/repo/other",
            config: {},
        });
        expect(first.tokens).toBe(second.tokens);
    });

    test("the shared cache drops the least-recently-resolved project's tokens past the cap", () => {
        const roots = Array.from(
            { length: MAX_TOKEN_CACHE_PROJECTS + 1 },
            (_, index) => `/repo/project-${index}`,
        );
        const tokens = createKernelClient({
            sessionId: SESSION,
            projectRoot: roots[0],
            config: {},
        }).tokens;
        tokens.rememberTokens(roots[0], [{ object_id: "mem_a", known_as_of: 1 }], 1);
        createKernelClient({ sessionId: SESSION, projectRoot: roots[1], config: {} });
        tokens.rememberTokens(roots[1], [{ object_id: "mem_b", known_as_of: 2 }], 2);
        for (let index = 2; index < MAX_TOKEN_CACHE_PROJECTS; index += 1) {
            createKernelClient({ sessionId: SESSION, projectRoot: roots[index], config: {} });
        }
        // Re-resolving the first root marks it most recent, so overflow evicts the second.
        createKernelClient({ sessionId: SESSION, projectRoot: roots[0], config: {} });
        createKernelClient({
            sessionId: SESSION,
            projectRoot: roots[MAX_TOKEN_CACHE_PROJECTS],
            config: {},
        });
        expect(tokens.get(roots[0], "mem_a")).toEqual({ object_id: "mem_a", known_as_of: 1 });
        expect(tokens.get(roots[1], "mem_b")).toBeUndefined();
    });

    test("explicit tokens bypass the shared cache's eviction order", () => {
        const shared = createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: {},
        }).tokens;
        shared.rememberTokens(PROJECT, [{ object_id: "mem_a", known_as_of: 1 }], 1);
        for (let index = 0; index <= MAX_TOKEN_CACHE_PROJECTS; index += 1) {
            createKernelClient({
                sessionId: SESSION,
                projectRoot: `/repo/isolated-${index}`,
                config: {},
                tokens: new TokenCache(),
            });
        }
        expect(shared.get(PROJECT, "mem_a")).toEqual({ object_id: "mem_a", known_as_of: 1 });
    });
});

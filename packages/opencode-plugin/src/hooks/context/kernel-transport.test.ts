import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { HostCallError } from "../../shared/host-client";
import {
    ConnectionIdentityChangedError,
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
    sharedStateForTest,
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

    test("the daemon's terminal store_unavailable answer while its store opens reads as unavailable:store_unavailable", async () => {
        const module = managedTransport();
        module.call = async () => {
            throw new HostCallError("terminal", "store is opening", "store_unavailable");
        };
        const result = await client(createKernelTransport(module)).read({
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

        const shared = sharedStateForTest(cfg)?.module;
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
        const shared = sharedStateForTest(config)?.module;
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

describe("createKernelTransport connection identity", () => {
    test("reports the module's generation and refuses a body built under an older one before any dial", async () => {
        const module = managedTransport();
        const transport = createKernelTransport(module);
        expect(transport.connectionIdentity?.()).toBe("0");
        module.disconnect();
        expect(transport.connectionIdentity?.()).toBe("1");
        await expect(
            transport.call({
                sessionId: SESSION,
                projectRoot: PROJECT,
                method: "kernel.read",
                body: { method: "kernel.read", v: 1 },
                connectionIdentity: "0",
            }),
        ).rejects.toBeInstanceOf(ConnectionIdentityChangedError);
    });

    test("sends generation-sensitive with the expected generation and turns the module's generation-changed answer into a refusal", async () => {
        const module = managedTransport();
        const seen: Array<{ generationSensitive?: boolean; expectedGeneration?: number }> = [];
        module.call = async (args) => {
            seen.push({
                generationSensitive: args.generationSensitive,
                expectedGeneration: args.expectedGeneration,
            });
            return {
                transport_status: "connection_generation_changed",
                previous_generation: 0,
                current_generation: 1,
            };
        };
        const transport = createKernelTransport(module);
        await expect(
            transport.call({
                sessionId: SESSION,
                projectRoot: PROJECT,
                method: "kernel.read",
                body: { method: "kernel.read", v: 1 },
            }),
        ).rejects.toBeInstanceOf(ConnectionIdentityChangedError);
        expect(seen).toEqual([{ generationSensitive: true, expectedGeneration: 0 }]);
    });
});

describe("shared-path connection identity", () => {
    let connectionFile = "";

    beforeEach(() => {
        connectionFile = join(
            mkdtempSync(join(tmpdir(), "kernel-transport-identity-")),
            "connection.json",
        );
        writeFileSync(connectionFile, "{}");
    });

    afterEach(() => {
        resetKernelClientsForTest();
        rmSync(dirname(connectionFile), { recursive: true, force: true });
    });

    test("a daemon turnover discovered inside a call is refused, and the client's retry is built under the new generation", async () => {
        const config = { subc: { connection_file: connectionFile } };
        const kernel = createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config });
        const shared = sharedStateForTest(config);
        if (!shared) throw new Error("the resolved client must have a shared transport");
        const { module } = shared;
        const generationsSeen: number[] = [];
        module.call = async () => {
            generationsSeen.push(module.generation);
            if (module.generation === 0) {
                module.disconnect();
                return {
                    transport_status: "connection_generation_changed",
                    previous_generation: 0,
                    current_generation: 1,
                };
            }
            return { state: { kind: "available" }, known_as_of: 1, tip: 1, gated: false, rows: [] };
        };

        const result = await kernel.read({ surface: "auto_inject" });

        expect(result.state).toEqual({ kind: "available" });
        expect(generationsSeen).toEqual([0, 1]);
    });

    test("a state with a call in flight is never evicted; the cap trims it once the call settles", async () => {
        const config = { subc: { connection_file: connectionFile } };
        const kernel = createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config });
        const shared = sharedStateForTest(config);
        if (!shared) throw new Error("the resolved client must have a shared transport");
        const { module } = shared;
        let settle: ((value: unknown) => void) | undefined;
        module.call = () => new Promise((resolve) => (settle = resolve));

        const pending = kernel.read({ surface: "auto_inject" });
        await Bun.sleep(0);
        expect(settle).toBeDefined();

        for (let index = 0; index < MAX_CONNECTION_FILE_STATES; index += 1) {
            createKernelClient({
                sessionId: SESSION,
                projectRoot: PROJECT,
                config: {
                    subc: { connection_file: `/tmp/kernel-transport-test-missing-${index}.json` },
                },
            });
        }
        // The busy state is skipped and the next idle one is evicted instead, so the busy module stays connected and reachable for a session close.
        expect(sharedStateForTest(config)?.module).toBe(module);
        expect(sharedConnectionFilesForTest()).toHaveLength(MAX_CONNECTION_FILE_STATES);
        expect(sharedConnectionFilesForTest()).not.toContain(
            "explicit:/tmp/kernel-transport-test-missing-0.json",
        );
        expect(module.generation).toBe(0);

        settle?.({
            state: { kind: "available" },
            known_as_of: 4,
            tip: 4,
            gated: false,
            rows: [
                {
                    object: {
                        object_id: "mem_a",
                        object_kind: "decision",
                        domain_id: "memory",
                        source_kind: "assistant",
                        source_id: "memory-lineage",
                        source_revision: 1,
                        created_commit_seq: 1,
                        invalidated_commit_seq: null,
                        superseded_by: null,
                        sensitivity: "normal",
                    },
                    visibility: "labeled",
                    labeled: true,
                    scope_id: "project:x",
                    token: { object_id: "mem_a", known_as_of: 4 },
                    decision: { decision_kind: "memory", payload: { summary: "s", rationale: "" } },
                },
            ],
        });
        const result = await pending;
        expect(result.state).toEqual({ kind: "available" });
        // The response's tokens landed in the state that served the call.
        expect(kernel.tokens.get(PROJECT, "mem_a")).toEqual({ object_id: "mem_a", known_as_of: 4 });

        // The next resolution trims the now-idle state.
        createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: { subc: { connection_file: "/tmp/kernel-transport-test-missing-extra.json" } },
        });
        expect(sharedStateForTest(config)).toBeUndefined();
        expect(sharedConnectionFilesForTest()).toHaveLength(MAX_CONNECTION_FILE_STATES);
        expect(module.generation).toBe(1);
    });

    test("a token write that names a superseded connection identity is dropped", () => {
        const config = { subc: { connection_file: connectionFile } };
        const kernel = createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config });
        const shared = sharedStateForTest(config);
        if (!shared) throw new Error("the resolved client must have a shared transport");
        const before = shared.transport.connectionIdentity?.();
        shared.module.disconnect();
        const after = shared.transport.connectionIdentity?.();
        expect(after).not.toBe(before);

        kernel.tokens.rememberTokens(PROJECT, [{ object_id: "stale", known_as_of: 9 }], 9, before);
        kernel.tokens.rememberTokens(PROJECT, [{ object_id: "fresh", known_as_of: 1 }], 1, after);

        expect(kernel.tokens.get(PROJECT, "stale")).toBeUndefined();
        expect(kernel.tokens.get(PROJECT, "fresh")).toEqual({ object_id: "fresh", known_as_of: 1 });
    });

    test("a state resolved while every other state is busy is kept, so the map overflows the cap instead of dropping it", async () => {
        const dir = dirname(connectionFile);
        const files = Array.from({ length: MAX_CONNECTION_FILE_STATES }, (_, index) =>
            join(dir, `busy-${index}.json`),
        );
        const settlers: Array<(value: unknown) => void> = [];
        const pending: Promise<unknown>[] = [];
        for (const file of files) {
            writeFileSync(file, "{}");
            const config = { subc: { connection_file: file } };
            const kernel = createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config });
            const shared = sharedStateForTest(config);
            if (!shared) throw new Error("the resolved client must have a shared transport");
            shared.module.call = () => new Promise((resolve) => settlers.push(resolve));
            pending.push(kernel.read({ surface: "auto_inject" }));
        }
        await Bun.sleep(0);
        expect(settlers).toHaveLength(MAX_CONNECTION_FILE_STATES);

        const config = { subc: { connection_file: connectionFile } };
        createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config });
        expect(sharedStateForTest(config)).toBeDefined();
        expect(sharedConnectionFilesForTest()).toHaveLength(MAX_CONNECTION_FILE_STATES + 1);

        for (const settle of settlers) {
            settle({
                state: { kind: "available" },
                known_as_of: 1,
                tip: 1,
                gated: false,
                rows: [],
            });
        }
        await Promise.all(pending);
        // The next new state trims back to the cap by dropping idle older states; the kept state survives.
        createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: { subc: { connection_file: join(dir, "extra.json") } },
        });
        expect(sharedConnectionFilesForTest()).toHaveLength(MAX_CONNECTION_FILE_STATES);
        expect(sharedStateForTest(config)).toBeDefined();
    });

    test("a view's identity changes when its state is evicted and replaced, so a body built before the eviction is refused", async () => {
        const config = { subc: { connection_file: connectionFile } };
        createKernelClient({ sessionId: SESSION, projectRoot: PROJECT, config });
        const shared = sharedStateForTest(config);
        if (!shared) throw new Error("the resolved client must have a shared transport");
        const view = shared.transport;
        const identityBefore = view.connectionIdentity?.();
        expect(identityBefore).toMatch(/^\d+:0$/);

        for (let index = 0; index < MAX_CONNECTION_FILE_STATES; index += 1) {
            createKernelClient({
                sessionId: SESSION,
                projectRoot: PROJECT,
                config: {
                    subc: { connection_file: `/tmp/kernel-transport-test-missing-${index}.json` },
                },
            });
        }
        expect(sharedStateForTest(config)).toBeUndefined();

        // The replacement module also starts at generation zero; only the epoch tells the two apart.
        const identityAfter = view.connectionIdentity?.();
        expect(identityAfter).toMatch(/^\d+:0$/);
        expect(identityAfter).not.toBe(identityBefore);
        await expect(
            view.call({
                sessionId: SESSION,
                projectRoot: PROJECT,
                method: "kernel.read",
                body: { method: "kernel.read", v: 1 },
                connectionIdentity: identityBefore as string,
            }),
        ).rejects.toBeInstanceOf(ConnectionIdentityChangedError);
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

    test("explicit tokens on the shared transport stay apart from the shared cache", () => {
        const tokens = new TokenCache();
        const isolated = createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: {},
            tokens,
        });
        const shared = createKernelClient({
            sessionId: "session-b",
            projectRoot: PROJECT,
            config: {},
        });
        isolated.tokens.rememberTokens(PROJECT, [{ object_id: "mem_a", known_as_of: 7 }], 7);
        expect(tokens.get(PROJECT, "mem_a")).toEqual({ object_id: "mem_a", known_as_of: 7 });
        expect(shared.tokens.get(PROJECT, "mem_a")).toBeUndefined();
        shared.tokens.rememberTokens(PROJECT, [{ object_id: "mem_b", known_as_of: 3 }], 3);
        expect(isolated.tokens.get(PROJECT, "mem_b")).toBeUndefined();
    });

    test("explicit tokens on the shared transport are emptied when the connection is replaced", () => {
        const tokens = new TokenCache();
        const client = createKernelClient({
            sessionId: SESSION,
            projectRoot: PROJECT,
            config: {},
            tokens,
        });
        client.tokens.rememberTokens(PROJECT, [{ object_id: "mem_a", known_as_of: 7 }], 7);
        expect(client.tokens.knownAsOfFor(PROJECT)).toBe(7);

        const shared = sharedStateForTest({})?.module;
        if (!shared) throw new Error("the resolved client must have a shared transport");
        // The same invalidation a daemon restart triggers inside the transport.
        shared.disconnect();

        expect(client.tokens.get(PROJECT, "mem_a")).toBeUndefined();
        expect(client.tokens.knownAsOfFor(PROJECT)).toBeUndefined();
        expect(tokens.knownAsOfFor(PROJECT)).toBeUndefined();
        // A lower position from the new daemon is accepted rather than being shadowed by the stale higher one.
        client.tokens.rememberTokens(PROJECT, [{ object_id: "mem_a", known_as_of: 2 }], 2);
        expect(client.tokens.get(PROJECT, "mem_a")).toEqual({ object_id: "mem_a", known_as_of: 2 });
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
});

import { describe, expect, test } from "bun:test";
import {
    mkdirSync,
    mkdtempSync,
    readFileSync,
    realpathSync,
    rmSync,
    symlinkSync,
    unlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import hostRelease from "../../../../../release/host-release.json";
import productionInputs from "../../../../../release/production-inputs.lock.json";
import {
    BROCA_CREDENTIAL_NAMES,
    Deadline,
    HostCallError,
    type HostClient,
    type HostClientOptions,
    type RouteHandle,
    StaleRouteHandleError,
    sameDaemonId,
} from "../../shared/host-client";
import { WaiterDetachedError } from "../../shared/host-lifecycle/policy";
import {
    __moduleTransportTest,
    buildManagedStartupEnvelope,
    HostModuleTransport,
    harnessForParentPackage,
} from "./module-transport";

const REPO_ROOT = join(import.meta.dir, "../../../../..");

type TransportInternals = {
    client: HostClient | null;
    connectionPromise: Promise<unknown> | null;
    connectionCertification: { expectedDaemonId?: Uint8Array } | null;
    connectionGeneration: number;
    nextProbeMs: number;
    clientOptions(deadline?: Deadline): HostClientOptions;
    canonicalRoot(root: string): string;
    invalidateConnection(client?: HostClient | null): Promise<void>;
    ensureConnected(
        deadline: Deadline,
        signal?: AbortSignal,
    ): Promise<{ client: HostClient; expectedDaemonId?: Uint8Array }>;
    ensureRoute(
        sessionId: string,
        projectRoot: string,
        deadline: Deadline,
        signal?: AbortSignal,
    ): Promise<unknown>;
    call: HostModuleTransport["call"];
    closeSession: HostModuleTransport["closeSession"];
};

function internals(transport: HostModuleTransport): TransportInternals {
    return transport as unknown as TransportInternals;
}

describe("HostModuleTransport shared connection wait", () => {
    test("each caller stops waiting at its own deadline without cancelling the shared flight", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        const shared = new Promise<never>(() => {});
        transport.connectionPromise = shared;

        const outcome = await Promise.race([
            transport.ensureConnected(Deadline.start(5)).catch((error: unknown) => error),
            Bun.sleep(50).then(() => "still_waiting"),
        ]);

        expect(outcome).toMatchObject({ code: "ETIMEDOUT" });
        expect(transport.connectionPromise).toBe(shared);
    });

    test("each caller aborts its own wait without cancelling the shared flight", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        const shared = new Promise<never>(() => {});
        transport.connectionPromise = shared;
        const controller = new AbortController();
        const reason = new Error("caller aborted");

        const waiting = transport.ensureConnected(Deadline.start(1_000), controller.signal);
        controller.abort(reason);

        await expect(waiting).rejects.toBe(reason);
        expect(transport.connectionPromise).toBe(shared);
    });

    // A second flight started after another caller's dial completed would overwrite `client`
    // unowned and make the first caller reject its otherwise valid response as a connection change.
    test("a demand that resumes after another caller connected adopts the live client instead of dialing", async () => {
        const daemonId = Uint8Array.from([1, 2, 3, 4]);
        const live = { authenticated: { daemonId } } as unknown as HostClient;
        let dialed = false;
        const transport = internals(
            new HostModuleTransport({
                demandStart: async () => {
                    // The faster caller finishes its dial while this demand is awaiting.
                    transport.client = live;
                    transport.connectionCertification = {
                        expectedDaemonId: Uint8Array.from(daemonId),
                    };
                    return { ok: true, storage: "ready", authenticatedDaemonId: daemonId };
                },
            }),
        );
        transport.clientOptions = () => {
            dialed = true;
            throw new Error("a second dial must not start");
        };

        const joined = await transport.ensureConnected(Deadline.start(1_000));

        expect(joined.client).toBe(live);
        expect(sameDaemonId(joined.expectedDaemonId, daemonId)).toBe(true);
        expect(dialed).toBe(false);
        expect(transport.client).toBe(live);
    });

    test("a live client whose daemon differs from the resumed demand is invalidated, not adopted", async () => {
        const live = {
            authenticated: { daemonId: Uint8Array.from([9, 9, 9, 9]) },
            closeAsync: async () => {},
        } as unknown as HostClient;
        const transport = internals(
            new HostModuleTransport({
                demandStart: async () => {
                    transport.client = live;
                    return {
                        ok: true,
                        storage: "ready",
                        authenticatedDaemonId: Uint8Array.from([1, 2, 3, 4]),
                    };
                },
            }),
        );
        transport.clientOptions = () => {
            throw new Error("a second dial must not start");
        };

        await expect(transport.ensureConnected(Deadline.start(1_000))).rejects.toMatchObject({
            code: "ECONNRESET",
        });
        expect(transport.client).toBeNull();
    });
});

describe("waiter detach is not a connection failure", () => {
    // A detached waiter carries ETIMEDOUT so callers classifying retryability on `code` see a
    // timeout. If that also classified as a connection failure, the `call` catch would
    // invalidate the shared connection and bump the generation, and the still-connecting
    // owner's candidate would be evicted -- one caller's deadline would abort the connect for
    // every caller waiting on the same flight.
    test("a deadline detach does not classify as a connection failure", () => {
        const detached = new WaiterDetachedError("deadline");
        expect(detached.code).toBe("ETIMEDOUT");
        expect(__moduleTransportTest.isConnectionFailure(detached)).toBe(false);
    });

    test("an abort detach does not classify as a connection failure", () => {
        expect(__moduleTransportTest.isConnectionFailure(new WaiterDetachedError("aborted"))).toBe(
            false,
        );
    });

    // The exclusion must be the class, not the code: a genuine ETIMEDOUT still invalidates.
    test("a plain ETIMEDOUT still classifies as a connection failure", () => {
        const error = Object.assign(new Error("connect timed out"), { code: "ETIMEDOUT" });
        expect(__moduleTransportTest.isConnectionFailure(error)).toBe(true);
    });
});

describe("module identity and send deadline", () => {
    test("the default module id is the daemon's default module id", () => {
        const rustSource = readFileSync(join(REPO_ROOT, "crates/daemon/src/lib.rs"), "utf8");
        const match = rustSource.match(/pub const DEFAULT_MODULE_ID: &str = "([a-z_-]+)";/);
        expect(match?.[1]).toBe(__moduleTransportTest.DEFAULT_MODULE_ID);
        expect(__moduleTransportTest.DEFAULT_MODULE_ID).toBe("context");
    });

    test("a transform send has one effective deadline of five seconds", async () => {
        expect(__moduleTransportTest.TRANSFORM_SEND_TIMEOUT_MS).toBe(5_000);
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        transport.connectionPromise = new Promise<never>(() => {});
        const outcome = await Promise.race([
            transport.ensureConnected(Deadline.start(5)).catch((error: unknown) => error),
            Bun.sleep(50).then(() => "still_waiting"),
        ]);
        expect(outcome).toMatchObject({ code: "ETIMEDOUT" });
    });

    test("a caller-supplied timeout shortens but never lifts the transform cap", async () => {
        const observed = new Map<string, number>();
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        transport.ensureRoute = async (sessionId, _projectRoot, deadline) => {
            observed.set(sessionId, deadline.remainingMs());
            throw new Error("stop before dialing");
        };
        const call = (sessionId: string, timeoutMs: number) =>
            transport
                .call({
                    sessionId,
                    projectRoot: "/tmp",
                    method: "transform",
                    body: { method: "transform" },
                    timeoutMs,
                })
                .catch(() => undefined);

        await call("longer", 15_000);
        await call("shorter", 1_000);

        expect(observed.get("longer")).toBeLessThanOrEqual(
            __moduleTransportTest.TRANSFORM_SEND_TIMEOUT_MS,
        );
        expect(observed.get("longer")).toBeGreaterThan(4_000);
        expect(observed.get("shorter")).toBeLessThanOrEqual(1_000);
    });
});

describe("route keys follow the filesystem", () => {
    test("a retargeted symlink root keys its new target, and a missing root keeps its last one", () => {
        const base = mkdtempSync(join(tmpdir(), "eidnara-transport-root-"));
        try {
            const projectA = join(base, "a");
            const projectB = join(base, "b");
            const current = join(base, "current");
            mkdirSync(projectA);
            mkdirSync(projectB);
            symlinkSync(projectA, current);
            const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
            expect(transport.canonicalRoot(current)).toBe(realpathSync.native(projectA));

            unlinkSync(current);
            symlinkSync(projectB, current);
            expect(transport.canonicalRoot(current)).toBe(realpathSync.native(projectB));

            unlinkSync(current);
            expect(transport.canonicalRoot(current)).toBe(realpathSync.native(projectB));
            expect(transport.canonicalRoot(join(base, "never-existed"))).toBe(
                join(base, "never-existed"),
            );
        } finally {
            rmSync(base, { recursive: true, force: true });
        }
    });
});

describe("transient route-open rejections retry inside the deadline", () => {
    test("an allowlisted terminal code is retried and a later success is returned", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        const route = { channel: 4, epoch: 1 } as unknown as RouteHandle;
        let attempts = 0;
        const client = {
            routeOpen: async () => {
                attempts += 1;
                if (attempts < 3) {
                    throw new HostCallError("terminal", "module is reloading", "module_reloading");
                }
                return route;
            },
        } as unknown as HostClient;
        transport.client = client;
        transport.ensureConnected = async () => ({ client });

        const ensured = await transport.ensureRoute("s", "/tmp", Deadline.start(5_000));
        expect(ensured).toMatchObject({ route });
        expect(attempts).toBe(3);
    });

    test("a non-allowlisted terminal code is not retried", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        let attempts = 0;
        const client = {
            routeOpen: async () => {
                attempts += 1;
                throw new HostCallError("terminal", "no such target", "unknown_target");
            },
        } as unknown as HostClient;
        transport.client = client;
        transport.ensureConnected = async () => ({ client });

        await expect(
            transport.ensureRoute("s", "/tmp", Deadline.start(5_000)),
        ).rejects.toMatchObject({ code: "unknown_target" });
        expect(attempts).toBe(1);
    });
});

describe("a local close wins over recovery", () => {
    test("a body is not replayed after closeSession fenced the in-flight opening", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        let opens = 0;
        let finishOpen: ((route: RouteHandle) => void) | undefined;
        const client = {
            routeOpen: () =>
                new Promise<RouteHandle>((resolve) => {
                    opens += 1;
                    finishOpen = resolve;
                }),
            closeRoute: async () => {},
            request: async () => ({ ok: true }),
        } as unknown as HostClient;
        transport.client = client;
        transport.ensureConnected = async () => ({ client });

        const call = transport.call({
            sessionId: "s",
            projectRoot: "/tmp",
            method: "session.status",
            body: { method: "session.status" },
        });
        await Bun.sleep(0);
        expect(opens).toBe(1);
        transport.closeSession("s");
        finishOpen?.({ channel: 9, epoch: 1 } as unknown as RouteHandle);

        await expect(call).rejects.toMatchObject({ code: "session_closed" });
        expect(opens).toBe(1);
    });

    test("a close during connection setup stops the body before it is written", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        let requests = 0;
        const route = { channel: 9, epoch: 1 } as unknown as RouteHandle;
        const client = {
            request: async () => {
                requests += 1;
                return { ok: true };
            },
        } as unknown as HostClient;
        transport.client = client;
        let finishSetup: (() => void) | undefined;
        transport.ensureRoute = async (sessionId) => {
            await new Promise<void>((resolve) => {
                finishSetup = resolve;
            });
            return { client, route, routeKey: `${sessionId}\0/tmp`, generation: 0 };
        };

        const call = transport.call({
            sessionId: "s",
            projectRoot: "/tmp",
            method: "session.delete",
            body: { method: "session.delete" },
        });
        await Bun.sleep(0);
        transport.closeSession("s");
        finishSetup?.();

        await expect(call).rejects.toMatchObject({ code: "session_closed" });
        expect(requests).toBe(0);
    });

    test("a call queued behind the lane when the session closes does not send", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        let requests = 0;
        let finishFirst: ((value: unknown) => void) | undefined;
        const route = { channel: 9, epoch: 1 } as unknown as RouteHandle;
        const client = {
            request: () => {
                requests += 1;
                return new Promise<unknown>((resolve) => {
                    finishFirst = resolve;
                });
            },
        } as unknown as HostClient;
        transport.client = client;
        transport.ensureRoute = async (sessionId) => ({
            client,
            route,
            routeKey: `${sessionId}\0/tmp`,
            generation: 0,
        });

        const first = transport
            .call({
                sessionId: "s",
                projectRoot: "/tmp",
                method: "session.status",
                body: { method: "session.status" },
            })
            .catch((error: unknown) => error);
        await Bun.sleep(0);
        expect(requests).toBe(1);
        const queued = transport
            .call({
                sessionId: "s",
                projectRoot: "/tmp",
                method: "session.delete",
                body: { method: "session.delete" },
            })
            .catch((error: unknown) => error);
        await Bun.sleep(0);
        transport.closeSession("s");
        finishFirst?.({ ok: true });

        // `closeSession` invalidates the connection under the in-flight call, so its late response is discarded too.
        expect(await first).toMatchObject({ code: "session_closed" });
        expect(await queued).toMatchObject({ code: "session_closed" });
        expect(requests).toBe(1);
    });
});

describe("credential rotation during a route bind", () => {
    test("the route binds and is cached under one credential snapshot even if the environment moves mid-bind", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        const credentialName = BROCA_CREDENTIAL_NAMES[0] as string;
        const previous = process.env[credentialName];
        const sources: Array<Record<string, string | undefined> | undefined> = [];
        const opened: RouteHandle[] = [];
        const client = {
            routeOpen: async (
                _target: unknown,
                _identity: unknown,
                options: { credentialSource?: Record<string, string | undefined> },
            ) => {
                sources.push(options.credentialSource);
                const route = { channel: opened.length + 1, epoch: 1 } as unknown as RouteHandle;
                opened.push(route);
                // A rotation lands while the bind is in flight and reverts before it settles (ABA).
                process.env[credentialName] = `${previous ?? ""}rotated`;
                await Bun.sleep(0);
                if (previous === undefined) delete process.env[credentialName];
                else process.env[credentialName] = previous;
                return route;
            },
            closeRoute: async () => {},
        } as unknown as HostClient;
        transport.client = client;
        transport.ensureConnected = async () => ({ client });
        try {
            const ensured = await transport.ensureRoute("s", "/tmp", Deadline.start(5_000));
            expect(opened).toHaveLength(1);
            expect(ensured).toMatchObject({ route: opened[0] });
            // The facade received a frozen snapshot, not the live environment.
            expect(sources[0]).toBeDefined();
            expect(Object.isFrozen(sources[0])).toBe(true);
            expect(sources[0]?.[credentialName]).toBe(previous);
            const again = await transport.ensureRoute("s", "/tmp", Deadline.start(5_000));
            expect(again).toMatchObject({ route: opened[0] });
            expect(opened).toHaveLength(1);
        } finally {
            if (previous === undefined) delete process.env[credentialName];
            else process.env[credentialName] = previous;
        }
    });
});

describe("route opening observes the caller's abort", () => {
    test("an abort while routeOpen is pending settles the caller and lets the open finish into the cache", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        let finishOpen: ((route: RouteHandle) => void) | undefined;
        const route = { channel: 3, epoch: 1 } as unknown as RouteHandle;
        const client = {
            routeOpen: () =>
                new Promise<RouteHandle>((resolve) => {
                    finishOpen = resolve;
                }),
        } as unknown as HostClient;
        transport.client = client;
        transport.ensureConnected = async () => ({ client });

        const controller = new AbortController();
        const reason = new Error("caller gave up");
        const waiting = transport.ensureRoute(
            "s",
            "/tmp",
            Deadline.start(10_000),
            controller.signal,
        );
        controller.abort(reason);
        await expect(waiting).rejects.toBe(reason);

        expect(finishOpen).toBeDefined();
        finishOpen?.(route);
        await Bun.sleep(0);
        const cached = await transport.ensureRoute("s", "/tmp", Deadline.start(10_000));
        expect(cached).toMatchObject({ route });
    });

    test("a joiner of a shared opening times out on its own deadline", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        const client = {
            routeOpen: () => new Promise<RouteHandle>(() => {}),
        } as unknown as HostClient;
        transport.client = client;
        transport.ensureConnected = async () => ({ client });

        const controller = new AbortController();
        const first = transport.ensureRoute("s", "/tmp", Deadline.start(30_000), controller.signal);
        controller.abort(new Error("first gave up"));
        await first.catch(() => undefined);

        const joined = await Promise.race([
            transport.ensureRoute("s", "/tmp", Deadline.start(20)).catch((error: unknown) => error),
            Bun.sleep(500).then(() => "still_waiting"),
        ]);
        expect(joined).toMatchObject({ code: "ETIMEDOUT" });
    });

    test("an opening bound under older credentials is fenced, not joined", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        const opens: Array<(route: RouteHandle) => void> = [];
        const closed: RouteHandle[] = [];
        const client = {
            routeOpen: () =>
                new Promise<RouteHandle>((resolve) => {
                    opens.push(resolve);
                }),
            closeRoute: async (route: RouteHandle) => {
                closed.push(route);
            },
        } as unknown as HostClient;
        transport.client = client;
        transport.ensureConnected = async () => ({ client });
        const credentialName = BROCA_CREDENTIAL_NAMES[0] as string;
        const previous = process.env[credentialName];
        try {
            const controller = new AbortController();
            const first = transport.ensureRoute(
                "s",
                "/tmp",
                Deadline.start(10_000),
                controller.signal,
            );
            controller.abort(new Error("first gave up"));
            await first.catch(() => undefined);
            expect(opens).toHaveLength(1);

            process.env[credentialName] = `${previous ?? ""}rotated`;
            const second = transport.ensureRoute("s", "/tmp", Deadline.start(10_000));
            await Bun.sleep(0);
            expect(opens).toHaveLength(2);

            const staleRoute = { channel: 1, epoch: 1 } as unknown as RouteHandle;
            const freshRoute = { channel: 2, epoch: 1 } as unknown as RouteHandle;
            opens[0]?.(staleRoute);
            opens[1]?.(freshRoute);
            expect(await second).toMatchObject({ route: freshRoute });
            await Bun.sleep(0);
            expect(closed).toEqual([staleRoute]);
        } finally {
            if (previous === undefined) delete process.env[credentialName];
            else process.env[credentialName] = previous;
        }
    });
});

describe("possibly sent bodies fence the session lane", () => {
    test("the lane stays held until the superseded connection's teardown settles", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        let releaseTeardown: (() => void) | undefined;
        const teardown = new Promise<void>((resolve) => {
            releaseTeardown = resolve;
        });
        transport.invalidateConnection = () => teardown;
        const route = { channel: 7, epoch: 1 } as unknown as RouteHandle;
        const client = {
            request: () =>
                Promise.reject(
                    new HostCallError("outcome_unknown", "deadline after send", "request_deadline"),
                ),
        } as unknown as HostClient;
        transport.client = client;
        transport.ensureRoute = async () => ({
            client,
            route,
            routeKey: "s\0/tmp",
            generation: 0,
        });

        await expect(
            transport.call({
                sessionId: "s",
                projectRoot: "/tmp",
                method: "session.status",
                body: { method: "session.status" },
            }),
        ).rejects.toMatchObject({ kind: "outcome_unknown" });

        let secondSettled = false;
        const second = transport
            .call({
                sessionId: "s",
                projectRoot: "/tmp",
                method: "session.status",
                body: { method: "session.status" },
                timeoutMs: 200,
            })
            .catch((error: unknown) => error)
            .finally(() => {
                secondSettled = true;
            });
        await Bun.sleep(20);
        expect(secondSettled).toBe(false);
        releaseTeardown?.();
        await second;
        expect(secondSettled).toBe(true);
    });
});

describe("call policy is keyed off the body it forwards", () => {
    test("a body naming a different method is rejected before any deadline is chosen", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        let ensured = 0;
        transport.ensureRoute = async () => {
            ensured += 1;
            throw new Error("must not be reached");
        };
        await expect(
            transport.call({
                sessionId: "s",
                projectRoot: "/tmp",
                method: "session.wrapup",
                body: { method: "transform" },
            }),
        ).rejects.toBeInstanceOf(TypeError);
        await expect(
            transport.call({ sessionId: "s", projectRoot: "/tmp", method: "transform", body: "x" }),
        ).rejects.toBeInstanceOf(TypeError);
        expect(ensured).toBe(0);
    });
});

describe("connection backoff is not a connection failure", () => {
    test("an active backoff propagates without a generation change", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        transport.nextProbeMs = performance.now() + 60_000;
        const before = transport.connectionGeneration;
        await expect(
            transport.call({
                sessionId: "s",
                projectRoot: "/tmp",
                method: "session.status",
                body: { method: "session.status" },
                generationSensitive: true,
            }),
        ).rejects.toMatchObject({ code: "EIDNARA_HOST_CONNECTION_BACKOFF" });
        expect(transport.connectionGeneration).toBe(before);
    });
});

describe("generation-sensitive not-sent outcomes", () => {
    function fakeRoute(transport: TransportInternals, request: () => Promise<unknown>) {
        const route = { channel: 7, epoch: 1 } as unknown as RouteHandle;
        const client = { request } as unknown as HostClient;
        transport.client = client;
        transport.ensureRoute = async () => ({
            client,
            route,
            routeKey: "s\0/tmp",
            generation: 0,
        });
        return { client, route };
    }

    test("a pre-send refusal without route or connection turnover propagates", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        const refusal = new HostCallError("not_sent", "admission refused", "memory_cap");
        fakeRoute(transport, () => Promise.reject(refusal));

        await expect(
            transport.call({
                sessionId: "s",
                projectRoot: "/tmp",
                method: "session.status",
                body: { method: "session.status" },
                generationSensitive: true,
            }),
        ).rejects.toBe(refusal);
    });

    test("a stale route handle reports a generation change", async () => {
        const transport = internals(new HostModuleTransport("/tmp/unused-eidnara-host.json"));
        let route: RouteHandle | undefined;
        fakeRoute(transport, () => Promise.reject(new StaleRouteHandleError(route as RouteHandle)));
        route = { channel: 7, epoch: 1 } as unknown as RouteHandle;

        await expect(
            transport.call({
                sessionId: "s",
                projectRoot: "/tmp",
                method: "session.status",
                body: { method: "session.status" },
                generationSensitive: true,
            }),
        ).resolves.toMatchObject({ transport_status: "connection_generation_changed" });
    });
});

describe("managed startup envelope harness closures", () => {
    const parents: readonly string[] = hostRelease.packages.parents;
    const resolveWithin = (root: string) => (path: string) => path.replace(/^\/proc\/self/, root);

    test("the closure harness names are exactly the lock's harnesses", () => {
        expect([...__moduleTransportTest.CLOSURE_HARNESSES].sort()).toEqual(
            Object.keys(productionInputs.harnesses).sort(),
        );
    });

    test("every parent package the contract names maps to at most one harness", () => {
        expect(harnessForParentPackage("@eidnara/opencode")).toBe("opencode");
        expect(harnessForParentPackage("@eidnara/pi")).toBe("pi");
        expect(harnessForParentPackage("@eidnara/cli")).toBeNull();
        for (const parent of parents) expect(() => harnessForParentPackage(parent)).not.toThrow();
    });

    test("a package outside packages.parents cannot build an envelope", () => {
        expect(() => buildManagedStartupEnvelope("@other/plugin", {})).toThrow(/parent package/);
    });

    test("@eidnara/opencode carries the OpenCode closure when its executable anchors resolve", () => {
        const closure = __moduleTransportTest.lockedHarnessClosure("opencode");
        const anchor = closure.anchors.runtime;
        if (!anchor) throw new Error("opencode closure names no runtime anchor");
        const envelope = buildManagedStartupEnvelope(
            "@eidnara/opencode",
            { OPENAI_API_KEY: "secret", PATH: "/poisoned" },
            `/proc/self/${anchor.source_path}`,
            undefined,
            resolveWithin("/opt/opencode"),
        );
        expect(envelope).toEqual({
            schema: 1,
            opencode: {
                manifest_sha256: productionInputs.harnesses.opencode.closure.sha256,
                source_roots: { runtime: "/opt/opencode" },
            },
            credentials: { OPENAI_API_KEY: "secret" },
        });
    });

    test("@eidnara/cli carries neither harness closure and raises no error", () => {
        const envelope = buildManagedStartupEnvelope(
            "@eidnara/cli",
            {},
            "/opt/anything/bin/eidnara",
            undefined,
            (path) => path,
        );
        expect(envelope).toEqual({ schema: 1 });
    });

    test("an anchor that does not resolve leaves the harness absent rather than guessing", () => {
        const envelope = buildManagedStartupEnvelope(
            "@eidnara/opencode",
            {},
            "/somewhere/else/binary",
            undefined,
            (path) => path,
        );
        expect(envelope).toEqual({ schema: 1 });
    });

    test("the lock's anchors name the manifest's executable, interpreter, or entrypoint node", () => {
        for (const [harness, spec] of Object.entries(productionInputs.harnesses)) {
            const manifest = JSON.parse(
                readFileSync(join(REPO_ROOT, spec.closure.manifest_path), "utf8"),
            ) as {
                executable: string | null;
                interpreter: string | null;
                entrypoint: string | null;
                source_roots: string[];
                nodes: Array<{ path: string; source_root: string; source_path: string }>;
            };
            const closure = __moduleTransportTest.lockedHarnessClosure(
                harness as "opencode" | "pi",
            );
            expect(Object.keys(closure.anchors).sort()).toEqual([...manifest.source_roots].sort());
            for (const [root, anchor] of Object.entries(closure.anchors)) {
                const expectedPath = manifest[anchor.from];
                const node = manifest.nodes.find(
                    (candidate) =>
                        candidate.path === expectedPath && candidate.source_root === root,
                );
                expect(node?.source_path, `${harness}/${root}`).toBe(anchor.source_path);
            }
        }
    });
});

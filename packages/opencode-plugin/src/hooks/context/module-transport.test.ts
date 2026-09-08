import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import hostRelease from "../../../../../release/host-release.json";
import productionInputs from "../../../../../release/production-inputs.lock.json";
import {
    Deadline,
    type HostClient,
    type HostClientOptions,
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
    clientOptions(deadline?: Deadline): HostClientOptions;
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
                .call({ sessionId, projectRoot: "/tmp", method: "transform", body: {}, timeoutMs })
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

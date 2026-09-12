import {
    afterAll,
    afterEach,
    beforeAll,
    beforeEach,
    describe,
    expect,
    setSystemTime,
    spyOn,
    test,
} from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import {
    DAEMON_ID,
    FakeDaemon,
    IDENTITY,
    KEY,
    ROUTE_CHANNEL,
    ROUTE_EPOCH,
    writeConnectionFile,
} from "./__tests__/fake-daemon";
import { HostClient, type HostClientOptions, type HostDiagnosticsEvent } from "./client";
import { credentialFingerprints } from "./credential-fingerprint";
import { HostCallError, HostClientError } from "./errors";
import { FrameType } from "./protocol";
import { serializedJsonText, serializeJsonBody } from "./serialized-json-body";
import { AdmissionClass, type BindIdentity } from "./types";

async function waitUntil(check: () => boolean, timeoutMs = 3_000): Promise<void> {
    const startedAt = Date.now();
    while (!check()) {
        if (Date.now() - startedAt > timeoutMs) throw new Error("waitUntil timed out");
        await delay(2);
    }
}

async function rejection(promise: Promise<unknown>): Promise<HostCallError> {
    try {
        await promise;
    } catch (error) {
        expect(error).toBeInstanceOf(HostCallError);
        return error as HostCallError;
    }
    throw new Error("expected the promise to reject");
}

const HOST_OPS = ["route.open", "catalog.list", "host.shutdown", "host.status"];

function catalogResponse(overrides: Record<string, unknown> = {}): Record<string, unknown> {
    const module = (id: string): Record<string, unknown> => ({
        module_id: id,
        module_version: "0.1.0",
        roles: [],
        control_ops: [],
    });
    return {
        op: "catalog.list",
        generation: 1,
        host_ops: HOST_OPS,
        modules: [module("context"), module("synapse"), module("broca")],
        ...overrides,
    };
}

const MANAGED_TARGET = { kind: "management_surface", module_id: "mod" } as const;

let tmpDir = "";
let fileCounter = 0;
let clients: HostClient[] = [];

beforeAll(async () => {
    tmpDir = await mkdtemp(path.join(os.tmpdir(), "eidnara-host-client-"));
});

afterAll(async () => {
    if (tmpDir) await rm(tmpDir, { recursive: true, force: true });
});

beforeEach(() => {
    clients = [];
});

afterEach(async () => {
    setSystemTime();
    for (const client of clients) await client.closeAsync().catch(() => {});
});

async function connected(
    overrides: Partial<HostClientOptions> = {},
): Promise<{ client: HostClient; daemon: FakeDaemon }> {
    const daemon = new FakeDaemon();
    const client = await HostClient.connect({
        connectionFile: await writeConnectionFile(path.join(tmpDir, `conn-${++fileCounter}.json`)),
        identity: IDENTITY,
        shutdownDeadlineMs: 500,
        requestTimeoutMs: 3_000,
        channelFactory: daemon.channelFactory,
        ...overrides,
    });
    clients.push(client);
    return { client, daemon };
}

describe("HostClient", () => {
    test("plain Pi-style objects cannot collide with the serialized body identity", async () => {
        const { client, daemon } = await connected();
        const opening = client.routeOpen(MANAGED_TARGET, IDENTITY);
        await daemon.acceptRouteOpen();
        const route = await opening;
        const body = {
            method: "transform",
            text: "not wire authority",
            serialized: "{}",
            body: {},
            value: {},
            page: {},
            bytes: 0,
            [Symbol("serialized JSON body")]: '{"method":"wrong"}',
        };
        expect(serializedJsonText(body)).toBeUndefined();
        const expected = new TextEncoder().encode(JSON.stringify(body));
        const waiting = client.request(route, body);
        void waiting.catch(() => {});
        const frame = await daemon.next();
        expect(frame.body).toEqual(expected);
        daemon.respond(frame.header, { ok: true });
        await waiting;
    });

    test("encodes a serialized snapshot without another body stringify", async () => {
        const { client, daemon } = await connected();
        const opening = client.routeOpen(MANAGED_TARGET, IDENTITY);
        await daemon.acceptRouteOpen();
        const route = await opening;
        let reads = 0;
        const body = {
            method: "transform",
            get messages() {
                reads += 1;
                return [{ text: reads === 1 ? "é😀\ud800" : "changed on later read" }];
            },
        };
        const stringify = spyOn(JSON, "stringify");
        try {
            const page = serializeJsonBody(body);
            const bytes = Buffer.byteLength(serializedJsonText(page));
            // The text is authoritative: it comes from the first getter read, and the
            // shallow view never rewrites it.
            const readsAfterBuild = reads;
            body.method = "mutated";
            (page.messages as Array<{ text: string }>)[0]!.text = "inspection edit";
            expect(Reflect.set(page, "method", "wrong")).toBe(false);
            const callsBeforeSend = stringify.mock.calls.length;
            const waiting = client.request(route, page);
            void waiting.catch(() => {});
            const frame = await daemon.next();
            expect(stringify.mock.calls.length).toBe(callsBeforeSend);
            expect(reads).toBe(readsAfterBuild);
            expect(frame.headerBytes).toBeDefined();
            const header = frame.headerBytes!;
            expect(new DataView(header.buffer, header.byteOffset).getUint32(0, true)).toBe(bytes);
            expect(frame.body).toEqual(
                new TextEncoder().encode(
                    '{"method":"transform","messages":[{"text":"é😀\\ud800"}]}',
                ),
            );
            expect(frame.body.byteLength).toBe(bytes);
            daemon.respond(frame.header, { ok: true });
            await waiting;
        } finally {
            stringify.mockRestore();
        }
    });

    test("managed call opens a cached route once and reuses it", async () => {
        const { client, daemon } = await connected();

        const first = client.call<{ ok: boolean }>("mod", "ping");
        const open = await daemon.acceptRouteOpen();
        expect(open.target).toEqual({ kind: "management_surface", module_id: "mod" });
        expect(await daemon.answerRouted({ ok: true })).toEqual({ method: "ping" });
        expect(await first).toEqual({ ok: true });

        const second = client.call<{ ok: boolean }>("mod", "ping", { n: 2 });
        expect(await daemon.answerRouted({ ok: true })).toEqual({
            method: "ping",
            params: { n: 2 },
        });
        expect(await second).toEqual({ ok: true });
        expect(client.cachedManagedRouteCount).toBe(1);
    });

    test("reopening after retirement derives credential_fingerprints from the current credential row", async () => {
        const credentialSource: Record<string, string | undefined> = {
            ANTHROPIC_API_KEY: "anthropic-secret",
        };
        const { client, daemon } = await connected({ credentialSource });

        const first = client.call("mod", "ping");
        const firstOpen = await daemon.acceptRouteOpen();
        const identity = firstOpen.identity as { credential_fingerprints?: Record<string, string> };
        expect(identity.credential_fingerprints).toEqual(
            credentialFingerprints(KEY, "opencode", credentialSource),
        );
        await daemon.answerRouted({ ok: true });
        await first;

        daemon.send({ ty: FrameType.Goodbye, channel: 0, epoch: 0 });
        await waitUntil(() => client.publication === null);

        delete credentialSource.ANTHROPIC_API_KEY;
        const second = client.call("mod", "ping");
        const secondOpen = await daemon.acceptRouteOpen();
        const reopened = secondOpen.identity as { credential_fingerprints?: unknown };
        expect(reopened.credential_fingerprints).toBeUndefined();
        await daemon.answerRouted({ ok: true });
        await second;
    });

    test("hostShutdown refuses to send when expectedDaemonId does not match the authenticated daemon", async () => {
        const { client, daemon } = await connected();

        const other = Uint8Array.from(DAEMON_ID, (byte) => byte ^ 0xff);
        let failure: unknown;
        try {
            await client.hostShutdown({ timeoutMs: 500, expectedDaemonId: other });
        } catch (error) {
            failure = error;
        }
        expect(failure).toBeInstanceOf(HostCallError);
        expect((failure as HostCallError).kind).toBe("not_sent");
        expect((failure as HostCallError).code).toBe("daemon_generation_changed");
        expect(daemon.drain()).toBeNull();

        const matching = client.hostShutdown({ expectedDaemonId: DAEMON_ID });
        const shutdown = await daemon.nextRequest();
        expect(shutdown.json).toEqual({ op: "host.shutdown" });
        daemon.respond(shutdown.header, { op: "host.shutdown" });
        await matching;
    });

    test("managed route cache releases slots whose route is gone", async () => {
        const { client, daemon } = await connected();

        const first = client.call("mod", "ping");
        await daemon.acceptRouteOpen();
        await daemon.answerRouted({ ok: true });
        await first;
        expect(client.cachedManagedRouteCount).toBe(1);

        daemon.send({ ty: FrameType.Goodbye, channel: ROUTE_CHANNEL, epoch: ROUTE_EPOCH });
        await waitUntil(() => client.cachedManagedRouteCount === 0);

        const second = client.call(
            "mod",
            "ping",
            {},
            { identity: { ...IDENTITY, session: "session-2" } },
        );
        await daemon.acceptRouteOpen();
        await daemon.answerRouted({ ok: true });
        await second;
        expect(client.cachedManagedRouteCount).toBe(1);

        daemon.send({ ty: FrameType.Goodbye, channel: 0, epoch: 0 });
        await waitUntil(() => client.publication === null);
        expect(client.cachedManagedRouteCount).toBe(0);
    });

    test("diagnostics rate limiting survives a backward wall-clock step", async () => {
        let nowMs = 1_000_000;
        const clock = (): number => nowMs;
        const wallStart = new Date("2026-01-01T00:00:00Z");
        setSystemTime(wallStart);
        const events: HostDiagnosticsEvent[] = [];
        const { client, daemon } = await connected({
            clock,
            diagnostics: (event) => events.push(event),
            maxDiagnosticEventsPerSecond: 2,
        });

        const first = client.hostStatus();
        const status = await daemon.nextRequest();
        daemon.respond(status.header, {
            op: "host.status",
            health: "ok",
            metrics: { components: {} },
        });
        await first;
        expect(events.length).toBe(2);

        setSystemTime(new Date(wallStart.getTime() - 3_600_000));
        nowMs += 2_000;
        const second = client.hostStatus();
        const again = await daemon.nextRequest();
        daemon.respond(again.header, {
            op: "host.status",
            health: "ok",
            metrics: { components: {} },
        });
        await second;
        expect(events.length).toBeGreaterThan(2);
    });

    test("an already-aborted signal or Sheddable admission is rejected not_sent before any byte is published", async () => {
        const { client, daemon } = await connected();

        const opening = client.routeOpen(MANAGED_TARGET, IDENTITY);
        await daemon.acceptRouteOpen();
        const handle = await opening;

        const controller = new AbortController();
        controller.abort();
        const aborted = await rejection(
            client.request(handle, { method: "ping" }, { signal: controller.signal }),
        );
        expect(aborted.kind).toBe("not_sent");
        expect(aborted.code).toBe("aborted");
        await aborted.cleanup;
        expect(daemon.drain()).toBeNull();

        const sheddable = await rejection(
            client.request(
                handle,
                { method: "ping" },
                { admissionClass: AdmissionClass.Sheddable },
            ),
        );
        expect(sheddable.kind).toBe("not_sent");
        expect(sheddable.code).toBe("invalid_admission_class");
        expect(daemon.drain()).toBeNull();
    });

    test("managed route cache keys cannot collide on delimiter bytes inside identity fields", async () => {
        const { client, daemon } = await connected();

        const first = client.call("mod", "ping", undefined, {
            identity: { ...IDENTITY, harness: "x\u0000y", session: "z" },
        });
        await daemon.acceptRouteOpen();
        await daemon.answerRouted({ ok: true });
        await first;

        const second = client.call("mod", "ping", undefined, {
            identity: { ...IDENTITY, harness: "x", session: "y\u0000z" },
        });
        const open = await daemon.nextRequest();
        expect(open.json.op).toBe("route.open");
        expect((open.json.identity as BindIdentity).session).toBe("y\u0000z");
        daemon.respond(open.header, { op: "route.open", route_channel: 8, route_epoch: 1 });
        const routed = await daemon.nextRequest();
        expect(routed.header.channel).toBe(8);
        daemon.respond(routed.header, { ok: true });
        await second;
        expect(client.cachedManagedRouteCount).toBe(2);
    });

    test("closeAsync is bounded by the shutdown deadline while a reconnect is still in flight", async () => {
        let reads = 0;
        let releaseReconnect: (() => void) | undefined;
        const stalled = new Promise<void>((resolve) => {
            releaseReconnect = resolve;
        });
        const { client, daemon } = await connected({
            handshakeTimeoutMs: 10_000,
            shutdownDeadlineMs: 100,
            connectionFileAfterOpen: () => {
                reads += 1;
                return reads === 1 ? undefined : stalled;
            },
        });

        daemon.send({ ty: FrameType.Goodbye, channel: 0, epoch: 0 });
        await waitUntil(() => client.publication === null);
        const reconnecting = client.hostStatus({ timeoutMs: 10_000 });
        reconnecting.catch(() => {});
        await waitUntil(() => reads === 2);

        const startedAt = performance.now();
        await client.closeAsync();
        expect(performance.now() - startedAt).toBeLessThan(2_000);
        releaseReconnect?.();
        let failure: unknown;
        try {
            await reconnecting;
        } catch (error) {
            failure = error;
        }
        expect(failure).toBeInstanceOf(HostClientError);
        expect((failure as HostClientError).code).toBe("client_closed");
    });

    test("a route Goodbye delivered with the route.open response fails the open instead of publishing a dead handle", async () => {
        const { client, daemon } = await connected({ sleep: async () => {} });

        const direct = client.routeOpen(MANAGED_TARGET, IDENTITY);
        const open = await daemon.nextRequest();
        daemon.respond(open.header, {
            op: "route.open",
            route_channel: ROUTE_CHANNEL,
            route_epoch: ROUTE_EPOCH,
        });
        daemon.send({ ty: FrameType.Goodbye, channel: ROUTE_CHANNEL, epoch: ROUTE_EPOCH });
        const failure = await rejection(direct);
        expect(failure.kind).toBe("terminal");
        expect(failure.code).toBe("route_gone");

        const managed = client.call("mod", "ping");
        const reopen = await daemon.nextRequest();
        daemon.respond(reopen.header, {
            op: "route.open",
            route_channel: ROUTE_CHANNEL,
            route_epoch: ROUTE_EPOCH,
        });
        daemon.send({ ty: FrameType.Goodbye, channel: ROUTE_CHANNEL, epoch: ROUTE_EPOCH });
        await daemon.acceptRouteOpen();
        await daemon.answerRouted({ ok: true });
        expect(await managed).toEqual({ ok: true });
    });

    test("a route.open success without a nameable handle retires the generation", async () => {
        const { client, daemon } = await connected();

        const opening = client.routeOpen(MANAGED_TARGET, IDENTITY);
        const open = await daemon.nextRequest();
        daemon.respond(open.header, { op: "route.open", route_channel: 0, route_epoch: 1 });
        const failure = await rejection(opening);
        expect(failure.code).toBe("malformed_control_response");
        expect(client.authenticated).toBeNull();
    });

    test("host.status requires metrics.components", async () => {
        const { client, daemon } = await connected();

        const missing = client.hostStatus();
        const first = await daemon.nextRequest();
        daemon.respond(first.header, { op: "host.status", health: "ok", metrics: {} });
        expect((await rejection(missing)).code).toBe("malformed_control_response");

        const present = client.hostStatus();
        const second = await daemon.nextRequest();
        daemon.respond(second.header, {
            op: "host.status",
            health: "ok",
            metrics: { components: { storage_state: "ready" } },
        });
        expect((await present).metrics).toEqual({ components: { storage_state: "ready" } });
    });

    test("catalog.list requires the fixed host_ops and the ordered module list", async () => {
        const { client, daemon } = await connected();

        const rejects = async (response: Record<string, unknown>): Promise<void> => {
            const snapshot = client.catalogSnapshot();
            const request = await daemon.nextRequest();
            daemon.respond(request.header, response);
            expect((await rejection(snapshot)).code).toBe("malformed_control_response");
        };
        await rejects(catalogResponse({ modules: [] }));
        await rejects(catalogResponse({ host_ops: HOST_OPS.slice(0, 3) }));
        await rejects(catalogResponse({ host_ops: [...HOST_OPS, "wake.create"] }));
        const shuffled = catalogResponse();
        shuffled.modules = (shuffled.modules as unknown[]).slice().reverse();
        await rejects(shuffled);

        const accepted = client.catalogSnapshot();
        const request = await daemon.nextRequest();
        daemon.respond(request.header, catalogResponse({ extra: true }));
        const snapshot = await accepted;
        expect(snapshot.hostOps).toEqual(HOST_OPS);
        expect(snapshot.modules.map((module) => module.module_id)).toEqual([
            "context",
            "synapse",
            "broca",
        ]);
    });

    test("an empty credential derivation removes a caller-supplied credential_fingerprints claim", async () => {
        const { client, daemon } = await connected({ credentialSource: {} });

        const call = client.call("mod", "ping", undefined, {
            identity: { ...IDENTITY, credential_fingerprints: { anthropic: "ab".repeat(32) } },
        });
        const open = await daemon.acceptRouteOpen();
        expect((open.identity as BindIdentity).credential_fingerprints).toBeUndefined();
        await daemon.answerRouted({ ok: true });
        await call;
    });

    test("a rejecting async diagnostics observer never surfaces an unhandled rejection", async () => {
        const unhandled: unknown[] = [];
        const onUnhandled = (reason: unknown): void => {
            unhandled.push(reason);
        };
        process.on("unhandledRejection", onUnhandled);
        try {
            const { client, daemon } = await connected({
                diagnostics: async () => {
                    throw new Error("observer failed");
                },
            });
            const status = client.hostStatus();
            const request = await daemon.nextRequest();
            daemon.respond(request.header, {
                op: "host.status",
                health: "ok",
                metrics: { components: {} },
            });
            await status;
            await delay(5);
        } finally {
            process.off("unhandledRejection", onUnhandled);
        }
        expect(unhandled).toEqual([]);
    });

    test("an owner's stale daemon expectation does not fail a joiner that expects the connected daemon", async () => {
        const { client, daemon } = await connected();
        const other = Uint8Array.from(DAEMON_ID, (byte) => byte ^ 0xff);

        const stale = client.call("mod", "ping", undefined, { expectedDaemonId: other });
        const fresh = client.call("mod", "ping", undefined, { expectedDaemonId: DAEMON_ID });
        expect((await rejection(stale)).code).toBe("daemon_generation_changed");

        await daemon.acceptRouteOpen();
        // The joiner still owns its replay token, so a host no-dispatch proof reopens the route.
        const routed = await daemon.nextRequest();
        daemon.fail(routed.header, { code: "unknown_channel", message: "stale route" });
        await daemon.acceptRouteOpen();
        await daemon.answerRouted({ ok: true });
        expect(await fresh).toEqual({ ok: true });
    });

    test("a non-canonical error body never exposes a code to replay policy", async () => {
        const { client, daemon } = await connected();

        const call = client.call("mod", "ping");
        await daemon.acceptRouteOpen();
        const routed = await daemon.nextRequest();
        daemon.fail(routed.header, { code: "unknown_channel" });
        const failure = await rejection(call);
        expect(failure.kind).toBe("terminal");
        expect(failure.code).toBe("malformed_error_body");
        expect(daemon.drain()).toBeNull();

        const advised = client.call("mod", "ping");
        const again = await daemon.nextRequest();
        daemon.fail(again.header, { code: "queue_full", message: "busy", retry_after_ms: 50 });
        const terminal = await rejection(advised);
        expect(terminal.code).toBe("queue_full");
        expect(terminal.retry_after_ms).toBe(50);
    });

    test("closeAsync retires the generation even when connection Goodbye cannot be queued", async () => {
        const { client, daemon } = await connected();
        daemon.link.native.produce = () => {
            throw new Error("shared-memory ring is full");
        };

        await client.closeAsync();
        expect(client.authenticated).toBeNull();
        expect(client.isClosed).toBe(true);
    });

    test("a route.open success that reuses a live channel retires the generation", async () => {
        const { client, daemon } = await connected();

        const first = client.routeOpen(MANAGED_TARGET, IDENTITY);
        await daemon.acceptRouteOpen();
        await first;

        const second = client.routeOpen(MANAGED_TARGET, { ...IDENTITY, session: "session-2" });
        const open = await daemon.nextRequest();
        daemon.respond(open.header, {
            op: "route.open",
            route_channel: ROUTE_CHANNEL,
            route_epoch: ROUTE_EPOCH + 1,
        });
        const failure = await rejection(second);
        expect(failure.code).toBe("malformed_control_response");
        expect(client.authenticated).toBeNull();
    });

    test("aborting a managed call during route setup detaches it without cancelling the shared open", async () => {
        const { client, daemon } = await connected();
        const controller = new AbortController();

        const aborted = client.call("mod", "ping", undefined, { signal: controller.signal });
        const other = client.call("mod", "ping");
        const open = await daemon.nextRequest();
        expect(open.json.op).toBe("route.open");
        controller.abort();
        const failure = await rejection(aborted);
        expect(failure.kind).toBe("not_sent");
        expect(failure.code).toBe("aborted");

        daemon.respond(open.header, {
            op: "route.open",
            route_channel: ROUTE_CHANNEL,
            route_epoch: ROUTE_EPOCH,
        });
        expect(await daemon.answerRouted({ ok: true })).toEqual({ method: "ping" });
        expect(await other).toEqual({ ok: true });
        expect(client.cachedManagedRouteCount).toBe(1);
    });
});

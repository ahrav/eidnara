import {
    afterAll,
    afterEach,
    beforeAll,
    beforeEach,
    describe,
    expect,
    setSystemTime,
    test,
} from "bun:test";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { type NativeChannel, type NativeReceiveLease, ProducerCursor } from "@eidnara/shm-native";
import { HostClient, type HostClientOptions, type HostDiagnosticsEvent } from "./client";
import type { ConnectionGenerationOptions } from "./connection";
import { credentialFingerprints } from "./credential-fingerprint";
import { HostCallError } from "./errors";
import {
    decodeHeader,
    type EnvelopeHeader,
    encodeHeader,
    FrameType,
    PROTOCOL_VERSION,
} from "./protocol";
import { ShmFrameChannel } from "./shm-frame-channel";
import type { BindIdentity } from "./types";

const IDENTITY: BindIdentity = {
    project_root: "/workspace/project",
    harness: "opencode",
    session: "session-1",
};
const KEY = Uint8Array.from({ length: 32 }, (_, i) => i + 1);
const DAEMON_ID = Uint8Array.from({ length: 16 }, (_, i) => 0x60 + i);
const ROUTE_CHANNEL = 7;
const ROUTE_EPOCH = 1;

interface PeerFrame {
    header: EnvelopeHeader;
    body: Uint8Array;
}

/**
 * `FakeLink` stands in for one shared-memory ring pair so these scenarios run without the native addon.
 */
class FakeLink {
    readonly fromClient: PeerFrame[] = [];
    private readonly toClient: PeerFrame[] = [];
    private ready: (() => void) | null = null;

    readonly native = {
        produce: (
            header: Uint8Array,
            capacity: number,
            fill: (cursor: ProducerCursor) => void,
            beforePublish?: () => void,
        ): void => {
            const body = new Uint8Array(capacity);
            const cursor = new ProducerCursor([body], capacity);
            fill(cursor);
            if (cursor.written !== capacity) throw new RangeError("producer underfill");
            beforePublish?.();
            this.fromClient.push({ header: decodeHeader(header), body });
        },
        reserve: (): never => {
            throw new Error("the client facade never reserves ring capacity");
        },
        drainOne: (deliver: (lease: NativeReceiveLease) => void): boolean => {
            const frame = this.toClient.shift();
            if (!frame) return false;
            deliver({
                header: encodeHeader(frame.header),
                byteLength: frame.body.byteLength,
                segmentCount: 1,
                segment: () => frame.body,
                release: () => {},
            } as unknown as NativeReceiveLease);
            return true;
        },
        startReadiness: (handler: () => void): void => {
            this.ready = handler;
            if (this.toClient.length > 0) queueMicrotask(handler);
        },
        peerClosed: (): boolean => false,
        close: (): void => {},
    } as unknown as NativeChannel;

    deliver(frame: PeerFrame): void {
        this.toClient.push(frame);
        const ready = this.ready;
        if (ready) queueMicrotask(ready);
    }
}

class FakeDaemon {
    readonly links: FakeLink[] = [];

    readonly channelFactory: NonNullable<ConnectionGenerationOptions["channelFactory"]> = ({
        budget,
        maxBodyLen,
        handlers,
    }) => {
        const link = new FakeLink();
        this.links.push(link);
        return new ShmFrameChannel({ nativeChannel: link.native, budget, maxBodyLen, handlers });
    };

    get link(): FakeLink {
        const link = this.links.at(-1);
        if (!link) throw new Error("no connection has been established");
        return link;
    }

    drain(): PeerFrame | null {
        return this.link.fromClient.shift() ?? null;
    }

    async next(timeoutMs = 3_000): Promise<PeerFrame> {
        const startedAt = Date.now();
        for (;;) {
            const frame = this.drain();
            if (frame) return frame;
            if (Date.now() - startedAt > timeoutMs) throw new Error("no frame arrived");
            await delay(1);
        }
    }

    async nextRequest(): Promise<{ header: EnvelopeHeader; json: Record<string, unknown> }> {
        const frame = await this.next();
        expect(frame.header.ty).toBe(FrameType.Request);
        return {
            header: frame.header,
            json: JSON.parse(Buffer.from(frame.body).toString("utf8")) as Record<string, unknown>,
        };
    }

    send(
        fields: Partial<EnvelopeHeader> & { ty: number },
        body: Uint8Array = new Uint8Array(),
    ): void {
        this.link.deliver({
            header: {
                len: body.byteLength,
                ver: PROTOCOL_VERSION,
                flags: 0,
                channel: 0,
                epoch: 0,
                corr: 0n,
                ...fields,
            },
            body,
        });
    }

    respond(request: EnvelopeHeader, value: unknown): void {
        this.send(
            {
                ty: FrameType.Response,
                channel: request.channel,
                epoch: request.epoch,
                corr: request.corr,
            },
            new Uint8Array(Buffer.from(JSON.stringify(value))),
        );
    }

    async acceptRouteOpen(): Promise<Record<string, unknown>> {
        const open = await this.nextRequest();
        expect(open.header.channel).toBe(0);
        expect(open.json.op).toBe("route.open");
        this.respond(open.header, {
            op: "route.open",
            route_channel: ROUTE_CHANNEL,
            route_epoch: ROUTE_EPOCH,
        });
        return open.json;
    }

    async answerRouted(result: unknown): Promise<Record<string, unknown>> {
        const routed = await this.nextRequest();
        expect(routed.header.channel).toBe(ROUTE_CHANNEL);
        this.respond(routed.header, result);
        return routed.json;
    }
}

async function waitUntil(check: () => boolean, timeoutMs = 3_000): Promise<void> {
    const startedAt = Date.now();
    while (!check()) {
        if (Date.now() - startedAt > timeoutMs) throw new Error("waitUntil timed out");
        await delay(2);
    }
}

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

async function writeConnectionFile(): Promise<string> {
    fileCounter += 1;
    const filePath = path.join(tmpDir, `conn-${fileCounter}.json`);
    const content = JSON.stringify({
        schema: 2,
        wire_version: 2,
        setup_socket: "/tmp/eidnara-host-client-test.sock",
        key: Array.from(KEY),
        daemon_id: Array.from(DAEMON_ID),
        pid: 4_242,
        daemon_ver: "eidnara-host/0.1.0",
    });
    await writeFile(filePath, content, { mode: 0o600 });
    return filePath;
}

async function connected(
    overrides: Partial<HostClientOptions> = {},
): Promise<{ client: HostClient; daemon: FakeDaemon }> {
    const daemon = new FakeDaemon();
    const client = await HostClient.connect({
        connectionFile: await writeConnectionFile(),
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
        daemon.respond(status.header, { op: "host.status", health: "ok", metrics: {} });
        await first;
        expect(events.length).toBe(2);

        setSystemTime(new Date(wallStart.getTime() - 3_600_000));
        nowMs += 2_000;
        const second = client.hostStatus();
        const again = await daemon.nextRequest();
        daemon.respond(again.header, { op: "host.status", health: "ok", metrics: {} });
        await second;
        expect(events.length).toBeGreaterThan(2);
    });
});

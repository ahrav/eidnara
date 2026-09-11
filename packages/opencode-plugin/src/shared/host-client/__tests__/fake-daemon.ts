import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { setTimeout as delay } from "node:timers/promises";
import { type NativeChannel, type NativeReceiveLease, ProducerCursor } from "@eidnara/shm-native";
import type { ConnectionGenerationOptions } from "../connection";
import {
    decodeHeader,
    type EnvelopeHeader,
    encodeHeader,
    FrameType,
    PROTOCOL_VERSION,
} from "../protocol";
import { ShmFrameChannel } from "../shm-frame-channel";
import type { BindIdentity } from "../types";

export const IDENTITY: BindIdentity = {
    project_root: "/workspace/project",
    harness: "opencode",
    session: "session-1",
};
export const KEY = Uint8Array.from({ length: 32 }, (_, i) => i + 1);
export const DAEMON_ID = Uint8Array.from({ length: 16 }, (_, i) => 0x60 + i);
export const ROUTE_CHANNEL = 7;
export const ROUTE_EPOCH = 1;

export interface PeerFrame {
    header: EnvelopeHeader;
    body: Uint8Array;
    headerBytes?: Uint8Array;
}

/** `FakeLink` stands in for one shared-memory ring pair so these scenarios run without the native addon. */
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
            this.fromClient.push({
                header: decodeHeader(header),
                headerBytes: header.slice(),
                body,
            });
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

export class FakeDaemon {
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
        return this.links.at(-1)?.fromClient.shift() ?? null;
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
        assert.equal(frame.header.ty, FrameType.Request);
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

    fail(request: EnvelopeHeader, body: unknown): void {
        this.send(
            {
                ty: FrameType.Error,
                channel: request.channel,
                epoch: request.epoch,
                corr: request.corr,
            },
            new Uint8Array(Buffer.from(JSON.stringify(body))),
        );
    }

    async acceptRouteOpen(): Promise<Record<string, unknown>> {
        const open = await this.nextRequest();
        assert.equal(open.header.channel, 0);
        assert.equal(open.json.op, "route.open");
        this.respond(open.header, {
            op: "route.open",
            route_channel: ROUTE_CHANNEL,
            route_epoch: ROUTE_EPOCH,
        });
        return open.json;
    }

    async answerRouted(result: unknown): Promise<Record<string, unknown>> {
        const routed = await this.nextRequest();
        assert.equal(routed.header.channel, ROUTE_CHANNEL);
        this.respond(routed.header, result);
        return routed.json;
    }
}

export async function writeConnectionFile(filePath: string): Promise<string> {
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

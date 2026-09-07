import { describe, expect, test } from "bun:test";
import { ConnectionGeneration } from "./connection";
import { Deadline } from "./deadline";
import { HostCallError } from "./errors";
import {
    type BoundedFrameProducer,
    type ByteBudget,
    CopyCounter,
    type DirectFrameBody,
    type FrameChannelCloseReason,
    type FrameChannelHandlers,
    type FrameChannelStats,
    type FrameSendHooks,
    type FrameSendTicket,
    type OutboundFrame,
    type ProducerFrameHeader,
    ReceiveLease,
    type SetupFrameChannel,
} from "./frame-channel";
import { type EnvelopeHeader, FrameType, HEADER_LEN, PROTOCOL_VERSION } from "./protocol";

const CHANNEL = 7;
const EPOCH = 1;

/**
 * FakeChannel reproduces the `ShmFrameChannel` failure surface that
 * `ConnectionGeneration` must tolerate: publication is synchronous, `produce`
 * refuses over-budget frames, `sendControl` throws `ring_full`, `close()`
 * throws after reporting a quarantine, and an `onFrame` exception closes the
 * channel as `protocol_violation`.
 */
class FakeChannel implements SetupFrameChannel {
    readonly produced: ProducerFrameHeader[] = [];
    readonly controls: EnvelopeHeader[] = [];
    readonly closes: { reason: FrameChannelCloseReason; error: unknown }[] = [];
    closeCalls = 0;
    closed = false;
    sendControlError: unknown = null;
    closeError: unknown = null;
    private readonly copies = new CopyCounter();
    private readonly leases = new Set<ReceiveLease>();

    constructor(
        readonly budget: ByteBudget,
        readonly handlers: FrameChannelHandlers,
    ) {}

    async start(_deadline: Deadline): Promise<void> {}

    beginFrames(): void {}

    produce(
        header: ProducerFrameHeader,
        body: DirectFrameBody,
        hooks?: FrameSendHooks,
    ): FrameSendTicket {
        if (this.closed) throw new HostCallError("not_sent", "fake channel closed");
        const reserved = HEADER_LEN + body.byteLength;
        if (this.budget.wouldExceed(reserved)) {
            throw new HostCallError(
                "not_sent",
                "aggregate connection memory cap would be exceeded",
                "memory_cap",
            );
        }
        this.produced.push(header);
        hooks?.onPublish?.();
        hooks?.onComplete?.();
        return { cancel: () => false };
    }

    reserve(): BoundedFrameProducer {
        throw new Error("reserve is not used by these tests");
    }

    send(_frame: OutboundFrame): FrameSendTicket {
        throw new Error("send is not used by these tests");
    }

    sendControl(header: EnvelopeHeader): void {
        if (this.closed) return;
        if (this.sendControlError !== null) throw this.sendControlError;
        this.controls.push(header);
    }

    async flush(): Promise<void> {}

    close(): void {
        if (this.closed) return;
        this.closed = true;
        this.closeCalls++;
        if (this.closeError !== null) {
            this.handlers.onClosed("quarantined", this.closeError);
            throw this.closeError;
        }
    }

    isClosed(): boolean {
        return this.closed;
    }

    stats(): FrameChannelStats {
        return {
            readerHeldBytes: 0,
            queueHeldBytes: 0,
            queuedDataFrames: 0,
            queuedControlFrames: 0,
            readPaused: false,
            activeTimers: 0,
            activeReceiveLeases: this.leases.size,
            quarantinedBytes: 0,
            ownedAdapterCopies: this.copies.copies,
        };
    }

    lease(bytes: Uint8Array): ReceiveLease {
        const lease: ReceiveLease = new ReceiveLease(
            [Uint8Array.from(bytes)],
            () => {
                this.leases.delete(lease);
                this.handlers.onLeaseReleased?.();
            },
            this.copies,
        );
        this.leases.add(lease);
        return lease;
    }

    /** Delivers one inbound frame with `drainReady`'s exception contract. */
    deliver(header: EnvelopeHeader, body: ReceiveLease): void {
        try {
            this.handlers.onFrame({ header, body });
        } catch (error) {
            this.closes.push({ reason: "protocol_violation", error });
            this.handlers.onClosed("protocol_violation", error);
            try {
                this.close();
            } catch {}
        }
    }
}

function header(ty: FrameType, corr: bigint, len: number, flags = 0): EnvelopeHeader {
    return { len, ver: PROTOCOL_VERSION, ty, flags, channel: CHANNEL, epoch: EPOCH, corr };
}

async function harness(options: { memoryCapBytes?: number; maxBodyLen?: number } = {}): Promise<{
    generation: ConnectionGeneration;
    channel: FakeChannel;
    retirements: { reason: string }[];
}> {
    let channel: FakeChannel | undefined;
    const retirements: { reason: string }[] = [];
    const generation = new ConnectionGeneration({
        credentials: { key: new Uint8Array(32), daemonId: new Uint8Array(16), daemonVer: "test" },
        memoryCapBytes: options.memoryCapBytes,
        maxBodyLen: options.maxBodyLen,
        channelFactory: ({ budget, handlers }) => {
            channel = new FakeChannel(budget, handlers);
            return channel;
        },
        onRetired: (info) => retirements.push({ reason: info.reason }),
    });
    await generation.start(Deadline.start(2_000));
    if (!channel) throw new Error("channel factory was not invoked");
    return { generation, channel, retirements };
}

describe("connection generation memory cap", () => {
    test("stream retention that would exceed the aggregate cap settles the request and keeps the cap intact", async () => {
        const cap = 1_024;
        const { generation, channel } = await harness({ memoryCapBytes: cap, maxBodyLen: 512 });
        try {
            const stream = generation.request({
                channel: CHANNEL,
                epoch: EPOCH,
                body: Buffer.from("{}"),
                deadline: Deadline.start(2_000),
                mode: "stream",
            });
            const item = new TextEncoder().encode(`{"p":"${"a".repeat(390)}"}`);
            expect(2 * item.byteLength).toBeLessThanOrEqual(cap);
            expect(3 * item.byteLength).toBeGreaterThan(cap);

            for (let index = 0; index < 3; index++) {
                channel.deliver(
                    header(FrameType.StreamData, stream.correlation, item.byteLength),
                    channel.lease(item),
                );
            }

            await expect(stream.result).rejects.toMatchObject({
                kind: "terminal",
                code: "memory_cap",
            });
            expect(channel.budget.used).toBeLessThanOrEqual(cap);
            expect(generation.stats().pendingHeldBytes).toBe(0);
            expect(generation.stats().pendingRequests).toBe(0);
            expect(channel.stats().activeReceiveLeases).toBe(0);
            expect(channel.controls.map((control) => control.ty)).toEqual([FrameType.Cancel]);

            const next = generation.request({
                channel: CHANNEL,
                epoch: EPOCH,
                body: Buffer.from("{}"),
                deadline: Deadline.start(2_000),
            });
            expect(next.correlation).toBe(stream.correlation + 1n);
            expect(generation.isRetired()).toBe(false);
        } finally {
            generation.retire("owner_close");
        }
    });
});

describe("connection generation retirement", () => {
    test("retirement settles and notifies the owner even when channel.close() throws", async () => {
        const { generation, channel, retirements } = await harness();
        channel.closeError = new Error(
            "receive lease alias state is uncertain; storage quarantined",
        );

        expect(() => generation.retire("owner_close")).not.toThrow();

        const info = await generation.retired;
        expect(info.reason).toBe("owner_close");
        expect(retirements).toEqual([{ reason: "owner_close" }]);
        expect(channel.closeCalls).toBe(1);
        expect(generation.isRetired()).toBe(true);
    });
});

describe("connection generation control frames", () => {
    test("a full outbound ring drops the Pong instead of retiring the generation", async () => {
        const { generation, channel } = await harness();
        try {
            const inflight = generation.request({
                channel: CHANNEL,
                epoch: EPOCH,
                body: Buffer.from("{}"),
                deadline: Deadline.start(2_000),
            });
            channel.sendControlError = new HostCallError(
                "not_sent",
                "shared-memory ring has no capacity for this frame",
                "ring_full",
            );
            const ping = channel.lease(new Uint8Array());
            channel.deliver(
                {
                    len: 0,
                    ver: PROTOCOL_VERSION,
                    ty: FrameType.Ping,
                    flags: 0,
                    channel: 0,
                    epoch: 0,
                    corr: 99n,
                },
                ping,
            );

            expect(generation.isRetired()).toBe(false);
            expect(channel.closes).toEqual([]);
            expect(ping.isReleased()).toBe(true);
            expect(generation.stats().droppedFrames).toBe(1);

            channel.sendControlError = null;
            channel.deliver(
                header(FrameType.Response, inflight.correlation, 2),
                channel.lease(new TextEncoder().encode("{}")),
            );
            expect((await inflight.result).kind).toBe("response");
        } finally {
            generation.retire("owner_close");
        }
    });
});

describe("connection generation response bodies", () => {
    test("a JSON body for a binary request is rejected without reading body bytes", async () => {
        const { generation, channel } = await harness();
        try {
            const request = generation.request({
                channel: CHANNEL,
                epoch: EPOCH,
                body: Buffer.from([1]),
                binary: true,
                responseMode: "binary",
                deadline: Deadline.start(2_000),
            });
            const lease = channel.lease(new TextEncoder().encode('{"ok":true}'));
            let reads = 0;
            const original = lease.segment.bind(lease);
            Object.defineProperty(lease, "segment", {
                value: (index: number) => {
                    reads++;
                    return original(index);
                },
            });

            channel.deliver(
                header(FrameType.Response, request.correlation, lease.byteLength, 0),
                lease,
            );

            await expect(request.result).rejects.toMatchObject({
                kind: "terminal",
                code: "expected_binary_response",
            });
            expect(reads).toBe(0);
            expect(lease.isReleased()).toBe(true);
            expect(generation.isRetired()).toBe(false);
        } finally {
            generation.retire("owner_close");
        }
    });
});

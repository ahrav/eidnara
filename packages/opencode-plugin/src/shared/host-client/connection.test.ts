import { describe, expect, test } from "bun:test";
import { ConnectionGeneration, type ConnectionGenerationOptions } from "./connection";
import { Deadline } from "./deadline";
import { HostCallError, SocketTimeoutError } from "./errors";
import {
    type BoundedFrameProducer,
    type ByteBudget,
    CopyCounter,
    type DirectFrameBody,
    type FrameChannelCloseReason,
    type FrameChannelHandlers,
    type FrameChannelStats,
    type FrameProducerCursor,
    type FrameSendHooks,
    type FrameSendTicket,
    type OutboundFrame,
    type ProducerFrameHeader,
    ReceiveLease,
    type SetupFrameChannel,
} from "./frame-channel";
import {
    type EnvelopeHeader,
    FrameType,
    HEADER_LEN,
    MAX_CORRELATION,
    PROTOCOL_VERSION,
} from "./protocol";

const CHANNEL = 7;
const EPOCH = 1;

/** Fills one fixed-size buffer; overflow is a test bug, not a contract under test. */
class ArrayCursor implements FrameProducerCursor {
    private cursor = 0;

    constructor(private readonly target: Uint8Array) {}

    get written(): number {
        return this.cursor;
    }

    get remaining(): number {
        return this.target.byteLength - this.cursor;
    }

    view(): Uint8Array {
        return this.target.subarray(this.cursor);
    }

    advance(bytes: number): void {
        if (bytes > this.remaining) throw new RangeError("fake cursor overflow");
        this.cursor += bytes;
    }

    write(bytes: Uint8Array): void {
        if (bytes.byteLength > this.remaining) throw new RangeError("fake cursor overflow");
        this.target.set(bytes, this.cursor);
        this.cursor += bytes.byteLength;
    }
}

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
    produceErrorAfterPublish: unknown = null;
    startHook: ((deadline: Deadline) => Promise<void>) | null = null;
    private readonly copies = new CopyCounter();
    private readonly leases = new Set<ReceiveLease>();

    constructor(
        readonly budget: ByteBudget,
        readonly handlers: FrameChannelHandlers,
    ) {}

    async start(deadline: Deadline): Promise<void> {
        await this.startHook?.(deadline);
    }

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
        body.fill(new ArrayCursor(new Uint8Array(body.byteLength)));
        this.produced.push(header);
        hooks?.onPublish?.();
        if (this.produceErrorAfterPublish !== null) throw this.produceErrorAfterPublish;
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

type HarnessOptions = Pick<
    ConnectionGenerationOptions,
    "memoryCapBytes" | "maxBodyLen" | "maxPendingRequests" | "firstCorrelation"
> &
    Partial<Pick<ConnectionGenerationOptions, "credentials">>;

interface Harness {
    generation: ConnectionGeneration;
    channel: FakeChannel;
    retirements: { reason: string }[];
}

/** Constructs a generation over a `FakeChannel` without starting it. */
function build(options: HarnessOptions = {}): Harness {
    let channel: FakeChannel | undefined;
    const retirements: { reason: string }[] = [];
    const generation = new ConnectionGeneration({
        ...options,
        credentials: options.credentials ?? {
            key: new Uint8Array(32),
            daemonId: new Uint8Array(16),
            daemonVer: "test",
        },
        channelFactory: ({ budget, handlers }) => {
            channel = new FakeChannel(budget, handlers);
            return channel;
        },
        onRetired: (info) => retirements.push({ reason: info.reason }),
    });
    if (!channel) throw new Error("channel factory was not invoked");
    return { generation, channel, retirements };
}

async function harness(options: HarnessOptions = {}): Promise<Harness> {
    const built = build(options);
    await built.generation.start(Deadline.start(2_000));
    return built;
}

function routedRequest(
    generation: ConnectionGeneration,
    extra: { mode?: "unary" | "stream" } = {},
) {
    return generation.request({
        channel: CHANNEL,
        epoch: EPOCH,
        body: Buffer.from("{}"),
        deadline: Deadline.start(2_000),
        ...extra,
    });
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
                kind: "outcome_unknown",
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

describe("connection generation stream refusals", () => {
    test("the item limit and a body-mode mismatch are outcome_unknown with a correlation-scoped Cancel", async () => {
        const { generation, channel } = await harness();
        try {
            const limited = generation.request({
                channel: CHANNEL,
                epoch: EPOCH,
                body: Buffer.from("{}"),
                deadline: Deadline.start(2_000),
                mode: "stream",
                maxStreamItems: 1,
            });
            for (let index = 0; index < 2; index++) {
                channel.deliver(
                    header(FrameType.StreamData, limited.correlation, 2),
                    channel.lease(new TextEncoder().encode("{}")),
                );
            }
            await expect(limited.result).rejects.toMatchObject({
                kind: "outcome_unknown",
                code: "stream_item_limit",
            });

            const binary = generation.request({
                channel: CHANNEL,
                epoch: EPOCH,
                body: Buffer.from([1]),
                deadline: Deadline.start(2_000),
                mode: "stream",
                responseMode: "binary",
            });
            const lease = channel.lease(new TextEncoder().encode("{}"));
            channel.deliver(header(FrameType.StreamData, binary.correlation, 2, 0), lease);
            await expect(binary.result).rejects.toMatchObject({
                kind: "outcome_unknown",
                code: "expected_binary_response",
            });
            expect(lease.isReleased()).toBe(true);

            expect(channel.controls).toEqual([
                expect.objectContaining({
                    ty: FrameType.Cancel,
                    channel: CHANNEL,
                    epoch: EPOCH,
                    corr: limited.correlation,
                }),
                expect.objectContaining({
                    ty: FrameType.Cancel,
                    channel: CHANNEL,
                    epoch: EPOCH,
                    corr: binary.correlation,
                }),
            ]);
            expect(generation.stats().pendingRequests).toBe(0);
            expect(generation.isRetired()).toBe(false);
        } finally {
            generation.retire("owner_close");
        }
    });
});

describe("connection generation admission", () => {
    test("pending capacity refuses admission without spending a correlation", async () => {
        const { generation, channel } = await harness({ maxPendingRequests: 2 });
        try {
            const first = routedRequest(generation);
            routedRequest(generation);
            let caught: unknown;
            try {
                routedRequest(generation);
            } catch (error) {
                caught = error;
            }
            expect(caught).toMatchObject({ kind: "not_sent", code: "pending_capacity" });
            expect(channel.produced).toHaveLength(2);

            channel.deliver(
                header(FrameType.Response, first.correlation, 2),
                channel.lease(new TextEncoder().encode("{}")),
            );
            await first.result;
            expect(routedRequest(generation).correlation).toBe(3n);
        } finally {
            generation.retire("owner_close");
        }
        expect(() => build({ maxPendingRequests: 0 })).toThrow(RangeError);
    });

    test("the final correlation is used once and the next request retires the generation", async () => {
        const { generation, channel, retirements } = await harness({
            firstCorrelation: MAX_CORRELATION,
        });
        const final = routedRequest(generation);
        expect(final.correlation).toBe(MAX_CORRELATION);

        let caught: unknown;
        try {
            routedRequest(generation);
        } catch (error) {
            caught = error;
        }
        expect(caught).toMatchObject({ kind: "not_sent", code: "correlations_exhausted" });
        expect(generation.isRetired()).toBe(true);
        expect(retirements).toEqual([{ reason: "correlations_exhausted" }]);
        expect(channel.produced).toHaveLength(1);
        await expect(final.result).rejects.toMatchObject({
            kind: "outcome_unknown",
            code: "generation_retired",
        });
    });

    test("request() re-entered from a fill callback is refused and the outer request settles", async () => {
        const { generation, channel } = await harness();
        try {
            let nested: unknown;
            const body: DirectFrameBody = {
                byteLength: 2,
                fill: (cursor) => {
                    try {
                        routedRequest(generation);
                    } catch (error) {
                        nested = error;
                    }
                    cursor.write(Buffer.from("{}"));
                },
            };
            const outer = generation.request({
                channel: CHANNEL,
                epoch: EPOCH,
                body,
                deadline: Deadline.start(2_000),
            });
            expect(nested).toMatchObject({ kind: "not_sent", code: "reentrant_request" });
            expect(channel.produced).toHaveLength(1);
            expect(generation.stats().pendingRequests).toBe(1);

            channel.deliver(
                header(FrameType.Response, outer.correlation, 2),
                channel.lease(new TextEncoder().encode("{}")),
            );
            expect((await outer.result).kind).toBe("response");
            expect(routedRequest(generation).correlation).toBe(outer.correlation + 1n);
        } finally {
            generation.retire("owner_close");
        }
    });

    test("a produce failure after onPublish is outcome_unknown and retires the generation", async () => {
        const { generation, channel, retirements } = await harness();
        const inflight = routedRequest(generation);
        channel.produceErrorAfterPublish = new Error("publication wake failed; ring quarantined");

        let caught: unknown;
        try {
            routedRequest(generation);
        } catch (error) {
            caught = error;
        }
        expect(caught).toBeInstanceOf(HostCallError);
        expect(caught).toMatchObject({ kind: "outcome_unknown", code: "write_failed" });
        expect((caught as HostCallError).cause).toBe(channel.produceErrorAfterPublish);
        expect(generation.isRetired()).toBe(true);
        expect(retirements).toEqual([{ reason: "write_failed" }]);
        expect(generation.stats().pendingRequests).toBe(0);
        await expect(inflight.result).rejects.toMatchObject({
            kind: "outcome_unknown",
            code: "generation_retired",
        });
    });
});

describe("connection generation abort", () => {
    test("aborting a published routed request sends Cancel; a channel-0 abort does not", async () => {
        const { generation, channel } = await harness();
        try {
            const routed = routedRequest(generation);
            const cleanup = routed.abort().cleanup;
            await expect(routed.result).rejects.toMatchObject({
                kind: "outcome_unknown",
                code: "aborted",
            });
            expect(channel.controls).toEqual([
                expect.objectContaining({
                    ty: FrameType.Cancel,
                    channel: CHANNEL,
                    epoch: EPOCH,
                    corr: routed.correlation,
                }),
            ]);
            channel.deliver(
                header(FrameType.Response, routed.correlation, 2),
                channel.lease(new TextEncoder().encode("{}")),
            );
            await cleanup;

            const control = generation.request({
                channel: 0,
                epoch: 0,
                body: Buffer.from("{}"),
                deadline: Deadline.start(2_000),
            });
            control.abort();
            await expect(control.result).rejects.toMatchObject({ kind: "outcome_unknown" });
            expect(channel.controls).toHaveLength(1);
        } finally {
            generation.retire("owner_close");
        }
    });
});

describe("connection generation setup", () => {
    test("a start() that settles after the deadline in the same turn retires as setup_deadline", async () => {
        let now = 0;
        const deadline = Deadline.start(10, () => now);
        const { generation, channel, retirements } = build();
        channel.startHook = async () => {
            now = 10;
        };

        await expect(generation.start(deadline)).rejects.toBeInstanceOf(SocketTimeoutError);
        expect(retirements).toEqual([{ reason: "setup_deadline" }]);
        expect(() => routedRequest(generation)).toThrow(HostCallError);
        expect(generation.stats().activeTimers).toBe(0);
    });

    test("authenticatedDaemonId is a snapshot of the credentials", async () => {
        const daemonId = Uint8Array.from({ length: 16 }, (_, index) => index);
        const { generation } = await harness({
            credentials: { key: new Uint8Array(32), daemonId, daemonVer: "test" },
        });
        try {
            daemonId[0] = 0xff;
            expect(generation.authenticatedDaemonId?.[0]).toBe(0);
            expect(generation.authenticatedDaemonId).not.toBe(daemonId);
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

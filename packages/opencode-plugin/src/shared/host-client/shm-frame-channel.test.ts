import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
    NativeChannel,
    type NativeReceiveLease,
    type ProducerCursor,
    probeCapabilities,
    RING_FULL_MESSAGE,
} from "@eidnara/shm-native";
import { HostCallError } from "./errors";
import {
    ByteBudget,
    type FrameChannelCloseReason,
    type InboundFrame,
    ProducerError,
} from "./frame-channel";
import {
    decodeHeader,
    type EnvelopeHeader,
    encodeHeader,
    FrameType,
    HEADER_LEN,
    MAX_FRAME_BODY_LEN,
    PROTOCOL_VERSION,
} from "./protocol";
import { ShmFrameChannel } from "./shm-frame-channel";
import {
    type ContractPeerFrame,
    type FrameChannelContractFactory,
    frameChannelContractScenarios,
    runFrameChannelContractScenario,
    waitUntil,
} from "./test-support/frame-channel-contract";

/**
 * The addon is a release build of the Rust cdylib, absent from checkouts that
 * only typecheck and lint. A claimed native target must at least load it, so a
 * missing build fails there instead of skipping. Full availability is a
 * separate question the capability probe answers: Bun 1.3.14 has no
 * `markAsUntransferable`, so the probe stops before the shared-memory
 * mechanism and these scenarios skip until a runtime implements it.
 */
function nativeAvailable(): boolean {
    const capability = probeCapabilities();
    if (process.env.EIDNARA_SHM_NATIVE_CLAIMED_TARGET === "1") {
        expect(capability.reason).not.toBe("addon_unavailable");
    }
    return capability.available;
}

test("production shared-memory delivery has no timer polling", () => {
    const source = readFileSync(new URL("./shm-frame-channel.ts", import.meta.url), "utf8");
    expect(source).not.toContain("setInterval");
    expect(source).not.toContain(".poll(");
});

function responseHeader(ty: FrameType, corr: bigint, length: number, flags = 0): EnvelopeHeader {
    return {
        len: length,
        ver: PROTOCOL_VERSION,
        ty,
        flags,
        channel: 7,
        epoch: 1,
        corr,
    };
}

function publish(peer: NativeChannel, header: EnvelopeHeader, body: Uint8Array): void {
    peer.produce(encodeHeader(header), body.byteLength, (cursor) => cursor.write(body));
}

function take(peer: NativeChannel): NativeReceiveLease {
    let lease: NativeReceiveLease | undefined;
    expect(peer.drainOne((value) => (lease = value))).toBe(true);
    if (!lease) throw new Error("missing native lease");
    return lease;
}

/** Empty-body Response leases with correlations 1..count, for fakes that stand in for the ring. */
function emptyLeases(count: number): NativeReceiveLease[] {
    return Array.from({ length: count }, (_, index) => ({
        header: encodeHeader(responseHeader(FrameType.Response, BigInt(index + 1), 0)),
        byteLength: 0,
        segmentCount: 1,
        segment: () => new Uint8Array(),
        release: () => {},
    })) as unknown as NativeReceiveLease[];
}

/** A fake ring that hands out `leases` in order and reports empty afterwards. */
function drainFrom(
    leases: NativeReceiveLease[],
): (deliver: (lease: NativeReceiveLease) => void) => boolean {
    return (deliver) => {
        const lease = leases.shift();
        if (!lease) return false;
        deliver(lease);
        return true;
    };
}

const shmContractFactory: FrameChannelContractFactory = async (overrides = {}) => {
    const pair = NativeChannel.createTestPair();
    const budget = new ByteBudget(overrides.memoryCapBytes ?? 128 * 1024 * 1024);
    const frames: ContractPeerFrame[] = [];
    const received: { header: EnvelopeHeader; body: Uint8Array }[] = [];
    const closes: { reason: FrameChannelCloseReason; error: unknown }[] = [];
    const hook: { current: ((frame: InboundFrame) => boolean | undefined) | null } = {
        current: null,
    };
    let reading = true;
    let cleaning = false;
    const channel = new ShmFrameChannel({
        nativeChannel: pair.first,
        budget,
        maxBodyLen: overrides.maxBodyLen ?? MAX_FRAME_BODY_LEN,
        handlers: {
            onFrame: (frame) => {
                if (hook.current?.(frame)) return;
                received.push({ header: frame.header, body: frame.body.takeOwned() });
            },
            onClosed: (reason, error) => closes.push({ reason, error }),
        },
    });
    channel.beginFrames();
    const drain = (): void => {
        if (!reading || cleaning) return;
        while (
            pair.second.drainOne((lease) => {
                const header = decodeHeader(lease.header);
                const body = new Uint8Array(lease.byteLength);
                let offset = 0;
                for (let index = 0; index < lease.segmentCount; index++) {
                    const segment = lease.segment(index);
                    body.set(segment, offset);
                    offset += segment.byteLength;
                }
                lease.release();
                frames.push({ ...header, body });
            })
        ) {}
    };
    const drainTimer = setInterval(drain, 0);
    const peer = {
        get frames(): readonly ContractPeerFrame[] {
            return frames;
        },
        async send(fields: {
            ty: number;
            flags?: number;
            channel?: number;
            epoch?: number;
            corr?: bigint;
            body?: Uint8Array;
        }): Promise<void> {
            const body = fields.body ?? new Uint8Array();
            publish(
                pair.second,
                {
                    len: body.byteLength,
                    ver: PROTOCOL_VERSION,
                    ty: fields.ty,
                    flags: fields.flags ?? 0,
                    channel: fields.channel ?? 0,
                    epoch: fields.epoch ?? 0,
                    corr: fields.corr ?? 0n,
                },
                body,
            );
        },
        async sendBurst(
            fields: readonly {
                ty: number;
                flags?: number;
                channel?: number;
                epoch?: number;
                corr?: bigint;
                body?: Uint8Array;
            }[],
        ): Promise<void> {
            for (const frame of fields) await this.send(frame);
        },
        waitFor: async (check: () => boolean, timeoutMs?: number) => waitUntil(check, timeoutMs),
        pauseReading: () => {
            reading = false;
        },
        resumeReading: () => {
            reading = true;
            drain();
        },
        end: () => pair.second.close(),
        destroy: () => pair.second.forceClose(),
    };
    return {
        channel,
        budget,
        peer,
        reusesReceiveStorage: true,
        received,
        closes,
        get frameHook() {
            return hook.current;
        },
        set frameHook(value) {
            hook.current = value;
        },
        async cleanup() {
            cleaning = true;
            clearInterval(drainTimer);
            if (!channel.isClosed()) channel.close();
            pair.second.close();
        },
    };
};

describe("frame channel semantic contract (shared-memory factory)", () => {
    for (const scenario of frameChannelContractScenarios) {
        test(scenario.name, async () => {
            if (!nativeAvailable()) return;
            await runFrameChannelContractScenario(scenario, shmContractFactory);
        }, 30_000);
    }
});

describe("mandatory shared-memory channel", () => {
    test("native reservation publishes directly and cannot cancel after publication", () => {
        if (!nativeAvailable()) return;
        const pair = NativeChannel.createTestPair();
        const channel = new ShmFrameChannel({
            nativeChannel: pair.first,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: { onFrame: () => {}, onClosed: () => {} },
        });
        const { len: _len, ...header } = responseHeader(FrameType.Request, 8n, 4);
        const producer = channel.reserve(header, 4);
        const alias = producer.view();
        producer.write(Buffer.from([1, 2, 3, 4]));
        const ticket = producer.commit(4);
        expect(ticket.cancel()).toBe(false);
        expect(alias.byteLength).toBe(0);
        const lease = take(pair.second);
        expect(Array.from(lease.segment(0))).toEqual([1, 2, 3, 4]);
        lease.release();
        expect(channel.stats().ownedAdapterCopies).toBe(0);
        channel.close();
        pair.second.close();
    });

    test("owned receive adapter records exactly one copy", async () => {
        if (!nativeAvailable()) return;
        const pair = NativeChannel.createTestPair();
        let owned: Uint8Array | undefined;
        const channel = new ShmFrameChannel({
            nativeChannel: pair.first,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: {
                onFrame: (frame) => {
                    owned = frame.body.takeOwned();
                },
                onClosed: () => {},
            },
        });
        channel.beginFrames();
        publish(pair.second, responseHeader(FrameType.Response, 1n, 4), Buffer.from("once"));
        await waitUntil(() => owned !== undefined);
        expect(Buffer.from(owned ?? []).toString()).toBe("once");
        expect(channel.stats().ownedAdapterCopies).toBe(1);
        channel.close();
        pair.second.close();
    });

    test("close reports quarantine and rejects alias cleanup failure", async () => {
        const closes: { reason: FrameChannelCloseReason; error: unknown }[] = [];
        let delivered = false;
        let nativeCloseCalls = 0;
        const nativeLease = {
            header: encodeHeader(responseHeader(FrameType.Response, 1n, 5)),
            byteLength: 5,
            segmentCount: 1,
            segment: () => new Uint8Array(Buffer.from("maybe")),
            release: () => {
                throw new Error("detach failed");
            },
        } as unknown as NativeReceiveLease;
        const native = {
            startReadiness: (handler: () => void) => handler(),
            drainOne: (deliver: (lease: NativeReceiveLease) => void) => {
                if (delivered) return false;
                delivered = true;
                deliver(nativeLease);
                return true;
            },
            close: () => {
                nativeCloseCalls++;
            },
            peerClosed: () => false,
        } as unknown as NativeChannel;
        const channel = new ShmFrameChannel({
            nativeChannel: native,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: {
                onFrame: () => {},
                onClosed: (reason, error) => closes.push({ reason, error }),
            },
        });
        channel.beginFrames();
        await waitUntil(() => channel.stats().activeReceiveLeases === 1);
        expect(() => channel.close()).toThrow(
            "receive lease alias state is uncertain; storage quarantined",
        );
        expect(channel.stats().activeReceiveLeases).toBe(0);
        expect(channel.stats().quarantinedBytes).toBe(5);
        expect(closes.map((entry) => entry.reason)).toEqual(["quarantined"]);
        expect(nativeCloseCalls).toBe(0);
    });

    test("produce and reserve enforce the configured frame limit before any charge", () => {
        const budget = new ByteBudget(1 << 30);
        let produceCalls = 0;
        const native = {
            produce: () => {
                produceCalls++;
            },
            reserve: () => {
                throw new Error("reserve must not be reached");
            },
            close: () => {},
            peerClosed: () => false,
        } as unknown as NativeChannel;
        const channel = new ShmFrameChannel({
            nativeChannel: native,
            budget,
            maxBodyLen: 64,
            handlers: { onFrame: () => {}, onClosed: () => {} },
        });
        const header = {
            ver: PROTOCOL_VERSION,
            ty: FrameType.Request,
            flags: 0,
            channel: 7,
            epoch: 1,
            corr: 1n,
        };
        const oversize = { byteLength: 65, fill: () => {} };
        expect(() => channel.produce(header, oversize)).toThrow(RangeError);
        const poisoned = { byteLength: Number.NaN, fill: () => {} };
        expect(() => channel.produce(header, poisoned)).toThrow(RangeError);
        expect(() => channel.reserve(header, 65)).toThrow(RangeError);
        // Nothing was charged and the native ring was never touched.
        expect(budget.used).toBe(0);
        expect(produceCalls).toBe(0);
    });

    test("close aborts outstanding reservations and returns their budget charge", () => {
        // The cap admits exactly one reservation, so a leaked charge would
        // refuse every later publication on the shared budget.
        const budget = new ByteBudget(HEADER_LEN + 4);
        let abortCalls = 0;
        let produceCalls = 0;
        const native = {
            reserve: () => ({
                segments: [new Uint8Array(new ArrayBuffer(4))],
                commit: () => {
                    throw new Error("commit must not be reached");
                },
                abort: () => {
                    abortCalls++;
                },
            }),
            produce: () => {
                produceCalls++;
            },
            close: () => {},
            peerClosed: () => false,
        } as unknown as NativeChannel;
        const handlers = { onFrame: () => {}, onClosed: () => {} };
        const channel = new ShmFrameChannel({
            nativeChannel: native,
            budget,
            maxBodyLen: 1 << 20,
            handlers,
        });
        const { len: _len, ...header } = responseHeader(FrameType.Request, 1n, 4);
        const producer = channel.reserve(header, 4);
        producer.write(Buffer.from([1, 2]));
        expect(budget.used).toBe(HEADER_LEN + 4);
        expect(channel.stats().queueHeldBytes).toBe(HEADER_LEN + 4);

        channel.close();
        expect(abortCalls).toBe(1);
        expect(budget.used).toBe(0);
        expect(channel.stats().queueHeldBytes).toBe(0);
        // The abandoned producer is retired, not left aliasing freed storage.
        let caught: unknown;
        try {
            producer.write(Buffer.from([3]));
        } catch (error) {
            caught = error;
        }
        expect(caught).toBeInstanceOf(ProducerError);
        expect((caught as ProducerError).code).toBe("producer_aborted");
        // Another channel on the same budget is admitted again.
        const sibling = new ShmFrameChannel({
            nativeChannel: native,
            budget,
            maxBodyLen: 1 << 20,
            handlers,
        });
        sibling.produce(header, { byteLength: 4, fill: () => {} });
        expect(produceCalls).toBe(1);
        expect(budget.used).toBe(0);
    });

    test("close frees a reserved ring slot without publishing it", () => {
        if (!nativeAvailable()) return;
        const pair = NativeChannel.createTestPair();
        const budget = new ByteBudget(1024);
        const channel = new ShmFrameChannel({
            nativeChannel: pair.first,
            budget,
            maxBodyLen: 1 << 20,
            handlers: { onFrame: () => {}, onClosed: () => {} },
        });
        const { len: _len, ...header } = responseHeader(FrameType.Request, 2n, 4);
        const producer = channel.reserve(header, 4);
        producer.write(Buffer.from([1, 2]));
        expect(budget.used).toBe(HEADER_LEN + 4);

        channel.close();
        expect(budget.used).toBe(0);
        expect(pair.second.drainOne(() => {})).toBe(false);
        pair.second.close();
    });

    test("sendControl after close is a silent no-op", () => {
        let produceCalls = 0;
        const native = {
            produce: () => {
                produceCalls++;
            },
            close: () => {},
            peerClosed: () => false,
        } as unknown as NativeChannel;
        const channel = new ShmFrameChannel({
            nativeChannel: native,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: {
                onFrame: () => {},
                onClosed: () => {},
            },
        });
        channel.close();
        expect(() => channel.sendControl(responseHeader(FrameType.Pong, 1n, 0))).not.toThrow();
        expect(produceCalls).toBe(0);
    });

    test("a full ring is retryable backpressure, not a terminal failure", () => {
        const budget = new ByteBudget(1 << 20);
        let produceBlockMs: number | undefined;
        let reserveBlockMs: number | undefined;
        const native = {
            produce: (
                _header: Uint8Array,
                _capacity: number,
                _fill: unknown,
                _beforePublish: unknown,
                timeoutMs: number,
            ) => {
                produceBlockMs = timeoutMs;
                throw new Error(RING_FULL_MESSAGE);
            },
            reserve: (_capacity: number, timeoutMs: number) => {
                reserveBlockMs = timeoutMs;
                throw new Error(RING_FULL_MESSAGE);
            },
            close: () => {},
            peerClosed: () => false,
        } as unknown as NativeChannel;
        const channel = new ShmFrameChannel({
            nativeChannel: native,
            budget,
            maxBodyLen: 1 << 20,
            handlers: { onFrame: () => {}, onClosed: () => {} },
        });
        const header = responseHeader(FrameType.Request, 1n, 4);
        const body = {
            byteLength: 4,
            fill: (cursor: ProducerCursor) => cursor.write(new Uint8Array(4)),
        };

        for (const attempt of [
            () => channel.produce(header, body),
            () => channel.reserve(header, 4),
        ]) {
            let caught: unknown;
            try {
                attempt();
            } catch (error) {
                caught = error;
            }
            expect(caught).toBeInstanceOf(HostCallError);
            expect((caught as HostCallError).kind).toBe("not_sent");
            expect((caught as HostCallError).code).toBe("ring_full");
        }
        // Neither publication path may hold the event loop for ring capacity;
        // the loop is also the only consumer draining the inbound ring.
        expect(produceBlockMs).toBe(0);
        expect(reserveBlockMs).toBe(0);
        // Every refused attempt returns its charge.
        expect(budget.used).toBe(0);
    });

    test("sendControl drops the frame on a full ring instead of throwing", () => {
        let produceCalls = 0;
        const native = {
            produce: () => {
                produceCalls++;
                throw new Error(RING_FULL_MESSAGE);
            },
            close: () => {},
            peerClosed: () => false,
        } as unknown as NativeChannel;
        const channel = new ShmFrameChannel({
            nativeChannel: native,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: { onFrame: () => {}, onClosed: () => {} },
        });
        expect(() => channel.sendControl(responseHeader(FrameType.Pong, 1n, 0))).not.toThrow();
        expect(produceCalls).toBe(1);
        expect(channel.isClosed()).toBe(false);
    });

    test("a saturated outbound ring cannot block inbound readiness", async () => {
        if (!nativeAvailable()) return;
        const pair = NativeChannel.createTestPair();
        const received: bigint[] = [];
        const channel = new ShmFrameChannel({
            nativeChannel: pair.first,
            budget: new ByteBudget(1 << 20),
            maxBodyLen: 1 << 20,
            handlers: {
                onFrame: (frame) => {
                    received.push(frame.header.corr);
                    frame.body.release();
                },
                onClosed: () => {},
            },
        });
        channel.beginFrames();
        const body = { byteLength: 0, fill: () => {} };
        for (let index = 0; index < pair.descriptorDepth; index++) {
            channel.produce(responseHeader(FrameType.Request, BigInt(index + 1), 0), body);
        }
        publish(pair.second, responseHeader(FrameType.Response, 99n, 0), new Uint8Array());

        expect(() => channel.produce(responseHeader(FrameType.Request, 100n, 0), body)).toThrow(
            HostCallError,
        );
        await waitUntil(() => received.length === 1);
        expect(received).toEqual([99n]);

        channel.close();
        pair.second.close();
    });

    test("a dropped setup socket retires the channel as eof after draining", async () => {
        const closes: { reason: FrameChannelCloseReason; error: unknown }[] = [];
        const frames: EnvelopeHeader[] = [];
        let peerAlive = true;
        let pending = true;
        let ready: (() => void) | undefined;
        const nativeLease = {
            header: encodeHeader(responseHeader(FrameType.Response, 7n, 4)),
            byteLength: 4,
            segmentCount: 1,
            segment: () => new Uint8Array(Buffer.from("last")),
            release: () => {},
        } as unknown as NativeReceiveLease;
        const native = {
            drainOne: (deliver: (lease: NativeReceiveLease) => void) => {
                if (!pending) return false;
                pending = false;
                deliver(nativeLease);
                return true;
            },
            startReadiness: (callback: () => void) => {
                ready = callback;
                callback();
            },
            close: () => {},
            peerClosed: () => !peerAlive,
        } as unknown as NativeChannel;
        const channel = new ShmFrameChannel({
            nativeChannel: native,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: {
                onFrame: (frame) => {
                    frames.push(frame.header);
                    frame.body.release();
                },
                onClosed: (reason, error) => closes.push({ reason, error }),
            },
        });
        channel.beginFrames();
        await waitUntil(() => frames.length === 1);
        expect(closes).toEqual([]);

        peerAlive = false;
        ready?.();
        await waitUntil(() => closes.length === 1);
        // The frame that was already in the ring is delivered before the
        // connection retires, so a graceful Goodbye is never lost.
        expect(frames).toHaveLength(1);
        expect(closes[0]?.reason).toBe("eof");
        expect(channel.isClosed()).toBe(true);
    });

    test("setup EOF waits for every frame beyond one drain budget", async () => {
        const closes: FrameChannelCloseReason[] = [];
        const received: bigint[] = [];
        const native = {
            drainOne: drainFrom(emptyLeases(65)),
            startReadiness: (callback: () => void) => callback(),
            close: () => {},
            peerClosed: () => true,
        } as unknown as NativeChannel;
        const channel = new ShmFrameChannel({
            nativeChannel: native,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: {
                onFrame: (frame) => {
                    received.push(frame.header.corr);
                    frame.body.release();
                },
                onClosed: (reason) => closes.push(reason),
            },
        });

        channel.beginFrames();
        expect(received).toHaveLength(64);
        expect(closes).toEqual([]);
        await waitUntil(() => closes.length === 1);
        expect(received).toHaveLength(65);
        expect(closes).toEqual(["eof"]);
    });

    test("sustained inbound traffic yields to the macrotask queue between drain batches", async () => {
        // total exceeds the synchronous and microtask drain capacity.
        const total = 64 * 18;
        const received: bigint[] = [];
        const native = {
            drainOne: drainFrom(emptyLeases(total)),
            startReadiness: (callback: () => void) => callback(),
            close: () => {},
            peerClosed: () => false,
        } as unknown as NativeChannel;
        const channel = new ShmFrameChannel({
            nativeChannel: native,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: {
                onFrame: (frame) => {
                    received.push(frame.header.corr);
                    frame.body.release();
                },
                onClosed: () => {},
            },
        });
        let drainedAtMacrotask = -1;
        setImmediate(() => {
            drainedAtMacrotask = received.length;
        });

        channel.beginFrames();
        await waitUntil(() => received.length === total);
        expect(drainedAtMacrotask).toBeGreaterThan(0);
        expect(drainedAtMacrotask).toBeLessThan(total);
        channel.close();
    });

    test("an owner close inside onFrame retires the channel without a detected close", () => {
        const closes: FrameChannelCloseReason[] = [];
        const received: bigint[] = [];
        const leases = emptyLeases(2);
        let nativeClosed = false;
        let drainsAfterClose = 0;
        const native = {
            drainOne: (deliver: (lease: NativeReceiveLease) => void) => {
                // Reject drain attempts after `close` to detect post-close draining.
                if (nativeClosed) {
                    drainsAfterClose++;
                    throw new Error("native channel is closed");
                }
                return drainFrom(leases)(deliver);
            },
            startReadiness: (callback: () => void) => callback(),
            close: () => {
                nativeClosed = true;
            },
            peerClosed: () => false,
        } as unknown as NativeChannel;
        const channel = new ShmFrameChannel({
            nativeChannel: native,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: {
                onFrame: (frame) => {
                    received.push(frame.header.corr);
                    frame.body.release();
                    channel.close();
                },
                onClosed: (reason) => closes.push(reason),
            },
        });

        channel.beginFrames();
        expect(received).toEqual([1n]);
        expect(channel.isClosed()).toBe(true);
        expect(drainsAfterClose).toBe(0);
        // Owner closes are not channel-detected closes.
        expect(closes).toEqual([]);
    });

    test("a throwing onClosed handler still retires the channel and its native handle", () => {
        let nativeCloseCalls = 0;
        const native = {
            drainOne: () => {
                throw new Error("shared-memory receive failed");
            },
            // Mirrors the addon's dispatch: a handler that throws is
            // unregistered and its owner is told through onDropped.
            startReadiness: (handler: () => void, onDropped?: (error: unknown) => void) => {
                try {
                    handler();
                } catch (error) {
                    onDropped?.(error);
                }
            },
            close: () => {
                nativeCloseCalls++;
            },
            peerClosed: () => false,
        } as unknown as NativeChannel;
        const channel = new ShmFrameChannel({
            nativeChannel: native,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: {
                onFrame: () => {},
                onClosed: () => {
                    throw new Error("handler bug");
                },
            },
        });

        expect(() => channel.beginFrames()).not.toThrow();
        expect(channel.isClosed()).toBe(true);
        expect(nativeCloseCalls).toBe(1);
    });

    test("a dropped readiness registration fail-closes the channel", () => {
        const closes: FrameChannelCloseReason[] = [];
        let nativeCloseCalls = 0;
        const native = {
            startReadiness: (_handler: () => void, onDropped?: (error: unknown) => void) => {
                onDropped?.(new Error("readiness handler threw"));
            },
            close: () => {
                nativeCloseCalls++;
            },
            peerClosed: () => false,
        } as unknown as NativeChannel;
        const channel = new ShmFrameChannel({
            nativeChannel: native,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: {
                onFrame: () => {},
                onClosed: (reason) => closes.push(reason),
            },
        });

        channel.beginFrames();
        expect(channel.isClosed()).toBe(true);
        expect(nativeCloseCalls).toBe(1);
        expect(closes).toEqual(["protocol_violation"]);
    });

    test("handler throw releases JSON lease before fail-close", async () => {
        if (!nativeAvailable()) return;
        const pair = NativeChannel.createTestPair();
        let alias: Uint8Array | undefined;
        const channel = new ShmFrameChannel({
            nativeChannel: pair.first,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: {
                onFrame: (frame) => {
                    alias = frame.body.segment(0);
                    throw new Error("decode failed");
                },
                onClosed: () => {},
            },
        });
        channel.beginFrames();
        publish(pair.second, responseHeader(FrameType.Response, 1n, 2), Buffer.from("{}"));
        await new Promise((resolve) => setTimeout(resolve, 10));
        expect(channel.isClosed()).toBe(true);
        expect(alias?.byteLength).toBe(0);
        pair.second.close();
    });

    test("wrong version is rejected before publication and role-invalid input closes", async () => {
        if (!nativeAvailable()) return;
        const pair = NativeChannel.createTestPair();
        const encoded = encodeHeader(responseHeader(FrameType.Response, 1n, 0));
        encoded[4] = PROTOCOL_VERSION + 1;
        expect(() => pair.second.produce(encoded, 0, () => {})).toThrow();
        expect(pair.first.drainOne(() => {})).toBe(false);
        pair.first.close();
        pair.second.close();

        const rolePair = NativeChannel.createTestPair();
        const closes: FrameChannelCloseReason[] = [];
        const channel = new ShmFrameChannel({
            nativeChannel: rolePair.first,
            budget: new ByteBudget(1024),
            maxBodyLen: 1 << 20,
            handlers: {
                onFrame: () => {},
                onClosed: (reason) => closes.push(reason),
            },
        });
        channel.beginFrames();
        publish(rolePair.second, responseHeader(FrameType.Request, 1n, 0), new Uint8Array());
        await waitUntil(() => closes.length === 1);
        expect(closes).toEqual(["role_violation"]);
        expect(channel.isClosed()).toBe(true);
        rolePair.second.close();
    });
});

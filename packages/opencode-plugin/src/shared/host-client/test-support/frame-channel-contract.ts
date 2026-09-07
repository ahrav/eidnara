/**
 * Reusable semantic contract suite for the complete-frame channel.
 *
 * Every scenario is expressed against the mandatory shared-memory channel.
 *
 */

import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { HostCallError } from "../errors";
import {
    type ByteBudget,
    type FrameChannel,
    type FrameChannelCloseReason,
    type InboundFrame,
    type OutboundFrame,
    ProducerError,
    type ProducerFrameHeader,
} from "../frame-channel";
import { type EnvelopeHeader, FrameType, MAX_FRAME_BODY_LEN, PROTOCOL_VERSION } from "../protocol";

export async function waitUntil(check: () => boolean, timeoutMs = 3_000): Promise<void> {
    const startedAt = Date.now();
    while (!check()) {
        if (Date.now() - startedAt > timeoutMs) throw new Error("waitUntil timed out");
        await delay(5);
    }
}

function expectHostCallError(
    error: unknown,
    kind: HostCallError["kind"],
    code?: string,
): HostCallError {
    assert.ok(error instanceof HostCallError, `expected HostCallError, got ${String(error)}`);
    assert.equal(error.kind, kind);
    if (code !== undefined) assert.equal(error.code, code);
    return error;
}

const CHANNEL = 5;
const EPOCH = 9;
/** One frame decoded by the remote end's own independent decoder. */
export interface ContractPeerFrame {
    ty: number;
    flags: number;
    channel: number;
    epoch: number;
    corr: bigint;
    len: number;
    body: Uint8Array;
}

export interface ContractPeerFrameFields {
    ty: number;
    flags?: number;
    channel?: number;
    epoch?: number;
    corr?: bigint;
    body?: Uint8Array;
    /** `len` and `ver` override encoded values for malformed frames; otherwise they use the encoded values. */
    len?: number;
    ver?: number;
}

/* */
export interface ContractPeer {
    readonly frames: readonly ContractPeerFrame[];
    send(fields: ContractPeerFrameFields): Promise<void>;
    /** sendBurst delivers frames as one coalesced burst when the transport allows. */
    sendBurst(fields: ContractPeerFrameFields[]): Promise<void>;
    waitFor(check: () => boolean, timeoutMs?: number): Promise<void>;
    /* */
    pauseReading(): void;
    resumeReading(): void;
    /** end sends a clean end-of-stream toward the channel. */
    end(): void;
    /** destroy performs abortive teardown toward the channel. */
    destroy(): void;
}

export interface ContractChannelOverrides {
    maxBodyLen?: number;
    memoryCapBytes?: number;
}

export interface ContractReceivedFrame {
    header: EnvelopeHeader;
    body: Uint8Array;
}

/* */
export interface FrameChannelContractHandle {
    channel: FrameChannel;
    budget: ByteBudget;
    peer: ContractPeer;
    /** reusesReceiveStorage is true when releasing a lease must revoke aliases before reusing backing storage. */
    reusesReceiveStorage: boolean;
    /** The contract factory retains owned bodies after the channel releases each inbound lease. */
    received: ContractReceivedFrame[];
    /** closes records channel-detected closes in order; owner closes do not appear. */
    closes: { reason: FrameChannelCloseReason; error: unknown }[];
    /** frameHook runs before each delivery is recorded. */
    frameHook: ((frame: InboundFrame) => boolean | undefined) | null;
    /** Factories must bracket the hook, record, and lease release so `deliveryDepth()` reports nested delivery depth correctly. */
    deliveryDepth(): number;
    cleanup(): Promise<void>;
}

export type FrameChannelContractFactory = (
    overrides?: ContractChannelOverrides,
) => Promise<FrameChannelContractHandle>;

export interface FrameChannelContractScenario {
    name: string;
    run(create: FrameChannelContractFactory): Promise<void>;
}

/* */
export async function runFrameChannelContractScenario(
    scenario: FrameChannelContractScenario,
    factory: FrameChannelContractFactory,
): Promise<void> {
    const handles: FrameChannelContractHandle[] = [];
    const tracked: FrameChannelContractFactory = async (overrides) => {
        const handle = await factory(overrides);
        handles.push(handle);
        return handle;
    };
    try {
        await scenario.run(tracked);
    } finally {
        for (const handle of handles) {
            await handle.cleanup();
        }
    }
}

function requestFrame(corr: bigint, body: Uint8Array): OutboundFrame {
    return {
        header: {
            len: body.length,
            ver: PROTOCOL_VERSION,
            ty: FrameType.Request,
            flags: 0,
            channel: CHANNEL,
            epoch: EPOCH,
            corr,
        },
        body,
    };
}

function producerHeader(corr: bigint): ProducerFrameHeader {
    return {
        ver: PROTOCOL_VERSION,
        ty: FrameType.Request,
        flags: 0,
        channel: CHANNEL,
        epoch: EPOCH,
        corr,
    };
}

function requestCorrs(peer: ContractPeer): bigint[] {
    return peer.frames.filter((frame) => frame.ty === FrameType.Request).map((frame) => frame.corr);
}

export const frameChannelContractScenarios: readonly FrameChannelContractScenario[] = [
    {
        // One logical writer preserves FIFO admission while inbound frames
        // keep making progress in wire order.
        name: "concurrent send and receive preserve FIFO order",
        async run(create) {
            const h = await create();
            for (let i = 1; i <= 6; i++) {
                h.channel.send(requestFrame(BigInt(i), Buffer.from([i])));
            }
            for (let i = 1; i <= 3; i++) {
                await h.peer.send({
                    ty: FrameType.Response,
                    channel: CHANNEL,
                    epoch: EPOCH,
                    corr: BigInt(100 + i),
                    body: Buffer.from(`r${i}`),
                });
            }
            await h.peer.waitFor(() => requestCorrs(h.peer).length >= 6);
            assert.deepEqual(requestCorrs(h.peer), [1n, 2n, 3n, 4n, 5n, 6n]);
            const bodies = h.peer.frames
                .filter((frame) => frame.ty === FrameType.Request)
                .map((frame) => frame.body[0]);
            assert.deepEqual(bodies, [1, 2, 3, 4, 5, 6]);
            await waitUntil(() => h.received.length >= 3);
            assert.deepEqual(
                h.received.map((frame) => frame.header.corr),
                [101n, 102n, 103n],
            );
        },
    },
    {
        // Publication start and local completion are distinct and fire
        // exactly once, in order; completion never claims peer receipt —
        // both fire while the peer is provably not consuming.
        name: "publication and local completion fire exactly once, in order",
        async run(create) {
            const h = await create();
            h.peer.pauseReading();
            const events: string[] = [];
            const ticket = h.channel.send(requestFrame(1n, Buffer.from("once")), {
                onPublish: () => events.push("publish"),
                onComplete: () => events.push("complete"),
            });
            await waitUntil(() => events.length >= 2);
            assert.deepEqual(events, ["publish", "complete"]);
            assert.equal(
                requestCorrs(h.peer).length,
                0,
                "completion fired before the peer consumed anything",
            );
            // A published frame is no longer cancellable.
            assert.equal(ticket.cancel(), false);
            h.peer.resumeReading();
            await h.peer.waitFor(() => requestCorrs(h.peer).length >= 1);
            assert.deepEqual(events, ["publish", "complete"]);
        },
    },
    {
        // A held reservation charges the aggregate budget until commit or abort.
        // The second body alone fits under the cap; only the accumulated total refuses it.
        name: "byte saturation refuses admission at the aggregate cap",
        async run(create) {
            const capped = await create({ memoryCapBytes: 1_000 });
            const held = capped.channel.reserve(producerHeader(1n), 600);
            assert.ok(capped.budget.used >= 600, "a held reservation charges the budget");
            let overCap: unknown;
            try {
                capped.channel.send(requestFrame(2n, Buffer.alloc(600)));
            } catch (error) {
                overCap = error;
            }
            expectHostCallError(overCap, "not_sent", "memory_cap");

            held.abort();
            assert.equal(capped.budget.used, 0, "abort returns the charge");
            capped.channel.send(requestFrame(3n, Buffer.alloc(600)));
            await capped.peer.waitFor(() => requestCorrs(capped.peer).includes(3n));
            assert.deepEqual(requestCorrs(capped.peer), [3n]);
        },
    },
    {
        // Coalesced frames deliver in wire order, each from the single
        // delivery loop — never recursively from within another delivery.
        name: "coalesced frames deliver in order without recursive re-entry",
        async run(create) {
            const h = await create();
            let maxDepth = 0;
            h.frameHook = () => {
                maxDepth = Math.max(maxDepth, h.deliveryDepth());
                return undefined;
            };
            await h.peer.sendBurst(
                [1n, 2n, 3n].map((corr) => ({
                    ty: FrameType.Response,
                    channel: CHANNEL,
                    epoch: EPOCH,
                    corr,
                    body: Buffer.from(`c${corr}`),
                })),
            );
            await waitUntil(() => h.received.length === 3);
            assert.deepEqual(
                h.received.map((frame) => frame.header.corr),
                [1n, 2n, 3n],
            );
            assert.equal(maxDepth, 1);
        },
    },
    {
        // The transport write cursor is not controllable from this scenario, so a
        // reservation may never split into a second segment; `BoundedFrameProducer`
        // unit tests cover multi-segment traversal against synthetic two-segment spans.
        name: "bounded producers commit empty, boundary, and large bodies exactly",
        async run(create) {
            const h = await create({});
            const sizes = [0, 64, 65, 1 << 20];
            for (let i = 0; i < sizes.length; i++) {
                const size = sizes[i] as number;
                const source = new Uint8Array(size).fill(i + 1);
                const producer = h.channel.reserve(producerHeader(BigInt(i + 1)), size);
                const aliases: Uint8Array[] = [];
                let offset = 0;
                while (offset < source.length) {
                    const view = producer.view();
                    const take = Math.min(view.length, source.length - offset);
                    view.set(source.subarray(offset, offset + take));
                    aliases.push(view);
                    producer.advance(take);
                    offset += take;
                }
                producer.commit(size);
                assert.equal(producer.written, size);
                for (const alias of aliases) assert.equal(alias.byteLength, 0);
            }
            await h.peer.waitFor(() => requestCorrs(h.peer).length === sizes.length, 60_000);
            const published = h.peer.frames.filter((frame) => frame.ty === FrameType.Request);
            assert.deepEqual(
                published.map((frame) => frame.body.length),
                sizes,
            );
            for (let i = 0; i < sizes.length; i++) {
                const expected = new Uint8Array(sizes[i] as number).fill(i + 1);
                const body = published[i]?.body ?? new Uint8Array();
                assert.equal(Buffer.compare(body, expected), 0, `body ${i + 1} bytes differ`);
            }
            assert.equal(h.channel.stats().ownedAdapterCopies, 0);
        },
    },
    {
        // The wire contract requires an admitted connection to accept one otherwise valid
        // maximum-size frame. Views are filled in place so each side holds one body copy.
        name: "a fresh channel commits and delivers an exact 64 MiB body",
        async run(create) {
            const h = await create({});
            const size = MAX_FRAME_BODY_LEN;
            const producer = h.channel.reserve(producerHeader(1n), size);
            while (producer.remaining > 0) {
                const view = producer.view();
                assert.ok(view.byteLength > 0, "view exposes the remaining capacity");
                view.fill(0xa5);
                producer.advance(view.byteLength);
            }
            producer.commit(size);
            await h.peer.waitFor(() => requestCorrs(h.peer).includes(1n), 60_000);
            const body = h.peer.frames.find((frame) => frame.corr === 1n)?.body;
            assert.equal(body?.byteLength, size);
            assert.equal(Buffer.compare(body ?? new Uint8Array(), Buffer.alloc(size, 0xa5)), 0);
        },
    },
    {
        name: "underfill, overflow, and abort return reservations without publication",
        async run(create) {
            const h = await create({ maxBodyLen: 64, memoryCapBytes: 1_024 });

            const underfill = h.channel.reserve(producerHeader(1n), 8);
            underfill.write(Buffer.from("four"));
            assert.throws(
                () => underfill.commit(8),
                (error) => error instanceof ProducerError && error.code === "producer_underfill",
            );

            const overflow = h.channel.reserve(producerHeader(2n), 8);
            assert.throws(
                () => overflow.write(Buffer.alloc(9)),
                (error) => error instanceof ProducerError && error.code === "producer_overflow",
            );

            h.channel.reserve(producerHeader(3n), 8).abort();
            assert.equal(h.channel.stats().queueHeldBytes, 0);
            assert.equal(h.channel.stats().queuedDataFrames, 0);
            assert.equal(h.budget.used, 0);
            assert.equal(requestCorrs(h.peer).length, 0);

            const valid = h.channel.reserve(producerHeader(4n), 4);
            valid.write(Buffer.from("good"));
            valid.commit(4);
            await h.peer.waitFor(() => requestCorrs(h.peer).includes(4n));
            // FIFO publication puts any late publication of 1-3 ahead of 4 on the wire.
            assert.deepEqual(requestCorrs(h.peer), [4n]);
            const published = h.peer.frames.find((frame) => frame.corr === 4n);
            assert.equal(Buffer.from(published?.body ?? []).toString(), "good");
        },
    },
    {
        name: "owned receive adapter copies once after transport lease release",
        async run(create) {
            const h = await create();
            h.frameHook = (frame) => {
                const segment = frame.body.segment(0);
                assert.equal(segment.byteOffset, 0);
                assert.equal(segment.byteLength, segment.buffer.byteLength);
                return undefined;
            };
            await h.peer.send({
                ty: FrameType.Response,
                channel: CHANNEL,
                epoch: EPOCH,
                corr: 1n,
                body: Buffer.from("owned"),
            });
            await waitUntil(() => h.received.length === 1);
            assert.equal(Buffer.from(h.received[0]?.body ?? []).toString(), "owned");
            assert.equal(h.channel.stats().activeReceiveLeases, 0);
            assert.equal(h.channel.stats().ownedAdapterCopies, 1);
        },
    },
    {
        name: "close revokes active receive aliases before storage reuse",
        async run(create) {
            const h = await create();
            const held: { frame: InboundFrame | null; alias: Uint8Array | null } = {
                frame: null,
                alias: null,
            };
            h.frameHook = (frame) => {
                held.frame = frame;
                held.alias = frame.body.segment(0);
                return true;
            };
            await h.peer.send({
                ty: FrameType.Response,
                channel: CHANNEL,
                epoch: EPOCH,
                corr: 1n,
                body: Buffer.from("lease"),
            });
            await waitUntil(() => held.frame !== null);
            assert.equal(held.alias?.byteLength, 5);
            h.channel.close();
            assert.equal(held.alias?.byteLength, h.reusesReceiveStorage ? 0 : 5);
            assert.equal(held.frame?.body.isReleased(), true);
            assert.equal(h.channel.stats().activeReceiveLeases, 0);
            assert.throws(() => held.frame?.body.segment(0), /released/);
        },
    },
    {
        // A structurally valid header that the host may not originate never reaches `onFrame`.
        name: "a role-invalid inbound frame closes the channel without delivery",
        async run(create) {
            const h = await create();
            await h.peer.send({
                ty: FrameType.Request,
                channel: CHANNEL,
                epoch: EPOCH,
                corr: 1n,
                body: Buffer.from("host-originated request"),
            });
            await waitUntil(() => h.closes.length === 1);
            assert.equal(h.closes[0]?.reason, "role_violation");
            assert.equal(h.received.length, 0);
            assert.equal(h.channel.isClosed(), true);
        },
    },
    {
        name: "a stream frame on the control channel closes the channel as a protocol violation",
        async run(create) {
            const h = await create();
            await h.peer.send({
                ty: FrameType.StreamData,
                channel: 0,
                epoch: 0,
                corr: 1n,
                body: Buffer.from("misrouted"),
            });
            await waitUntil(() => h.closes.length === 1);
            assert.equal(h.closes[0]?.reason, "protocol_violation");
            assert.equal(h.received.length, 0);
            assert.equal(h.channel.isClosed(), true);
        },
    },
    {
        name: "send rejects a header whose len disagrees with the body and publishes nothing",
        async run(create) {
            const h = await create();
            const frame = requestFrame(1n, Buffer.from("four"));
            assert.throws(() =>
                h.channel.send({ header: { ...frame.header, len: 3 }, body: frame.body }),
            );
            assert.equal(h.budget.used, 0);
            assert.equal(h.channel.stats().queueHeldBytes, 0);
            h.channel.send(requestFrame(2n, Buffer.from("ok")));
            await h.peer.waitFor(() => requestCorrs(h.peer).includes(2n));
            assert.deepEqual(requestCorrs(h.peer), [2n]);
        },
    },
    {
        name: "produce rejects a body that fills fewer bytes than it declares",
        async run(create) {
            const h = await create();
            let publishes = 0;
            assert.throws(() =>
                h.channel.produce(
                    producerHeader(1n),
                    { byteLength: 8, fill: (cursor) => cursor.write(Buffer.from("four")) },
                    { onPublish: () => publishes++ },
                ),
            );
            assert.equal(publishes, 0);
            assert.equal(h.budget.used, 0);
            assert.equal(h.channel.stats().queueHeldBytes, 0);
            h.channel.produce(producerHeader(2n), {
                byteLength: 4,
                fill: (cursor) => cursor.write(Buffer.from("good")),
            });
            await h.peer.waitFor(() => requestCorrs(h.peer).includes(2n));
            assert.deepEqual(requestCorrs(h.peer), [2n]);
        },
    },
    {
        name: "sendControl publishes a pure-header frame the peer decodes",
        async run(create) {
            const h = await create();
            h.channel.sendControl({
                len: 0,
                ver: PROTOCOL_VERSION,
                ty: FrameType.Pong,
                flags: 0,
                channel: 0,
                epoch: 0,
                corr: 7n,
            });
            await h.peer.waitFor(() => h.peer.frames.some((frame) => frame.ty === FrameType.Pong));
            const pong = h.peer.frames.find((frame) => frame.ty === FrameType.Pong);
            assert.equal(pong?.corr, 7n);
            assert.equal(pong?.channel, 0);
            assert.equal(pong?.epoch, 0);
            assert.equal(pong?.len, 0);
        },
    },
    {
        name: "peer end-of-stream closes the channel with eof and delivers nothing",
        async run(create) {
            const h = await create();
            h.peer.end();
            await waitUntil(() => h.closes.length === 1);
            assert.equal(h.closes[0]?.reason, "eof");
            assert.equal(h.received.length, 0);
            assert.equal(h.channel.isClosed(), true);
        },
    },
    {
        name: "abortive peer teardown closes the channel and delivers nothing",
        async run(create) {
            const h = await create();
            h.peer.destroy();
            await waitUntil(() => h.closes.length === 1);
            const reason = h.closes[0]?.reason;
            assert.ok(
                reason === "eof" || reason === "protocol_violation",
                `close reason ${String(reason)} is not a retirement`,
            );
            assert.equal(h.received.length, 0);
            assert.equal(h.channel.isClosed(), true);
        },
    },
];

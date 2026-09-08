import { describe, expect, test } from "bun:test";
import {
    ByteBudget,
    CopyCounter,
    type FrameChannel,
    type FrameChannelCloseReason,
    type InboundFrame,
    ReceiveLease,
} from "../frame-channel";
import { type EnvelopeHeader, FrameType, PROTOCOL_VERSION } from "../protocol";
import {
    type ContractPeer,
    type ContractPeerFrame,
    type ContractPeerFrameFields,
    type ContractReceivedFrame,
    type FrameChannelContractFactory,
    type FrameChannelContractHandle,
    type FrameChannelContractScenario,
    frameChannelContractScenarios,
    runFrameChannelContractScenario,
} from "./frame-channel-contract";

type DeliveryMode = "loop" | "recursive";

function scenarioNamed(name: string): FrameChannelContractScenario {
    const scenario = frameChannelContractScenarios.find((candidate) => candidate.name === name);
    if (!scenario) throw new Error(`no contract scenario named ${name}`);
    return scenario;
}

function notExercised(): never {
    throw new Error("the fake channel does not exercise this method");
}

const NOT_EXERCISED_CHANNEL: FrameChannel = {
    produce: notExercised,
    reserve: notExercised,
    send: notExercised,
    sendControl: notExercised,
    flush: notExercised,
    close: notExercised,
    isClosed: () => false,
    stats: notExercised,
};

function inboundHeader(fields: ContractPeerFrameFields, len: number): EnvelopeHeader {
    return {
        len,
        ver: fields.ver ?? PROTOCOL_VERSION,
        ty: fields.ty as EnvelopeHeader["ty"],
        flags: fields.flags ?? 0,
        channel: fields.channel ?? 0,
        epoch: fields.epoch ?? 0,
        corr: fields.corr ?? 0n,
    };
}

/**
 * `loop` delivers each frame of a burst from a single loop. `recursive` delivers the next
 * frame from inside the previous lease's release callback, so the previous delivery remains
 * on the call stack while the next one runs.
 */
class FakeHandle implements FrameChannelContractHandle {
    readonly channel = NOT_EXERCISED_CHANNEL;
    readonly budget = new ByteBudget(1 << 20);
    readonly reusesReceiveStorage = false;
    readonly received: ContractReceivedFrame[] = [];
    readonly closes: { reason: FrameChannelCloseReason; error: unknown }[] = [];
    frameHook: ((frame: InboundFrame) => boolean | undefined) | null = null;
    readonly peer: ContractPeer;

    private activeDeliveries = 0;
    private readonly copies = new CopyCounter();

    constructor(private readonly mode: DeliveryMode) {
        const frames: ContractPeerFrame[] = [];
        this.peer = {
            frames,
            send: async (fields) => {
                this.deliverBurst([fields]);
            },
            sendBurst: async (fields) => {
                this.deliverBurst(fields);
            },
            waitFor: async (check) => {
                if (!check()) throw new Error("fake peer state never changes asynchronously");
            },
            pauseReading: () => {},
            resumeReading: () => {},
            end: () => {},
            destroy: () => {},
        };
    }

    deliveryDepth(): number {
        return this.activeDeliveries;
    }

    async cleanup(): Promise<void> {}

    private deliverBurst(burst: ContractPeerFrameFields[]): void {
        if (this.mode === "loop") {
            for (const fields of burst) this.deliver(fields, () => {});
            return;
        }
        const deliverFrom = (index: number): void => {
            const fields = burst[index];
            if (!fields) return;
            this.deliver(fields, () => deliverFrom(index + 1));
        };
        deliverFrom(0);
    }

    private deliver(fields: ContractPeerFrameFields, onRelease: () => void): void {
        const body = fields.body ?? new Uint8Array(0);
        const segment = new Uint8Array(new ArrayBuffer(body.byteLength));
        segment.set(body);
        const lease = new ReceiveLease([segment], onRelease, this.copies);
        const frame: InboundFrame = { header: inboundHeader(fields, body.byteLength), body: lease };

        this.activeDeliveries++;
        try {
            const hold = this.frameHook?.(frame);
            if (hold === true) return;
            // `lease.release()` can reenter delivery, so record the frame first to preserve delivery order.
            const owned = new Uint8Array(lease.byteLength);
            owned.set(lease.segment(0));
            this.received.push({ header: frame.header, body: owned });
            lease.release();
        } finally {
            this.activeDeliveries--;
        }
    }
}

function factoryFor(mode: DeliveryMode): FrameChannelContractFactory {
    return async () => new FakeHandle(mode);
}

describe("contract scenario: coalesced frames deliver in order without recursive re-entry", () => {
    const scenario = scenarioNamed("coalesced frames deliver in order without recursive re-entry");

    test("accepts a channel that delivers each coalesced frame from one loop", async () => {
        await runFrameChannelContractScenario(scenario, factoryFor("loop"));
    });

    test("rejects a channel that delivers the next frame from inside the previous delivery", async () => {
        await expect(
            runFrameChannelContractScenario(scenario, factoryFor("recursive")),
        ).rejects.toThrow();
    });

    test("the recursive fake really nests deliveries", async () => {
        const handle = new FakeHandle("recursive");
        const depths: number[] = [];
        handle.frameHook = () => {
            depths.push(handle.deliveryDepth());
            return undefined;
        };
        await handle.peer.sendBurst([
            { ty: FrameType.Response, channel: 5, epoch: 9, corr: 1n, body: new Uint8Array(1) },
            { ty: FrameType.Response, channel: 5, epoch: 9, corr: 2n, body: new Uint8Array(1) },
        ]);
        expect(depths).toEqual([1, 2]);
    });
});

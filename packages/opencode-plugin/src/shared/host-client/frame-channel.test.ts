import { describe, expect, test } from "bun:test";
import { BoundedFrameProducer, headerViolation, type StorageReleaseOutcome } from "./frame-channel";
import { type EnvelopeHeader, FrameType, PROTOCOL_VERSION } from "./protocol";

/** Producer segments must be exact-bounds ArrayBuffers, so each segment gets its own backing store. */
function exactSegments(...lengths: number[]): Uint8Array[] {
    return lengths.map((length) => new Uint8Array(new ArrayBuffer(length)));
}

/** `WebAssembly.Memory.buffer` cannot be transferred, so the producer's transfer-based detachment fails on it. */
function undetachableSegment(): Uint8Array {
    return new Uint8Array(new WebAssembly.Memory({ initial: 1 }).buffer);
}

interface ReleaseObservation {
    outcome: StorageReleaseOutcome;
    aliasLengthsAtRelease: number[];
}

function producerWith(
    segments: readonly Uint8Array[],
    capacity: number,
    aliases: Uint8Array[],
    observations: ReleaseObservation[],
): BoundedFrameProducer {
    return new BoundedFrameProducer(
        segments,
        capacity,
        () => ({ publish: () => ({ cancel: () => false }) }),
        (outcome) => {
            observations.push({
                outcome,
                aliasLengthsAtRelease: aliases.map((alias) => alias.byteLength),
            });
        },
    );
}

describe("BoundedFrameProducer.abort", () => {
    test("revokes every writable alias before the reservation is returned", () => {
        const aliases: Uint8Array[] = [];
        const observations: ReleaseObservation[] = [];
        const producer = producerWith(exactSegments(4, 4), 8, aliases, observations);

        aliases.push(producer.view());
        producer.advance(4);
        aliases.push(producer.view());

        producer.abort();

        expect(observations).toHaveLength(1);
        // The transport may hand this span to another reserver synchronously
        // inside the release callback, so no alias may still be live here.
        expect(observations[0]?.aliasLengthsAtRelease).toEqual([0, 0]);
        expect(observations[0]?.outcome).toBe("released");
    });

    test("reports quarantined when an alias cannot be detached", () => {
        const aliases: Uint8Array[] = [];
        const observations: ReleaseObservation[] = [];
        const segment = undetachableSegment();
        const producer = producerWith([segment], 16, aliases, observations);
        aliases.push(producer.view());

        producer.abort();

        expect(observations).toHaveLength(1);
        expect(observations[0]?.outcome).toBe("quarantined");
        expect(aliases[0]?.byteLength).toBeGreaterThan(0);
    });

    test("overflow aborts detach before release and surface the producer error", () => {
        const aliases: Uint8Array[] = [];
        const observations: ReleaseObservation[] = [];
        const producer = producerWith(exactSegments(8), 8, aliases, observations);
        aliases.push(producer.view());

        expect(() => producer.write(new Uint8Array(9))).toThrow(/producer_overflow/);

        expect(observations).toHaveLength(1);
        expect(observations[0]?.aliasLengthsAtRelease).toEqual([0]);
        expect(observations[0]?.outcome).toBe("released");
    });

    test("is idempotent after the first abort", () => {
        const observations: ReleaseObservation[] = [];
        const producer = producerWith(exactSegments(8), 8, [], observations);
        producer.abort();
        producer.abort();
        expect(observations).toHaveLength(1);
    });
});

describe("BoundedFrameProducer.commit", () => {
    test("detaches aliases before publication and never touches the release callback", () => {
        const aliases: Uint8Array[] = [];
        const observations: ReleaseObservation[] = [];
        const producer = producerWith(exactSegments(4, 4), 8, aliases, observations);
        aliases.push(producer.view());
        producer.advance(4);
        aliases.push(producer.view());
        producer.advance(4);

        producer.commit(8);

        expect(aliases.map((alias) => alias.byteLength)).toEqual([0, 0]);
        expect(observations).toHaveLength(0);
    });

    test("an undetachable alias fails the commit and quarantines the reservation", () => {
        const aliases: Uint8Array[] = [];
        const observations: ReleaseObservation[] = [];
        const producer = producerWith([undetachableSegment()], 4, aliases, observations);
        producer.write(new Uint8Array([1, 2, 3, 4]));

        expect(() => producer.commit(4)).toThrow(/detachment failed/);

        expect(observations).toHaveLength(1);
        expect(observations[0]?.outcome).toBe("quarantined");
    });

    test("a failed publish releases the reservation as released", () => {
        const observations: ReleaseObservation[] = [];
        const producer = new BoundedFrameProducer(
            exactSegments(4),
            4,
            () => ({
                publish: () => {
                    throw new Error("publish rejected");
                },
            }),
            (outcome) => {
                observations.push({ outcome, aliasLengthsAtRelease: [] });
            },
        );
        producer.write(new Uint8Array(4));

        expect(() => producer.commit(4)).toThrow(/publish rejected/);

        expect(observations.map((observation) => observation.outcome)).toEqual(["released"]);
    });
});

describe("BoundedFrameProducer segment traversal", () => {
    test("view and advance place bytes across the segment boundary in order", () => {
        const segments = exactSegments(4, 4);
        const producer = producerWith(segments, 8, [], []);
        const source = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]);
        let offset = 0;
        const viewLengths: number[] = [];
        while (offset < source.length) {
            const view = producer.view();
            viewLengths.push(view.byteLength);
            view.set(source.subarray(offset, offset + view.byteLength));
            producer.advance(view.byteLength);
            offset += view.byteLength;
        }

        expect(viewLengths).toEqual([4, 4]);
        expect(Array.from(segments[0] as Uint8Array)).toEqual([1, 2, 3, 4]);
        expect(Array.from(segments[1] as Uint8Array)).toEqual([5, 6, 7, 8]);
        expect(producer.view().byteLength).toBe(0);
    });

    test("write splits one source across segments and continues from a mid-segment cursor", () => {
        const segments = exactSegments(4, 4);
        const producer = producerWith(segments, 8, [], []);

        producer.write(new Uint8Array([1, 2]));
        producer.write(new Uint8Array([3, 4, 5, 6]));
        producer.write(new Uint8Array([7, 8]));

        expect(producer.written).toBe(8);
        expect(Array.from(segments[0] as Uint8Array)).toEqual([1, 2, 3, 4]);
        expect(Array.from(segments[1] as Uint8Array)).toEqual([5, 6, 7, 8]);
    });

    test("view stops at the capacity even when the segment has more room", () => {
        const segments = exactSegments(4, 4);
        const producer = producerWith(segments, 6, [], []);
        producer.advance(4);

        expect(producer.view().byteLength).toBe(2);
    });
});

function violationHeader(fields: Partial<EnvelopeHeader> & { ty: number }): EnvelopeHeader {
    return {
        len: 0,
        ver: PROTOCOL_VERSION,
        flags: 0,
        channel: 7,
        epoch: 1,
        corr: 1n,
        ...fields,
    } as EnvelopeHeader;
}

describe("headerViolation", () => {
    test("accepts routed stream frames and control-channel terminals", () => {
        expect(headerViolation(violationHeader({ ty: FrameType.StreamData, len: 4 }))).toBeNull();
        expect(headerViolation(violationHeader({ ty: FrameType.StreamEnd }))).toBeNull();
        expect(
            headerViolation(violationHeader({ ty: FrameType.Response, channel: 0, epoch: 0 })),
        ).toBeNull();
        expect(
            headerViolation(violationHeader({ ty: FrameType.Error, channel: 0, epoch: 0 })),
        ).toBeNull();
    });

    test("rejects stream frames on channel 0 even with a nonzero correlation", () => {
        for (const ty of [FrameType.StreamData, FrameType.StreamEnd]) {
            expect(headerViolation(violationHeader({ ty, channel: 0, epoch: 0 }))).toEqual({
                reason: "protocol_violation",
                detail: "stream frame on channel 0",
            });
        }
    });

    test("rejects terminal and stream frames with correlation 0", () => {
        for (const ty of [
            FrameType.Response,
            FrameType.Error,
            FrameType.StreamData,
            FrameType.StreamEnd,
        ]) {
            expect(headerViolation(violationHeader({ ty, corr: 0n }))?.reason).toBe(
                "protocol_violation",
            );
        }
    });

    test("rejects StreamEnd with a body", () => {
        expect(headerViolation(violationHeader({ ty: FrameType.StreamEnd, len: 1 }))).toEqual({
            reason: "protocol_violation",
            detail: "StreamEnd with a non-empty body",
        });
    });
});

describe("BoundedFrameProducer constructor", () => {
    test("a rejected reservation is released without quarantine", () => {
        const observations: ReleaseObservation[] = [];
        expect(
            () =>
                new BoundedFrameProducer(
                    exactSegments(4),
                    5,
                    () => ({ publish: () => ({ cancel: () => false }) }),
                    (outcome) => {
                        observations.push({ outcome, aliasLengthsAtRelease: [] });
                    },
                ),
        ).toThrow(/exceeds reserved spans/);
        expect(observations.map((observation) => observation.outcome)).toEqual(["released"]);
    });
});

import { describe, expect, test } from "bun:test";
import {
    buildFlags,
    DecodeError,
    type DecodeErrorCode,
    decodeHeader,
    type EnvelopeHeader,
    encodeHeader,
    FROZEN_PREFIX_LEN,
    FrameType,
    flagsAdmissionClass,
    flagsBinary,
    flagsLast,
    flagsPriority,
    HEADER_LEN,
    isLegalConsumerToHostType,
    isLegalHostToConsumerType,
    isPureHeader,
    MAX_CORRELATION,
    MAX_FRAME_BODY_LEN,
    PROTOCOL_VERSION,
    settledCorrelationNamespace,
} from "./protocol";
import { AdmissionClass, Priority } from "./types";

const ROUTE_OPEN_HEADER_HEX = "a70000000200020000000000000100000000000000";
const ROUTED_REQUEST_HEADER_HEX = "2c00000002000407004d0000000200000000000000";
/** The wire doc Section 7.2 compact canonical `route.open` request body. */
const ROUTE_OPEN_CANONICAL_BODY =
    '{"op":"route.open","target":{"kind":"tool_provider","module_id":"context"},"identity":{"project_root":"/workspace/project","harness":"opencode","session":"session-1"}}';

function hexToBytes(hex: string): Uint8Array {
    const bytes = new Uint8Array(hex.length / 2);
    for (let i = 0; i < bytes.length; i++) {
        bytes[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
    }
    return bytes;
}

function bytesToHex(bytes: Uint8Array): string {
    return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function decodeHex(hex: string): EnvelopeHeader {
    return decodeHeader(hexToBytes(hex));
}

function expectDecodeError(bytes: Uint8Array, code: DecodeErrorCode): void {
    let caught: unknown;
    try {
        decodeHeader(bytes);
    } catch (error) {
        caught = error;
    }
    expect(caught).toBeInstanceOf(DecodeError);
    expect((caught as DecodeError).code).toBe(code);
}

/** Returns a valid StreamData header. */
function validHeaderBytes(): Uint8Array {
    return hexToBytes(
        bytesToHex(
            encodeHeader({
                len: 0,
                ver: PROTOCOL_VERSION,
                ty: FrameType.StreamData,
                flags: 0,
                channel: 7,
                epoch: 77,
                corr: 5n,
            }),
        ),
    );
}

describe("committed wire-doc Section 6.4 vectors", () => {
    test("the canonical route.open header declares the Section 7.2 body's exact byte length", () => {
        // The committed header and the committed body must agree; the vector is not a free constant.
        // An independent length keeps the header literal honest instead of self-consistent.
        const bodyLen = new TextEncoder().encode(ROUTE_OPEN_CANONICAL_BODY).length;
        expect(bodyLen).toBe(167);
        expect(decodeHex(ROUTE_OPEN_HEADER_HEX).len).toBe(bodyLen);
    });

    test("each committed vector decodes field-by-field and encodes back to the exact bytes", () => {
        const vectors = [
            {
                name: "canonical route.open request",
                hex: ROUTE_OPEN_HEADER_HEX,
                len: 167,
                priority: Priority.Interactive,
                channel: 0,
                epoch: 0,
                corr: 1n,
            },
            {
                name: "routed Background request",
                hex: ROUTED_REQUEST_HEADER_HEX,
                len: 44,
                priority: Priority.Background,
                channel: 7,
                epoch: 77,
                corr: 2n,
            },
        ];
        for (const vector of vectors) {
            const header = decodeHex(vector.hex);
            expect(header.len).toBe(vector.len);
            expect(header.ver).toBe(2);
            expect(header.ty).toBe(FrameType.Request);
            expect(flagsBinary(header.flags)).toBe(false);
            expect(flagsPriority(header.flags)).toBe(vector.priority);
            expect(flagsLast(header.flags)).toBe(false);
            expect(flagsAdmissionClass(header.flags)).toBe(AdmissionClass.Normal);
            expect(header.channel).toBe(vector.channel);
            expect(header.epoch).toBe(vector.epoch);
            expect(header.corr).toBe(vector.corr);

            const bytes = encodeHeader({
                len: vector.len,
                ver: PROTOCOL_VERSION,
                ty: FrameType.Request,
                flags: buildFlags(false, vector.priority, false),
                channel: vector.channel,
                epoch: vector.epoch,
                corr: vector.corr,
            });
            expect(bytesToHex(bytes)).toBe(vector.hex);
        }
    });
});

describe("header round trips", () => {
    test("round-trips the minimum and maximum legal field values", () => {
        const minimum: EnvelopeHeader = {
            len: 0,
            ver: PROTOCOL_VERSION,
            ty: FrameType.Request,
            flags: buildFlags(false, Priority.Passive, false),
            channel: 0,
            epoch: 0,
            corr: 0n,
        };
        const minimumBytes = encodeHeader(minimum);
        expect(minimumBytes.length).toBe(HEADER_LEN);
        expect(decodeHeader(minimumBytes)).toEqual(minimum);

        const maximum: EnvelopeHeader = {
            len: MAX_FRAME_BODY_LEN,
            ver: PROTOCOL_VERSION,
            ty: FrameType.StreamData,
            flags: buildFlags(true, Priority.Background, true, AdmissionClass.Sheddable),
            channel: 0xffff,
            epoch: 0xffff_ffff,
            corr: MAX_CORRELATION,
        };
        const decoded = decodeHeader(encodeHeader(maximum));
        expect(decoded).toEqual(maximum);
        expect(decoded.corr).toBe(0xffff_ffff_ffff_ffffn);
    });

    test("decodes from a nonzero byte offset within a larger buffer", () => {
        const padded = new Uint8Array(7 + HEADER_LEN);
        padded.set(hexToBytes(ROUTED_REQUEST_HEADER_HEX), 7);
        const header = decodeHeader(padded.subarray(7));
        expect(header.channel).toBe(7);
        expect(header.epoch).toBe(77);
        expect(header.corr).toBe(2n);
    });
});

describe("structural rejections before body handling", () => {
    test("rejects a declared body of 64 MiB + 1 while accepting exactly 64 MiB", () => {
        const bytes = validHeaderBytes();
        const view = new DataView(bytes.buffer);
        view.setUint32(0, MAX_FRAME_BODY_LEN, true);
        expect(decodeHeader(bytes).len).toBe(67_108_864);
        view.setUint32(0, MAX_FRAME_BODY_LEN + 1, true);
        expectDecodeError(bytes, "frame_body_too_large");
    });

    test("rejects each illegal prefix, flag, and epoch encoding with its own decode code", () => {
        // Every row mutates one field of an otherwise valid routed StreamData header.
        const rows: { name: string; mutate: (bytes: Uint8Array) => void; code: DecodeErrorCode }[] =
            [
                ...[0, 1, 3, 255].map((ver) => ({
                    name: `version ${ver}`,
                    mutate: (bytes: Uint8Array) => {
                        bytes[4] = ver;
                    },
                    code: "unsupported_version" as const,
                })),
                ...[12, 13, 255].map((ty) => ({
                    name: `frame type ${ty}`,
                    mutate: (bytes: Uint8Array) => {
                        bytes[5] = ty;
                    },
                    code: "unknown_frame_type" as const,
                })),
                {
                    name: "priority bits value 3",
                    mutate: (bytes) => {
                        bytes[6] = 0b0000_0110;
                    },
                    code: "reserved_priority_bits",
                },
                {
                    name: "admission class bits value 3",
                    mutate: (bytes) => {
                        bytes[6] = 0b0011_0000;
                    },
                    code: "reserved_admission_class",
                },
                ...[0b0100_0000, 0b1000_0000, 0b1100_0000].map((reserved) => ({
                    name: `reserved flag bits ${reserved.toString(2)}`,
                    mutate: (bytes: Uint8Array) => {
                        bytes[6] = reserved;
                    },
                    code: "reserved_flag_bits" as const,
                })),
                {
                    name: "nonzero epoch on the control channel",
                    mutate: (bytes) => {
                        new DataView(bytes.buffer).setUint16(7, 0, true);
                    },
                    code: "nonzero_epoch_on_control_channel",
                },
                {
                    name: "zero epoch on a routed channel",
                    mutate: (bytes) => {
                        new DataView(bytes.buffer).setUint32(9, 0, true);
                    },
                    code: "zero_epoch_on_routed_channel",
                },
            ];
        for (const row of rows) {
            const bytes = validHeaderBytes();
            row.mutate(bytes);
            expectDecodeError(bytes, row.code);
        }
    });

    test("rejects truncated prefix and truncated header", () => {
        const bytes = validHeaderBytes();
        expectDecodeError(bytes.subarray(0, FROZEN_PREFIX_LEN - 1), "too_short_for_prefix");
        expectDecodeError(bytes.subarray(0, HEADER_LEN - 1), "too_short_for_header");
    });

    test("Sheddable admission is legal only on Push and StreamData", () => {
        const sheddable = buildFlags(false, Priority.Passive, false, AdmissionClass.Sheddable);
        for (const ty of [FrameType.Push, FrameType.StreamData]) {
            const bytes = validHeaderBytes();
            bytes[5] = ty;
            bytes[6] = sheddable;
            expect(decodeHeader(bytes).ty).toBe(ty);
        }
        for (const ty of [
            FrameType.Request,
            FrameType.Response,
            FrameType.StreamEnd,
            FrameType.Error,
        ]) {
            const bytes = validHeaderBytes();
            bytes[5] = ty;
            bytes[6] = sheddable;
            expectDecodeError(bytes, "sheddable_illegal_frame_type");
        }
    });

    test("rejects a declared body on every pure-header frame type", () => {
        for (const ty of [FrameType.Cancel, FrameType.Ping, FrameType.Pong, FrameType.Goodbye]) {
            expect(isPureHeader(ty)).toBe(true);
            const bytes = validHeaderBytes();
            const view = new DataView(bytes.buffer);
            bytes[5] = ty;
            bytes[6] = 0;
            view.setUint32(0, 1, true);
            expectDecodeError(bytes, "pure_header_frame_with_body");
        }
    });

    test("pure-header frames require binary=0, last=0, admission Normal; any priority is fine", () => {
        const illegal = [
            buildFlags(true, Priority.Passive, false),
            buildFlags(false, Priority.Passive, true),
            buildFlags(false, Priority.Passive, false, AdmissionClass.Expedite),
        ];
        for (const flags of illegal) {
            const bytes = validHeaderBytes();
            bytes[5] = FrameType.Ping;
            bytes[6] = flags;
            expectDecodeError(bytes, "pure_header_frame_flags");
        }
        const bytes = validHeaderBytes();
        bytes[5] = FrameType.Ping;
        bytes[6] = buildFlags(false, Priority.Interactive, false);
        expect(decodeHeader(bytes).ty).toBe(FrameType.Ping);
    });
});

describe("encode-side field validation", () => {
    function header(overrides: Partial<EnvelopeHeader>): EnvelopeHeader {
        return {
            len: 0,
            ver: PROTOCOL_VERSION,
            ty: FrameType.Request,
            flags: 0,
            channel: 0,
            epoch: 0,
            corr: 1n,
            ...overrides,
        };
    }

    test("rejects out-of-range or non-integer numeric fields and correlations outside u64", () => {
        expect(() => encodeHeader(header({ channel: 0x1_0000 }))).toThrow(RangeError);
        expect(() => encodeHeader(header({ channel: -1 }))).toThrow(RangeError);
        expect(() => encodeHeader(header({ channel: 1.5 }))).toThrow(RangeError);
        expect(() => encodeHeader(header({ epoch: 0x1_0000_0000 }))).toThrow(RangeError);
        expect(() => encodeHeader(header({ len: -1 }))).toThrow(RangeError);
        expect(() => encodeHeader(header({ corr: -1n }))).toThrow(RangeError);
        expect(() => encodeHeader(header({ corr: MAX_CORRELATION + 1n }))).toThrow(RangeError);
    });

    test("rejects structurally illegal headers with the shared decode taxonomy", () => {
        expect(() => encodeHeader(header({ ver: 1 }))).toThrow(DecodeError);
        expect(() => encodeHeader(header({ ty: FrameType.Ping, len: 4 }))).toThrow(DecodeError);
        expect(() => encodeHeader(header({ channel: 7, epoch: 0 }))).toThrow(DecodeError);
    });
});

describe("frame build and encode", () => {
    test("encodeHeader + body emits header then exactly len body bytes", () => {
        const body = new TextEncoder().encode('{"op":"catalog.list"}');
        const frameHeader: EnvelopeHeader = {
            len: body.length,
            ver: PROTOCOL_VERSION,
            ty: FrameType.Request,
            flags: buildFlags(false, Priority.Interactive, false),
            channel: 0,
            epoch: 0,
            corr: 3n,
        };
        const bytes = Buffer.concat([encodeHeader(frameHeader), body]);
        expect(bytes.length).toBe(HEADER_LEN + body.length);
        expect(decodeHeader(bytes)).toEqual(frameHeader);
        expect(bytes.subarray(HEADER_LEN)).toEqual(Buffer.from(body));
    });
});

describe("direction legality", () => {
    test("each direction accepts exactly its listed frame types", () => {
        const hostToConsumer = new Set<FrameType>([
            FrameType.Response,
            FrameType.Error,
            FrameType.StreamData,
            FrameType.StreamEnd,
            FrameType.Ping,
            FrameType.Push,
            FrameType.Goodbye,
        ]);
        const consumerToHost = new Set<FrameType>([
            FrameType.Request,
            FrameType.Cancel,
            FrameType.Pong,
            FrameType.Goodbye,
        ]);
        for (const ty of Object.values(FrameType)) {
            expect(isLegalHostToConsumerType(ty)).toBe(hostToConsumer.has(ty));
            expect(isLegalConsumerToHostType(ty)).toBe(consumerToHost.has(ty));
        }
    });

    test("Hello and HelloAck stay numerically decodable yet role-invalid from the host", () => {
        for (const ty of [FrameType.Hello, FrameType.HelloAck]) {
            const bytes = validHeaderBytes();
            const view = new DataView(bytes.buffer);
            bytes[5] = ty;
            view.setUint16(7, 0, true);
            view.setUint32(9, 0, true);
            expect(decodeHeader(bytes).ty).toBe(ty);
            expect(isLegalHostToConsumerType(ty)).toBe(false);
            expect(isLegalConsumerToHostType(ty)).toBe(false);
        }
    });

    test("correlation settlement is direction-scoped", () => {
        expect(settledCorrelationNamespace(FrameType.Response)).toBe("consumer");
        expect(settledCorrelationNamespace(FrameType.Error)).toBe("consumer");
        expect(settledCorrelationNamespace(FrameType.StreamData)).toBe("consumer");
        expect(settledCorrelationNamespace(FrameType.StreamEnd)).toBe("consumer");
        expect(settledCorrelationNamespace(FrameType.Pong)).toBe("host");
        expect(settledCorrelationNamespace(FrameType.Ping)).toBeUndefined();
        expect(settledCorrelationNamespace(FrameType.Request)).toBeUndefined();
        expect(settledCorrelationNamespace(FrameType.Goodbye)).toBeUndefined();
    });
});

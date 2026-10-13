import { describe, expect, it } from "bun:test";

import { rowLedger } from "./ledger";
import { serializePassRow } from "./rows";
import { WRITER_FIXTURE_ROWS } from "./writer-fixture";

const daemonLedger = {
    admit_us: 300,
    lane_gate_and_decode_us: 4_100,
    derivation_us: null,
    transform_core_us: 6_900,
    native_attach_us: 2_800,
    residual_us: 900,
    total_us: 15_000,
    window_messages: 301,
    window_blocks: 301,
};

const observed = {
    prefixGuardMs: 2.1,
    prepareMs: 0.4,
    wireBuildMs: 3.3,
    serializeMs: 1.9,
    transportMs: 17.2,
    applyMs: 2.6,
    daemonLedger,
};

describe("pass row ledger", () => {
    it("reconciles both ledgers into a row the writer serializes", () => {
        const ledger = rowLedger(31_000, observed);
        // The hook outside the named stages and the 17.2 ms round trip is other plugin work.
        expect(ledger.other_us).toBe(31_000 - 10_300 - 17_200);
        // The round trip less the daemon's span is transport and encode.
        expect(ledger.transport_and_encode_us).toBe(17_200 - 15_000);
        const base = { ...(WRITER_FIXTURE_ROWS[0] as Parameters<typeof serializePassRow>[0]) };
        const line = serializePassRow({ ...base, response_us: 31_000, ...ledger });
        expect(JSON.parse(line).daemon_total_us).toBe(15_000);
    });

    it("carries a native pass's derive_ck stage inside the reconciled daemon total", () => {
        const derived = rowLedger(31_000, {
            ...observed,
            daemonLedger: { ...daemonLedger, lane_gate_and_decode_us: 2_900, derivation_us: 1_200 },
        });
        expect(derived.derivation_us).toBe(1_200);
        expect(derived.lane_gate_and_decode_us).toBe(2_900);
        expect(derived.daemon_total_us).toBe(15_000);
        const base = { ...(WRITER_FIXTURE_ROWS[0] as Parameters<typeof serializePassRow>[0]) };
        expect(() => serializePassRow({ ...base, response_us: 31_000, ...derived })).not.toThrow();
        const malformed = rowLedger(31_000, {
            ...observed,
            daemonLedger: { ...daemonLedger, derivation_us: -1 },
        });
        expect(malformed.daemon_total_us).toBeNull();
    });

    it("reads the derivation stage a base-arm daemon carries under an alternate key, keeping a measured zero", () => {
        const { derivation_us: _, ...unnamed } = daemonLedger;
        const base = { ...(WRITER_FIXTURE_ROWS[0] as Parameters<typeof serializePassRow>[0]) };
        const derived = rowLedger(31_000, {
            ...observed,
            daemonLedger: { ...unnamed, lane_gate_and_decode_us: 2_900, derive_us: 1_200 },
        });
        expect(derived.derivation_us).toBe(1_200);
        expect(derived.daemon_total_us).toBe(15_000);
        expect(() => serializePassRow({ ...base, response_us: 31_000, ...derived })).not.toThrow();
        // A zero under the alternate key is a measured derivation, distinct from a CK profile's null.
        const zero = rowLedger(31_000, {
            ...observed,
            daemonLedger: { ...unnamed, derive_us: 0 },
        });
        expect(zero.derivation_us).toBe(0);
        expect(zero.daemon_total_us).toBe(15_000);
        const ck = rowLedger(31_000, {
            ...observed,
            daemonLedger: { ...unnamed, derive_us: null },
        });
        expect(ck.derivation_us).toBeNull();
        expect(ck.daemon_total_us).toBe(15_000);
        expect(() => serializePassRow({ ...base, response_us: 31_000, ...ck })).not.toThrow();
        // A ledger that omits the derivation stage keeps its daemon total when the named stages sum to it.
        const unstaged = rowLedger(31_000, { ...observed, daemonLedger: unnamed });
        expect(unstaged.derivation_us).toBeNull();
        expect(unstaged.daemon_total_us).toBe(15_000);
        // rowLedger sets daemon_total_us to null when the stages sum to a value other than total_us,
        // whether the derivation key is derivation_us or an alternate.
        const overdrawn = rowLedger(31_000, {
            ...observed,
            daemonLedger: { ...unnamed, lane_gate_and_decode_us: 5_000, derive_us: 1_200 },
        });
        expect(overdrawn.daemon_total_us).toBeNull();
        const short = rowLedger(31_000, {
            ...observed,
            daemonLedger: { ...daemonLedger, lane_gate_and_decode_us: 2_900 },
        });
        expect(short.daemon_total_us).toBeNull();
    });

    it("marks every stage unavailable without a daemon ledger and keeps the daemon ledger on a negative residual", () => {
        const none = rowLedger(31_000, { ...observed, daemonLedger: undefined });
        expect(Object.values(none).every((value) => value === null)).toBe(true);
        expect(Object.values(rowLedger(31_000, undefined)).every((value) => value === null)).toBe(
            true,
        );
        const negative = rowLedger(20_000, observed);
        expect(negative.daemon_total_us).toBe(15_000);
        expect(negative.prefix_guard_us).toBeNull();
        const malformed = rowLedger(31_000, {
            ...observed,
            daemonLedger: { ...daemonLedger, total_us: 1.5 },
        });
        expect(malformed.daemon_total_us).toBeNull();
    });
});

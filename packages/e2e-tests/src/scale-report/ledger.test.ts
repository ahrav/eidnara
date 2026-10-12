import { describe, expect, it } from "bun:test";

import { rowLedger } from "./ledger";
import { serializePassRow } from "./rows";
import { WRITER_FIXTURE_ROWS } from "./writer-fixture";

const daemonLedger = {
    admit_us: 300,
    lane_gate_and_decode_us: 4_100,
    derive_ck_us: null,
    transform_core_us: 6_900,
    native_attach_us: 2_800,
    residual_us: 900,
    total_us: 15_000,
    window_messages: 301,
    window_blocks: 301,
};

const observed = {
    elapsedMs: 24.5,
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

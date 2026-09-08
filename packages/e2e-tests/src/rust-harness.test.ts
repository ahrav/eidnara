import { describe, expect, it } from "bun:test";
import { parseRustPassLine } from "./rust-harness";

// One line in the exact shape `rust-mode-transform.ts` logs, so a format drift fails here
// instead of silently zeroing a timing the perf suite bounds.
const PASS_LINE =
    "[eidnara] rust pass: decision=DEFER reason=steady served_from=transform in=12 out=12 " +
    "applied=true row_version=7 elapsed=41.7 ms module=23.4 ms stages=prefix_guard:6.2 " +
    "ordinal_resolve:1.1 clone:0.4 wire_build:3.9 wire_messages:3 transport:9.8 " +
    "transport_pages:1 transport_bytes:20480 apply:1.2 other:0.8";

describe("parseRustPassLine", () => {
    it("reads top-level fields and every stage timing, including the first stage after stages=", () => {
        const pass = parseRustPassLine(PASS_LINE);
        expect(pass).not.toBeNull();
        expect(pass).toMatchObject({
            decision: "DEFER",
            reason: "steady",
            servedFrom: "transform",
            inputCount: 12,
            outputCount: 12,
            applied: true,
            rowVersion: 7,
            elapsedMs: 41.7,
            moduleElapsedMs: 23.4,
            prefixGuardMs: 6.2,
            wireBuildMs: 3.9,
            wireMessages: 3,
            transportMs: 9.8,
            transportPages: 1,
            transportBytes: 20480,
        });
        expect(pass?.adapterElapsedMs).toBeCloseTo(18.3, 5);
    });

    it("does not confuse transport with transport_pages or transport_bytes", () => {
        const pass = parseRustPassLine(PASS_LINE);
        expect(pass?.transportMs).toBe(9.8);
        expect(pass?.transportPages).toBe(1);
    });

    it("ignores lines without the marker", () => {
        expect(parseRustPassLine("[eidnara] something else entirely")).toBeNull();
    });
});

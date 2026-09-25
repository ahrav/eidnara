import { describe, expect, it } from "bun:test";
import {
    MAX_PREPARE_LEAD,
    PREPARE_LEAD_ENV,
    parseSweepArm,
    prepareLeadEnv,
} from "./compaction-timing-scenario";

describe("prepareLeadEnv", () => {
    it("pins the variable empty for the default arm, so an exported override cannot reach the baseline", () => {
        expect(prepareLeadEnv(undefined)).toEqual({ [PREPARE_LEAD_ENV]: "" });
    });

    it("passes an in-range lead through, including both bounds", () => {
        expect(prepareLeadEnv(0)).toEqual({ [PREPARE_LEAD_ENV]: "0" });
        expect(prepareLeadEnv(10)).toEqual({ [PREPARE_LEAD_ENV]: "10" });
        expect(prepareLeadEnv(MAX_PREPARE_LEAD)).toEqual({
            [PREPARE_LEAD_ENV]: String(MAX_PREPARE_LEAD),
        });
    });

    it("rejects a lead the daemon would clamp or ignore", () => {
        for (const lead of [-1, MAX_PREPARE_LEAD + 1, 2.5, Number.NaN]) {
            expect(() => prepareLeadEnv(lead)).toThrow(/lead/);
        }
    });
});

describe("parseSweepArm", () => {
    it("reads unset and `default` as the baseline arm", () => {
        expect(parseSweepArm(undefined)).toBeUndefined();
        expect(parseSweepArm("default")).toBeUndefined();
    });

    it("reads an in-range integer as that lead", () => {
        expect(parseSweepArm("0")).toBe(0);
        expect(parseSweepArm("2")).toBe(2);
        expect(parseSweepArm(String(MAX_PREPARE_LEAD))).toBe(MAX_PREPARE_LEAD);
    });

    it("rejects a lead outside the daemon's range instead of reporting one the daemon never ran", () => {
        for (const arm of ["-3", "21", "25"]) {
            expect(() => parseSweepArm(arm)).toThrow(/EIDNARA_E2E_TIMING_ARM/);
        }
    });

    it("rejects text that is not an integer, including an empty value", () => {
        for (const arm of ["", " ", "2.5", "lead-2"]) {
            expect(() => parseSweepArm(arm)).toThrow(/EIDNARA_E2E_TIMING_ARM/);
        }
    });
});

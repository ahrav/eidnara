import { describe, expect, it } from "bun:test";
import { normalizeSDKResponse } from "./normalize-sdk-response";

const FB = Symbol("fallback");
const normalize = (response: unknown, options?: { preferResponseOnMissingData?: boolean }) =>
    normalizeSDKResponse<unknown>(response, FB, options);

describe("normalizeSDKResponse", () => {
    it("returns fallback for null and undefined", () => {
        expect(normalize(null)).toBe(FB);
        expect(normalize(undefined)).toBe(FB);
    });

    it("passes arrays through untouched", () => {
        const arr = [1, 2];
        expect(normalize(arr)).toBe(arr);
    });

    it("unwraps the SDK envelope", () => {
        const data = { id: "sess_1" };
        expect(normalize({ data, error: undefined })).toBe(data);
        expect(normalize({ data: 0 })).toBe(0);
        expect(normalize({ data: "" })).toBe("");
    });

    it("returns fallback for an envelope whose data is null or undefined", () => {
        expect(normalize({ data: null, error: { code: 500 } })).toBe(FB);
        expect(normalize({ data: undefined })).toBe(FB);
    });

    it("returns fallback for non-enveloped objects and primitives by default", () => {
        expect(normalize({ id: "sess_1" })).toBe(FB);
        expect(normalize(true)).toBe(FB);
        expect(normalize("raw")).toBe(FB);
        expect(normalize(42)).toBe(FB);
    });

    it("returns the response itself when preferResponseOnMissingData is set", () => {
        const opts = { preferResponseOnMissingData: true };
        const bare = { id: "sess_1" };
        expect(normalize(bare, opts)).toBe(bare);
        const envelopeWithoutData = { data: undefined, error: undefined };
        expect(normalize(envelopeWithoutData, opts)).toBe(envelopeWithoutData);
        expect(normalize(true, opts)).toBe(true);
    });

    it("returns fallback when the envelope check or data read throws", () => {
        const hasTrap = new Proxy(
            {},
            {
                has() {
                    throw new Error("has trap");
                },
            },
        );
        expect(normalize(hasTrap)).toBe(FB);
        expect(
            normalize({
                get data(): never {
                    throw new Error("data getter");
                },
            }),
        ).toBe(FB);
    });
});

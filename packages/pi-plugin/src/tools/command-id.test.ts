import { describe, expect, it } from "bun:test";

import { boundedCommandId, COMMAND_ID_MAX_BYTES } from "./command-id";

describe("boundedCommandId", () => {
    it("returns an id at the byte limit unchanged", () => {
        const id = "x".repeat(COMMAND_ID_MAX_BYTES);
        expect(boundedCommandId(id)).toBe(id);
    });

    it("hashes an id one byte over the limit", () => {
        const id = "x".repeat(COMMAND_ID_MAX_BYTES + 1);
        const bounded = boundedCommandId(id);
        expect(bounded).toMatch(/^pi-[0-9a-f]{64}$/);
        expect(Buffer.byteLength(bounded)).toBeLessThanOrEqual(COMMAND_ID_MAX_BYTES);
    });

    it("measures UTF-8 bytes, not characters", () => {
        // 64 four-byte code points are 64 characters but 256 bytes.
        const id = "\u{1F600}".repeat(64);
        expect(id.length).toBeGreaterThan(64);
        expect(boundedCommandId(id)).toMatch(/^pi-[0-9a-f]{64}$/);
    });

    it("maps equal oversized ids to equal bounded ids and distinct ids apart", () => {
        const a = "a".repeat(200);
        const b = `${"a".repeat(199)}b`;
        expect(boundedCommandId(a)).toBe(boundedCommandId(a));
        expect(boundedCommandId(a)).not.toBe(boundedCommandId(b));
    });
});

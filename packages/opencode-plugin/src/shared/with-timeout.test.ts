import { describe, expect, it } from "bun:test";

import { withTimeout } from "./with-timeout";

describe("withTimeout", () => {
    it("returns the settled value and clears the timer", async () => {
        const value = await withTimeout(Promise.resolve(7), 1_000, "unused");
        expect(value).toBe(7);
    });

    it("propagates the wrapped rejection", async () => {
        await expect(
            withTimeout(Promise.reject(new Error("inner")), 1_000, "unused"),
        ).rejects.toThrow("inner");
    });

    it("rejects with the given message when the promise does not settle in time", async () => {
        const pending = new Promise<never>(() => {});
        await expect(withTimeout(pending, 10, "read timed out")).rejects.toThrow("read timed out");
    });
});

import { describe, expect, test } from "bun:test";
import { escalationBands } from "./escalation-bands";

describe("escalationBands", () => {
    test.each([
        [65, 85],
        [80, 85],
        [88, 90],
        [90, 92],
    ])("keeps the force band ordered for threshold %i", (threshold, expectedForce) => {
        const bands = escalationBands(threshold);
        expect(bands.forceMaterializationPercentage).toBe(expectedForce);
        expect(threshold).toBeLessThan(bands.forceMaterializationPercentage);
        expect(bands.forceMaterializationPercentage).toBeGreaterThanOrEqual(85);
        expect(bands.forceMaterializationPercentage).toBeLessThan(95);
        expect(bands.emergencyPercentage).toBe(95);
    });
});

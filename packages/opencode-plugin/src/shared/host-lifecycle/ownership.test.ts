import { describe, expect, test } from "bun:test";
import { mayDemandStart, resolveConnectionOrigin } from "./ownership";

describe("connection-origin provenance (U3 scenario 2)", () => {
    test("each configuration resolves its origin, and only managed-default may demand-start", () => {
        const canonical = "/home/user/.local/share/eidnara/run/connection.json";
        for (const [config, origin, demandStart] of [
            [{}, "managed-default", true],
            // The canonical text is still an explicit choice.
            [{ connectionFile: canonical }, "explicit", false],
            [{ injected: true }, "injected", false],
            // Injection wins over a simultaneously supplied path.
            [{ connectionFile: "/x", injected: true }, "injected", false],
        ] as const) {
            const resolved = resolveConnectionOrigin(config);
            expect(resolved).toBe(origin);
            expect(mayDemandStart(resolved)).toBe(demandStart);
        }
    });
});

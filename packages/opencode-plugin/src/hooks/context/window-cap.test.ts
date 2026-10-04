import { describe, expect, it } from "bun:test";

import cap from "./__fixtures__/window-cap.json";
import { inspectReferenceableMessages } from "./transform-capture";
import { openCodeMessageSize } from "./window-cap";

describe("OpenCode window-cap size", () => {
    it("counts the daemon's blocks and bounds its bytes for every parity message", () => {
        for (const [index, parity] of cap.parity.opencode.entries()) {
            const inspection = inspectReferenceableMessages([parity.message]);
            if (!inspection.ok) throw new Error(`parity message ${index} is not inspectable`);
            const size = openCodeMessageSize(parity.message, inspection.messageWireBytes[0] ?? 0);
            expect({ index, blocks: size.blocks }).toEqual({ index, blocks: parity.blocks });
            expect(size.bytes).toBeGreaterThanOrEqual(parity.bytes);
        }
    });
});

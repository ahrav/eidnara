import { describe, expect, it } from "bun:test";

import cap from "./__fixtures__/window-cap.json";
import { encodeOpenCodeMessagesToCk } from "./module-wire";
import { copyWindow, inspectReferenceableMessages } from "./transform-capture";
import { openCodeMessageSize, openCodeSlotSizes, type TransformWindowSize } from "./window-cap";

function sizeAlone(host: readonly unknown[], index: number): TransformWindowSize | undefined {
    const message = copyWindow(host, index, index + 1);
    const inspection = message && inspectReferenceableMessages(message);
    if (!message || !inspection?.ok) return undefined;
    return openCodeMessageSize(message[0], inspection.messageUtf8Bytes[0] ?? 0);
}

/** Parity messages repeated across several measured runs, each with escaped and multi-byte text. */
function mixedHost(count: number): unknown[] {
    return Array.from({ length: count }, (_, index) => {
        const parity = cap.parity.opencode[index % cap.parity.opencode.length];
        const message = structuredClone(parity?.message) as { parts: unknown[] };
        message.parts.push({
            id: `p${index}`,
            type: "text",
            text: `"${index}"\n\u0001 \u00e9 ${"x".repeat(index)}`,
        });
        return message;
    });
}

describe("OpenCode window-cap size", () => {
    it("counts the daemon's blocks and bounds its bytes for every parity message", () => {
        for (const [index, parity] of cap.parity.opencode.entries()) {
            expect(encodeOpenCodeMessagesToCk([parity.message])[0]).toEqual(parity.ingress);
            const inspection = inspectReferenceableMessages([parity.message]);
            if (!inspection.ok) throw new Error(`parity message ${index} is not inspectable`);
            const size = openCodeMessageSize(parity.message, inspection.messageUtf8Bytes[0] ?? 0);
            expect({ index, blocks: size.blocks }).toEqual({ index, blocks: parity.blocks });
            expect(size.bytes).toBeGreaterThanOrEqual(parity.bytes);
        }
    });

    it("gives each slot of a newest-first walk the size its own inspection gives it", () => {
        const host = mixedHost(100);
        const sizeOf = openCodeSlotSizes(host);
        for (let index = host.length - 1; index >= 0; index -= 1)
            expect({ index, size: sizeOf(index) }).toEqual({ index, size: sizeAlone(host, index) });
    });

    it("leaves a refused slot unsized and sizes every other slot of its run", () => {
        const host = mixedHost(80);
        Object.defineProperty(host[60], "parts", { get: () => [], enumerable: true });
        host[50] = new Proxy({ parts: [] }, {});
        const sizeOf = openCodeSlotSizes(host);
        for (let index = host.length - 1; index >= 0; index -= 1) {
            const alone = sizeAlone(host, index);
            expect({ index, size: sizeOf(index) }).toEqual({ index, size: alone });
            expect(alone === undefined).toBe(index === 60 || index === 50);
        }
    });

    it.each([
        ["three-byte text", [{ id: "p1", type: "text", text: "\u6f22".repeat(100_000) }]],
        ["an ignored text part", [{ id: "p1", type: "text", text: "queued", ignored: true }]],
        [
            "a mix of ignored and kept text",
            [
                { id: "p1", type: "text", text: "kept \u20ac" },
                { id: "p2", type: "text", text: "\u4e2d".repeat(5_000), ignored: true },
            ],
        ],
    ])("bounds the encoded CK of %s", (_, parts) => {
        const message = { info: { id: "msg_1", role: "user", sessionID: "ses" }, parts };
        const size = openCodeSlotSizes([message])(0);
        const [encoded] = encodeOpenCodeMessagesToCk([message]);
        const blocks = (encoded?.ck.content ?? []) as unknown[];
        expect(size?.blocks).toBe(blocks.length);
        const utf8 = blocks.reduce<number>(
            (sum, block) => sum + Buffer.byteLength(JSON.stringify(block)),
            0,
        );
        expect(size?.bytes ?? 0).toBeGreaterThanOrEqual(utf8);
    });
});

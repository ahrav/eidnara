import cap from "./__fixtures__/window-cap.json";
import { encodeOpenCodeMessagesToCk } from "./module-wire";
import { copyWindow, inspectReferenceableMessages } from "./transform-capture";

/** Half the daemon's window cap in CK blocks: the most a cold import's suffix sends. */
export const HALF_CAP_BLOCKS: number = cap.half_cap_blocks;
/** Half the daemon's window cap in canonical CK block bytes. */
export const HALF_CAP_BYTES: number = cap.half_cap_bytes;
/**
 * Bytes a CK block's canonical text adds over its host source: the daemon's block identity stamp,
 * the CK kind envelope, and the framing Pi's codec puts around a summary or a bash execution.
 */
export const BLOCK_OVERHEAD_BYTES = 512;

/** A host message's size as the daemon counts it against the window cap. */
export interface TransformWindowSize {
    blocks: number;
    bytes: number;
}

/** Whether `size` is within half the window cap in both blocks and bytes. */
export function withinHalfCap(size: TransformWindowSize): boolean {
    return size.blocks <= HALF_CAP_BLOCKS && size.bytes <= HALF_CAP_BYTES;
}

/**
 * The CK blocks the daemon counts for one OpenCode message: the request carries the blocks
 * `encodeOpenCodeMessagesToCk` builds, and a synthetic message's blocks are not counted.
 */
function openCodeBlocks(message: unknown): number {
    const [encoded] = encodeOpenCodeMessagesToCk([message]);
    const ck = encoded?.ck as { content: unknown[]; meta: { synthetic: boolean } } | undefined;
    return !ck || ck.meta.synthetic ? 0 : ck.content.length;
}

/**
 * An OpenCode message's size against the cap: the daemon's block count, and its canonical bytes
 * bounded from above by the message's UTF-8 JSON bytes plus each block's overhead.
 */
export function openCodeMessageSize(message: unknown, utf8Bytes: number): TransformWindowSize {
    const blocks = openCodeBlocks(message);
    return { blocks, bytes: blocks === 0 ? 0 : utf8Bytes + blocks * BLOCK_OVERHEAD_BYTES };
}

const MEASURED_RUN_SLOTS = 32;

/**
 * The sizes it returns hold until the caller awaits, since the host array can change across an
 * await. A slot that its own inspection refuses, or throws on, does so when it is asked for.
 */
export function openCodeSlotSizes(
    host: readonly unknown[],
): (index: number) => TransformWindowSize | undefined {
    const sizes = new Map<number, TransformWindowSize>();
    let runs = true;
    const alone = (index: number): TransformWindowSize | undefined => {
        const message = copyWindow(host, index, index + 1);
        const inspection = message && inspectReferenceableMessages(message);
        if (!message || !inspection?.ok) return undefined;
        return openCodeMessageSize(message[0], inspection.messageUtf8Bytes[0] ?? 0);
    };
    return (index) => {
        const known = sizes.get(index);
        if (known || !runs) return known ?? alone(index);
        const start = Math.max(0, index + 1 - MEASURED_RUN_SLOTS);
        const run = copyWindow(host, start, index + 1);
        let inspection: ReturnType<typeof inspectReferenceableMessages> | undefined;
        try {
            inspection = run && inspectReferenceableMessages(run);
        } catch {
            inspection = undefined;
        }
        if (!run || !inspection?.ok) {
            runs = false;
            return alone(index);
        }
        for (let slot = start; slot <= index; slot += 1)
            sizes.set(
                slot,
                openCodeMessageSize(
                    run[slot - start],
                    inspection.messageUtf8Bytes[slot - start] ?? 0,
                ),
            );
        return sizes.get(index);
    };
}

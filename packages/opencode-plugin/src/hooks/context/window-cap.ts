import { isRecord } from "../../shared/record-type-guard";
import cap from "./__fixtures__/window-cap.json";
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

/** Part types the daemon's OpenCode codec decodes to no block. */
const OPENCODE_BLOCKLESS_PARTS = new Set(["compaction", "snapshot", "patch", "agent", "retry"]);

function isSyntheticPart(part: unknown): boolean {
    return isRecord(part) && (part.synthetic === true || part.syntheticTodoMarker === true);
}

/** The CK blocks `decode_opencode_shared` gives one OpenCode message: a part is one block, a finished tool part two. */
function openCodeBlocks(message: unknown): number {
    const parts = isRecord(message) && Array.isArray(message.parts) ? message.parts : [];
    if (parts.length > 0 && parts.every(isSyntheticPart)) return 0;
    let blocks = 0;
    for (const part of parts) {
        if (!isRecord(part) || typeof part.type !== "string") {
            blocks += 1;
        } else if (part.type === "text") {
            blocks += part.ignored === true ? 0 : 1;
        } else if (part.type === "tool") {
            const state = isRecord(part.state) ? part.state : undefined;
            const status = typeof state?.status === "string" ? state.status : part.status;
            blocks += status === "completed" || status === "error" ? 2 : 1;
        } else if (!OPENCODE_BLOCKLESS_PARTS.has(part.type)) {
            blocks += 1;
        }
    }
    return blocks;
}

/**
 * An OpenCode message's size against the cap: the daemon's block count exactly, and its canonical
 * bytes bounded from above by the message's wire bytes plus each block's overhead.
 */
export function openCodeMessageSize(message: unknown, wireBytes: number): TransformWindowSize {
    const blocks = openCodeBlocks(message);
    return { blocks, bytes: blocks === 0 ? 0 : wireBytes + blocks * BLOCK_OVERHEAD_BYTES };
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
        return openCodeMessageSize(message[0], inspection.messageWireBytes[0] ?? 0);
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
                    inspection.messageWireBytes[slot - start] ?? 0,
                ),
            );
        return sizes.get(index);
    };
}

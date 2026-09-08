import { createHash } from "node:crypto";

/** The daemon rejects a facade `command_id` longer than this many UTF-8 bytes. */
export const COMMAND_ID_MAX_BYTES = 128;

/** Hashing makes repeated oversized ids produce the same bounded id, so a retry keeps its identity. */
export function boundedCommandId(id: string): string {
    if (Buffer.byteLength(id) <= COMMAND_ID_MAX_BYTES) return id;
    return `pi-${createHash("sha256").update(id).digest("hex")}`;
}

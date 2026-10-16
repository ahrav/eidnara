import { parentPort, workerData } from "node:worker_threads";
import { NativeChannel, type NativeReceiveLease } from "../index.ts";

// Attaches a channel pair inside the worker's own environment and receives one lease. Mode
// `closed` releases the lease and closes both channels, `open` releases the lease and leaves
// both channels for the cleanup hook, and `kept` keeps the lease's alias reachable while the
// worker's environment is torn down.
function header(length: number): Uint8Array {
    const bytes = new Uint8Array(20);
    const view = new DataView(bytes.buffer);
    view.setUint32(0, length, true);
    view.setUint8(4, 3);
    view.setUint16(6, 1, true);
    view.setUint32(8, 1, true);
    view.setBigUint64(12, 1n, true);
    return bytes;
}

const pair = NativeChannel.createTestPair();
pair.first.produce(header(5), 5, (cursor) => {
    cursor.view().set([1, 2, 3, 4, 5]);
    cursor.advance(5);
});
let lease: NativeReceiveLease | undefined;
pair.second.drainOne((value) => {
    lease = value;
});
const alias = lease?.segment(0);
const aliasBytes = alias?.byteLength ?? -1;
const mode = (workerData as { mode: "closed" | "open" | "kept" }).mode;
if (mode !== "kept") lease?.release();
if (mode === "closed") {
    pair.first.close();
    pair.second.close();
}
(globalThis as { heldAlias?: Uint8Array }).heldAlias = alias;
parentPort?.postMessage({ aliasBytes });

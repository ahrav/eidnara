import { type KernelClient, type KernelMemorySnapshot, kernelMemorySnapshotFrom } from "./client";
import { leavesCanonicalRows, unavailable } from "./state";

/** When the gate preserves canonical rows, the snapshot pairs available ungated data with the gated state, and one daemon connection answers both reads: a connection that changed between them makes the snapshot `snapshot_diverged`, and the caller's next read asks the live connection. An unavailable ungated read supplies its own state. */
export async function readSearchMemorySnapshot(
    client: KernelClient,
    signal?: AbortSignal,
): Promise<KernelMemorySnapshot> {
    const options = signal ? { signal } : {};
    const connection = client.connectionIdentity();
    const gated = await client.read({ surface: "explicit_search", gated: true, ...options });
    if (!leavesCanonicalRows(gated.state)) return kernelMemorySnapshotFrom(gated);
    const canonical = kernelMemorySnapshotFrom(
        await client.read({ surface: "explicit_search", gated: false, ...options }),
    );
    if (canonical.state.kind !== "available") return canonical;
    if (client.connectionIdentity() !== connection) {
        return { state: unavailable("snapshot_diverged"), rows: [], knownAsOf: null };
    }
    return { ...canonical, state: gated.state };
}

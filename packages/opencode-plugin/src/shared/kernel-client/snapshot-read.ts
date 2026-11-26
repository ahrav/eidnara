import { type KernelClient, type KernelMemorySnapshot, kernelMemorySnapshotFrom } from "./client";
import { leavesCanonicalRows } from "./state";

/** When the gate preserves canonical rows, the snapshot pairs available ungated data with the gated state. An unavailable ungated read supplies its own state. */
export async function readSearchMemorySnapshot(
    client: KernelClient,
    signal?: AbortSignal,
): Promise<KernelMemorySnapshot> {
    const options = signal ? { signal } : {};
    const gated = await client.read({ surface: "explicit_search", gated: true, ...options });
    if (!leavesCanonicalRows(gated.state)) return kernelMemorySnapshotFrom(gated);
    const canonical = kernelMemorySnapshotFrom(
        await client.read({ surface: "explicit_search", gated: false, ...options }),
    );
    return canonical.state.kind === "available" ? { ...canonical, state: gated.state } : canonical;
}

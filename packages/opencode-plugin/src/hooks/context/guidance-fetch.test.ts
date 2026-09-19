import { describe, expect, it } from "bun:test";
import { createGuidanceFetcher } from "./guidance-fetch";
import type { RustModeModuleClient } from "./rust-mode-transform";

function fetcher(respond: (call: { method: string; body: unknown }) => unknown) {
    const calls: { method: string; body: unknown; signal?: AbortSignal }[] = [];
    const client: RustModeModuleClient = {
        call: async ({ method, body, signal }) => {
            const call = { method, body, signal };
            calls.push(call);
            return respond(call);
        },
        deleteSession: async () => {},
        closeSession: () => {},
    };
    const fetchGuidance = createGuidanceFetcher({
        moduleClient: client,
        projectRootForSession: async () => "/project",
        promptSurfaceRuntime: undefined,
        promptSurface: undefined,
        language: undefined,
    });
    return { fetchGuidance, calls };
}

const args = (isCacheBusting: boolean) => ({
    sessionId: "ses",
    toolPresent: true,
    modelKey: "provider/model",
    isCacheBusting,
});

describe("guidance fetcher cache", () => {
    it("serves cached bytes on a plain pass and refetches on a cache-busting pass", async () => {
        let bytes = "## Eidnara\n\nv1";
        const { fetchGuidance, calls } = fetcher(() => ({ bytes }));
        expect(await fetchGuidance(args(false))).toBe("## Eidnara\n\nv1");
        expect(await fetchGuidance(args(false))).toBe("## Eidnara\n\nv1");
        expect(calls).toHaveLength(1);
        bytes = "## Eidnara\n\nv2";
        expect(await fetchGuidance(args(true))).toBe("## Eidnara\n\nv2");
        expect(calls).toHaveLength(2);
        expect(calls[1]?.signal).toBeInstanceOf(AbortSignal);
    });

    it("keeps the cached block when a cache-busting refetch fails", async () => {
        let fail = false;
        const { fetchGuidance, calls } = fetcher(() => {
            if (fail) throw new Error("daemon unavailable");
            return { bytes: "## Eidnara\n\nv1" };
        });
        expect(await fetchGuidance(args(false))).toBe("## Eidnara\n\nv1");
        fail = true;
        // The prompt keeps the block it already carried, so the hash does not flip.
        expect(await fetchGuidance(args(true))).toBe("## Eidnara\n\nv1");
        expect(calls).toHaveLength(2);
        // A different variant has no cached bytes to fall back on: the failure surfaces.
        await expect(fetchGuidance({ ...args(false), toolPresent: false })).rejects.toThrow(
            "daemon unavailable",
        );
    });

    it("surfaces the first failure when nothing is cached", async () => {
        const { fetchGuidance } = fetcher(() => {
            throw new Error("daemon unavailable");
        });
        await expect(fetchGuidance(args(false))).rejects.toThrow("daemon unavailable");
        const { fetchGuidance: noBytes } = fetcher(() => ({}));
        await expect(noBytes(args(false))).rejects.toThrow("guidance.get returned no bytes");
    });

    it("forgets a session on clear", async () => {
        const { fetchGuidance, calls, clear } = (() => {
            const built = fetcher(() => ({ bytes: "## Eidnara\n\nv1" }));
            return { ...built, clear: built.fetchGuidance.clearSession };
        })();
        await fetchGuidance(args(false));
        clear("ses");
        await fetchGuidance(args(false));
        expect(calls).toHaveLength(2);
    });
});

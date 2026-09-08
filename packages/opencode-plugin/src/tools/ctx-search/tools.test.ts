import { describe, expect, it } from "bun:test";
import { KernelClient, MAX_READ_OBJECT_IDS } from "../../shared/kernel-client";
import { renderAntiMemoryContent } from "../../shared/kernel-client/anti-memory";
import { FakeKernel, FakeKernelTransport } from "../../shared/kernel-client-testing/fake-kernel";
import { estimateTokens } from "../../shared/token-estimator";
import type { KernelClientResolver } from "../ctx-memory/types";
import { MAX_RENDERED_RESULT_TOKENS } from "./bounds";
import { CTX_SEARCH_LIGHT_DESCRIPTION, createCtxSearchTools, executeCtxSearch } from "./tools";
import type { CtxSearchToolDeps } from "./types";

const toolContext = (sessionID = "ses-search") =>
    ({ sessionID, directory: "/tmp/ctx-search" }) as never;

const OBJECT_A = `mem_${"a".repeat(32)}`;
const OBJECT_B = `mem_${"b".repeat(32)}`;

/** A kernel client over an in-memory fake; `transport.calls` records every daemon round trip. */
function kernelHarness(
    kernel = new FakeKernel(),
    enabled = true,
): {
    kernel: FakeKernel;
    transport: FakeKernelTransport;
    kernelClient: KernelClientResolver;
    deps: CtxSearchToolDeps;
} {
    const transport = new FakeKernelTransport(kernel);
    const kernelClient: KernelClientResolver = ({ sessionId, projectRoot }) =>
        new KernelClient({ transport, enabled, sessionId, projectRoot });
    return {
        kernel,
        transport,
        kernelClient,
        deps: { kernelClient, resolveProjectPath: () => "git:repo-project" },
    };
}

function seed(kernel: FakeKernel, object_id: string, summary: string, kind = "ARCHITECTURE"): void {
    kernel.seedDecision({ object_id, decision_kind: kind, summary });
}

/** Seeds `count` memories whose summaries all contain `text`; ids sort in seed order. */
function seedMany(kernel: FakeKernel, count: number, text: (index: number) => string): void {
    for (let index = 0; index < count; index += 1) {
        kernel.seedDecision({
            object_id: `mem_${String(index).padStart(32, "0")}`,
            decision_kind: "ARCHITECTURE",
            summary: text(index),
        });
    }
}

describe("createCtxSearchTools", () => {
    it("validates required query", async () => {
        const tools = createCtxSearchTools(kernelHarness().deps);

        const result = await tools.ctx_search.execute({ query: "   " }, toolContext());

        expect(result).toBe("Error: 'query' is required.");
    });

    it("rejects an over-cap query with the native string error before any work", async () => {
        const harness = kernelHarness();
        const resolveCalls: string[] = [];
        const tools = createCtxSearchTools({
            kernelClient: harness.kernelClient,
            resolveProjectPath: (directory) => {
                resolveCalls.push(directory);
                return "git:repo-project";
            },
        });
        const byteResult = await tools.ctx_search.execute(
            { query: "a".repeat(16 * 1024 + 1) },
            toolContext(),
        );
        expect(byteResult).toStartWith("Error: query is too large:");

        const atomResult = await tools.ctx_search.execute(
            { query: Array.from({ length: 65 }, (_, index) => `a${index}`).join(" ") },
            toolContext(),
        );
        expect(atomResult).toStartWith("Error: query is too complex:");

        expect(harness.transport.calls).toHaveLength(0);
        expect(resolveCalls).toHaveLength(0);
    });

    it("clamps an over-cap limit to 50 instead of rejecting", async () => {
        const harness = kernelHarness();
        seedMany(harness.kernel, 60, (index) => `Clamped probe ${index}.`);
        const tools = createCtxSearchTools(harness.deps);
        const result = await tools.ctx_search.execute(
            { query: "clamped probe", limit: 10_000 },
            toolContext(),
        );
        expect(result).toStartWith('Found 50 results for "clamped probe":');
    });

    it("preserves an explicit empty sources list as no sources", async () => {
        const harness = kernelHarness();
        seed(harness.kernel, OBJECT_A, "This memory would appear.");
        const tools = createCtxSearchTools(harness.deps);

        const result = await tools.ctx_search.execute(
            { query: "appear", sources: [] },
            toolContext(),
        );

        expect(result).toContain("No results found");
        expect(harness.transport.calls).toHaveLength(0);
    });

    it("rejects a non-array or unknown sources value instead of searching nothing", async () => {
        const tools = createCtxSearchTools(kernelHarness().deps);

        const nonArray = await tools.ctx_search.execute(
            { query: "appear", sources: 42 as unknown as string[] },
            toolContext(),
        );
        expect(nonArray).toContain("Error: 'sources' must be an array");

        // A misspelled source silently dropped would run an empty-source search and report a misleading "No results found". commentlint: allow(JUDGE)
        const misspelled = await tools.ctx_search.execute(
            { query: "appear", sources: ["memor"] as unknown as ["memory"] },
            toolContext(),
        );
        expect(misspelled).toContain('Error: unknown source: "memor"');
        expect(misspelled).toContain("Supported sources: memory.");
        expect(misspelled).not.toContain("No results found");
    });

    it('treats sources: ["memory"] like an omitted sources argument', async () => {
        const harness = kernelHarness();
        seed(harness.kernel, OBJECT_A, "The cache must stay offline.", "CONSTRAINTS");
        const tools = createCtxSearchTools(harness.deps);
        const explicit = await tools.ctx_search.execute(
            { query: "offline cache", sources: ["memory"] },
            toolContext(),
        );
        const implicit = await tools.ctx_search.execute({ query: "offline cache" }, toolContext());
        expect(explicit).toBe(implicit);
        expect(explicit).toContain(`id=${OBJECT_A}`);
    });

    it("ranks text queries over memory summaries served by the daemon", async () => {
        const harness = kernelHarness();
        seed(harness.kernel, OBJECT_A, "The cache must stay offline.", "CONSTRAINTS");
        seed(harness.kernel, OBJECT_B, "Handlers end in Handler.", "NAMING");
        const tools = createCtxSearchTools(harness.deps);
        const result = await tools.ctx_search.execute({ query: "offline cache" }, toolContext());
        expect(result).toContain("[1] [memory]");
        expect(result).toContain(`id=${OBJECT_A}`);
        expect(result).toContain("category=CONSTRAINTS");
        expect(result).not.toContain(`id=${OBJECT_B}`);
        const read = harness.transport.calls[0]?.body as { surface: string; gated: boolean };
        expect(read).toMatchObject({ surface: "explicit_search", gated: true });
    });

    it("renders a rejected-approach memory as an anti-memory warning", async () => {
        const harness = kernelHarness();
        harness.kernel.seedDecision({
            object_id: OBJECT_A,
            decision_kind: "REJECTED_APPROACH",
            summary: renderAntiMemoryContent({
                trigger: "Choosing a cache backend",
                rejectedStrategy: "Use Redis",
                rejectionReason: "The project must work offline",
                saferAlternative: "Use the embedded store",
            }),
            rationale: "Redis needs a network hop.",
        });
        const tools = createCtxSearchTools(harness.deps);
        const result = await tools.ctx_search.execute({ query: "redis cache" }, toolContext());
        expect(result).toContain("[1] [anti-memory warning]");
        expect(result).toContain("⚠ Previously rejected: Use Redis.");
        expect(result).toContain("Safer alternative: Use the embedded store.");
        expect(result).toContain(`(see ${OBJECT_A})`);
        expect(result).toContain("Rationale: Redis needs a network hop.");
    });

    it("notes a truncated memory read so partial hits do not read as a complete search", async () => {
        const harness = kernelHarness();
        seed(harness.kernel, OBJECT_A, "Alpha memory truncation probe.");
        harness.kernel.readTruncated = true;
        const tools = createCtxSearchTools(harness.deps);

        const result = await tools.ctx_search.execute(
            { query: "truncation probe", sources: ["memory"] },
            toolContext(),
        );

        expect(result).toContain("the memory read was truncated");
        expect(result).toContain(`id=${OBJECT_A}`);
    });

    it("resolves an object-id query through the filtered read", async () => {
        const harness = kernelHarness();
        seed(harness.kernel, OBJECT_A, "Direct id hit.");
        const tools = createCtxSearchTools(harness.deps);
        const result = await tools.ctx_search.execute({ query: OBJECT_A }, toolContext());
        expect(result).toContain("[1] [memory]");
        expect(result).toContain(`id=${OBJECT_A}`);
        expect(result).toContain("Direct id hit.");
        expect(result).toContain("match=exact");
        const read = harness.transport.calls[0]?.body as {
            surface: string;
            gated: boolean;
            object_ids?: string[];
        };
        expect(read).toMatchObject({ surface: "explicit_search", gated: true });
        expect(read.object_ids).toEqual([OBJECT_A]);
    });

    it("resolves an object-id query whose filtered read is byte-truncated through chunked reads", async () => {
        const harness = kernelHarness();
        seed(harness.kernel, OBJECT_A, "Oldest row, dropped by the byte budget.");
        seed(harness.kernel, OBJECT_B, "Newest row.");
        harness.kernel.filteredReadRowCap = 1;
        const tools = createCtxSearchTools(harness.deps);
        const result = await tools.ctx_search.execute(
            { query: `${OBJECT_A} ${OBJECT_B}` },
            toolContext(),
        );
        expect(result).toContain(`id=${OBJECT_A}`);
        expect(result).toContain("Oldest row, dropped by the byte budget.");
        expect(result).toContain(`id=${OBJECT_B}`);
        expect(result).not.toContain("unresolved");
        const idReads = harness.transport.calls.map(
            (call) => (call.body as { object_ids?: string[] }).object_ids,
        );
        expect(idReads[0]).toEqual([OBJECT_A, OBJECT_B]);
        expect(idReads.slice(1).sort()).toEqual([[OBJECT_A], [OBJECT_B]]);
    });

    it("names an object id whose single-row read stays truncated instead of dropping it silently", async () => {
        const harness = kernelHarness();
        harness.kernel.readTruncated = true;
        const tools = createCtxSearchTools(harness.deps);
        const result = await tools.ctx_search.execute({ query: OBJECT_A }, toolContext());
        expect(result).toStartWith(
            `Memory: unresolved object id (the daemon read stayed truncated): ${OBJECT_A}`,
        );
    });

    it("keeps a comma-joined id list over the filter bound on filtered reads so an old id beyond the row cap resolves", async () => {
        const harness = kernelHarness();
        const count = MAX_READ_OBJECT_IDS + 1;
        seedMany(harness.kernel, count, (index) => `Row ${index}.`);
        // The unfiltered snapshot would drop the oldest row; every named id must still resolve.
        harness.kernel.readRowCap = MAX_READ_OBJECT_IDS;
        const ids = Array.from(
            { length: count },
            (_, index) => `mem_${String(index).padStart(32, "0")}`,
        );
        const execution = await executeCtxSearch(
            harness.deps,
            { query: ids.join(","), limit: 50 },
            toolContext(),
        );
        expect(execution.status).toBe("complete");
        if (execution.status !== "complete") return;
        expect(execution.prePack.map((hit) => hit.publicClaimId)).toEqual(ids.slice(0, 50));
        expect(execution.text).not.toContain("unresolved");
        expect(execution.text).not.toContain("truncated");
        const idReads = harness.transport.calls.map(
            (call) => (call.body as { object_ids?: string[] }).object_ids ?? [],
        );
        expect(idReads.map((read) => read.length).sort((a, b) => a - b)).toEqual([
            1,
            MAX_READ_OBJECT_IDS,
        ]);
        expect(idReads.flat().sort()).toEqual([...ids].sort());
    });

    it("honors the requested limit for a multi-id query", async () => {
        const harness = kernelHarness();
        seed(harness.kernel, OBJECT_A, "First.");
        seed(harness.kernel, OBJECT_B, "Second.");
        const tools = createCtxSearchTools(harness.deps);
        const result = await tools.ctx_search.execute(
            { query: `${OBJECT_A} ${OBJECT_B}`, limit: 1 },
            toolContext(),
        );
        expect(result).toContain("[1] [memory]");
        expect(result).not.toContain("[2] [memory]");
    });

    it("renders the state text when the memory source is not available", async () => {
        const harness = kernelHarness();
        harness.kernel.surfaceStates.set("explicit_search", {
            kind: "stale",
            lag_positions: 4,
            oldest_unconsumed_age_ms: 20,
        });
        const tools = createCtxSearchTools(harness.deps);
        const memoryOnly = await tools.ctx_search.execute(
            { query: "anything", sources: ["memory"] },
            toolContext(),
        );
        expect(memoryOnly).toBe(
            "Error: Memory results may lag recent changes; the projector has not caught up.",
        );
        const implicit = await tools.ctx_search.execute({ query: "anything" }, toolContext());
        expect(implicit).toBe(memoryOnly);
    });

    it("renders the disabled state from a disabled kernel client", async () => {
        const harness = kernelHarness(new FakeKernel(), false);
        const tools = createCtxSearchTools(harness.deps);
        const result = await tools.ctx_search.execute({ query: "anything" }, toolContext());
        expect(result).toBe("Error: Memory is disabled by configuration (memory.enabled = false).");
        expect(harness.transport.calls).toHaveLength(0);
    });

    it("answers unavailable without a daemon", async () => {
        const harness = kernelHarness();
        harness.transport.fileExists = false;
        const tools = createCtxSearchTools(harness.deps);
        const result = await tools.ctx_search.execute(
            { query: "anything", sources: ["memory"] },
            toolContext(),
        );
        expect(result).toBe("Error: Memory is unavailable because the daemon is not running.");
        expect(result.toLowerCase()).not.toContain("retry");
        expect(harness.transport.calls).toHaveLength(0);
    });

    it("advertises only the memory source in both the full and light descriptions", () => {
        const tools = createCtxSearchTools(kernelHarness().deps);
        for (const description of [tools.ctx_search.description, CTX_SEARCH_LIGHT_DESCRIPTION]) {
            expect(description).toContain("memory daemon");
            expect(description).toContain("mem_<32hex>");
            for (const absent of ["ctx_expand", "compacted", "commits", "notes"]) {
                expect(description).not.toContain(absent);
            }
        }
    });
});

describe("executeCtxSearch", () => {
    it("returns invalid outcomes with the same error text the tool returns", async () => {
        const { deps } = kernelHarness();
        const tools = createCtxSearchTools(deps);
        const execution = await executeCtxSearch(deps, { query: "   " }, toolContext());
        expect(execution.status).toBe("invalid");
        expect(execution.text).toBe(
            String(await tools.ctx_search.execute({ query: "   " }, toolContext())),
        );
    });

    it("keeps direct object-id lookup byte-identical between the tool and the structured helper", async () => {
        const harness = kernelHarness();
        seed(harness.kernel, OBJECT_A, "Direct id hit for the structured helper.");
        const tools = createCtxSearchTools(harness.deps);
        const args = { query: OBJECT_A };
        const execution = await executeCtxSearch(harness.deps, args, toolContext());
        expect(execution.status).toBe("complete");
        if (execution.status !== "complete") return;
        expect(execution.text).toBe(String(await tools.ctx_search.execute(args, toolContext())));
        expect(execution.reason).toBe("delivered");
        expect(execution.prePack).toHaveLength(1);
        expect(execution.delivered).toEqual(execution.prePack);
        expect(execution.omittedCount).toBe(0);
        expect(execution.text).toContain(`id=${OBJECT_A}`);
    });

    it("keeps a packing-omitted result in prePack but out of delivered", async () => {
        const harness = kernelHarness();
        const filler = Array.from({ length: 300 }, (_, index) =>
            ((index * 2654435761) % 36).toString(36),
        ).join(" ");
        seedMany(harness.kernel, 50, (index) => `${filler} tail-${index} big`);
        const execution = await executeCtxSearch(
            harness.deps,
            { query: "big", limit: 50 },
            toolContext(),
        );
        expect(execution.status).toBe("complete");
        if (execution.status !== "complete") return;
        expect(execution.reason).toBe("delivered");
        expect(execution.prePack).toHaveLength(50);
        expect(execution.delivered.length).toBeLessThan(50);
        expect(execution.delivered).toEqual(execution.prePack.slice(0, execution.delivered.length));
        expect(execution.omittedCount).toBe(50 - execution.delivered.length);
        const omitted = execution.prePack[execution.prePack.length - 1];
        expect(execution.delivered).not.toContain(omitted);
        expect(execution.text).not.toContain(`id=${omitted?.publicClaimId}`);
    });

    it("returns an empty-results completed delivery, not a failure, for zero results", async () => {
        const execution = await executeCtxSearch(
            kernelHarness().deps,
            { query: "missing" },
            toolContext(),
        );
        expect(execution.status).toBe("complete");
        if (execution.status !== "complete") return;
        expect(execution.reason).toBe("empty-results");
        expect(execution.prePack).toEqual([]);
        expect(execution.delivered).toEqual([]);
        expect(execution.text).toContain("No results found");
    });

    it("counts the truncation note in tokenCount and keeps the whole text under the budget", async () => {
        const harness = kernelHarness();
        const filler = Array.from({ length: 300 }, (_, index) =>
            ((index * 2654435761) % 36).toString(36),
        ).join(" ");
        seedMany(harness.kernel, 50, (index) => `${filler} tail-${index} big`);
        harness.kernel.readTruncated = true;
        const execution = await executeCtxSearch(
            harness.deps,
            { query: "big", limit: 50 },
            toolContext(),
        );
        expect(execution.status).toBe("complete");
        if (execution.status !== "complete") return;
        expect(execution.text).toStartWith("Memory: the memory read was truncated");
        expect(execution.tokenCount).toBe(estimateTokens(execution.text));
        expect(execution.tokenCount).toBeLessThanOrEqual(MAX_RENDERED_RESULT_TOKENS);
    });
});

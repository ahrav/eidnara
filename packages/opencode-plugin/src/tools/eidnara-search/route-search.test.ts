import { describe, expect, it } from "bun:test";
import { KernelClient } from "../../shared/kernel-client";
import { renderAntiMemoryContent } from "../../shared/kernel-client/anti-memory";
import { FakeKernel, FakeKernelTransport } from "../../shared/kernel-client-testing/fake-kernel";
import type { KernelClientResolver } from "../eidnara-memory/types";
import { executeEidnaraSearch } from "./execute";
import { ROUTE_MAX_SELECTORS, ROUTE_SEARCH_DEADLINE_MS, searchThroughRoute } from "./route-search";

const ctx = { sessionID: "ses-route", directory: "/tmp/eidnara-route" };
const id = (n: number) => `mem_${String(n).padStart(32, "0")}`;

type Entry = {
    object?: string;
    revision?: number;
    lanes?: string[];
    labeled?: boolean;
    causality?: string;
    /** The fused position; the daemon keeps it when it filters earlier entries out. */
    position?: number;
};

function fused(entries: Entry[], extra: Record<string, unknown> = {}) {
    return {
        kind: "fused",
        degraded: false,
        truncated: false,
        lanes: {
            exact: { status: "complete" },
            lexical: { status: "complete" },
            dense: { status: "complete" },
        },
        entries: entries.map((entry, index) => ({
            occurrence_id: `occ-${index}`,
            position: entry.position ?? index + 1,
            score: 1 / (index + 1),
            lanes: Object.fromEntries(
                (entry.lanes ?? ["dense"]).map((lane) => [lane, { position: index + 1, raw: 0.5 }]),
            ),
            ...(entry.object
                ? {
                      canonical: {
                          decision_object_id: entry.object,
                          source_revision: entry.revision ?? 1,
                          visibility: entry.labeled ? "labeled" : "visible",
                          causality: entry.causality ?? "unknown",
                      },
                  }
                : {}),
        })),
        ...extra,
    };
}

function harness(transport = new FakeKernelTransport(new FakeKernel())) {
    const kernelClient: KernelClientResolver = ({ sessionId, projectRoot }) =>
        new KernelClient({ transport, enabled: true, sessionId, projectRoot });
    const deps = { kernelClient, resolveProjectPath: () => "git:route-project" };
    const run = (args: Record<string, unknown>, abort?: AbortSignal) =>
        executeEidnaraSearch(deps, args, abort ? { ...ctx, abort } : ctx);
    return { kernel: transport.kernel, transport, run };
}

const methods = (transport: FakeKernelTransport) => transport.calls.map((call) => call.method);

describe("eidnara_search through the fused route", () => {
    it("renders hydrated canonical text in fused order for a match only the route can rank", async () => {
        const { kernel, transport, run } = harness();
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "CONSTRAINTS",
            summary: "Keep the store embedded.",
        });
        kernel.seedDecision({
            object_id: id(2),
            decision_kind: "NAMING",
            summary: "Handlers end in Handler.",
        });
        kernel.seedDecision({
            object_id: id(3),
            decision_kind: "NAMING",
            summary: "Unranked memory.",
        });
        const bodies: Record<string, unknown>[] = [];
        kernel.routeReply = (body) => {
            bodies.push(body);
            return fused([
                { object: id(2), lanes: ["dense"] },
                {},
                { object: id(1), lanes: ["lexical"] },
                { object: id(2), lanes: ["exact"] },
            ]);
        };
        const execution = await run({ query: "semantic phrasing" });
        expect(execution.status).toBe("complete");
        if (execution.status !== "complete") return;
        expect(execution.prePack.map((result) => result.objectId)).toEqual([id(2), id(1)]);
        expect(execution.text).toContain(`[1] [memory] score=1.00 id=${id(2)}`);
        expect(execution.text).toContain("match=fused lanes=exact,dense");
        expect(execution.text).toContain("Handlers end in Handler.");
        expect(execution.text).toContain("Keep the store embedded.");
        expect(execution.text).not.toContain("Unranked memory.");
        expect(execution.text).not.toContain("legacy snapshot");
        expect(methods(transport)).toEqual(["retrieval.query", "kernel.read"]);
        expect(transport.calls[1]?.body).toMatchObject({ object_ids: [id(2), id(1)] });
        expect(bodies[0]).toMatchObject({ query: "semantic phrasing", destination: "local" });
        expect(bodies[0]?.remaining_ms as number).toBeLessThanOrEqual(ROUTE_SEARCH_DEADLINE_MS);
        expect(Number.isSafeInteger(bodies[0]?.remaining_ms)).toBe(true);
    });

    it("renders the survivors of a ranking whose filtered entries left position gaps", async () => {
        const { kernel, transport, run } = harness();
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "NAMING",
            summary: "Second survivor.",
        });
        kernel.seedDecision({
            object_id: id(2),
            decision_kind: "NAMING",
            summary: "First survivor.",
        });
        kernel.routeReply = () =>
            fused([
                { object: id(2), position: 2 },
                { object: id(1), position: 4 },
            ]);
        const execution = await run({ query: "survivor" });
        if (execution.status !== "complete") throw new Error(execution.text);
        expect(execution.prePack.map((result) => result.objectId)).toEqual([id(2), id(1)]);
        expect(execution.text).toContain("First survivor.");
        expect(execution.text).toContain("Second survivor.");
        expect(methods(transport)).toEqual(["retrieval.query", "kernel.read"]);
    });

    it("refuses with the daemon's typed state when the route answers a state-only envelope", async () => {
        const cases = [
            {
                state: { kind: "unavailable", reason: "store_starting" },
                text: "Memory is unavailable while the store opens",
            },
            {
                state: { kind: "invalid", reason: "project_mismatch" },
                text: "The request named a project other than the bound one.",
            },
        ];
        for (const { state, text } of cases) {
            const { kernel, transport, run } = harness();
            kernel.seedDecision({
                object_id: id(1),
                decision_kind: "NAMING",
                summary: "memory text",
            });
            kernel.routeReply = () => ({ state });
            const execution = await run({ query: "memory text" });
            expect(execution.status).toBe("invalid");
            expect(execution.text).toContain(text);
            expect(execution.text).not.toContain("does not recognize");
            expect(methods(transport)).toEqual(["retrieval.query"]);
        }
    });

    it("translates explicit memory ids into route selectors and refuses more than the route probes", async () => {
        const { kernel, transport, run } = harness();
        kernel.seedDecision({ object_id: id(1), decision_kind: "NAMING", summary: "Direct hit." });
        let sent: unknown;
        kernel.routeReply = (body) => {
            sent = body.query;
            return fused([{ object: id(1), lanes: ["exact"] }]);
        };
        const direct = await run({ query: `${id(1)}@7 ${id(1)}` });
        expect(sent).toBe(`id:${id(1)}`);
        expect(direct.text).toContain("Direct hit.");

        transport.calls.length = 0;
        const atLimit = Array.from({ length: ROUTE_MAX_SELECTORS }, (_, n) => id(n + 10)).join(" ");
        await run({ query: atLimit });
        expect(methods(transport)).toEqual(["retrieval.query", "kernel.read"]);
        transport.calls.length = 0;
        const many = Array.from({ length: ROUTE_MAX_SELECTORS + 1 }, (_, n) => id(n + 10)).join(
            " ",
        );
        const refused = await run({ query: many });
        expect(refused.status).toBe("invalid");
        expect(transport.calls).toHaveLength(0);
    });

    it("limits results and leaves sources: [] without any daemon call", async () => {
        const { kernel, transport, run } = harness();
        for (let n = 1; n <= 40; n += 1) {
            kernel.seedDecision({
                object_id: id(n),
                decision_kind: "NAMING",
                summary: `Memory ${n}.`,
            });
        }
        kernel.routeReply = () =>
            fused(Array.from({ length: 32 }, (_, n) => ({ object: id(n + 1) })));
        const limited = await run({ query: "memory", limit: 3 });
        if (limited.status !== "complete") throw new Error(limited.text);
        expect(limited.prePack.map((result) => result.objectId)).toEqual([id(1), id(2), id(3)]);
        expect(transport.calls[1]?.body).toMatchObject({ object_ids: [id(1), id(2), id(3)] });

        transport.calls.length = 0;
        const none = await run({ query: "memory", sources: [] });
        expect(none.status).toBe("complete");
        expect(transport.calls).toHaveLength(0);
    });

    it("ranks a memory-only source subset through the route exactly as the default sources do", async () => {
        const rendered = async (sources?: string[]) => {
            const { kernel, transport, run } = harness();
            kernel.seedDecision({ object_id: id(1), decision_kind: "NAMING", summary: "First." });
            kernel.seedDecision({ object_id: id(2), decision_kind: "NAMING", summary: "Second." });
            let request: Record<string, unknown> = {};
            kernel.routeReply = (body) => {
                const { remaining_ms: _remaining, ...rest } = body;
                request = rest;
                return fused([{ object: id(2) }, { object: id(1) }]);
            };
            const execution = await run(
                sources === undefined ? { query: "order" } : { query: "order", sources },
            );
            if (execution.status !== "complete") throw new Error(execution.text);
            return {
                order: execution.prePack.map((result) => result.objectId),
                methods: methods(transport),
                request,
                text: execution.text,
            };
        };
        const memoryOnly = await rendered(["memory"]);
        expect(memoryOnly.order).toEqual([id(2), id(1)]);
        expect(memoryOnly.methods).toEqual(["retrieval.query", "kernel.read"]);
        expect(memoryOnly.request).toMatchObject({ query: "order", destination: "local" });
        expect(await rendered()).toEqual(memoryOnly);
    });

    it("renders no memory text when the hydration read answers a project mismatch", async () => {
        const { kernel, transport, run } = harness();
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "NAMING",
            summary: "Text bound to the ranked project.",
        });
        kernel.routeReply = () => fused([{ object: id(1) }]);
        kernel.surfaceStates.set("explicit_search", {
            kind: "invalid",
            reason: "project_mismatch",
        });
        const execution = await run({ query: "bound project" });
        expect(execution.status).toBe("invalid");
        expect(execution.text).toContain("The request named a project other than the bound one.");
        expect(execution.text).not.toContain("Text bound to the ranked project.");
        expect(methods(transport)).toEqual(["retrieval.query", "kernel.read"]);
    });

    it("answers a healthy empty ranking without the snapshot scan", async () => {
        const { kernel, transport, run } = harness();
        kernel.seedDecision({ object_id: id(1), decision_kind: "NAMING", summary: "memory text" });
        kernel.routeReply = () => fused([]);
        const execution = await run({ query: "memory text" });
        expect(execution.text).toContain("No results found");
        expect(methods(transport)).toEqual(["retrieval.query"]);

        transport.calls.length = 0;
        kernel.routeReply = () => fused([{}, {}]);
        const unmatched = await run({ query: "memory text" });
        expect(unmatched.text).toContain(
            "Memory: the fused ranking matched no current memory decisions.",
        );
        expect(methods(transport)).toEqual(["retrieval.query"]);
    });

    it("falls back to the labeled snapshot scan only for a disabled or unavailable route", async () => {
        for (const terminal of ["disabled", "lane_unavailable"]) {
            const { kernel, transport, run } = harness();
            kernel.seedDecision({
                object_id: id(1),
                decision_kind: "NAMING",
                summary: "memory text",
            });
            kernel.routeReply = () => ({ kind: "terminal", terminal, reason: "no_family" });
            const execution = await run({ query: "memory text" });
            expect(execution.text).toContain(
                "Memory: the fused search route is unavailable (no_family); results come from the legacy snapshot scan.",
            );
            expect(execution.text).toContain(`id=${id(1)}`);
            expect(methods(transport)).toEqual(["retrieval.query", "kernel.read"]);
        }
        const refusals: unknown[] = [
            { kind: "terminal", terminal: "unauthorized" },
            { kind: "terminal", terminal: "deadline" },
            { kind: "terminal", terminal: "cancelled" },
            { kind: "terminal", terminal: "required_context_failure" },
            {
                ...fused([]),
                entries: [
                    {
                        occurrence_id: "occ-0",
                        position: 1,
                        lanes: { dense: { position: 1, raw: 0.5 } },
                        canonical: {
                            decision_object_id: id(1),
                            source_revision: 1,
                            visibility: "hidden",
                            causality: "unknown",
                        },
                    },
                ],
            },
            {
                ...fused([]),
                entries: [
                    {
                        occurrence_id: "occ-0",
                        position: 2,
                        lanes: { dense: { position: 1, raw: 0.5 } },
                    },
                    {
                        occurrence_id: "occ-1",
                        position: 2,
                        lanes: { dense: { position: 2, raw: 0.4 } },
                    },
                ],
            },
            {
                ...fused([]),
                entries: [
                    {
                        occurrence_id: "occ-0",
                        position: 1,
                        lanes: { sparse: { position: 1, raw: 0.5 } },
                    },
                ],
            },
            { kind: "fused", entries: "not-a-list" },
            { kind: "terminal", terminal: "made_up" },
            fused([{ object: id(1), causality: "trusted" }]),
            {
                ...fused([]),
                entries: [
                    {
                        occurrence_id: "occ-0",
                        position: 1,
                        lanes: { dense: { position: 1, raw: 0.5 } },
                        canonical: {
                            decision_object_id: id(1),
                            source_revision: 1,
                            visibility: "visible",
                        },
                    },
                ],
            },
        ];
        for (const reply of refusals) {
            const { kernel, transport, run } = harness();
            kernel.seedDecision({
                object_id: id(1),
                decision_kind: "NAMING",
                summary: "memory text",
            });
            kernel.routeReply = () => reply as Record<string, unknown>;
            const execution = await run({ query: "memory text" });
            expect(execution.status).toBe("invalid");
            expect(execution.text).not.toContain(id(1));
            expect(methods(transport)).toEqual(["retrieval.query"]);
        }
    });

    it("renders no stale, withdrawn, or expired text that changed after ranking", async () => {
        const { kernel, run } = harness();
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "NAMING",
            summary: "Revised text.",
            source_revision: 2,
        });
        kernel.seedDecision({
            object_id: id(2),
            decision_kind: "NAMING",
            summary: "Other domain.",
            domain_id: "notes",
        });
        kernel.seedDecision({
            object_id: id(3),
            decision_kind: "REJECTED_APPROACH",
            summary: renderAntiMemoryContent({
                trigger: "cache",
                rejectedStrategy: "Use Redis",
                rejectionReason: "offline",
                saferAlternative: null,
                expiresAt: 1,
            }),
        });
        kernel.seedDecision({
            object_id: id(4),
            decision_kind: "NAMING",
            summary: "Still current.",
        });
        kernel.routeReply = () =>
            fused([
                { object: id(1), revision: 1 },
                { object: id(2) },
                { object: id(3) },
                { object: id(5) },
                { object: id(4) },
                { object: id(4), revision: 2 },
            ]);
        const execution = await run({ query: "anything" });
        if (execution.status !== "complete") throw new Error(execution.text);
        expect(execution.prePack).toEqual([]);
        expect(execution.text).not.toContain("Revised text.");
        expect(execution.text).not.toContain("Other domain.");
        expect(execution.text).not.toContain("Use Redis");
        expect(execution.text).not.toContain("Still current.");
        expect(execution.text).toContain(
            "Memory: 3 ranked memories changed or became unreadable after ranking and were not rendered.",
        );
    });

    it("gives a ranked decision of another domain no result place and no stale note", async () => {
        const { kernel, run } = harness();
        // Each memory is ranked beside the statement record that approves it and repeats its text.
        const ranked: { object: string }[] = [];
        for (const index of [1, 2, 3]) {
            kernel.seedDecision({
                object_id: id(index),
                decision_kind: "NAMING",
                summary: `User stated: fact ${index}.`,
            });
            kernel.seedDecision({
                object_id: id(index + 10),
                decision_kind: "adr_accepted",
                summary: `User stated: fact ${index}.`,
                domain_id: "user-statements",
            });
            ranked.push({ object: id(index + 10) }, { object: id(index) });
        }
        kernel.routeReply = () => fused(ranked);
        const execution = await run({ query: "fact", limit: 2 });
        if (execution.status !== "complete") throw new Error(execution.text);
        expect(execution.prePack.map((result) => result.objectId)).toEqual([id(1), id(2)]);
        expect(execution.text).not.toContain("changed or became unreadable");
        expect(execution.text).not.toContain(id(11));
    });

    it("gives a ranked decision that left the store before the read no result place", async () => {
        const { kernel, transport, run } = harness();
        // A correction retired the first memory and its statement record after the ranking, so the read returns neither row.
        kernel.seedDecision({
            object_id: id(2),
            decision_kind: "NAMING",
            summary: "User stated: fact 2.",
        });
        kernel.routeReply = () => fused([{ object: id(11) }, { object: id(1) }, { object: id(2) }]);
        const tip = kernel.tip;
        const execution = await run({ query: "fact", limit: 2 });
        if (execution.status !== "complete") throw new Error(execution.text);
        expect(execution.prePack.map((result) => result.objectId)).toEqual([id(2)]);
        expect(execution.text).toContain(
            "Memory: 2 ranked memories changed or became unreadable after ranking and were not rendered.",
        );
        const reads = transport.calls.filter((call) => call.method === "kernel.read");
        expect(reads.map((call) => (call.body as { as_of?: number | null }).as_of ?? null)).toEqual(
            [null, tip],
        );
    });

    it("gives a ranked memory whose row is present a place, whatever became of it", async () => {
        const { kernel, transport, run } = harness();
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "NAMING",
            summary: "Revised text.",
            source_revision: 2,
        });
        for (const index of [2, 3]) {
            kernel.seedDecision({
                object_id: id(index),
                decision_kind: "NAMING",
                summary: `Fact ${index}.`,
            });
        }
        kernel.routeReply = () =>
            fused([{ object: id(1), revision: 1 }, { object: id(2) }, { object: id(3) }]);
        const execution = await run({ query: "fact", limit: 2 });
        if (execution.status !== "complete") throw new Error(execution.text);
        expect(execution.prePack.map((result) => result.objectId)).toEqual([id(2)]);
        const named = transport.calls
            .filter((call) => call.method === "kernel.read")
            .flatMap((call) => (call.body as { object_ids?: string[] }).object_ids ?? []);
        expect(named).toEqual([id(1), id(2)]);
    });

    it("gives a ranked id the read could not resolve a place and names it", async () => {
        const { kernel, transport, run } = harness();
        for (const index of [1, 2]) {
            kernel.seedDecision({
                object_id: id(index),
                decision_kind: "NAMING",
                summary: `Fact ${index}.`,
            });
        }
        // Every filtered read answers truncated with no row, so a single named id stays unresolved.
        kernel.filteredReadRowCap = 0;
        kernel.routeReply = () => fused([{ object: id(1) }, { object: id(2) }]);
        const execution = await run({ query: "fact", limit: 1 });
        if (execution.status !== "complete") throw new Error(execution.text);
        expect(execution.prePack).toEqual([]);
        expect(execution.text).toContain(`Memory: unresolved ranked memory ids: ${id(1)}`);
        expect(execution.text).not.toContain("changed or became unreadable");
        expect(transport.calls.filter((call) => call.method === "kernel.read")).toHaveLength(1);
    });

    it("ends a ranking of decisions that all left the store within one read per decision", async () => {
        const { kernel, transport, run } = harness();
        const ranked = Array.from({ length: 32 }, (_, index) => ({ object: id(index + 1) }));
        kernel.routeReply = () => fused(ranked);
        const execution = await run({ query: "fact", limit: 1 });
        if (execution.status !== "complete") throw new Error(execution.text);
        expect(execution.prePack).toEqual([]);
        expect(execution.text).toContain(
            "Memory: 32 ranked memories changed or became unreadable after ranking and were not rendered.",
        );
        expect(transport.calls.filter((call) => call.method === "kernel.read")).toHaveLength(32);
    });

    it("reads every refill at the first read's store state when a commit lands between them", async () => {
        class CommittingTransport extends FakeKernelTransport {
            reads = 0;
            async call(args: Parameters<FakeKernelTransport["call"]>[0]): Promise<unknown> {
                const reply = await super.call(args);
                if (args.method === "kernel.read") {
                    this.reads += 1;
                    // Another writer commits after each read answers.
                    this.kernel.seedDecision({
                        object_id: id(90 + this.reads),
                        decision_kind: "NAMING",
                        summary: `Unrelated commit ${this.reads}.`,
                    });
                }
                return reply;
            }
        }
        const transport = new CommittingTransport(new FakeKernel());
        const { kernel, run } = harness(transport);
        const ranked: { object: string }[] = [];
        for (const index of [1, 2, 3]) {
            kernel.seedDecision({
                object_id: id(index),
                decision_kind: "NAMING",
                summary: `User stated: fact ${index}.`,
            });
            kernel.seedDecision({
                object_id: id(index + 10),
                decision_kind: "adr_accepted",
                summary: `User stated: fact ${index}.`,
                domain_id: "user-statements",
            });
            ranked.push({ object: id(index + 10) }, { object: id(index) });
        }
        kernel.routeReply = () => fused(ranked);
        const tip = kernel.tip;
        const execution = await run({ query: "fact", limit: 2 });
        if (execution.status !== "complete") throw new Error(execution.text);
        expect(execution.prePack.map((result) => result.objectId)).toEqual([id(1), id(2)]);
        const reads = transport.calls.filter((call) => call.method === "kernel.read");
        expect(reads.length).toBeGreaterThan(1);
        expect(reads.map((call) => (call.body as { as_of?: number | null }).as_of ?? null)).toEqual(
            [null, ...reads.slice(1).map(() => tip)],
        );
        expect(kernel.tip).toBeGreaterThan(tip);
    });

    it("settles for a limit that is not a whole number", async () => {
        const { kernel, transport } = harness();
        for (const index of [1, 2, 3]) {
            kernel.seedDecision({
                object_id: id(index),
                decision_kind: "NAMING",
                summary: `Fact ${index}.`,
            });
        }
        kernel.routeReply = () => fused([{ object: id(1) }, { object: id(2) }, { object: id(3) }]);
        const search = async (limit: number) => {
            const client = new KernelClient({
                transport,
                enabled: true,
                sessionId: ctx.sessionID,
                projectRoot: "git:route-project",
            });
            const before = transport.calls.filter((call) => call.method === "kernel.read").length;
            const outcome = await searchThroughRoute({ client, query: "fact", limit });
            if (outcome.kind !== "ranked") throw new Error(JSON.stringify(outcome));
            const reads =
                transport.calls.filter((call) => call.method === "kernel.read").length - before;
            return { ids: outcome.results.map((result) => result.objectId), reads };
        };
        expect(await search(0.5)).toEqual({ ids: [], reads: 0 });
        expect(await search(1.5)).toEqual({ ids: [id(1)], reads: 1 });
        expect(await search(0)).toEqual({ ids: [], reads: 0 });
        expect(await search(-1)).toEqual({ ids: [], reads: 0 });
        expect(await search(Number.NaN)).toEqual({ ids: [], reads: 0 });
        expect(await search(Number.POSITIVE_INFINITY)).toEqual({
            ids: [id(1), id(2), id(3)],
            reads: 1,
        });
    });

    it("labels a result the route validated as labeled", async () => {
        const { kernel, run } = harness();
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "NAMING",
            summary: "Disputed rule.",
            labeled: false,
        });
        kernel.seedDecision({
            object_id: id(2),
            decision_kind: "NAMING",
            summary: "Verified rule.",
            labeled: false,
        });
        kernel.routeReply = () =>
            fused([{ object: id(1) }, { object: id(2) }, { object: id(1), labeled: true }]);
        const execution = await run({ query: "rule" });
        expect(execution.text).toContain(
            `id=${id(1)} category=NAMING match=fused lanes=dense provenance=unknown trust=[labeled]`,
        );
        expect(execution.text).toContain(
            `id=${id(2)} category=NAMING match=fused lanes=dense provenance=unknown\n`,
        );
    });

    it("renders each result's causal class and keeps the route's order whatever the class", async () => {
        const { kernel, run } = harness();
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "NAMING",
            summary: "Unknown first.",
            labeled: false,
        });
        kernel.seedDecision({
            object_id: id(2),
            decision_kind: "NAMING",
            summary: "Observed second.",
            labeled: false,
        });
        kernel.routeReply = () =>
            fused([{ object: id(1) }, { object: id(2), causality: "direct_observation" }]);
        const execution = await run({ query: "rule" });
        const first = execution.text.indexOf(
            `id=${id(1)} category=NAMING match=fused lanes=dense provenance=unknown\n`,
        );
        const second = execution.text.indexOf(
            `id=${id(2)} category=NAMING match=fused lanes=dense provenance=direct_observation\n`,
        );
        expect(first).toBeGreaterThan(-1);
        expect(second).toBeGreaterThan(first);
    });

    it("treats one decision ranked under two causal classes as changed after ranking", async () => {
        const { kernel, run } = harness();
        kernel.seedDecision({ object_id: id(1), decision_kind: "NAMING", summary: "Split rule." });
        kernel.routeReply = () =>
            fused([{ object: id(1) }, { object: id(1), causality: "direct_observation" }]);
        const execution = await run({ query: "rule" });
        expect(execution.text).not.toContain("Split rule.");
        expect(execution.text).toContain("changed or became unreadable after ranking");
    });

    it("stops at the caller's abort and renders nothing withdrawn after ranking", async () => {
        const { kernel, transport, run } = harness();
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "NAMING",
            summary: "Withdrawn text.",
        });
        kernel.routeReply = () => {
            const object = kernel.objects.get(id(1));
            if (object) object.invalidated_commit_seq = kernel.tip;
            return fused([{ object: id(1) }]);
        };
        const withdrawn = await run({ query: "anything" });
        expect(withdrawn.text).not.toContain("Withdrawn text.");
        expect(withdrawn.text).toContain("1 ranked memory changed or became unreadable");

        transport.calls.length = 0;
        const controller = new AbortController();
        kernel.routeReply = () => {
            controller.abort();
            return fused([{ object: id(1) }]);
        };
        const aborted = await run({ query: "anything" }, controller.signal);
        expect(aborted.status).toBe("invalid");
        expect(methods(transport)).toEqual(["retrieval.query"]);

        transport.calls.length = 0;
        const early = await run({ query: "anything" }, controller.signal);
        expect(early.status).toBe("invalid");
        expect(transport.calls).toHaveLength(0);
    });

    it("gives hydration only what remains of the search deadline", async () => {
        const { kernel, transport, run } = harness();
        kernel.seedDecision({ object_id: id(1), decision_kind: "NAMING", summary: "Timed text." });
        kernel.routeReply = () => {
            const until = performance.now() + 40;
            while (performance.now() < until) {
                // The ranking spends 40 ms of the search's budget.
            }
            return fused([{ object: id(1) }]);
        };
        await run({ query: "anything" });
        expect(methods(transport)).toEqual(["retrieval.query", "kernel.read"]);
        expect(transport.calls[1]?.timeoutMs ?? Infinity).toBeLessThanOrEqual(
            ROUTE_SEARCH_DEADLINE_MS - 35,
        );
    });

    it("reports a degraded or truncated ranking and renders a ranked anti-memory warning", async () => {
        const { kernel, run } = harness();
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "REJECTED_APPROACH",
            summary: renderAntiMemoryContent({
                trigger: "cache backend",
                rejectedStrategy: "Use Redis",
                rejectionReason: "The project must work offline",
                saferAlternative: "Use the embedded store",
            }),
        });
        kernel.routeReply = () =>
            fused([{ object: id(1) }], {
                degraded: true,
                truncated: true,
                lanes: {
                    exact: { status: "complete" },
                    lexical: { status: "complete" },
                    dense: { status: "unavailable", reason: "lane_busy" },
                },
            });
        const execution = await run({ query: "cache" });
        expect(execution.text).toContain(
            "Memory: the fused ranking is degraded: dense unavailable (lane_busy).",
        );
        expect(execution.text).toContain(
            "Memory: the fused ranking was truncated at the route's result bound.",
        );
        expect(execution.text).toContain("[anti-memory warning]");
        expect(execution.text).toContain("Previously rejected: Use Redis.");
    });

    it("names every bound a degraded lane stopped at", async () => {
        const { kernel, run } = harness();
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "DECISION",
            summary: "offline store",
        });
        kernel.routeReply = () =>
            fused([{ object: id(1) }], {
                degraded: true,
                lanes: {
                    exact: { status: "complete" },
                    lexical: {
                        status: "incomplete",
                        reason: "common_terms",
                        also: ["rank_budget"],
                    },
                    dense: { status: "complete" },
                },
            });
        const execution = await run({ query: "offline" });
        expect(execution.text).toContain(
            "Memory: the fused ranking is degraded: lexical incomplete (common_terms, rank_budget).",
        );
    });

    it("runs a search once more when the daemon connection changed under its contest read, and refuses one whose connection changes under both passes, on the route and on the fallback", async () => {
        const observation = "Assistant reported: The client limit is 500 now.";
        for (const [fallback, replacements] of [
            [false, 1],
            [true, 1],
            [false, 2],
            [true, 2],
        ] as const) {
            class MovingTransport extends FakeKernelTransport {
                identity = 0;
                passReads = 0;
                connectionIdentity(): string {
                    return String(this.identity);
                }
                async call(args: Parameters<FakeKernelTransport["call"]>[0]): Promise<unknown> {
                    if (args.method === "retrieval.query") this.passReads = 0;
                    const reply = await super.call(args);
                    // The daemon is replaced after the contest read, the second read of a pass, in each of the first `replacements` passes.
                    if (
                        args.method === "kernel.read" &&
                        ++this.passReads === 2 &&
                        this.identity < replacements
                    )
                        this.identity += 1;
                    return reply;
                }
            }
            const transport = new MovingTransport(new FakeKernel());
            const { kernel, run } = harness(transport);
            kernel.seedDecision({
                object_id: id(2),
                decision_kind: "CONFIG_VALUES",
                summary: observation,
                source_id: "memory_capture",
            });
            kernel.seedDecision({
                object_id: id(1),
                decision_kind: "CONFIG_VALUES",
                summary: "User stated: The gateway admits at most 300 clients.",
                rationale: JSON.stringify({
                    source: "memory_capture",
                    contested_by: { memory: id(2), stated_seq: 1 },
                }),
                source_id: "memory_capture",
                labeled: false,
            });
            kernel.routeReply = () =>
                fallback ? { kind: "terminal", terminal: "disabled" } : fused([{ object: id(1) }]);
            const execution = await run({ query: fallback ? id(1) : "gateway clients" });
            const rankings = methods(transport).filter((method) => method === "retrieval.query");
            expect(rankings).toHaveLength(2);
            if (replacements === 1) {
                // The second pass ranks and reads on the connection that replaced the first.
                expect(execution.status).toBe("complete");
                expect(execution.text).toContain("500 now");
                continue;
            }
            expect(execution.status).toBe("invalid");
            expect(execution.text).toContain("the known snapshot is ahead of the store");
            expect(execution.text).not.toContain("500 now");
        }
    });

    it("reads a contesting observation at the snapshot the fallback's scan read", async () => {
        const observation = "Assistant reported: The limit is 500 now.";
        class CommittingTransport extends FakeKernelTransport {
            snapshot: number | null = null;
            async call(args: Parameters<FakeKernelTransport["call"]>[0]): Promise<unknown> {
                const reply = (await super.call(args)) as {
                    state?: { kind?: string };
                    known_as_of?: number;
                };
                // A commit lands after the scan's page is read.
                if (
                    args.method === "kernel.read" &&
                    this.snapshot === null &&
                    reply.state?.kind === "available"
                ) {
                    this.snapshot = reply.known_as_of ?? null;
                    this.kernel.seedDecision({
                        object_id: id(3),
                        decision_kind: "CONFIG_VALUES",
                        summary: "The deploy window opens at noon.",
                    });
                }
                return reply;
            }
        }
        const transport = new CommittingTransport(new FakeKernel());
        const { kernel, run } = harness(transport);
        kernel.seedDecision({
            object_id: id(2),
            decision_kind: "CONFIG_VALUES",
            summary: observation,
            source_id: "memory_capture",
        });
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "CONFIG_VALUES",
            summary: "User stated: The gateway admits at most 300 clients.",
            rationale: JSON.stringify({
                source: "memory_capture",
                contested_by: { memory: id(2), stated_seq: 1 },
            }),
            source_id: "memory_capture",
            labeled: false,
        });
        kernel.routeReply = () => ({ kind: "terminal", terminal: "disabled" });
        const execution = await run({ query: "gateway clients" });
        if (execution.status !== "complete") throw new Error(execution.text);
        expect(execution.text).toContain(`Contested by ${id(2)}: ${JSON.stringify(observation)}`);
        // The scan keeps the rows that match the query, so the observation is read by id.
        const contestRead = transport.calls.filter((call) => call.method === "kernel.read").at(-1)
            ?.body as { as_of: number | null; object_ids?: string[] };
        expect(transport.snapshot).not.toBeNull();
        expect(kernel.tip).toBeGreaterThan(transport.snapshot as number);
        expect(contestRead).toMatchObject({ as_of: transport.snapshot, object_ids: [id(2)] });
    });

    it("shows a contested statement with its observation's served text through the route", async () => {
        const { kernel, run } = harness();
        const observation = "Assistant reported: The client limit is 500 now.";
        kernel.seedDecision({
            object_id: id(2),
            decision_kind: "CONFIG_VALUES",
            summary: observation,
            source_id: "memory_capture",
        });
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "CONFIG_VALUES",
            summary: "User stated: The gateway admits at most 300 clients.",
            rationale: JSON.stringify({
                source: "memory_capture",
                contested_by: { memory: id(2), stated_seq: 1 },
            }),
            source_id: "memory_capture",
            labeled: false,
        });
        kernel.routeReply = () => fused([{ object: id(1) }]);
        const execution = await run({ query: "gateway clients" });
        if (execution.status !== "complete") throw new Error(execution.text);
        expect(execution.text).toContain(`Contested by ${id(2)}: ${JSON.stringify(observation)}`);
    });

    it("serves a failed command's record with its category, label, and text on the route and on the fallback", async () => {
        const bytes = (text: string) => new TextEncoder().encode(text).length;
        const outcome =
            "Tool failure recorded from the result of bash: the command failed with exit code 101.";
        const resolved = " Resolved: a later run of this command reported no failure.";
        const closing = "test result: FAILED. 1 passed; 1 failed";
        const summary = [
            `${outcome}${resolved}`,
            "Command: cargo test -p api",
            "Output lines:",
            ...[0, 1, 2, 3].map((index) => `error: e${index} ${"x".repeat(149)}`),
            "width is zero",
            closing,
        ].join("\n");
        const rationale = JSON.stringify({
            at: "2026-03-02T10:15:00Z",
            at_ms: 1772446500000,
            at_source: "result",
            call: "call-1",
            command: "cargo test -p api",
            exit_code: 101,
            place: 5,
            resolved_at: "2026-03-02T10:20:00Z",
            resolved_session: "s2",
            session: "s1",
            source: "tool_failure",
            state: "resolved",
            tool: "bash",
        });
        // The daemon keeps a summary within 900 bytes, and the tool renders 1,024 bytes of a fact.
        expect(bytes(summary)).toBeGreaterThan(860);
        expect(bytes(summary)).toBeLessThanOrEqual(900);
        expect(bytes(`${summary}\n${rationale}`)).toBeGreaterThan(1024);
        for (const fallback of [false, true]) {
            const { kernel, run } = harness();
            kernel.seedDecision({
                object_id: id(1),
                decision_kind: "TOOL_FAILURE",
                summary,
                rationale,
                source_id: "tool_failure",
                labeled: true,
            });
            kernel.routeReply = () =>
                fallback
                    ? { kind: "terminal", terminal: "disabled" }
                    : fused([{ object: id(1), labeled: true }]);
            const execution = await run({ query: "cargo test failed" });
            if (execution.status !== "complete") throw new Error(execution.text);
            expect(execution.prePack.map((result) => result.objectId)).toEqual([id(1)]);
            const lines = execution.text.split("\n");
            const header = lines.findIndex((line) => line.includes(` id=${id(1)} `));
            expect(lines[header]).toContain(" category=TOOL_FAILURE ");
            expect(lines[header]).toContain(` match=${fallback ? "lexical" : "fused"}`);
            expect(lines[header]).toMatch(/ trust=\[labeled\]$/);
            // The first line states the outcome and the resolution, and the last kept line is whole.
            expect(lines[header + 1]).toBe(`${outcome}${resolved}`);
            expect(lines[header + 2]).toBe("Command: cargo test -p api");
            expect(execution.text).toContain(
                `\nwidth is zero\n${closing}\n{"at":"2026-03-02T10:15:00Z"`,
            );
        }
    });

    it("refuses a ranking whose daemon connection changes before hydration in both passes", async () => {
        class MovingTransport extends FakeKernelTransport {
            identity = 0;
            connectionIdentity(): string {
                return String(this.identity);
            }
        }
        const transport = new MovingTransport(new FakeKernel());
        const { kernel, run } = harness(transport);
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "NAMING",
            summary: "Old daemon text.",
        });
        kernel.routeReply = () => {
            transport.identity += 1;
            return fused([{ object: id(1) }]);
        };
        const execution = await run({ query: "anything" });
        expect(execution.status).toBe("invalid");
        expect(execution.text).not.toContain("Old daemon text.");
        expect(methods(transport)).toEqual([
            "retrieval.query",
            "kernel.read",
            "retrieval.query",
            "kernel.read",
        ]);
    });

    it("asks the route again when the daemon connection changed after the route reported itself disabled", async () => {
        class MovingTransport extends FakeKernelTransport {
            identity = "first";
            connectionIdentity(): string {
                return this.identity;
            }
        }
        const transport = new MovingTransport(new FakeKernel());
        const { kernel, run } = harness(transport);
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "NAMING",
            summary: "Live daemon text.",
        });
        const askedUnder: string[] = [];
        kernel.routeReply = () => {
            askedUnder.push(transport.identity);
            if (transport.identity === "first") {
                // The first connection reports the route disabled and is replaced before the scan reads.
                transport.identity = "second";
                return { kind: "terminal", terminal: "disabled" };
            }
            return fused([{ object: id(1) }]);
        };
        const execution = await run({ query: "anything" });
        expect(execution.status).toBe("complete");
        expect(execution.text).toContain("Live daemon text.");
        expect(askedUnder).toEqual(["first", "second"]);
        // The first pass sends no scan read: the second route query follows the first.
        expect(methods(transport).slice(0, 2)).toEqual(["retrieval.query", "retrieval.query"]);
    });

    it("hydrates a second ranking on the connection that replaced the first ranking's", async () => {
        class MovingTransport extends FakeKernelTransport {
            identity = "first";
            connectionIdentity(): string {
                return this.identity;
            }
        }
        const transport = new MovingTransport(new FakeKernel());
        const { kernel, run } = harness(transport);
        kernel.seedDecision({
            object_id: id(1),
            decision_kind: "NAMING",
            summary: "Live daemon text.",
        });
        const rankedUnder: string[] = [];
        const budgets: number[] = [];
        kernel.routeReply = (body) => {
            rankedUnder.push(transport.identity);
            budgets.push(body.remaining_ms as number);
            transport.identity = "second";
            const until = performance.now() + 40;
            while (performance.now() < until) {
                // Each ranking spends 40 ms of the search's budget.
            }
            return fused([{ object: id(1) }]);
        };
        const execution = await run({ query: "anything" });
        expect(execution.status).toBe("complete");
        expect(execution.text).toContain("Live daemon text.");
        expect(rankedUnder).toEqual(["first", "second"]);
        // The second ranking has what the first left of the one route budget.
        expect(budgets[1] ?? Infinity).toBeLessThanOrEqual(ROUTE_SEARCH_DEADLINE_MS - 35);
    });
});

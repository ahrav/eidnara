import { describe, expect, it } from "bun:test";
import { KernelClient } from "../../shared/kernel-client";
import { renderAntiMemoryContent } from "../../shared/kernel-client/anti-memory";
import { FakeKernel, FakeKernelTransport } from "../../shared/kernel-client-testing/fake-kernel";
import type { KernelClientResolver } from "../eidnara-memory/types";
import { executeEidnaraSearch } from "./execute";
import { ROUTE_MAX_SELECTORS, ROUTE_SEARCH_DEADLINE_MS } from "./route-search";

const ctx = { sessionID: "ses-route", directory: "/tmp/eidnara-route" };
const id = (n: number) => `mem_${String(n).padStart(32, "0")}`;

type Entry = { object?: string; revision?: number; lanes?: string[]; labeled?: boolean };

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
            position: index + 1,
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
            "Memory: 4 ranked memories changed or became unreadable after ranking and were not rendered.",
        );
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
            `id=${id(1)} category=NAMING match=fused lanes=dense trust=[labeled]`,
        );
        expect(execution.text).toContain(`id=${id(2)} category=NAMING match=fused lanes=dense\n`);
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

    it("refuses a ranking whose daemon connection changed before hydration", async () => {
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
            summary: "Old daemon text.",
        });
        kernel.routeReply = () => {
            transport.identity = "second";
            return fused([{ object: id(1) }]);
        };
        const execution = await run({ query: "anything" });
        expect(execution.status).toBe("invalid");
        expect(execution.text).not.toContain("Old daemon text.");
    });
});

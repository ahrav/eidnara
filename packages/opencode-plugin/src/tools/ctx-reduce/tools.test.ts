import { describe, expect, it } from "bun:test";
import type { RustToolBackends } from "../../plugin/rust-tool-backends";
import { createCtxReduceTools } from "./tools";

type ReduceInput = Parameters<NonNullable<RustToolBackends["reduce"]>>[0];

const toolContext = (sessionID = "ses-1") => ({ sessionID, directory: "/repo/project" }) as never;

function recordingReduce(response: unknown = { ok: true, queued: 1 }) {
    const calls: ReduceInput[] = [];
    const reduce: NonNullable<RustToolBackends["reduce"]> = async (input) => {
        calls.push(input);
        return response;
    };
    return { calls, reduce };
}

describe("createCtxReduceTools", () => {
    describe("ctx_reduce", () => {
        it("requires the drop parameter", async () => {
            const { calls, reduce } = recordingReduce();
            const tools = createCtxReduceTools({ rustToolBackends: { reduce } });

            const result = await tools.ctx_reduce.execute({}, toolContext());

            expect(result).toContain("Error");
            expect(result).toContain("'drop' must be provided");
            expect(calls).toHaveLength(0);
        });

        it("accepts reduced compatibility fields alongside a real drop", async () => {
            const { calls, reduce } = recordingReduce();
            const tools = createCtxReduceTools({ rustToolBackends: { reduce } });

            const result = await tools.ctx_reduce.execute(
                {
                    drop: "3",
                    reduced: true,
                    summary: JSON.stringify({ drop: "8" }),
                },
                toolContext(),
            );

            expect(result).toBe("Queued: drop §3§.");
            expect(calls).toHaveLength(1);
            expect(calls[0]).toMatchObject({ sessionId: "ses-1", drop: "3" });
        });

        it("uses the raw drop string and stable call id in rust mode", async () => {
            const { calls, reduce } = recordingReduce();
            const rustTools = createCtxReduceTools({ rustToolBackends: { reduce } });
            const rustContext = {
                sessionID: "ses-1",
                directory: "/repo/project",
                callID: "call-1",
            } as never;

            const rustResult = await rustTools.ctx_reduce.execute({ drop: "3-5" }, rustContext);

            expect(rustResult).toBe("Queued: drop 3-5.");
            expect(calls[0]).toMatchObject({
                drop: "3-5",
                projectRoot: "/repo/project",
            });
            expect(calls[0]?.commandId).toBe("oc-ses-1-call-1");

            await rustTools.ctx_reduce.execute({ drop: "3-5" }, rustContext);
            expect(calls[1]?.commandId).toBe(calls[0]?.commandId);

            await rustTools.ctx_reduce.execute({ drop: "3-5" }, {
                sessionID: "ses-1",
                directory: "/repo/project",
                callID: "call-2",
            } as never);
            expect(calls[2]?.commandId).not.toBe(calls[0]?.commandId);
        });

        it("returns the existing failure wording when the rust module rejects a drop", async () => {
            const tools = createCtxReduceTools({
                rustToolBackends: {
                    reduce: async () => {
                        throw new Error("module unavailable");
                    },
                },
            });

            await expect(tools.ctx_reduce.execute({ drop: "3-5" }, toolContext())).resolves.toBe(
                "Error: Failed to queue ctx_reduce operations. module unavailable",
            );
        });

        it("returns terminal success wording when all requested tags are already queued", async () => {
            const { reduce } = recordingReduce({ ok: true, queued: 0 });
            const tools = createCtxReduceTools({ rustToolBackends: { reduce } });

            const result = await tools.ctx_reduce.execute({ drop: "10-12" }, toolContext());

            expect(result).toBe(
                "All requested tags were already queued or processed. No new action is needed.",
            );
        });

        it("acknowledges only the tags the daemon accepted when some were unknown", async () => {
            const { reduce } = recordingReduce({
                ok: true,
                queued: 1,
                accepted: [1],
                unknown: [99],
            });
            const tools = createCtxReduceTools({ rustToolBackends: { reduce } });

            const result = await tools.ctx_reduce.execute({ drop: "1,99" }, toolContext());

            expect(result).toBe("Queued: drop §1§. Tags 99 not found.");
            expect(result).not.toContain("§99§");
        });

        it("rebuilds ranges from the accepted tags so unknown members never appear as queued", async () => {
            const { reduce } = recordingReduce({
                ok: true,
                queued: 4,
                accepted: [1, 2, 3, 7],
                unknown: [4, 5, 99],
            });
            const tools = createCtxReduceTools({ rustToolBackends: { reduce } });

            const result = await tools.ctx_reduce.execute({ drop: "1-5, §99§, 7" }, toolContext());

            expect(result).toBe("Queued: drop §1§-§3§, §7§. Tags 4, 5, 99 not found.");
        });

        it("echoes the raw request only when the daemon reports no accepted list", async () => {
            const { reduce } = recordingReduce({ ok: true, queued: 3 });
            const tools = createCtxReduceTools({ rustToolBackends: { reduce } });

            const result = await tools.ctx_reduce.execute({ drop: "3-5" }, toolContext());

            expect(result).toBe("Queued: drop 3-5.");
        });

        it("reports unknown tags when every accepted tag was already queued", async () => {
            const { reduce } = recordingReduce({
                ok: true,
                queued: 0,
                accepted: [1],
                unknown: [99],
            });
            const tools = createCtxReduceTools({ rustToolBackends: { reduce } });

            const result = await tools.ctx_reduce.execute({ drop: "1,99" }, toolContext());

            expect(result).toBe(
                "All known requested tags were already queued or processed. No new action is needed. Tags 99 not found.",
            );
        });

        it("issues fallback command ids that differ across tool incarnations", async () => {
            const first = recordingReduce();
            const second = recordingReduce();
            const firstTools = createCtxReduceTools({ rustToolBackends: { reduce: first.reduce } });
            const secondTools = createCtxReduceTools({
                rustToolBackends: { reduce: second.reduce },
            });

            await firstTools.ctx_reduce.execute({ drop: "1" }, toolContext());
            await firstTools.ctx_reduce.execute({ drop: "2" }, toolContext());
            await secondTools.ctx_reduce.execute({ drop: "1" }, toolContext());

            const ids = [
                first.calls[0]?.commandId,
                first.calls[1]?.commandId,
                second.calls[0]?.commandId,
            ];
            expect(new Set(ids).size).toBe(3);
            for (const id of ids) {
                expect(id).toMatch(/^oc-ses-1-[0-9a-f]{12}-\d+$/);
            }
        });

        it("returns the unavailable error when no reduce backend is registered", async () => {
            const tools = createCtxReduceTools({ rustToolBackends: {} });

            const result = await tools.ctx_reduce.execute({ drop: "3" }, toolContext());

            expect(result).toBe(
                "Error: Failed to queue ctx_reduce operations. The daemon backend is unavailable.",
            );
        });
    });
});

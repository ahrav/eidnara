import { describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fakeContext } from "../__tests__/test-utils";
import type { PiRustToolBackends } from "../rust-tool-backends";
import { createCtxReduceTool } from "./ctx-reduce";

type ReduceInput = Parameters<NonNullable<PiRustToolBackends["reduce"]>>[0];

function recordingReduce(response: unknown = { ok: true, queued: 1 }) {
    const calls: ReduceInput[] = [];
    const reduce: NonNullable<PiRustToolBackends["reduce"]> = async (input) => {
        calls.push(input);
        return response;
    };
    return { calls, reduce };
}

async function callTool(
    backends: PiRustToolBackends,
    params: Record<string, unknown>,
    options: { callId?: string; sessionId?: string } = {},
) {
    const tool = createCtxReduceTool({ rustToolBackends: backends });
    const result = await tool.execute(
        options.callId ?? "call-1",
        params as never,
        new AbortController().signal,
        undefined,
        fakeContext(options.sessionId ?? "ses-reduce", "/repo/project") as never,
    );
    const text = (result.content[0] as { text: string }).text;
    return { result, text, isError: result.isError === true };
}

describe("Pi ctx_reduce tool", () => {
    it("requires the drop parameter", async () => {
        const { calls, reduce } = recordingReduce();
        const { isError, text } = await callTool({ reduce }, {});
        expect(isError).toBe(true);
        expect(text).toContain("'drop' must be provided");
        expect(calls).toHaveLength(0);
    });

    it("forwards the raw drop string, the session, a stable command id, and the abort signal", async () => {
        const { calls, reduce } = recordingReduce();
        const tool = createCtxReduceTool({ rustToolBackends: { reduce } });
        const signal = new AbortController().signal;
        const execute = (callId: string) =>
            tool.execute(
                callId,
                { drop: "3-5" } as never,
                signal,
                undefined,
                fakeContext("ses-1", "/repo/project") as never,
            );

        const first = await execute("call-1");
        expect((first.content[0] as { text: string }).text).toBe("Queued: drop 3-5.");
        expect(calls[0]).toEqual({
            sessionId: "ses-1",
            projectRoot: "/repo/project",
            drop: "3-5",
            commandId: "pi-ses-1-call-1",
            signal,
        });

        await execute("call-1");
        expect(calls[1]?.commandId).toBe(calls[0]?.commandId);

        await execute("call-2");
        expect(calls[2]?.commandId).not.toBe(calls[0]?.commandId);
    });

    it("routes on the git root when invoked from a subdirectory, like the session commands", async () => {
        const repo = mkdtempSync(join(tmpdir(), "eidnara-reduce-root-"));
        mkdirSync(join(repo, ".git"));
        const subdir = join(repo, "packages", "inner");
        mkdirSync(subdir, { recursive: true });
        const { calls, reduce } = recordingReduce();
        const tool = createCtxReduceTool({ rustToolBackends: { reduce } });

        await tool.execute(
            "call-1",
            { drop: "3" } as never,
            new AbortController().signal,
            undefined,
            fakeContext("ses-1", subdir) as never,
        );

        expect(calls[0]?.projectRoot).toBe(realpathSync.native(repo));
    });

    it("hashes a command id longer than 128 bytes", async () => {
        const { calls, reduce } = recordingReduce();
        await callTool({ reduce }, { drop: "3" }, { callId: "c".repeat(200) });
        expect(calls[0]?.commandId).toMatch(/^pi-[0-9a-f]{64}$/);
        expect(Buffer.byteLength(calls[0]?.commandId ?? "")).toBeLessThanOrEqual(128);
    });

    it("accepts reduced compatibility fields alongside a real drop", async () => {
        const { calls, reduce } = recordingReduce();
        const { isError, text } = await callTool(
            { reduce },
            { drop: "3", reduced: true, summary: JSON.stringify({ drop: "8" }) },
        );
        expect(isError).toBe(false);
        expect(text).toBe("Queued: drop §3§.");
        expect(calls).toHaveLength(1);
        expect(calls[0]).toMatchObject({ drop: "3" });
    });

    it("reports already-queued work when the backend queues nothing", async () => {
        const { reduce } = recordingReduce({ ok: true, queued: 0 });
        const { isError, text } = await callTool({ reduce }, { drop: "1,2" });
        expect(isError).toBe(false);
        expect(text).toBe(
            "All requested tags were already queued or processed. No new action is needed.",
        );
    });

    it("maps a backend rejection to the failure wording", async () => {
        const { reduce } = recordingReduce({ ok: false, error: { message: "tag 9 is protected" } });
        const rejected = await callTool({ reduce }, { drop: "9" });
        expect(rejected.isError).toBe(true);
        expect(rejected.text).toBe(
            "Error: Failed to queue ctx_reduce operations. tag 9 is protected",
        );

        const thrown = await callTool(
            {
                reduce: async () => {
                    throw new Error("module unavailable");
                },
            },
            { drop: "3-5" },
        );
        expect(thrown.isError).toBe(true);
        expect(thrown.text).toBe(
            "Error: Failed to queue ctx_reduce operations. module unavailable",
        );
    });

    it("returns the unavailable error when no reduce backend is registered", async () => {
        const { isError, text } = await callTool({}, { drop: "3" });
        expect(isError).toBe(true);
        expect(text).toBe(
            "Error: Failed to queue ctx_reduce operations. The daemon backend is unavailable.",
        );
    });
});

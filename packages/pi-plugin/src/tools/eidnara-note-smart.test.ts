import { afterEach, describe, expect, it } from "bun:test";
import {
    __wakePlaneTest,
    WAKE_PLANE_CAPABILITY,
} from "@eidnara/opencode/features/context/conditional-notes/wake-plane";

import { fakeContext } from "../__tests__/test-utils";
import type { PiRustNoteToolRequest, PiRustToolBackends } from "../rust-tool-backends";
import { createEidnaraNoteTool } from "./eidnara-note";

const CWD = "/workspace/project-a";
const SESSION = "ses-note-1";
afterEach(() => __wakePlaneTest.reset());

function recordingNote(response: unknown = "module note result") {
    const requests: PiRustNoteToolRequest[] = [];
    const note: NonNullable<PiRustToolBackends["note"]> = async (request) => {
        requests.push(request);
        return response;
    };
    return { requests, note };
}

async function callNote(args: {
    rustToolBackends: PiRustToolBackends;
    resolveProjectPath?: (directory: string) => string | undefined;
    callId?: string;
    signal?: AbortSignal;
    params: Record<string, unknown>;
}) {
    const tool = createEidnaraNoteTool({
        resolveProjectPath: args.resolveProjectPath ?? (() => "git:project-a"),
        rustToolBackends: args.rustToolBackends,
    });
    const result = await tool.execute(
        args.callId ?? "call-1",
        args.params as never,
        args.signal ?? new AbortController().signal,
        undefined,
        fakeContext(SESSION, CWD) as never,
    );
    return {
        result,
        text: (result.content[0] as { text: string }).text,
        isError: result.isError === true,
    };
}

describe("Pi eidnara_note", () => {
    it("routes ordinary actions directly to Rust with project, command, and cancellation identity", async () => {
        const { requests, note } = recordingNote("Saved session note #1.");
        const signal = new AbortController().signal;
        const { isError } = await callNote({
            signal,
            rustToolBackends: { note },
            params: { action: "write", content: "body" },
        });
        expect(isError).toBe(false);
        expect(requests[0]).toEqual({
            commandId: "call-1",
            sessionId: SESSION,
            projectRoot: CWD,
            memoryProject: "git:project-a",
            action: "write",
            content: "body",
            surfaceCondition: undefined,
            filter: undefined,
            limit: undefined,
            offset: undefined,
            noteId: undefined,
            signal,
        });
    });

    it("forwards conditioned writes uncompiled so Rust owns refusal and no-write semantics", async () => {
        const { requests, note } = recordingNote("Error: no live note evaluator; note not written");
        const { isError, text } = await callNote({
            rustToolBackends: { note },
            params: { action: "write", content: "wait", surface_condition: "when release exists" },
        });
        expect(isError).toBe(true);
        expect(text).toContain("note not written");
        expect(requests[0]).toMatchObject({
            commandId: "call-1",
            surfaceCondition: "when release exists",
        });
        expect(requests[0]).not.toHaveProperty("compileStatus");
    });

    it("keeps wake-plane write guidance and strips its condition", async () => {
        __wakePlaneTest.setCatalogProbe(async () => [
            { module_id: "scheduled-wakes", roles: [], control_ops: [WAKE_PLANE_CAPABILITY] },
        ]);
        const { requests, note } = recordingNote("Saved session note #1.");
        const { text } = await callNote({
            rustToolBackends: { note },
            params: { action: "write", content: "body", surface_condition: "when done" },
        });
        expect(requests[0]?.surfaceCondition).toBeUndefined();
        expect(text).toContain("create a scheduled wake instead; stored as a plain note");
    });

    it("keeps conditioned updates for Rust refusal when wake plane is active", async () => {
        __wakePlaneTest.setCatalogProbe(async () => [
            { module_id: "scheduled-wakes", roles: [], control_ops: [WAKE_PLANE_CAPABILITY] },
        ]);
        const { requests, note } = recordingNote("Error: no live note evaluator; note not updated");
        const { isError } = await callNote({
            rustToolBackends: { note },
            params: { action: "update", note_id: 3, surface_condition: "when done" },
        });
        expect(isError).toBe(true);
        expect(requests[0]?.surfaceCondition).toBe("when done");
    });

    it("requires stable command ids for mutations before transport", async () => {
        const { requests, note } = recordingNote();
        const { isError, text } = await callNote({
            rustToolBackends: { note },
            callId: "",
            params: { action: "write", content: "body" },
        });
        expect(isError).toBe(true);
        expect(text).toContain("requires a stable tool-call identity");
        expect(requests).toHaveLength(0);
    });

    it("hashes overlong command ids stably", async () => {
        const { requests, note } = recordingNote();
        const callId = "c".repeat(200);
        await callNote({
            rustToolBackends: { note },
            callId,
            params: { action: "write", content: "x" },
        });
        await callNote({
            rustToolBackends: { note },
            callId,
            params: { action: "write", content: "x" },
        });
        expect(requests[0]?.commandId).toMatch(/^pi-[0-9a-f]{64}$/);
        expect(requests[1]?.commandId).toBe(requests[0]?.commandId);
    });

    it("preserves project, backend, and authority_draining failures", async () => {
        expect(
            (await callNote({ rustToolBackends: {}, params: { action: "read" } })).text,
        ).toContain("does not support eidnara_note");
        expect(
            (
                await callNote({
                    rustToolBackends: { note: async () => "x" },
                    resolveProjectPath: () => undefined,
                    params: { action: "read" },
                })
            ).text,
        ).toContain("Could not resolve project identity");
        const drained = await callNote({
            rustToolBackends: { note: async () => ({ error: { code: "authority_draining" } }) },
            params: { action: "write", content: "retry me" },
        });
        expect(drained.text).toContain("Write REFUSED and NOT saved");
    });
});

import { afterEach, describe, expect, it } from "bun:test";
import {
    compileSurfaceCondition,
    conditionCompileReplySuffix,
    conditionCompileStorageFields,
} from "@eidnara/opencode/features/context/smart-notes/condition-compiler";
import {
    __wakePlaneTest,
    WAKE_PLANE_CAPABILITY,
} from "@eidnara/opencode/features/context/smart-notes/wake-plane";
import type {
    RustNoteToolRequest,
    RustToolBackends,
} from "@eidnara/opencode/plugin/rust-tool-backends";

import { fakeContext } from "../__tests__/test-utils";
import { createCtxNoteTool } from "./ctx-note";

const CWD = "/workspace/project-a";
const SESSION = "ses-note-1";

afterEach(() => {
    __wakePlaneTest.reset();
});

const resolveProjectPath = (directory: string) =>
    directory.includes("project-b") ? "git:project-b" : "git:project-a";

function recordingNote(response: unknown = "module note result") {
    const requests: RustNoteToolRequest[] = [];
    const note: NonNullable<RustToolBackends["note"]> = async (request) => {
        requests.push(request);
        return response;
    };
    return { requests, note };
}

async function callNote(args: {
    rustToolBackends: RustToolBackends;
    resolveProjectPath?: (directory: string) => string | undefined;
    callId?: string;
    sessionId?: string;
    cwd?: string;
    signal?: AbortSignal;
    params: Record<string, unknown>;
}) {
    const tool = createCtxNoteTool({
        resolveProjectPath: args.resolveProjectPath ?? resolveProjectPath,
        rustToolBackends: args.rustToolBackends,
    });
    const result = await tool.execute(
        args.callId ?? "call-1",
        args.params as never,
        args.signal ?? new AbortController().signal,
        undefined,
        fakeContext(args.sessionId ?? SESSION, args.cwd ?? CWD) as never,
    );
    const text = (result.content[0] as { text: string }).text;
    return { result, text, isError: result.isError === true };
}

describe("Pi ctx_note", () => {
    it("defaults to read when no action or content is given", async () => {
        const { requests, note } = recordingNote("## Notes\n\nNo session notes or smart notes.");
        const { isError, text } = await callNote({ rustToolBackends: { note }, params: {} });

        expect(isError).toBe(false);
        expect(text).toBe("## Notes\n\nNo session notes or smart notes.");
        expect(requests).toHaveLength(1);
        expect(requests[0]?.action).toBe("read");
    });

    it("sends writes to the daemon facade with the project identity and the abort signal", async () => {
        const { requests, note } = recordingNote({
            content: [{ type: "text", text: "Saved session note #1." }],
        });
        const signal = new AbortController().signal;
        const { isError, text } = await callNote({
            signal,
            rustToolBackends: {
                authorityState: async ({ domain }) => (domain === "notes" ? "MODULE" : "TS"),
                note,
            },
            params: { action: "write", content: "module owned note" },
        });

        expect(isError).toBe(false);
        expect(text).toBe("Saved session note #1.");
        expect(requests).toHaveLength(1);
        expect(requests[0]).toEqual({
            commandId: "call-1",
            sessionId: SESSION,
            projectRoot: CWD,
            projectPath: "git:project-a",
            memoryProject: "git:project-a",
            action: "write",
            content: "module owned note",
            surfaceCondition: undefined,
            filter: undefined,
            limit: undefined,
            offset: undefined,
            noteId: undefined,
            signal,
        });
    });

    it("compiles surface_condition when the daemon evaluates notes for the project", async () => {
        const { requests, note } = recordingNote("Created smart note #1.");
        const surfaceCondition = "when path /tmp/project-binding-key exists";
        const expected = await compileSurfaceCondition(surfaceCondition, { projectPath: CWD });

        const { isError, text } = await callNote({
            rustToolBackends: { note, noteEvaluationAvailable: () => true },
            params: {
                action: "write",
                content: "Never inspect key material.",
                surface_condition: surfaceCondition,
            },
        });

        expect(isError).toBe(false);
        expect(expected.status).toBe("refused");
        expect(requests).toHaveLength(1);
        expect(requests[0]).toMatchObject({
            action: "write",
            surfaceCondition,
            ...conditionCompileStorageFields(expected),
        });
        expect(text).toBe(`Created smart note #1.${conditionCompileReplySuffix(expected)}`);
        expect(text).toContain("Retina compile refused: fenced path");
    });

    it("rejects smart-note writes without a command id when evaluation is unavailable", async () => {
        const { requests, note } = recordingNote("must not be called");
        const { isError, text } = await callNote({
            rustToolBackends: { note, noteEvaluationAvailable: () => false },
            callId: "",
            params: {
                action: "write",
                content: "wait for release",
                surface_condition: "when release exists",
            },
        });

        expect(isError).toBe(true);
        expect(text).toBe(
            "Error: Smart-note evaluation is unavailable for this Rust-authority project; the note was not written.",
        );
        expect(requests).toHaveLength(0);
    });

    it("stores surface_condition as a plain note when the wake plane is present", async () => {
        __wakePlaneTest.setCatalogProbe(async () => [
            { module_id: "scheduled-wakes", roles: [], control_ops: [WAKE_PLANE_CAPABILITY] },
        ]);
        const { requests, note } = recordingNote("Saved session note #1.");
        const { text } = await callNote({
            rustToolBackends: { note, noteEvaluationAvailable: () => true },
            params: {
                action: "write",
                content: "Wake-plane module note",
                surface_condition: "When the scheduled operation completes",
            },
        });

        expect(requests).toHaveLength(1);
        expect(requests[0]?.surfaceCondition).toBeUndefined();
        expect(requests[0]?.compileStatus).toBeUndefined();
        expect(text).toBe(
            "Saved session note #1.\nwake plane active — create a scheduled wake instead; stored as a plain note.",
        );
    });

    it("refuses writes while the notes authority is PREPARING", async () => {
        const { requests, note } = recordingNote();
        const { isError, text } = await callNote({
            rustToolBackends: { authorityState: async () => "PREPARING", note },
            params: { action: "write", content: "not yet" },
        });

        expect(isError).toBe(true);
        expect(text).toBe(
            "Error: Rust notes authority is not ready. Write REFUSED and NOT saved; RESEND after authority is ready.\nContent to resend:\nnot yet",
        );
        expect(requests).toHaveLength(0);
    });

    it("returns an error when the project identity cannot be resolved", async () => {
        const { requests, note } = recordingNote();
        const { isError, text } = await callNote({
            rustToolBackends: { note },
            resolveProjectPath: () => undefined,
            params: { action: "write", content: "orphan note" },
        });

        expect(isError).toBe(true);
        expect(text).toBe("Error: Could not resolve project identity for ctx_note.");
        expect(requests).toHaveLength(0);
    });

    it("maps a module drain rejection to the resend refusal", async () => {
        const { isError, text } = await callNote({
            rustToolBackends: {
                note: async () => ({
                    error: { code: "authority_draining", message: "authority is draining" },
                }),
            },
            params: { action: "write", content: "retry me" },
        });

        expect(isError).toBe(true);
        expect(text).toContain("Write REFUSED and NOT saved");
        expect(text).toContain("Content to resend:\nretry me");
    });

    it("returns the transport error when the backend has no note facade", async () => {
        const { isError, text } = await callNote({
            rustToolBackends: {},
            params: { action: "read" },
        });

        expect(isError).toBe(true);
        expect(text).toBe(
            "Error: Rust notes authority is active, but this module transport does not support ctx_note.",
        );
    });
});

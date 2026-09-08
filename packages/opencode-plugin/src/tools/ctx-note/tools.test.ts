import { beforeEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
    compileSurfaceCondition,
    conditionCompileReplySuffix,
    conditionCompileStorageFields,
} from "../../features/context/smart-notes/condition-compiler";
import {
    __wakePlaneTest,
    WAKE_PLANE_CAPABILITY,
} from "../../features/context/smart-notes/wake-plane";
import {
    type RustNoteToolRequest,
    type RustToolBackends,
    RustToolSessionDeletedError,
} from "../../plugin/rust-tool-backends";
import { createCtxNoteTools } from "./tools";

// OpenCode passes the model's tool-call id to plugin tools as `callID`.
const toolContext = (sessionID = "ses-note", directory = "/workspace/project-a") =>
    ({ sessionID, directory, callID: `call-${sessionID}` }) as never;

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

describe("createCtxNoteTools", () => {
    beforeEach(() => {
        __wakePlaneTest.reset();
    });

    it("routes notes only when the notes domain reports module authority", async () => {
        const { requests, note } = recordingNote({
            content: [{ type: "text", text: "module note result" }],
        });
        const moduleTools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async ({ domain }) => (domain === "notes" ? "MODULE" : "TS"),
                note,
                noteEvaluationAvailable: () => true,
            },
        });
        const preparingTools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => "PREPARING",
                note,
            },
        });

        const moduleResult = await moduleTools.ctx_note.execute(
            { action: "write", content: "module owned note" },
            toolContext(),
        );
        const preparingResult = await preparingTools.ctx_note.execute(
            { action: "write", content: "not yet" },
            toolContext(),
        );

        expect(moduleResult).toBe("module note result");
        expect(requests).toHaveLength(1);
        expect(requests[0]).toMatchObject({
            action: "write",
            sessionId: "ses-note",
        });
        expect(requests[0]).not.toHaveProperty("projectRoot");
        expect(requests[0]).not.toHaveProperty("projectPath");
        expect(requests[0]).not.toHaveProperty("memoryProject");
        expect(preparingResult).toBe(
            "Error: Rust notes authority is not ready. Write REFUSED and NOT saved; RESEND the same ctx_note call (action=write) after authority is ready.\nContent to resend:\nnot yet",
        );
    });

    it("refuses when the notes domain reports TS authority", async () => {
        const { requests, note } = recordingNote();
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => "TS",
                note,
            },
        });

        const result = await tools.ctx_note.execute(
            { action: "write", content: "TS owned note" },
            toolContext(),
        );

        expect(result).toContain("Rust notes authority is not ready");
        expect(result).toContain("Write REFUSED and NOT saved");
        expect(result).toContain("Content to resend:\nTS owned note");
        expect(requests).toHaveLength(0);
    });

    it("leaves project resolution to the facade when no preflight hook is registered", async () => {
        const { requests, note } = recordingNote("Saved session note #1.");
        const tools = createCtxNoteTools({
            resolveProjectPath: () => undefined,
            rustToolBackends: { note },
        });

        const result = await tools.ctx_note.execute(
            { action: "write", content: "probe-less note" },
            toolContext(),
        );

        expect(result).toBe("Saved session note #1.");
        expect(requests).toHaveLength(1);
    });

    it("reports a failed authority probe", async () => {
        const { requests, note } = recordingNote();
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => {
                    throw new Error("daemon down");
                },
                note,
            },
        });

        const result = await tools.ctx_note.execute({ action: "read" }, toolContext());

        expect(result).toBe("Error: Rust notes authority is unavailable. daemon down");
        expect(requests).toHaveLength(0);
    });

    it("returns an error when the project identity cannot be resolved", async () => {
        const { requests, note } = recordingNote();
        const tools = createCtxNoteTools({
            resolveProjectPath: () => undefined,
            rustToolBackends: { authorityState: async () => "MODULE", note },
        });

        const result = await tools.ctx_note.execute(
            { action: "write", content: "orphan note" },
            toolContext(),
        );

        expect(result).toBe(
            "Error: Could not resolve project identity for ctx_note preflight checks.",
        );
        expect(requests).toHaveLength(0);
    });

    it("reports a deleted session without claiming the module returned an invalid response", async () => {
        const tools = createCtxNoteTools({
            rustToolBackends: {
                note: async () => {
                    throw new RustToolSessionDeletedError();
                },
            },
        });

        const result = await tools.ctx_note.execute({ action: "read" }, toolContext());

        expect(result).toBe(
            "Error: Session was deleted before ctx_note could run; nothing was read.",
        );
    });

    it("returns the transport error when the backend has no note facade", async () => {
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: { authorityState: async () => "MODULE" },
        });

        const result = await tools.ctx_note.execute({ action: "read" }, toolContext());

        expect(result).toBe(
            "Error: Rust notes authority is active, but this module transport does not support ctx_note.",
        );
    });

    it("maps a raced module drain rejection to the transition retry message", async () => {
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => "MODULE",
                note: async () => ({
                    error: {
                        code: "authority_draining",
                        message: "authority is draining",
                    },
                }),
            },
        });
        const result = await tools.ctx_note.execute(
            { action: "write", content: "retry me" },
            toolContext(),
        );
        expect(result).toContain("Write REFUSED and NOT saved");
        expect(result).toContain("RESEND");
        expect(result).toContain("Content to resend:\nretry me");
    });

    it("does not echo content attached to a read-only module refusal", async () => {
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => "MODULE",
                note: async () => ({
                    error: { code: "authority_draining", message: "authority is draining" },
                }),
            },
        });
        const result = await tools.ctx_note.execute(
            { action: "read", content: "read-only content must not echo" },
            toolContext(),
        );
        expect(result).toContain("REFUSED and NOT applied");
        expect(result).toContain("RESEND");
        expect(result).not.toContain("read-only content must not echo");
    });

    it("preserves update arguments in the transition refusal so a retry stays an update", async () => {
        const { requests, note } = recordingNote();
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: { authorityState: async () => "PREPARING", note },
        });

        const result = await tools.ctx_note.execute(
            {
                action: "update",
                note_id: 7,
                content: "edited body",
                surface_condition: "when release v2 exists",
            },
            toolContext(),
        );

        expect(requests).toHaveLength(0);
        expect(result).toBe(
            'Error: Rust notes authority is not ready. Update REFUSED and NOT applied; RESEND the same ctx_note call (action=update, note_id=7, surface_condition="when release v2 exists") after authority is ready.\nContent to resend:\nedited body',
        );
        expect(result).not.toContain("Write REFUSED");
    });

    it("preserves the condition of a refused conditioned write", async () => {
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => "MODULE",
                noteEvaluationAvailable: () => true,
                note: async () => ({
                    error: { code: "authority_draining", message: "authority is draining" },
                }),
            },
        });

        const result = await tools.ctx_note.execute(
            { action: "write", content: "wait for tag", surface_condition: "when tag v9 exists" },
            toolContext(),
        );

        expect(result).toContain("Write REFUSED and NOT saved");
        expect(result).toContain('(action=write, surface_condition="when tag v9 exists")');
        expect(result).toContain("Content to resend:\nwait for tag");
    });

    it("names the dismissed note in a transition refusal", async () => {
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: { authorityState: async () => "DRAINING" },
        });

        const result = await tools.ctx_note.execute(
            { action: "dismiss", note_id: 3, content: "superseded by #9" },
            toolContext(),
        );

        // The daemon stores dismiss content as the resolution, so the retry text keeps it.
        expect(result).toBe(
            "Error: Rust notes authority is not ready. Dismiss REFUSED and NOT applied; RESEND the same ctx_note call (action=dismiss, note_id=3) after authority is ready.\nContent to resend:\nsuperseded by #9",
        );
    });

    it("downgrades module-authority smart authoring to a regular note when wake plane is present", async () => {
        __wakePlaneTest.setCatalogProbe(async () => [
            { module_id: "scheduled-wakes", roles: [], control_ops: [WAKE_PLANE_CAPABILITY] },
        ]);
        const { requests, note } = recordingNote("Saved session note #1.");
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => "MODULE",
                note,
            },
        });

        const result = await tools.ctx_note.execute(
            {
                action: "write",
                content: "Wake-plane module note",
                surface_condition: "When the scheduled operation completes",
            },
            toolContext(),
        );

        expect(requests).toHaveLength(1);
        expect(requests[0]?.surfaceCondition).toBeUndefined();
        expect(requests[0]?.compileStatus).toBeUndefined();
        expect(result).toBe(
            "Saved session note #1.\nwake plane active — create a scheduled wake instead; stored as a plain note.",
        );
    });

    it("refuses a conditioned update before reaching the backend when the wake plane is present", async () => {
        __wakePlaneTest.setCatalogProbe(async () => [
            { module_id: "scheduled-wakes", roles: [], control_ops: [WAKE_PLANE_CAPABILITY] },
        ]);
        const { requests, note } = recordingNote("must not be called");
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => "MODULE",
                noteEvaluationAvailable: () => true,
                note,
            },
        });
        const refusal =
            "Error: wake plane active — scheduled wakes own condition evaluation; resend the update without surface_condition, or create a scheduled wake instead. Note not updated.";

        // The daemon retains an omitted condition, so a content+condition update
        // cannot be downgraded by dropping the condition the way a write can.
        const withContent = await tools.ctx_note.execute(
            {
                action: "update",
                note_id: 4,
                content: "new body",
                surface_condition: "When the scheduled operation completes",
            },
            toolContext(),
        );
        const conditionOnly = await tools.ctx_note.execute(
            { action: "update", note_id: 4, surface_condition: "when release exists" },
            toolContext(),
        );

        expect(requests).toHaveLength(0);
        expect(withContent).toBe(refusal);
        expect(conditionOnly).toBe(refusal);
    });

    it("forwards a content-only update untouched when the wake plane is present", async () => {
        __wakePlaneTest.setCatalogProbe(async () => [
            { module_id: "scheduled-wakes", roles: [], control_ops: [WAKE_PLANE_CAPABILITY] },
        ]);
        const { requests, note } = recordingNote("Updated note #4: new body");
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: { authorityState: async () => "MODULE", note },
        });

        const result = await tools.ctx_note.execute(
            { action: "update", note_id: 4, content: "new body" },
            toolContext(),
        );

        expect(requests).toHaveLength(1);
        expect(requests[0]).toMatchObject({ action: "update", noteId: 4, content: "new body" });
        expect(result).toBe("Updated note #4: new body");
    });

    it("forwards a conditioned update to the module backend when the wake plane is absent", async () => {
        __wakePlaneTest.setCatalogProbe(async () => [
            { module_id: "other-module", roles: [], control_ops: ["other.operation"] },
        ]);
        const { requests, note } = recordingNote("Updated note #4: body");
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => "MODULE",
                noteEvaluationAvailable: () => true,
                note,
            },
        });

        await tools.ctx_note.execute(
            { action: "update", note_id: 4, surface_condition: "when release exists" },
            toolContext(),
        );

        expect(requests).toHaveLength(1);
        expect(requests[0]?.surfaceCondition).toBe("when release exists");
        expect(requests[0]?.compileStatus).toBeDefined();
    });

    it("passes the unchanged condition to the module backend when the wake plane is absent", async () => {
        __wakePlaneTest.setCatalogProbe(async () => [
            { module_id: "other-module", roles: [], control_ops: ["other.operation"] },
        ]);
        const { requests, note } = recordingNote("Created smart note #1.");
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => "MODULE",
                noteEvaluationAvailable: () => true,
                note,
            },
        });

        const result = await tools.ctx_note.execute(
            {
                action: "write",
                content: "Wake-plane absent module note",
                surface_condition: "When the scheduled operation completes",
            },
            toolContext(),
        );

        expect(requests[0]?.surfaceCondition).toBe("When the scheduled operation completes");
        expect(result).toContain("Created smart note #1");
    });

    it("compiles a fenced MODULE-authority smart note before the facade write", async () => {
        const { requests, note } = recordingNote({
            content: [{ type: "text", text: "Created smart note #1." }],
        });
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => "MODULE",
                noteEvaluationAvailable: () => true,
                note,
            },
        });
        const surfaceCondition = "when path /tmp/project-binding-key exists";
        const expected = await compileSurfaceCondition(surfaceCondition, {
            projectPath: "/workspace/project-a",
        });

        const result = await tools.ctx_note.execute(
            {
                action: "write",
                content: "Never inspect key material.",
                surface_condition: surfaceCondition,
            },
            toolContext(),
        );

        expect(expected.status).toBe("refused");
        expect(requests).toHaveLength(1);
        expect(requests[0]).toMatchObject({
            action: "write",
            content: "Never inspect key material.",
            surfaceCondition,
            ...conditionCompileStorageFields(expected),
        });
        expect(requests[0]?.compileStatus).toBe("refused");
        expect(result).toBe(`Created smart note #1.${conditionCompileReplySuffix(expected)}`);
        expect(result).toContain("Retina compile refused: fenced path");
    });

    it("forwards a conditioned write uncompiled, with its command id, when local evaluation is unavailable", async () => {
        const { requests, note } = recordingNote("Error: no live note evaluator");
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => "MODULE",
                note,
                noteEvaluationAvailable: () => false,
            },
        });
        const result = await tools.ctx_note.execute(
            {
                action: "write",
                content: "wait for release",
                surface_condition: "when release exists",
            },
            toolContext(),
        );
        // The daemon owns the refusal so a redelivered call replays the recorded response by command id.
        expect(result).toBe("Error: no live note evaluator");
        expect(requests).toHaveLength(1);
        expect(requests[0]).toMatchObject({
            action: "write",
            commandId: "call-ses-note",
            surfaceCondition: "when release exists",
        });
        expect(requests[0]).not.toHaveProperty("compileStatus");
        expect(requests[0]).not.toHaveProperty("compiledConfig");
    });

    it("refuses a mutation when the host supplies no tool-call id, before any backend call", async () => {
        const { requests, note } = recordingNote("must not be called");
        let authorityProbes = 0;
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: {
                authorityState: async () => {
                    authorityProbes += 1;
                    return "MODULE";
                },
                note,
                noteEvaluationAvailable: () => true,
            },
        });
        const noCallId = { sessionID: "ses-note", directory: "/workspace/project-a" } as never;

        const write = await tools.ctx_note.execute(
            { action: "write", content: "would duplicate on redelivery" },
            noCallId,
        );
        const update = await tools.ctx_note.execute(
            { action: "update", note_id: 3, content: "changed" },
            noCallId,
        );
        const dismiss = await tools.ctx_note.execute({ action: "dismiss", note_id: 3 }, noCallId);

        expect(write).toBe(
            "Error: ctx_note write requires a stable tool-call identity from the host; the note was not written.",
        );
        expect(update).toContain("ctx_note update requires a stable tool-call identity");
        expect(update).toContain("the note was not updated.");
        expect(dismiss).toContain("the note was not dismissed.");
        expect(requests).toHaveLength(0);
        expect(authorityProbes).toBe(0);
    });

    it("serves a read without a tool-call id and sends no command id", async () => {
        const { requests, note } = recordingNote("## Notes");
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: { authorityState: async () => "MODULE", note },
        });

        const result = await tools.ctx_note.execute({ action: "read" }, {
            sessionID: "ses-note",
            directory: "/workspace/project-a",
        } as never);

        expect(result).toBe("## Notes");
        expect(requests).toHaveLength(1);
        expect(requests[0]?.action).toBe("read");
        expect(requests[0]).not.toHaveProperty("commandId");
    });

    it("keeps an explicit read's controls instead of replaying a reduced-call summary", async () => {
        const { requests, note } = recordingNote("## Notes");
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: { authorityState: async () => "MODULE", note },
        });

        const result = await tools.ctx_note.execute(
            {
                reduced: true,
                summary: JSON.stringify({ action: "write", content: "stale replayed write" }),
                filter: "all",
                limit: 5,
            },
            toolContext(),
        );

        expect(result).toBe("## Notes");
        expect(requests).toHaveLength(1);
        expect(requests[0]).toMatchObject({ action: "read", filter: "all", limit: 5 });
        expect(requests[0]?.content).toBeUndefined();
    });

    it("floors fractional pagination before forwarding so the daemon selects the requested page", async () => {
        const { requests, note } = recordingNote("## Notes");
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: { authorityState: async () => "MODULE", note },
        });

        await tools.ctx_note.execute({ action: "read", limit: 2.9, offset: 3.1 }, toolContext());
        await tools.ctx_note.execute({ action: "read", limit: 0, offset: -4 }, toolContext());

        expect(requests).toHaveLength(2);
        expect(requests[0]?.limit).toBe(2);
        expect(requests[0]?.offset).toBe(3);
        // A zero limit would be clamped to one note by the daemon; dropping it yields the default page.
        expect(requests[1]?.limit).toBeUndefined();
        expect(requests[1]?.offset).toBeUndefined();
    });

    it("bounds an over-long tool-call id so the daemon accepts it as the command id", async () => {
        const { requests, note } = recordingNote("Saved session note #1.");
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: { authorityState: async () => "MODULE", note },
        });
        const longCallId = `call-${"x".repeat(200)}`;

        await tools.ctx_note.execute({ action: "write", content: "bounded id" }, {
            sessionID: "ses-note",
            directory: "/workspace/project-a",
            callID: longCallId,
        } as never);
        await tools.ctx_note.execute({ action: "write", content: "bounded id" }, {
            sessionID: "ses-note",
            directory: "/workspace/project-a",
            callID: longCallId,
        } as never);

        expect(requests).toHaveLength(2);
        const commandId = requests[0]?.commandId;
        expect(commandId).toMatch(/^oc-[0-9a-f]{64}$/);
        expect(Buffer.byteLength(commandId ?? "")).toBeLessThanOrEqual(128);
        expect(requests[1]?.commandId).toBe(commandId);
    });

    it("defaults to read (not write) when content is an empty string and no action is given", async () => {
        // When action is absent, empty content and surface_condition default to read rather than a write.
        const { requests, note } = recordingNote("## Notes\n\nNo session notes or smart notes.");
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: { authorityState: async () => "MODULE", note },
        });

        const result = await tools.ctx_note.execute(
            { content: "", surface_condition: "" },
            toolContext(),
        );

        expect(requests).toHaveLength(1);
        expect(requests[0]?.action).toBe("read");
        expect(requests[0]?.surfaceCondition).toBe("");
        expect(result).toBe("## Notes\n\nNo session notes or smart notes.");
    });
});

describe("ctx_note raw argument fallback", () => {
    it("returns a tool error for a non-string content instead of throwing", async () => {
        const { requests, note } = recordingNote();
        const tools = createCtxNoteTools({
            resolveProjectPath,
            rustToolBackends: { authorityState: async () => "MODULE", note },
        });
        const result = await tools.ctx_note.execute(
            { action: "write", content: 123 } as never,
            toolContext(),
        );
        expect(result).toBe("Error: 'content' must be a string.");
        expect(requests).toHaveLength(0);

        const conditionResult = await tools.ctx_note.execute(
            { action: "write", content: "ok", surface_condition: { not: "a string" } } as never,
            toolContext(),
        );
        expect(conditionResult).toBe("Error: 'surface_condition' must be a string.");
        expect(requests).toHaveLength(0);
    });
});

describe("ctx_note smart-note compile root", () => {
    it("resolves a relative file condition against the repository root, not the nested working directory", async () => {
        const root = realpathSync(mkdtempSync(join(tmpdir(), "ctx-note-root-")));
        try {
            mkdirSync(join(root, ".git"));
            mkdirSync(join(root, "packages", "foo"), { recursive: true });
            mkdirSync(join(root, "nested", "deeper"), { recursive: true });
            writeFileSync(join(root, "packages", "foo", "flag.txt"), "pending\n");

            const { requests, note } = recordingNote({
                content: [{ type: "text", text: "Created smart note #1." }],
            });
            const tools = createCtxNoteTools({
                resolveProjectPath,
                rustToolBackends: {
                    authorityState: async () => "MODULE",
                    noteEvaluationAvailable: () => true,
                    note,
                },
            });

            await tools.ctx_note.execute(
                {
                    action: "write",
                    content: "Wait for the flag to flip.",
                    surface_condition: "when file packages/foo/flag.txt contains done",
                },
                toolContext("ses-note", join(root, "nested", "deeper")),
            );

            expect(requests).toHaveLength(1);
            expect(requests[0]?.compileStatus).toBe("compiled");
            const config = JSON.parse(requests[0]?.compiledConfig ?? "{}") as {
                kind?: string;
                path?: string;
            };
            expect(config.kind).toBe("file_contains");
            expect(config.path).toBe(join(root, "packages", "foo", "flag.txt"));
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});

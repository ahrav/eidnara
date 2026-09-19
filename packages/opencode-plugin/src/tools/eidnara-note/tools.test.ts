import { beforeEach, describe, expect, it } from "bun:test";

import {
    __wakePlaneTest,
    WAKE_PLANE_CAPABILITY,
} from "../../features/context/conditional-notes/wake-plane";
import {
    type RustNoteToolRequest,
    type RustToolBackends,
    RustToolSessionDeletedError,
} from "../../plugin/rust-tool-backends";
import { createEidnaraNoteTools } from "./tools";

const toolContext = (callID: string | undefined = "call-ses-note") =>
    ({ sessionID: "ses-note", directory: "/workspace/project-a", callID }) as never;

function recordingNote(response: unknown = "module note result") {
    const requests: RustNoteToolRequest[] = [];
    const note: NonNullable<RustToolBackends["note"]> = async (request) => {
        requests.push(request);
        return response;
    };
    return { requests, note };
}

describe("createEidnaraNoteTools", () => {
    beforeEach(() => __wakePlaneTest.reset());

    it("routes ordinary note actions directly to Rust", async () => {
        const { requests, note } = recordingNote();
        const tool = createEidnaraNoteTools({ rustToolBackends: { note } }).eidnara_note;
        for (const args of [
            { action: "write", content: "body" },
            { action: "read" },
            { action: "update", note_id: 4, content: "changed" },
            { action: "dismiss", note_id: 4 },
        ] as const) {
            expect(await tool.execute(args, toolContext())).toBe("module note result");
        }
        expect(requests.map(({ action }) => action)).toEqual([
            "write",
            "read",
            "update",
            "dismiss",
        ]);
        expect(requests[0]?.commandId).toBe("call-ses-note");
        expect(requests[1]).not.toHaveProperty("projectRoot");
    });

    it("forwards conditioned writes uncompiled so Rust owns refusal and no-write semantics", async () => {
        const { requests, note } = recordingNote("Error: no live note evaluator; note not written");
        const tool = createEidnaraNoteTools({ rustToolBackends: { note } }).eidnara_note;
        const result = await tool.execute(
            { action: "write", content: "wait", surface_condition: "when release exists" },
            toolContext(),
        );
        expect(result).toBe("Error: no live note evaluator; note not written");
        expect(requests).toEqual([
            expect.objectContaining({
                action: "write",
                commandId: "call-ses-note",
                surfaceCondition: "when release exists",
            }),
        ]);
        expect(requests[0]).not.toHaveProperty("compileStatus");
    });

    it("requires stable command ids for mutations before transport", async () => {
        const { requests, note } = recordingNote();
        const tool = createEidnaraNoteTools({ rustToolBackends: { note } }).eidnara_note;
        const result = await tool.execute({ action: "write", content: "body" }, {
            sessionID: "ses-note",
            directory: "/workspace/project-a",
        } as never);
        expect(result).toContain("requires a stable tool-call identity");
        expect(requests).toHaveLength(0);
    });

    it("bounds mutation command ids and keeps retries stable", async () => {
        const { requests, note } = recordingNote();
        const tool = createEidnaraNoteTools({ rustToolBackends: { note } }).eidnara_note;
        const id = `call-${"x".repeat(200)}`;
        await tool.execute({ action: "write", content: "body" }, toolContext(id));
        await tool.execute({ action: "write", content: "body" }, toolContext(id));
        expect(requests[0]?.commandId).toMatch(/^oc-[0-9a-f]{64}$/);
        expect(requests[1]?.commandId).toBe(requests[0]?.commandId);
    });

    it("keeps wake-plane write guidance and strips its condition", async () => {
        __wakePlaneTest.setCatalogProbe(async () => [
            { module_id: "scheduled-wakes", roles: [], control_ops: [WAKE_PLANE_CAPABILITY] },
        ]);
        const { requests, note } = recordingNote("Saved session note #1.");
        const tool = createEidnaraNoteTools({ rustToolBackends: { note } }).eidnara_note;
        const result = await tool.execute(
            { action: "write", content: "body", surface_condition: "when done" },
            toolContext(),
        );
        expect(requests[0]?.surfaceCondition).toBeUndefined();
        expect(result).toContain("create a scheduled wake instead; stored as a plain note");
    });

    it("refuses wake-plane conditioned updates without transport", async () => {
        __wakePlaneTest.setCatalogProbe(async () => [
            { module_id: "scheduled-wakes", roles: [], control_ops: [WAKE_PLANE_CAPABILITY] },
        ]);
        const { requests, note } = recordingNote();
        const tool = createEidnaraNoteTools({ rustToolBackends: { note } }).eidnara_note;
        const result = await tool.execute(
            { action: "update", note_id: 4, content: "body", surface_condition: "when done" },
            toolContext(),
        );
        expect(result).toContain("Note not updated");
        expect(requests).toHaveLength(0);
    });

    it("renders authority_draining while direct Rust routing remains active", async () => {
        const tool = createEidnaraNoteTools({
            rustToolBackends: {
                note: async () => ({ error: { code: "authority_draining", message: "draining" } }),
            },
        }).eidnara_note;
        const result = await tool.execute({ action: "write", content: "retry me" }, toolContext());
        expect(result).toContain("Write REFUSED and NOT saved");
        expect(result).toContain("Content to resend:\nretry me");
    });

    it("reports missing backend, deleted session, and malformed replies", async () => {
        expect(
            await createEidnaraNoteTools({ rustToolBackends: {} }).eidnara_note.execute(
                { action: "read" },
                toolContext(undefined),
            ),
        ).toContain("does not support eidnara_note");
        expect(
            await createEidnaraNoteTools({
                rustToolBackends: {
                    note: async () => {
                        throw new RustToolSessionDeletedError();
                    },
                },
            }).eidnara_note.execute({ action: "read" }, toolContext(undefined)),
        ).toContain("Session was deleted");
        expect(
            await createEidnaraNoteTools({
                rustToolBackends: { note: async () => 42 },
            }).eidnara_note.execute({ action: "read" }, toolContext(undefined)),
        ).toContain("invalid eidnara_note response");
    });

    it("floors pagination and rejects malformed string fields", async () => {
        const { requests, note } = recordingNote();
        const tool = createEidnaraNoteTools({ rustToolBackends: { note } }).eidnara_note;
        await tool.execute({ action: "read", limit: 2.9, offset: 3.1 }, toolContext());
        expect(requests[0]).toMatchObject({ limit: 2, offset: 3 });
        expect(await tool.execute({ action: "write", content: 3 } as never, toolContext())).toBe(
            "Error: 'content' must be a string.",
        );
    });
});

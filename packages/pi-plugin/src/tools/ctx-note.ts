import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { resolveProjectRootDirectory } from "@eidnara/opencode/features/context/project-identity";
import {
    compileSurfaceCondition,
    conditionCompileReplySuffix,
    conditionCompileStorageFields,
} from "@eidnara/opencode/features/context/smart-notes/condition-compiler";
import { wakePlaneStatus } from "@eidnara/opencode/features/context/smart-notes/wake-plane";
import type {
    RustAuthorityState,
    RustNoteToolRequest,
} from "@eidnara/opencode/plugin/rust-tool-backends";
import { isRustAuthorityDrainingError } from "@eidnara/opencode/plugin/rust-tool-backends";
import { CTX_NOTE_DESCRIPTION } from "@eidnara/opencode/tools/ctx-note/constants";
import type { CtxNoteArgs } from "@eidnara/opencode/tools/ctx-note/types";
import { unwrapImitatedReducedArgs } from "@eidnara/opencode/tools/unwrap-imitated-reduced-args";
import { type Static, Type } from "typebox";
import type { PiRustNoteToolRequest, PiRustToolBackends } from "../rust-tool-backends";
import { boundedCommandId } from "./command-id";

const ACTION_VALUES = ["write", "read", "dismiss", "update"] as const;
const FILTER_VALUES = ["all", "active", "pending", "ready", "dismissed"] as const;

const ParamsSchema = Type.Object(
    {
        action: Type.Optional(
            Type.Union(
                ACTION_VALUES.map((value) => Type.Literal(value)),
                {
                    description:
                        "Operation to perform. Defaults to 'write' when content is provided, otherwise 'read'.",
                },
            ),
        ),
        content: Type.Optional(
            Type.String({ description: "Note text to store when action is 'write'." }),
        ),
        surface_condition: Type.Optional(
            Type.String({
                description:
                    "Externally verifiable condition for smart notes. The daemon's note evaluator checks this using gh CLI, web fetches, file reads, git, etc. — NOT your conversation history. Use only for things like GitHub PR/issue state, release tags, file contents, or workflow runs. DO NOT use for 'when the user mentions X' / 'when we revisit Y' / 'when relevant to current task' — the evaluator has no access to session context. For session-relative reminders, omit this and write a regular note.",
            }),
        ),
        filter: Type.Optional(
            Type.Union(
                FILTER_VALUES.map((value) => Type.Literal(value)),
                {
                    description:
                        "Optional read filter. Defaults to active session notes + ready smart notes. Use 'all' to inspect every status or 'pending' to inspect unsurfaced smart notes.",
                },
            ),
        ),
        limit: Type.Optional(
            Type.Number({
                description: "Max notes per section for read, newest first (default: 25)",
            }),
        ),
        offset: Type.Optional(
            Type.Number({
                description: "Skip this many newest notes for read — page older ones (default: 0)",
            }),
        ),
        note_id: Type.Optional(
            Type.Number({
                description: "Note ID (required for 'dismiss' and 'update' actions).",
            }),
        ),
    },
    { additionalProperties: true },
);

type CtxNoteParams = Static<typeof ParamsSchema>;

function ok(text: string) {
    return { content: [{ type: "text" as const, text }], details: undefined };
}

function err(text: string) {
    return {
        content: [{ type: "text" as const, text }],
        details: undefined,
        isError: true,
    };
}

export interface CtxNoteToolDeps {
    /**
     * The tool resolves the session directory's project identity at call time.
     * Every action needs the identity; the tool returns an explanatory error
     * when resolveProjectPath is undefined or yields no identity.
     */
    resolveProjectPath?: (directory: string) => string | undefined;
    rustToolBackends: PiRustToolBackends;
}

function noteAuthorityRefusal(args: CtxNoteArgs, action: RustNoteToolRequest["action"]): string {
    const readiness = "Rust notes authority is not ready.";
    if ((action === "write" || action === "update") && typeof args.content === "string") {
        return `Error: ${readiness} Write REFUSED and NOT saved; RESEND after authority is ready.\nContent to resend:\n${args.content}`;
    }
    return `Error: ${readiness} Request REFUSED and NOT applied; RESEND after authority is ready.`;
}

function moduleNoteText(
    response: unknown,
    args: CtxNoteArgs,
    action: RustNoteToolRequest["action"],
): string | null {
    let value = response;
    if (value !== null && typeof value === "object" && "result" in value) {
        value = (value as { result?: unknown }).result;
    }
    if (isRustAuthorityDrainingError(value)) {
        return noteAuthorityRefusal(args, action);
    }
    if (typeof value === "string") return value;
    if (value !== null && typeof value === "object") {
        const record = value as Record<string, unknown>;
        if (record.ok === false || record.error || typeof record.message === "string") {
            const error = record.error;
            const message =
                typeof error === "string"
                    ? error
                    : error !== null && typeof error === "object" && "message" in error
                      ? String((error as { message?: unknown }).message)
                      : typeof record.message === "string"
                        ? record.message
                        : "module rejected ctx_note";
            return `Error: ${message}`;
        }
        const content = record.content;
        if (Array.isArray(content)) {
            const text = content.find(
                (item): item is { text: string } =>
                    item !== null &&
                    typeof item === "object" &&
                    typeof (item as { text?: unknown }).text === "string",
            )?.text;
            if (text) return text;
        }
    }
    return null;
}

export function createCtxNoteTool(deps: CtxNoteToolDeps): ToolDefinition<typeof ParamsSchema> {
    return {
        name: "ctx_note",
        label: "Eidnara: Notes",
        description: CTX_NOTE_DESCRIPTION,
        parameters: ParamsSchema,
        async execute(toolCallId, rawParams: CtxNoteParams, signal, _onUpdate, ctx) {
            const args = unwrapImitatedReducedArgs(
                rawParams as CtxNoteArgs,
                ["action", "content"],
                {
                    action: { type: "enum", values: ACTION_VALUES },
                    content: "string",
                    surface_condition: "string",
                    filter: { type: "enum", values: FILTER_VALUES },
                    limit: "number",
                    offset: "number",
                    note_id: "number",
                },
            );
            const sessionId = ctx.sessionManager.getSessionId();
            // The daemon keys routes and lineage by `(session, root)`; the commands that act on queued drops route on this same git-root spelling.
            const projectRoot = resolveProjectRootDirectory(ctx.cwd);
            // A string-only check would classify empty content as write and reject it.
            const action = args.action ?? (args.content?.trim() ? "write" : "read");
            const wakePlaneActive =
                action === "write" &&
                Boolean(args.surface_condition?.trim()) &&
                (await wakePlaneStatus()) === "present";
            const surfaceCondition = wakePlaneActive ? undefined : args.surface_condition?.trim();

            const projectIdentity = deps.resolveProjectPath?.(projectRoot);
            if (!projectIdentity) {
                return err("Error: Could not resolve project identity for ctx_note.");
            }

            let notesAuthority: RustAuthorityState | null = null;
            if (deps.rustToolBackends.authorityState) {
                try {
                    notesAuthority = await deps.rustToolBackends.authorityState({
                        projectPath: projectIdentity,
                        projectRoot,
                        domain: "notes",
                    });
                } catch (error) {
                    return err(
                        `Error: Rust notes authority is unavailable. ${error instanceof Error ? error.message : String(error)}`,
                    );
                }
            }
            if (notesAuthority !== null && notesAuthority !== "MODULE") {
                return err(noteAuthorityRefusal(args, action));
            }

            const rustNote = deps.rustToolBackends.note;
            if (!rustNote) {
                return err(
                    "Error: Rust notes authority is active, but this module transport does not support ctx_note.",
                );
            }
            const callId = toolCallId?.trim();
            const commandId = callId ? boundedCommandId(callId) : undefined;
            let compilation: Awaited<ReturnType<typeof compileSurfaceCondition>> | undefined;
            if ((action === "write" || action === "update") && surfaceCondition) {
                if (deps.rustToolBackends.noteEvaluationAvailable?.(projectIdentity) === true) {
                    compilation = await compileSurfaceCondition(surfaceCondition, {
                        projectPath: projectRoot,
                    });
                } else if (!commandId) {
                    return err(
                        "Error: Smart-note evaluation is unavailable for this Rust-authority project; the note was not written.",
                    );
                }
            }
            const request: PiRustNoteToolRequest = {
                ...(commandId ? { commandId } : {}),
                sessionId,
                projectRoot,
                memoryProject: projectIdentity,
                action,
                content: args.content,
                surfaceCondition,
                ...(compilation ? conditionCompileStorageFields(compilation) : {}),
                filter: args.filter,
                limit: args.limit,
                offset: args.offset,
                noteId: args.note_id,
                ...(signal ? { signal } : {}),
            };
            try {
                const text = moduleNoteText(await rustNote(request), args, action);
                if (text === null) {
                    return err("Error: Rust module returned an invalid ctx_note response.");
                }
                if (text.startsWith("Error:")) return err(text);
                if (wakePlaneActive) {
                    return ok(
                        `${text}\nwake plane active — create a scheduled wake instead; stored as a plain note.`,
                    );
                }
                if (compilation) return ok(text + conditionCompileReplySuffix(compilation));
                return ok(text);
            } catch (error) {
                if (isRustAuthorityDrainingError(error)) {
                    return err(noteAuthorityRefusal(args, action));
                }
                return err(
                    `Error: Rust module ctx_note failed. ${error instanceof Error ? error.message : String(error)}`,
                );
            }
        },
    };
}

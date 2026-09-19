import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { wakePlaneStatus } from "@eidnara/opencode/features/context/conditional-notes/wake-plane";
import { resolveProjectRootDirectory } from "@eidnara/opencode/features/context/project-identity";
import type { RustNoteToolRequest } from "@eidnara/opencode/plugin/rust-tool-backends";
import { isRustAuthorityDrainingError } from "@eidnara/opencode/plugin/rust-tool-backends";
import { EIDNARA_NOTE_DESCRIPTION } from "@eidnara/opencode/tools/eidnara-note/constants";
import type { EidnaraNoteArgs } from "@eidnara/opencode/tools/eidnara-note/types";
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
                    "Externally verifiable condition for conditional notes. No evaluator ships with this plugin, so Rust refuses conditioned notes unless another host has registered a live evaluator. Scheduled-wake integrations may instead store a plain note and return scheduling guidance.",
            }),
        ),
        filter: Type.Optional(
            Type.Union(
                FILTER_VALUES.map((value) => Type.Literal(value)),
                {
                    description:
                        "Optional read filter. Defaults to active session notes + ready conditional notes. Use 'all' to inspect every status or 'pending' to inspect unsurfaced conditional notes.",
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

type EidnaraNoteParams = Static<typeof ParamsSchema>;

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

export interface EidnaraNoteToolDeps {
    /**
     * The tool resolves the session directory's project identity at call time.
     * Every action needs the identity; the tool returns an explanatory error
     * when resolveProjectPath is undefined or yields no identity.
     */
    resolveProjectPath?: (directory: string) => string | undefined;
    rustToolBackends: PiRustToolBackends;
}

function noteAuthorityRefusal(
    args: EidnaraNoteArgs,
    action: RustNoteToolRequest["action"],
): string {
    const readiness = "Rust notes authority is not ready.";
    if ((action === "write" || action === "update") && typeof args.content === "string") {
        return `Error: ${readiness} Write REFUSED and NOT saved; RESEND after authority is ready.\nContent to resend:\n${args.content}`;
    }
    return `Error: ${readiness} Request REFUSED and NOT applied; RESEND after authority is ready.`;
}

function moduleNoteText(
    response: unknown,
    args: EidnaraNoteArgs,
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
                        : "module rejected eidnara_note";
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

export function createEidnaraNoteTool(
    deps: EidnaraNoteToolDeps,
): ToolDefinition<typeof ParamsSchema> {
    return {
        name: "eidnara_note",
        label: "Eidnara: Notes",
        description: EIDNARA_NOTE_DESCRIPTION,
        parameters: ParamsSchema,
        async execute(toolCallId, rawParams: EidnaraNoteParams, signal, _onUpdate, ctx) {
            const args = unwrapImitatedReducedArgs(
                rawParams as EidnaraNoteArgs,
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
            const callId = toolCallId?.trim();
            if (action !== "read" && !callId) {
                const outcome =
                    action === "write" ? "written" : action === "update" ? "updated" : "dismissed";
                return err(
                    `Error: eidnara_note ${action} requires a stable tool-call identity from the host; the note was not ${outcome}.`,
                );
            }
            const commandId = callId ? boundedCommandId(callId) : undefined;
            const wakePlaneActive =
                (action === "write" || action === "update") &&
                Boolean(args.surface_condition?.trim()) &&
                (await wakePlaneStatus()) === "present";
            if (wakePlaneActive && action === "update") {
                return err(
                    "Error: wake plane active — scheduled wakes own condition evaluation; resend the update without surface_condition, or create a scheduled wake instead. Note not updated.",
                );
            }
            const surfaceCondition = wakePlaneActive ? undefined : args.surface_condition?.trim();

            const projectIdentity = deps.resolveProjectPath?.(projectRoot);
            if (!projectIdentity) {
                return err("Error: Could not resolve project identity for eidnara_note.");
            }

            const rustNote = deps.rustToolBackends.note;
            if (!rustNote) {
                return err(
                    "Error: Rust notes authority is active, but this module transport does not support eidnara_note.",
                );
            }
            const request: PiRustNoteToolRequest = {
                ...(commandId ? { commandId } : {}),
                sessionId,
                projectRoot,
                memoryProject: projectIdentity,
                action,
                content: args.content,
                surfaceCondition,
                filter: args.filter,
                limit: args.limit,
                offset: args.offset,
                noteId: args.note_id,
                ...(signal ? { signal } : {}),
            };
            try {
                const text = moduleNoteText(await rustNote(request), args, action);
                if (text === null) {
                    return err("Error: Rust module returned an invalid eidnara_note response.");
                }
                if (text.startsWith("Error:")) return err(text);
                if (wakePlaneActive) {
                    return ok(
                        `${text}\nwake plane active — create a scheduled wake instead; stored as a plain note.`,
                    );
                }
                return ok(text);
            } catch (error) {
                if (isRustAuthorityDrainingError(error)) {
                    return err(noteAuthorityRefusal(args, action));
                }
                return err(
                    `Error: Rust module eidnara_note failed. ${error instanceof Error ? error.message : String(error)}`,
                );
            }
        },
    };
}

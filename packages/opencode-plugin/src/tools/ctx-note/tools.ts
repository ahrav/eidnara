import { type ToolDefinition, tool } from "@opencode-ai/plugin";

import { resolveProjectRootDirectory } from "../../features/context/project-identity";
import {
    compileSurfaceCondition,
    conditionCompileReplySuffix,
    conditionCompileStorageFields,
} from "../../features/context/smart-notes/condition-compiler";
import { wakePlaneStatus } from "../../features/context/smart-notes/wake-plane";
import type {
    RustAuthorityState,
    RustNoteToolRequest,
    RustToolBackends,
} from "../../plugin/rust-tool-backends";
import {
    boundedCommandId,
    isRustAuthorityDrainingError,
    toolCallIdFromContext,
} from "../../plugin/rust-tool-backends";
import { unwrapImitatedReducedArgs } from "../unwrap-imitated-reduced-args";
import { CTX_NOTE_DESCRIPTION } from "./constants";
import type { CtxNoteArgs } from "./types";

export { CTX_NOTE_LIGHT_DESCRIPTION } from "../light-descriptions";

export interface CtxNoteToolDeps {
    /**
     * The tool resolves the session directory's project identity at call time.
     * Every action needs the identity; the tool returns an explanatory error
     * when resolveProjectPath is undefined or yields no identity.
     */
    resolveProjectPath?: (directory: string) => string | undefined;
    rustToolBackends: RustToolBackends;
}

/**
 * The refusal preserves action, note_id, surface_condition, and content so a
 * retry keeps its mutation semantics: an update stays an update, a
 * conditioned write stays conditioned, and a dismissal keeps its resolution.
 */
function noteAuthorityRefusal(args: CtxNoteArgs, action: RustNoteToolRequest["action"]): string {
    const readiness = "Rust notes authority is not ready.";
    if (action === "read") {
        return `Error: ${readiness} Request REFUSED and NOT applied; RESEND after authority is ready.`;
    }
    const verb = action === "write" ? "Write" : action === "update" ? "Update" : "Dismiss";
    const outcome = action === "write" ? "NOT saved" : "NOT applied";
    const preserved = [`action=${action}`];
    if (typeof args.note_id === "number") preserved.push(`note_id=${args.note_id}`);
    const condition = args.surface_condition?.trim();
    if (condition) preserved.push(`surface_condition=${JSON.stringify(condition)}`);
    const content = typeof args.content === "string" ? `\nContent to resend:\n${args.content}` : "";
    return `Error: ${readiness} ${verb} REFUSED and ${outcome}; RESEND the same ctx_note call (${preserved.join(", ")}) after authority is ready.${content}`;
}

/**
 * The daemon decodes pagination with `Value::as_u64` and clamps `limit` to at
 * least one, so a fractional value would fall back to the default page and a
 * zero limit would return a single note. Flooring keeps the requested page;
 * a value below `minimum` is dropped so the daemon applies its default.
 * commentlint: allow(JUDGE)
 */
function pageNumber(value: number | undefined, minimum: number): number | undefined {
    if (typeof value !== "number" || !Number.isFinite(value)) return value;
    const floored = Math.floor(value);
    return floored >= minimum ? floored : undefined;
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

const ctxNoteArgsShape = {
    action: tool.schema
        .enum(["write", "read", "dismiss", "update"])
        .optional()
        .describe(
            "Operation to perform. Defaults to 'write' when content is provided, otherwise 'read'.",
        ),
    content: tool.schema.string().optional().describe("Note text to store when action is 'write'."),
    surface_condition: tool.schema
        .string()
        .optional()
        .describe(
            "Externally verifiable condition for smart notes. The daemon's note evaluator checks this using gh CLI, web fetches, file reads, git, etc. — NOT your conversation history. Use only for things like GitHub PR/issue state, release tags, file contents, or workflow runs. DO NOT use for 'when the user mentions X' / 'when we revisit Y' / 'when relevant to current task' — the evaluator has no access to session context. For session-relative reminders, omit this and write a regular note.",
        ),
    filter: tool.schema
        .enum(["all", "active", "pending", "ready", "dismissed"])
        .optional()
        .describe(
            "Optional read filter. Defaults to active session notes + ready smart notes. Use 'all' to inspect every status or 'pending' to inspect unsurfaced smart notes.",
        ),
    limit: tool.schema
        .number()
        .optional()
        .describe("Max notes per section for read, newest first (default: 25)"),
    offset: tool.schema
        .number()
        .optional()
        .describe("Skip this many newest notes for read — page older ones (default: 0)"),
    note_id: tool.schema
        .number()
        .optional()
        .describe("Note ID (required for 'dismiss' and 'update' actions)."),
};
// The tool definition exposes only the documented argument shape to the model
// The parser preserves extra arguments so execute() receives fields not exposed to the model provider.
const ctxNoteArgsSchema = tool.schema.object(ctxNoteArgsShape).passthrough();

function createCtxNoteTool(deps: CtxNoteToolDeps): ToolDefinition {
    return tool({
        description: CTX_NOTE_DESCRIPTION,
        args: ctxNoteArgsShape,
        async execute(rawArgs: CtxNoteArgs, toolContext) {
            const parsedArgs = ctxNoteArgsSchema.safeParse(rawArgs);
            let args = (parsedArgs.success ? parsedArgs.data : rawArgs) as CtxNoteArgs;
            args = unwrapImitatedReducedArgs(
                args,
                ["action", "content", "surface_condition", "filter", "limit", "offset", "note_id"],
                {
                    action: { type: "enum", values: ["write", "read", "dismiss", "update"] },
                    content: "string",
                    surface_condition: "string",
                    filter: {
                        type: "enum",
                        values: ["all", "active", "pending", "ready", "dismissed"],
                    },
                    limit: "number",
                    offset: "number",
                    note_id: "number",
                },
            );
            const sessionId = toolContext.sessionID;
            // The schema fallback keeps raw arguments, so a non-string `content` or `surface_condition` would throw at `.trim()` before the backend `try` can turn it into a tool error.
            for (const field of ["content", "surface_condition"] as const) {
                const value = (args as Record<string, unknown>)[field];
                if (value !== undefined && value !== null && typeof value !== "string") {
                    return `Error: '${field}' must be a string.`;
                }
            }
            // A string-only check would classify empty content as write and reject it.
            const action = args.action ?? (args.content?.trim() ? "write" : "read");
            // The command id is the daemon ledger's replay key for a redelivered mutation, so
            // mutations require a host tool-call identity. `read` has no ledger entry.
            const callId = toolCallIdFromContext(toolContext);
            if (action !== "read" && !callId) {
                const outcome =
                    action === "write" ? "written" : action === "update" ? "updated" : "dismissed";
                return `Error: ctx_note ${action} requires a stable tool-call identity from the host; the note was not ${outcome}.`;
            }
            const commandId = callId ? boundedCommandId(callId) : undefined;
            // When `wakePlaneStatus()` returns `"present"`, scheduled wakes evaluate `surface_condition`.
            const wakePlaneActive =
                (action === "write" || action === "update") &&
                Boolean(args.surface_condition?.trim()) &&
                (await wakePlaneStatus()) === "present";
            if (wakePlaneActive && action === "update") {
                return "Error: wake plane active — scheduled wakes own condition evaluation; resend the update without surface_condition, or create a scheduled wake instead. Note not updated.";
            }
            const surfaceCondition = wakePlaneActive ? undefined : args.surface_condition?.trim();

            // The tool resolves toolContext.directory on every call.
            const projectIdentity = deps.resolveProjectPath?.(toolContext.directory);
            if (!projectIdentity) {
                return "Error: Could not resolve project identity for ctx_note.";
            }

            let notesAuthority: RustAuthorityState | null = null;
            if (deps.rustToolBackends.authorityState) {
                try {
                    notesAuthority = await deps.rustToolBackends.authorityState({
                        projectPath: projectIdentity,
                        projectRoot: toolContext.directory,
                        domain: "notes",
                    });
                } catch (error) {
                    return `Error: Rust notes authority is unavailable. ${error instanceof Error ? error.message : String(error)}`;
                }
            }
            if (notesAuthority !== null && notesAuthority !== "MODULE") {
                return noteAuthorityRefusal(args, action);
            }

            const rustNote = deps.rustToolBackends.note;
            if (!rustNote) {
                return "Error: Rust notes authority is active, but this module transport does not support ctx_note.";
            }
            let compilation: Awaited<ReturnType<typeof compileSurfaceCondition>> | undefined;
            // Only a live local evaluator compiles the condition; the daemon's `refuse_conditioned_note_without_evaluator` owns the uncompiled case. commentlint: allow(JUDGE)
            if (
                (action === "write" || action === "update") &&
                surfaceCondition &&
                deps.rustToolBackends.noteEvaluationAvailable?.(projectIdentity) === true
            ) {
                // Resolve relative paths and default repository predicates against the repository root.
                compilation = await compileSurfaceCondition(surfaceCondition, {
                    projectPath: resolveProjectRootDirectory(toolContext.directory),
                });
            }
            const request: RustNoteToolRequest = {
                ...(commandId ? { commandId } : {}),
                sessionId,
                projectRoot: toolContext.directory,
                projectPath: projectIdentity,
                memoryProject: projectIdentity,
                action,
                content: args.content,
                surfaceCondition,
                ...(compilation ? conditionCompileStorageFields(compilation) : {}),
                filter: args.filter,
                limit: pageNumber(args.limit, 1),
                offset: pageNumber(args.offset, 0),
                noteId: args.note_id,
            };
            try {
                const text = moduleNoteText(await rustNote(request), args, action);
                if (text === null) {
                    return "Error: Rust module returned an invalid ctx_note response.";
                }
                if (text.startsWith("Error:")) return text;
                if (wakePlaneActive) {
                    return `${text}\nwake plane active — create a scheduled wake instead; stored as a plain note.`;
                }
                if (compilation) return text + conditionCompileReplySuffix(compilation);
                return text;
            } catch (error) {
                if (isRustAuthorityDrainingError(error)) {
                    return noteAuthorityRefusal(args, action);
                }
                return `Error: Rust module ctx_note failed. ${error instanceof Error ? error.message : String(error)}`;
            }
        },
    });
}

export function createCtxNoteTools(deps: CtxNoteToolDeps): Record<string, ToolDefinition> {
    return {
        ctx_note: createCtxNoteTool(deps),
    };
}

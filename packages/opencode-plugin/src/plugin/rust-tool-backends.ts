import type { RustModeModuleClient } from "../hooks/context/rust-mode-transform";

export type RustAuthorityDomain = "memories" | "notes";
export type RustAuthorityState = "TS" | "PREPARING" | "MODULE" | "DRAINING";

export interface RustNoteToolRequest {
    /** The host assigns this MCP tool-use ID. */
    commandId?: string;
    sessionId: string;
    projectRoot: string;
    projectPath: string;
    memoryProject: string;
    action: "write" | "read" | "update" | "dismiss";
    content?: string;
    surfaceCondition?: string;
    compiledProvider?: string | null;
    compiledConfig?: string | null;
    compiledAt?: number | null;
    compileStatus?: "compiled" | "plain" | "refused";
    filter?: "all" | "active" | "pending" | "ready" | "dismissed";
    limit?: number;
    offset?: number;
    noteId?: number;
    /** The harness's tool-call abort signal; the transport settles an aborted call without waiting for the daemon. */
    signal?: AbortSignal;
}

export function toolCallIdFromContext(context: unknown): string | undefined {
    if (context === null || typeof context !== "object") return undefined;
    const record = context as Record<string, unknown>;
    for (const field of [
        "callID",
        "callId",
        "toolUseId",
        "toolCallId",
        "tool_use_id",
        "tool_call_id",
    ]) {
        const value = record[field];
        if (typeof value === "string" && value.trim()) return value.trim();
    }
    return undefined;
}

export interface RustToolBackends {
    reduce?: (args: {
        sessionId: string;
        projectRoot: string;
        drop: string;
        commandId: string;
        signal?: AbortSignal;
    }) => Promise<unknown>;
    authorityState?: (args: {
        projectPath: string;
        projectRoot: string;
        domain: RustAuthorityDomain;
    }) => Promise<RustAuthorityState | null>;
    note?: (args: RustNoteToolRequest) => Promise<unknown>;
    noteEvaluationAvailable?: (projectPath: string) => boolean;
}

export function createRustToolBackends(moduleClient: RustModeModuleClient): RustToolBackends {
    return {
        reduce: ({ sessionId, projectRoot, drop, commandId, signal }) =>
            moduleClient.call({
                sessionId,
                projectRoot,
                method: "agent_drops.append",
                body: {
                    method: "agent_drops.append",
                    v: 1,
                    session_id: sessionId,
                    drop,
                    command_id: commandId,
                },
                ...(signal ? { signal } : {}),
            }),
        note: ({
            commandId,
            sessionId,
            projectRoot,
            memoryProject,
            action,
            content,
            surfaceCondition,
            compiledProvider,
            compiledConfig,
            compiledAt,
            compileStatus,
            filter,
            limit,
            offset,
            noteId,
            signal,
        }) =>
            moduleClient.call({
                sessionId,
                projectRoot,
                method: "ctx_note",
                body: {
                    name: "ctx_note",
                    arguments: {
                        ...(commandId ? { command_id: commandId } : {}),
                        action,
                        content,
                        memory_project: memoryProject,
                        surface_condition: surfaceCondition,
                        ...(compileStatus
                            ? {
                                  compiled_provider: compiledProvider,
                                  compiled_config: compiledConfig,
                                  compiled_at: compiledAt,
                                  compile_status: compileStatus,
                              }
                            : {}),
                        filter,
                        limit,
                        offset,
                        note_id: noteId,
                    },
                },
                ...(signal ? { signal } : {}),
            }),
        // The daemon's `ctx_note` facade stores the compiled fields, so the compiler runs for every conditioned note. commentlint: allow(JUDGE)
        noteEvaluationAvailable: () => true,
    };
}

export function isRustAuthorityDrainingError(error: unknown): boolean {
    let current = error;
    for (let depth = 0; depth < 3; depth += 1) {
        if (!current || typeof current !== "object") break;
        const record = current as {
            code?: unknown;
            cause?: unknown;
            error?: unknown;
            result?: unknown;
        };
        if (record.code === "authority_draining") return true;
        current = record.cause ?? record.error ?? record.result;
    }
    return error instanceof Error && error.message.includes("authority_draining");
}

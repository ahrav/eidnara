import type { RustModeModuleClient } from "@eidnara/opencode/hooks/context/rust-mode-transform";
import type {
    RustNoteToolRequest,
    RustToolBackends,
} from "@eidnara/opencode/plugin/rust-tool-backends";

/** Pi's daemon tool backends take the project root per invocation because a Pi process has no live-session map: `/cd` and multi-root sessions move the root between calls, and the daemon keys routes and lineage by `(session, root)`, where OpenCode pins the root by session instead. */
export interface PiRustReduceRequest {
    sessionId: string;
    projectRoot: string;
    drop: string;
    commandId: string;
    /** When supplied, the harness abort signal; the transport settles a cancelled call without waiting out its request budget. */
    signal?: AbortSignal;
}

export interface PiRustNoteToolRequest extends RustNoteToolRequest {
    projectRoot: string;
    /** The daemon facade stores the note under this project identity. */
    memoryProject: string;
    signal?: AbortSignal;
}

export type PiRustToolBackends = Pick<
    RustToolBackends,
    "authorityState" | "noteEvaluationAvailable"
> & {
    reduce?: (args: PiRustReduceRequest) => Promise<unknown>;
    note?: (args: PiRustNoteToolRequest) => Promise<unknown>;
};

export function createPiRustToolBackends(moduleClient: RustModeModuleClient): PiRustToolBackends {
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
        // `noteEvaluationAvailable` stays absent: the daemon accepts a conditioned write only while a `note.evaluation.register` heartbeat is live for the project, and no shipped host registers one, so the tool must surface the daemon's refusal instead of compiling the condition.
    };
}

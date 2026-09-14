/**
 *
 * The shared guidance advertises a tool only when Pi registers it.
 * If guidance advertises an unregistered tool, Pi returns "tool not found" when the agent invokes it.
 *
 * `eidnara_note` is omitted for `--no-session` child processes because it resolves to the ephemeral child session.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { resolveProjectIdentityForSession } from "@eidnara/opencode/features/context/project-identity";
import type { KernelClientResolver } from "@eidnara/opencode/shared/kernel-client";
import type { PromptSurfaceConfig } from "@eidnara/opencode/shared/prompt-surface";
import type { PromptSurfaceRuntime } from "@eidnara/opencode/shared/prompt-surface-runtime";
import { createPromptSurfaceRuntime } from "@eidnara/opencode/shared/prompt-surface-runtime";
import type { PiRustToolBackends } from "../rust-tool-backends";
import { createEidnaraMemoryTool } from "./eidnara-memory";
import { createEidnaraNoteTool } from "./eidnara-note";
import { createEidnaraReduceTool } from "./eidnara-reduce";
import { createEidnaraSearchTool } from "./eidnara-search";
import { registerTodosCommand } from "./todo-view-pi";
import { createTodowriteTool } from "./todowrite";

export interface RegisterToolsOptions {
    /** Serves `eidnara_memory` and the `memory` source of `eidnara_search`. */
    kernelClient: KernelClientResolver;
    /** Serves `eidnara_reduce` and `eidnara_note` through the daemon facades. */
    rustToolBackends: PiRustToolBackends;
    /** The resolver uses the user-level home-project setting to resolve the current directory's project identity. */
    resolveProjectIdentity?: (ctx: { cwd: string }) => string | undefined;
    /** `memoryToolEnabled=false` omits `eidnara_memory` from the registered surface.
     * The context_researcher needs read-only `eidnara_search`; the main agent keeps `eidnara_memory`. */
    memoryToolEnabled?: boolean;
    /** `sessionScopedToolsDisabled=true` omits `eidnara_note` from the registered surface.
     * `--no-session` context_researcher children set `sessionScopedToolsDisabled`.
     * In `--no-session` children, `eidnara_note` resolves `ctx.sessionManager.getSessionId()` to the ephemeral child session. */
    sessionScopedToolsDisabled?: boolean;
    /* */
    todowriteEnabled?: boolean;
    /** Main Pi entry registers /todos; lean subagent entries keep commands off. */
    todowriteCommandEnabled?: boolean;
    /** `compactionOff=true` omits `eidnara_reduce` while leaving the other Pi tools available. */
    compactionOff?: boolean;
    promptSurface?: PromptSurfaceConfig;
    promptSurfaceRuntime?: PromptSurfaceRuntime;
}

export function registerEidnaraTools(pi: ExtensionAPI, opts: RegisterToolsOptions): void {
    const resolveProjectPath = opts.resolveProjectIdentity
        ? (directory: string) => opts.resolveProjectIdentity?.({ cwd: directory })
        : (directory: string) => resolveProjectIdentityForSession(directory);
    const promptSurfaceRuntime =
        opts.promptSurfaceRuntime ??
        createPromptSurfaceRuntime({
            userConfigDirectory: process.cwd(),
            warn: (message) => console.warn(`[eidnara][pi] config warning: ${message}`),
        });
    const registration = promptSurfaceRuntime.resolveRegistration(opts.promptSurface);
    const surfaceTool = <T extends { name: string; description: string }>(definition: T): T => ({
        ...definition,
        description: registration.descriptionFor(definition.name, definition.description),
    });

    pi.registerTool(
        surfaceTool(
            createEidnaraSearchTool({ kernelClient: opts.kernelClient, resolveProjectPath }),
        ),
    );

    if (opts.memoryToolEnabled !== false) {
        pi.registerTool(
            surfaceTool(
                createEidnaraMemoryTool({ kernelClient: opts.kernelClient, resolveProjectPath }),
            ),
        );
    }

    // `eidnara_note` resolves the ephemeral child session in `--no-session` children, so omit it to prevent orphaned notes.
    if (!opts.sessionScopedToolsDisabled) {
        pi.registerTool(
            surfaceTool(
                createEidnaraNoteTool({
                    resolveProjectPath,
                    rustToolBackends: opts.rustToolBackends,
                }),
            ),
        );
    }

    if (opts.todowriteEnabled !== false) {
        pi.registerTool(createTodowriteTool());
        if (opts.todowriteCommandEnabled !== false) {
            registerTodosCommand(pi);
        }
    }

    if (!opts.sessionScopedToolsDisabled && !opts.compactionOff) {
        pi.registerTool(
            surfaceTool(createEidnaraReduceTool({ rustToolBackends: opts.rustToolBackends })),
        );
    }
}

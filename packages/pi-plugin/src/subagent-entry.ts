/**
 * `PiSubagentRunner` loads this entry to register subagent tools.
 *
 * `PiSubagentRunner` loads this entry only in child Pi processes.
 * The subagent entry registers only Eidnara tools intended for subagents.
 *
 * `EIDNARA_PI_SUBAGENT=1` makes `./index.ts` return before registering its handlers.
 * Child Pi processes keep extension discovery enabled for provider models and AFT tools.
 * `EIDNARA_PI_SUBAGENT=1` prevents recursion by making the full entry return before registration.
 * The subagent entry is unguarded because child Pi processes must load its scoped tools.
 *
 * The full entry must not load in subagents because its handlers can recursively spawn subagents and alter subagent prompts.
 * The full entry injects key files, project documentation, user profiles, and session history into subagent prompts.
 *
 * `--no-session` children receive `ctx_search`; `todowrite` is enabled unless configuration disables it.
 * `ctx_search` provides read-only search over shared memories, messages, and Git.
 *
 * Hidden child sessions omit `ctx_note` and `ctx_expand` because they have no useful transcript or parent note ID.
 *
 * `PiSubagentRunner` starts child Pi processes with `EIDNARA_PI_SUBAGENT=1` and this entry's `--extension` path.
 *
 * Pi applies the per-agent `--tools` list to the complete registry.
 * OMP applies `--tools` only to built-ins and appends discovered extension tools afterward.
 * OMP's `--tools` list budgets tools but does not sandbox extension tools.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { resolveProjectIdentityForSession } from "@eidnara/opencode/features/context/project-identity";
import {
    configureManagedDemandStart,
    createLazyManagedDemandStart,
} from "@eidnara/opencode/hooks/context/module-transport";
import { setHarness } from "@eidnara/opencode/shared/harness";
import { log } from "@eidnara/opencode/shared/logger";
import { loadPiConfig } from "./config";
import { createPiKernelClientResolver } from "./kernel-client-pi";
import { registerEidnaraTools } from "./tools";

const managedDemandStart = createLazyManagedDemandStart({
    declaringModuleUrl: import.meta.url,
    parentPackageName: "@eidnara/pi",
});

export default function eidnaraSubagentExtension(pi: ExtensionAPI): void {
    configureManagedDemandStart(managedDemandStart);
    // Shared-core session writes tag rows with `harness='pi'`.
    setHarness("pi");

    pi.on("session_start", async () => {
        try {
            const { config: cfg, registrationPromptSurface } = loadPiConfig({
                cwd: process.cwd(),
            });

            registerEidnaraTools(pi, {
                kernelClient: createPiKernelClientResolver(
                    // The resolver loads configuration from `projectRoot`; a `/cd` into another project dials with that project's `memory.enabled` and connection file, not the startup ones. commentlint: allow(JUDGE)
                    (projectRoot) => loadPiConfig({ cwd: projectRoot }).config,
                ),
                rustToolBackends: {},
                resolveProjectIdentity: (ctx) =>
                    resolveProjectIdentityForSession(ctx.cwd, cfg.allow_home_project),
                memoryToolEnabled: false,
                // Hidden child sessions omit `ctx_note` and `ctx_expand` because they have no useful transcript or parent note ID.
                sessionScopedToolsDisabled: true,
                todowriteEnabled: cfg.todowrite.enabled !== false,
                todowriteCommandEnabled: false,
                promptSurface: registrationPromptSurface,
            });

            log(
                `[pi-subagent] registered tools: ctx_search${cfg.todowrite.enabled !== false ? ", todowrite" : ""}` +
                    ` (ctx_note omitted: --no-session child; memory=${cfg.memory.enabled})`,
            );
        } catch (err) {
            const message = err instanceof Error ? err.message : String(err);
            log(`[pi-subagent] startup failed: ${message}`);
            process.exitCode = 1;
            throw err;
        }
    });
}

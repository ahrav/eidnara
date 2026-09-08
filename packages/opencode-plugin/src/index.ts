import path from "node:path";
import type { Hooks, Plugin, PluginModule } from "@opencode-ai/plugin";

import {
    buildHiddenAgentConfig,
    buildHiddenAgentRegistrations,
} from "./agents/hidden-agent-registrations";
import { denyTaskRoutingToCallerAgents } from "./agents/permissions";
import { loadPluginConfigDetailed } from "./config";
import { isCompactionEnabled } from "./config/agent-disable";
import { getEidnaraBuiltinCommands } from "./features/builtin-commands/commands";
import { SIDEKICK_SYSTEM_PROMPT } from "./features/context/sidekick/agent";
import { SMART_NOTE_COMPILER_SYSTEM_PROMPT } from "./features/context/smart-notes/compiler-prompt";
import { createLiveSessionState } from "./hooks/context/live-session-state";
import {
    configureManagedDemandStart,
    createLazyManagedDemandStart,
    HostModuleTransport,
} from "./hooks/context/module-transport";
import { preloadTokenizer } from "./hooks/context/read-session-formatting";
import type { RustModeModuleClient } from "./hooks/context/rust-mode-transform";
import {
    type ConfigWarningDelivery,
    createConfigWarningDelivery,
    formatConfigWarning,
} from "./plugin/config-warning";
import { cleanupConflictWarnings, sendConflictWarning } from "./plugin/conflict-warning-hook";
import { createEventHandler } from "./plugin/event";
import { createSessionHooksAsync } from "./plugin/hooks/create-session-hooks";
import { createMessagesTransformHandler } from "./plugin/messages-transform";
import { registerRpcHandlers } from "./plugin/rpc-handlers";
import { createToolRegistry } from "./plugin/tool-registry";
import {
    type ConflictResult,
    detectConflicts,
    resolveCompactionForBoot,
} from "./shared/conflict-detector";
import { getEidnaraStorageDir } from "./shared/data-path";
import { setKeepSubagents } from "./shared/keep-subagents";
import { log } from "./shared/logger";
import { refreshModelLimitsFromApi } from "./shared/models-dev-cache";
import { createPromptSurfaceRuntime } from "./shared/prompt-surface-runtime";
import { EidnaraRpcServer } from "./shared/rpc-server";

const managedDemandStart = createLazyManagedDemandStart({
    declaringModuleUrl: import.meta.url,
    parentPackageName: "@eidnara/opencode",
});

const server: Plugin = async (ctx) => {
    // Broca child processes must not initialize Eidnara.
    // Do not use the buffered logger: it arms a flush timer and appends to the Eidnara log file.
    if (process.env.EIDNARA_BROCA_CHILD === "1") {
        console.error(
            "[eidnara] broca child detected (EIDNARA_BROCA_CHILD=1); skipping plugin startup",
        );
        return {};
    }
    configureManagedDemandStart(managedDemandStart);
    const loadedPluginConfig = loadPluginConfigDetailed(ctx.directory);
    const pluginConfig = loadedPluginConfig.config;
    const promptSurfaceRuntime = createPromptSurfaceRuntime({
        warn: (message) => log(`[eidnara] config warning: ${message}`),
    });
    setKeepSubagents(pluginConfig.keep_subagents === true);

    let configWarning: ConfigWarningDelivery | null = null;
    if (pluginConfig.configWarnings?.length) {
        for (const w of pluginConfig.configWarnings) {
            log(`[eidnara] config warning: ${w}`);
        }
        configWarning = createConfigWarningDelivery(
            ctx.client,
            formatConfigWarning(pluginConfig.configWarnings),
        );
        // sendIgnoredMessage routes TUI notifications to toasts and Desktop notifications to ignored messages via isTuiConnected().
        // Passing no agent, model, or variant records the ignored message with the defaults.
        // Using the defaults attributes the notice to the default agent rather than the session agent.
        // Using the defaults switches the model on the next user turn and invalidates the prefix cache.
        // `resolvePromptContext` reads real session messages and returns `null` for fresh or empty sessions.
        // A project with no session yet keeps the warning pending; the `chat.message` hook delivers it to the first session the user prompts.
        setTimeout(() => {
            void configWarning?.deliverToFirstSession();
        }, 3000);
    }

    // Pass `resolvedCompaction` explicitly because `detectConflicts` does not re-derive it from config files.
    // When Eidnara compaction is off, native `compaction.auto=true` is not a conflict.
    // When Eidnara compaction is off, native `compaction.auto=true` leaves the plugin enabled.
    //
    // `ctx.client.config.get()` returns the configuration shown by `opencode debug config`.
    // Native compaction state is read from `ctx.client.config.get()`, not re-derived from config files.
    // File-based re-derivation defaults `compaction.auto` to `true` when no configuration file resolves.
    // File-based detection can disable the plugin when `auto=false` is defined in an unresolved configuration layer.
    // If the resolved-config fetch fails or times out, conflict detection uses the file-based check.
    let conflictResult: ConflictResult | null = null;
    if (pluginConfig.enabled) {
        const resolvedCompaction = await resolveCompactionForBoot(ctx.client);
        if (resolvedCompaction === null) {
            log(
                "[eidnara] resolved-config fetch failed; using file-based compaction detection (the running server's resolved config may differ — `opencode debug config` is authoritative)",
            );
        }
        conflictResult = detectConflicts(ctx.directory, {
            compactionEnabled: isCompactionEnabled(pluginConfig),
            resolvedCompaction: resolvedCompaction ?? undefined,
        });
        if (conflictResult.hasConflict) {
            pluginConfig.enabled = false;
            log(`[eidnara] disabled due to conflicts: ${conflictResult.reasons.join("; ")}`);
        } else {
            log("[eidnara] no conflicts detected, plugin enabled");
        }
    }

    const liveSessionState = createLiveSessionState();
    const rustModeModuleClient: RustModeModuleClient | undefined =
        pluginConfig.transform_mode === "rust"
            ? new HostModuleTransport(pluginConfig.subc?.connection_file)
            : undefined;

    const hooks = await createSessionHooksAsync({
        ctx,
        pluginConfig,
        liveSessionState,
        rustModeModuleClient,
        promptSurfaceRuntime,
    });
    const eidnara = hooks.eidnara;

    const tools = createToolRegistry({
        pluginConfig,
        rustToolBackends: hooks.rustToolBackends ?? {},
        resolveSessionDirectory: hooks.resolveSessionDirectory,
        promptSurfaceRuntime,
        registrationPromptSurface: loadedPluginConfig.registrationPromptSurface,
    });

    // The function-scope handle lets the `server.instance.disposed` cleanup handler stop the server.
    let rpcServer: EidnaraRpcServer | null = null;

    // A null hook means the directory has no project identity (a home directory without `allow_home_project`); the RPC handlers read kernel memory for their directory, so they honor the same refusal.
    if (pluginConfig.enabled && eidnara) {
        // RPC communication between the TUI and server bypasses the SQLite plugin_messages bus.
        rpcServer = new EidnaraRpcServer(getEidnaraStorageDir(), ctx.directory);
        registerRpcHandlers(rpcServer, {
            directory: ctx.directory,
            config: pluginConfig,
            client: ctx.client,
            liveSessionState,
            rustModeModuleClient,
            nativeCompaction: conflictResult?.nativeCompaction,
        });
        rpcServer.start().catch((err) => {
            log(`[eidnara] RPC server failed to start: ${err}`);
        });

        // Startup warms the model-context-limit cache from OpenCode's SDK once.
        // The resolver refreshes model context limits from the OpenCode API.
        // Until refresh completes, resolution uses the persisted last-known-good limit, then 128k.
        //
        // Startup retries up to 3 times when OpenCode's provider service is unavailable.
        // The refresh runs fire-and-forget so it never blocks plugin initialization.
        //
        // Do NOT refresh periodically. A later refresh can lower a limit during an active session.
        void refreshModelLimitsFromApi(ctx.client, { retries: 3, retryDelayMs: 1000 });
    }

    // Desktop has no dialog surface, so `sendConflictWarning` covers Desktop.
    if (conflictResult?.hasConflict) {
        // The handler sends the warning to the project's last active session without awaiting it.
        void sendConflictWarning(
            ctx.client as unknown as Record<string, unknown>,
            ctx.directory,
            conflictResult,
        );
    } else if (pluginConfig.enabled) {
        // The handler removes leftover conflict warnings only when no conflict exists and pluginConfig.enabled.
        const serverUrl = (ctx as Record<string, unknown>).serverUrl;
        const serverUrlStr =
            serverUrl instanceof URL ? serverUrl.toString().replace(/\/$/, "") : undefined;
        void cleanupConflictWarnings(
            ctx.client as unknown as Record<string, unknown>,
            ctx.directory,
            serverUrlStr,
        );
    }

    // Only the setup wizard and `doctor` add the TUI sidebar entry; startup must not restore an entry the user removed.

    // Disposal matches `ownInstanceDirectory`, not a shared project identity.
    const ownInstanceDirectory = ctx.directory;

    return {
        tool: tools,
        event: createEventHandler({
            eidnara: {
                event: async (input) => {
                    await eidnara?.event?.(input);
                },
            },
            // `onInstanceDisposed` cleans up only this instance's process-resident resources.
            onInstanceDisposed: (disposedDirectory: string) => {
                if (path.resolve(disposedDirectory) !== path.resolve(ownInstanceDirectory)) return;
                try {
                    rpcServer?.stop();
                } catch {
                    // best-effort
                }
                log("[eidnara] instance disposed — stopped RPC server");
            },
        }),
        "experimental.chat.messages.transform": createMessagesTransformHandler({
            eidnara,
            getEidnara: () => eidnara,
            transformMode: pluginConfig.transform_mode,
            // SAFETY: wrapper matches the hook's runtime call shape.
        }) as unknown as NonNullable<Hooks["experimental.chat.messages.transform"]>,
        "experimental.chat.system.transform": async (input, output) => {
            await eidnara?.["experimental.chat.system.transform"]?.(input, output);
        },
        "command.execute.before": async (input, output) => {
            await eidnara?.["command.execute.before"]?.(input, output);
        },
        "chat.message": async (input, _output) => {
            // The first prompt awaits `preloadTokenizer()` so later synchronous estimates use the installed package.
            await preloadTokenizer();
            // Fire-and-forget: a pending delivery must not delay the user's prompt.
            if (configWarning?.pending && input.sessionID) {
                void configWarning.deliverTo(input.sessionID);
            }
            await eidnara?.["chat.message"]?.(input);
        },
        "tool.execute.after": async (input, _output) => {
            await eidnara?.["tool.execute.after"]?.(input);
        },
        "experimental.text.complete": async (input, output) => {
            await eidnara?.["experimental.text.complete"]?.(input, output);
        },
        config: async (config) => {
            try {
                if (pluginConfig.enabled !== true) {
                    return;
                }
                const commandConfig = {
                    ...(config.command ?? {}),
                    ...getEidnaraBuiltinCommands(isCompactionEnabled(pluginConfig)),
                    ...(pluginConfig.command ?? {}),
                };

                config.command = commandConfig;
                // Hidden-agent overrides remove `thinking_level` because OpenCode does not accept it as an agent config field.
                const sidekickAgentOverrides = pluginConfig.sidekick
                    ? (() => {
                          const {
                              timeout_ms: _timeoutMs,
                              system_prompt: _systemPrompt,
                              thinking_level: _thinkingLevel,
                              ...agentOverrides
                          } = pluginConfig.sidekick;
                          return agentOverrides;
                      })()
                    : undefined;
                const registrations = buildHiddenAgentRegistrations({
                    smartNoteCompilerPrompt: SMART_NOTE_COMPILER_SYSTEM_PROMPT,
                    sidekickPrompt: SIDEKICK_SYSTEM_PROMPT,
                    sidekickOverrides: sidekickAgentOverrides,
                });

                const agentConfig = { ...(config.agent ?? {}) } as NonNullable<typeof config.agent>;
                const agentConfigRecord = agentConfig as Record<string, Record<string, unknown>>;
                const internalAgentIds = registrations.map((registration) => registration.id);
                for (const reg of registrations) {
                    if (typeof reg.prompt !== "string" || reg.prompt.length === 0) {
                        log(
                            `[eidnara] skipping hidden agent '${reg.id}' — prompt unavailable at config time (dir=${ctx.directory}); will re-register on a later complete pass`,
                        );
                        continue;
                    }
                    agentConfigRecord[reg.id] = buildHiddenAgentConfig(
                        reg.prompt,
                        reg.allowedTools,
                        reg.maxSteps,
                        reg.overrides,
                        reg.id,
                        reg.lockPermissions === true,
                        reg.description,
                    );
                }
                const callerAgentConfig = denyTaskRoutingToCallerAgents(
                    agentConfigRecord,
                    internalAgentIds,
                );
                config.agent = callerAgentConfig as NonNullable<typeof config.agent>;
            } catch (error) {
                // Command and agent registration failures must not prevent plugin loading.
                // Registration failures retain the previous agent configuration.
                const e = error as { message?: string; stack?: string };
                log(
                    `[eidnara] config hook failed (commands/agents NOT registered; transform still active): ${e?.message ?? error}`,
                    e?.stack
                        ? { stackHead: e.stack.split("\n").slice(0, 6).join("\n") }
                        : undefined,
                );
            }
        },
    };
};

const plugin: PluginModule = {
    id: "eidnara-opencode",
    server,
};

export default plugin;

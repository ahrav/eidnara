/**
 * Pi loads this extension once per session from `pi.extensions`.
 *
 * Session state lives in the Rust daemon; this entry reaches it through `HostModuleTransport`.
 * The extension retains only per-session UI state in process memory: the task-list snapshot and system prompt hash.
 *
 * `loadPiConfig()` reads project config from `$cwd/.eidnara/eidnara.jsonc` and user config from `~/.config/eidnara/eidnara.jsonc`.
 * `loadPiConfig()` uses schema defaults when neither config file exists.
 */

import { resolve } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { isCompactionEnabled } from "@eidnara/opencode/config/agent-disable";
import type { EidnaraConfig, SidekickConfig } from "@eidnara/opencode/config/schema/eidnara";
import { resolveProjectIdentityForSession } from "@eidnara/opencode/features/context/project-identity";
import { setCtxReduceRegisteredGlobally } from "@eidnara/opencode/hooks/context/ctx-reduce-availability";
import { closeKernelSession } from "@eidnara/opencode/hooks/context/kernel-transport";
import {
    configureManagedDemandStart,
    createHostModuleClient,
    createLazyManagedDemandStart,
} from "@eidnara/opencode/hooks/context/module-transport";
import type { RustModeModuleClient } from "@eidnara/opencode/hooks/context/rust-mode-transform";
import { normalizeTodoStateJson } from "@eidnara/opencode/hooks/context/todo-view";
import { createRustToolBackends } from "@eidnara/opencode/plugin/rust-tool-backends";
import { setHarness } from "@eidnara/opencode/shared/harness";
import { piModelRefToCanonical } from "@eidnara/opencode/shared/harness-provider-map";
import { log } from "@eidnara/opencode/shared/logger";
import {
    createPromptSurfaceGuidanceEpochCache,
    createPromptSurfaceRuntime,
} from "@eidnara/opencode/shared/prompt-surface-runtime";
import { resolveFallbackChain } from "@eidnara/opencode/shared/resolve-fallbacks";

import { handlePiCloneSessionStart } from "./clone-inheritance";
import { type PiSidekickConfig, registerCtxAugCommand } from "./commands/ctx-aug";
import { registerCtxFlushCommand } from "./commands/ctx-flush";
import { registerCtxRecompCommand } from "./commands/ctx-recomp";
import { registerCtxStatusCommand } from "./commands/ctx-status";
import { registerCtxWrapupCommand } from "./commands/ctx-wrapup";
import type { DaemonSessionDeps } from "./commands/daemon-session-routes";
import { registerCtxStatusEntryRenderer } from "./commands/pi-command-utils";
import { loadPiConfig } from "./config";
import { createPiKernelClientResolver } from "./kernel-client-pi";
import { registerStatusLine } from "./status-line";
import { stripTagPrefixFromAssistantMessage } from "./strip-tag-prefix";
import { configurePiSubagentExtensions, EIDNARA_PI_SUBAGENT_ENV } from "./subagent-runner";
import { clearPiSystemPromptSession, processSystemPromptForCache } from "./system-prompt";
import { registerEidnaraTools } from "./tools";
import {
    parseTodos,
    registerTodoOverlay,
    registerTodoStateLifecycle,
    rememberTodowriteToolCallTodos,
    setTodoSnapshot,
} from "./tools/todo-view-pi";

const PREFIX = "[eidnara][pi]";
const managedDemandStart = createLazyManagedDemandStart({
    declaringModuleUrl: import.meta.url,
    parentPackageName: "@eidnara/pi",
});

// ---------------------------------------------------------------------------
//
// `@gotgenes/pi-subagents` creates child sessions in the parent process, so a process-global latch prevents duplicate initialization.
// `EIDNARA_PI_SUBAGENT=1` excludes spawned subagents but not in-process children, which inherit the parent environment.
//
// The latch uses `Symbol.for` on `globalThis` so duplicate module instances share it.
// Later initializations register no watchers, timers, or background scans.
// The parent's already-registered extension instance keeps serving its session.
//
// Pi clears the parent's latch on `session_shutdown` so `/reload` can initialize a new extension.
// Child `session_shutdown` events cannot invoke the parent's handlers.
// A no-op child registers no shutdown handler and cannot clear the parent's latch.
// ---------------------------------------------------------------------------
const PI_ACTIVE_LATCH = Symbol.for("eidnara.pi.active");

function isPiEidnaraActiveInProcess(): boolean {
    return (globalThis as Record<symbol, unknown>)[PI_ACTIVE_LATCH] === true;
}

function markPiEidnaraActive(): void {
    (globalThis as Record<symbol, unknown>)[PI_ACTIVE_LATCH] = true;
}

function clearPiEidnaraActive(): void {
    try {
        delete (globalThis as Record<symbol, unknown>)[PI_ACTIVE_LATCH];
    } catch {
        // Some runtimes disallow delete on globalThis; fall back to overwrite.
        (globalThis as Record<symbol, unknown>)[PI_ACTIVE_LATCH] = undefined;
    }
}

/**
 * In normal mode, Eidnara cancels Pi's event because Eidnara owns compaction.
 * Compaction-off mode lets Pi's native compaction proceed.
 */
export async function handlePiSessionBeforeCompact(args: {
    compactionOff: boolean;
    ctx: { sessionManager?: { getSessionId?: () => string | undefined } };
}): Promise<{ cancel: true } | undefined> {
    if (args.compactionOff) {
        info("session_before_compact: native Pi compaction proceeds (compaction-off mode)");
        return;
    }
    info("session_before_compact: cancelling — eidnara owns compaction");
    return { cancel: true };
}

export function canonicalPiModelKey(provider: string, model: string): string {
    return piModelRefToCanonical(`${provider}/${model}`);
}

type TodoOverlayUpdater = { update: (sessionId?: string) => void };

type CompatiblePiTodoCapture = {
    todos: Exclude<ReturnType<typeof parseTodos>, null>;
};

function getCompatiblePiTodoCapture(todos: unknown): CompatiblePiTodoCapture | null {
    if (!Array.isArray(todos)) return null;
    if (normalizeTodoStateJson(todos) === null) return null;
    const parsed = parseTodos(todos);
    if (parsed === null) return null;
    return { todos: parsed };
}

function applyCompatiblePiTodoCapture(args: {
    sessionId: string;
    todowriteEnabled: boolean;
    todoOverlay?: TodoOverlayUpdater;
    toolCallId?: string;
    capture: CompatiblePiTodoCapture;
}): void {
    rememberTodowriteToolCallTodos(args.toolCallId, args.capture.todos);
    if (args.todowriteEnabled) {
        setTodoSnapshot(args.sessionId, args.capture.todos);
        args.todoOverlay?.update(args.sessionId);
    }
}

/**
 * Capture `todowrite` payloads only when they match Eidnara's task-list enum contract.
 * Third-party Pi extensions can reuse the `todowrite` tool name.
 * Incompatible `todowrite` payloads must not update the transcript render cache.
 */
export function capturePiTodowriteArgsIfCompatible(args: {
    sessionId: string;
    todos: unknown;
    todowriteEnabled: boolean;
    todoOverlay?: TodoOverlayUpdater;
    toolCallId?: string;
}): boolean {
    const capture = getCompatiblePiTodoCapture(args.todos);
    if (capture === null) return false;
    applyCompatiblePiTodoCapture({ ...args, capture });
    return true;
}

/**
 * Capture only the first compatible `todowrite` call from an assistant `message_end` payload.
 * Accepting compatible payloads preserves interoperation with third-party tools that share the `todowrite` name.
 */
export function capturePiTodowriteMessageIfCompatible(args: {
    sessionId: string;
    message: unknown;
    todowriteEnabled: boolean;
    todoOverlay?: TodoOverlayUpdater;
}): boolean {
    const msg = args.message as { role?: unknown; content?: unknown } | undefined;
    if (msg?.role !== "assistant" || !Array.isArray(msg.content)) {
        return false;
    }

    for (const block of msg.content) {
        if (!block || typeof block !== "object") continue;
        const b = block as {
            type?: unknown;
            name?: unknown;
            arguments?: unknown;
        };
        if (b.type !== "toolCall") continue;
        if (typeof b.name !== "string") continue;
        if (b.name !== "todowrite") continue;
        const capture = getCompatiblePiTodoCapture(
            (b.arguments as { todos?: unknown } | null | undefined)?.todos,
        );
        if (capture === null) continue;
        applyCompatiblePiTodoCapture({ ...args, capture });
        return true;
    }

    return false;
}

function info(message: string, data?: unknown): void {
    log(`${PREFIX} ${message}`, data);
}

function warn(message: string, data?: unknown): void {
    log(`${PREFIX} WARN ${message}`, data);
}

// Deduplicate config summaries and warnings by resolved directory when `args.dedupe` is true.
const loggedPiConfigDirs = new Set<string>();

function logPiConfigLoad(args: {
    dir: string;
    loadedFromPaths: string[];
    warnings: string[];
    dedupe?: boolean;
}): void {
    const key = resolve(args.dir);
    if (args.dedupe && loggedPiConfigDirs.has(key)) return;
    if (args.dedupe) {
        loggedPiConfigDirs.add(key);
    }
    if (args.loadedFromPaths.length > 0) {
        info(`config loaded from: ${args.loadedFromPaths.join(", ")}`);
    } else {
        info("config: no eidnara.jsonc found, using schema defaults");
    }
    for (const warning of args.warnings) {
        warn(`config: ${warning}`);
    }
}

export const __test = {
    logPiConfigLoad,
    resetLoggedPiConfigDirs(): void {
        loggedPiConfigDirs.clear();
    },
    isPiEidnaraActiveInProcess,
    markPiEidnaraActive,
    clearPiEidnaraActive,
};

setHarness("pi");

// ---------------------------------------------------------------------------
// Config-driven resolvers
//
// Each resolver returns `undefined` when its feature is disabled, allowing registration helpers to short-circuit.
// ---------------------------------------------------------------------------

export function resolveSidekickFromConfig(config: EidnaraConfig): PiSidekickConfig | undefined {
    const sidekick = config.sidekick as SidekickConfig | undefined;
    if (!sidekick || sidekick.disable === true) return undefined;
    const model = sidekick.model?.trim();
    if (!model || model.length === 0) return undefined;
    return {
        model,
        systemPrompt: sidekick.system_prompt,
        timeoutMs: sidekick.timeout_ms,
        thinking_level: sidekick.thinking_level,
        fallbackModels: resolveFallbackChain(sidekick.fallback_models),
        language: config.language,
        allowHomeProject: config.allow_home_project,
    };
}

export default async function (pi: ExtensionAPI): Promise<void> {
    if (process.env[EIDNARA_PI_SUBAGENT_ENV] === "1") {
        log(
            `${PREFIX} subagent child detected (${EIDNARA_PI_SUBAGENT_ENV}=1); skipping full extension registration`,
        );
        return;
    }
    // `@gotgenes/pi-subagents` runs child agent sessions in the parent process.
    // Child sessions inherit the parent's environment and re-run this factory, so the spawned-child guard does not run.
    // The process-global latch records that the full Eidnara runtime is active in this process.
    // A second initialization starts no watchers, timers, or background scans.
    // Disposal and `/reload` re-arm the latch for a later registration.
    if (isPiEidnaraActiveInProcess()) {
        log(
            `${PREFIX} in-process re-init detected (Eidnara already active in this process); skipping full extension registration`,
        );
        return;
    }
    configureManagedDemandStart(managedDemandStart);
    markPiEidnaraActive();

    // Only a registered runtime installs the `session_shutdown` handler that clears the latch.
    // A disabled or failed startup therefore clears it here, or `/reload` could never initialize a later enabled configuration.
    let registered = false;
    try {
        registered = await startPiEidnaraRuntime(pi);
    } finally {
        if (!registered) clearPiEidnaraActive();
    }
}

/** Returns `true` once every hook, tool, and command is registered; `false` when configuration disables the runtime. */
async function startPiEidnaraRuntime(pi: ExtensionAPI): Promise<boolean> {
    // The boot project affects only initial config loading and logging.
    // Identity and path resolution use `ctx.cwd` for each hook and command, so cwd switches follow the active project without reloading config.
    const projectDir = process.cwd();
    // Invalid config fields use defaults per key.
    //
    // `warn()` surfaces invalid-config warnings to users.
    const { config, warnings, loadedFromPaths, registrationPromptSurface } = loadPiConfig({
        cwd: projectDir,
    });
    const promptSurfaceRuntime = createPromptSurfaceRuntime({
        warn: (message) => warn(`config: ${message}`),
    });
    const promptSurfaceGuidanceEpochs = createPromptSurfaceGuidanceEpochCache(promptSurfaceRuntime);
    const projectIdentity =
        resolveProjectIdentityForSession(projectDir, config.allow_home_project) ?? "";
    info(`loaded | harness=pi | project=${projectIdentity} | dir=${projectDir}`);
    // Pi registers tools once per process, so compaction registration does not follow later /cd config changes.
    const compactionOff = !isCompactionEnabled(config);
    setCtxReduceRegisteredGlobally(!compactionOff);
    // Pi configures child-runner extensions once at boot because the allowlist is user-tier only.
    // The returned merged config strips project-level subagent extension settings.
    configurePiSubagentExtensions(config.pi?.subagent_extensions);
    logPiConfigLoad({
        dir: projectDir,
        loadedFromPaths,
        warnings,
        dedupe: true,
    });

    if (!config.enabled) {
        info("plugin DISABLED via config (enabled: false) — skipping registration");
        return false;
    }

    // The connection file is user-tier configuration, so one daemon client serves every project in this process.
    const moduleClient: RustModeModuleClient = createHostModuleClient(config.subc?.connection_file);
    const rustToolBackends = createRustToolBackends(moduleClient);
    // Each command routes on its own `ctx.cwd`, so the deps carry no project root.
    const daemonSessionDeps: DaemonSessionDeps = {
        moduleClient,
        compactionOff,
    };

    type ResolvedPiProjectDeps = {
        projectDir: string;
        projectIdentity: string;
        config: EidnaraConfig;
        sidekickConfig: PiSidekickConfig | undefined;
    };

    // Pi resolves runtime dependencies per cwd because /cd and multi-root sessions can switch projects while registrations remain process-wide.
    // The memoized project-dependency accessor resolves project-sensitive configuration for the active cwd.
    const projectDepsByDir = new Map<string, ResolvedPiProjectDeps>();

    // One resolver serves every memory surface; it reads the configuration of the
    // project root it is asked about, so a `/cd` into another project dials with
    // that project's settings.
    const kernelClient = createPiKernelClientResolver(
        (projectRoot) => resolveProjectDepsForDir(projectRoot).config,
    );

    function buildProjectDeps(
        dir: string,
        identity: string,
        cfg: EidnaraConfig,
    ): ResolvedPiProjectDeps {
        return {
            projectDir: dir,
            projectIdentity: identity,
            config: cfg,
            sidekickConfig: resolveSidekickFromConfig(cfg),
        };
    }

    function resolveProjectDepsForDir(dir: string): ResolvedPiProjectDeps {
        const cached = projectDepsByDir.get(dir);
        if (cached) return cached;
        const switchedLoad = loadPiConfig({ cwd: dir });
        logPiConfigLoad({
            dir,
            loadedFromPaths: switchedLoad.loadedFromPaths,
            warnings: switchedLoad.warnings,
            dedupe: true,
        });
        const switchedConfig = switchedLoad.config;
        const switchedIdentity =
            resolveProjectIdentityForSession(dir, switchedConfig.allow_home_project) ?? "";
        const built = buildProjectDeps(dir, switchedIdentity, switchedConfig);
        projectDepsByDir.set(dir, built);
        return built;
    }

    function resolveCurrentProjectDeps(ctx: { cwd: string }): ResolvedPiProjectDeps {
        return resolveProjectDepsForDir(ctx.cwd);
    }

    const bootProjectDeps = buildProjectDeps(projectDir, projectIdentity, config);
    projectDepsByDir.set(projectDir, bootProjectDeps);
    const todowriteEnabled = bootProjectDeps.config.todowrite.enabled !== false;
    const todowriteOverlayEnabled =
        todowriteEnabled && bootProjectDeps.config.todowrite.overlay !== false;

    // Pi registers tools and slash commands once per process.
    registerEidnaraTools(pi, {
        kernelClient,
        rustToolBackends,
        // Pi registers tools once per process even though /cd can change projects.
        memoryToolEnabled: true,
        resolveProjectIdentity: (ctx) => resolveCurrentProjectDeps(ctx).projectIdentity,
        sessionScopedToolsDisabled: false,
        todowriteEnabled,
        todowriteCommandEnabled: true,
        compactionOff,
        promptSurface: registrationPromptSurface,
        promptSurfaceRuntime,
    });
    info(
        compactionOff
            ? todowriteEnabled
                ? "registered tools: ctx_search, ctx_memory, ctx_note, todowrite; registered /todos (ctx_reduce unavailable in compaction-off mode)"
                : "registered tools: ctx_search, ctx_memory, ctx_note (ctx_reduce unavailable in compaction-off mode; todowrite disabled)"
            : todowriteEnabled
              ? "registered tools: ctx_search, ctx_memory, ctx_note, todowrite, ctx_reduce; registered /todos"
              : "registered tools: ctx_search, ctx_memory, ctx_note, ctx_reduce (todowrite disabled)",
    );

    pi.on("session_start", async (event, ctx) => {
        await handlePiCloneSessionStart(event, ctx, {});
    });

    if (todowriteEnabled) {
        registerTodoStateLifecycle(pi);
    }
    const todoOverlay = todowriteOverlayEnabled ? registerTodoOverlay(pi) : undefined;
    info(
        todowriteOverlayEnabled
            ? "registered todowrite overlay"
            : "registered todowrite overlay: DISABLED (todowrite.enabled=false or todowrite.overlay=false)",
    );

    registerCtxAugCommand(pi, (ctx) => resolveCurrentProjectDeps(ctx).sidekickConfig);
    info(
        bootProjectDeps.sidekickConfig
            ? `registered /ctx-aug (sidekick model=${bootProjectDeps.sidekickConfig.model})`
            : "registered /ctx-aug (sidekick disabled — set sidekick.disable=false and sidekick.model in config)",
    );

    const statusEntryRendererAvailable = registerCtxStatusEntryRenderer(pi);
    info(
        statusEntryRendererAvailable
            ? "registered model-invisible ctx-status entry renderer"
            : "ctx-status entry renderer unavailable; using visible-message fallback",
    );

    registerCtxStatusCommand(pi, {
        ...daemonSessionDeps,
        kernelClient,
        resolveProjectSettings: (ctx) => {
            const { projectIdentity: identity, config: cfg } = resolveCurrentProjectDeps(ctx);
            return {
                projectIdentity: identity,
                protectedTags: cfg.protected_tags,
                executeThresholdPercentage: cfg.execute_threshold_percentage,
                historyBudgetPercentage: cfg.history_budget_percentage,
                executeThresholdTokens: cfg.execute_threshold_tokens,
            };
        },
    });
    info("registered /ctx-status");
    registerStatusLine(pi, { projectIdentity });
    info("registered eidnara status line");

    registerCtxFlushCommand(pi, daemonSessionDeps);
    info("registered /ctx-flush");

    registerCtxRecompCommand(pi, daemonSessionDeps);
    info("registered /ctx-recomp");

    registerCtxWrapupCommand(pi, daemonSessionDeps);
    info("registered /ctx-wrapup");

    const systemPromptRefreshSessions = new Set<string>();

    pi.on("before_agent_start", async (event, ctx) => {
        try {
            const effectiveProjectDeps = resolveCurrentProjectDeps(ctx);
            const effectiveConfig = effectiveProjectDeps.config;

            // `sessionManager.getSessionId()` is available only after Pi creates a session.
            // `before_agent_start` resolves the session ID because it fires once per agent turn.
            const sm = ctx.sessionManager;
            let sessionId: string | undefined;
            if (sm !== undefined) {
                const getId = (sm as { getSessionId?: () => string | undefined }).getSessionId;
                if (typeof getId === "function") {
                    try {
                        const id = getId.call(sm);
                        if (typeof id === "string" && id.length > 0) sessionId = id;
                    } catch {}
                }
            }

            // The handler resolves `effectiveConfig` from the current checkout after a project switch.
            // A switched-into project may contain `.eidnara/eidnara.jsonc`.
            if (effectiveConfig.system_prompt_injection?.enabled === false) {
                return;
            }
            const skipSigs = effectiveConfig.system_prompt_injection?.skip_signatures ?? [];
            if (skipSigs.some((sig) => sig.length > 0 && event.systemPrompt.includes(sig))) {
                return;
            }

            // Without `sessionId`, the handler skips cache processing; a later turn with `sessionId` initializes the hash and sticky date.
            if (!sessionId) return;

            const isCacheBusting = systemPromptRefreshSessions.has(sessionId);

            const promptSurfaceModel = (ctx as { model?: { provider?: unknown; id?: unknown } })
                .model;
            const promptSurfaceModelKey =
                typeof promptSurfaceModel?.provider === "string" &&
                promptSurfaceModel.provider.length > 0 &&
                typeof promptSurfaceModel.id === "string" &&
                promptSurfaceModel.id.length > 0
                    ? canonicalPiModelKey(promptSurfaceModel.provider, promptSurfaceModel.id)
                    : undefined;
            const promptSurface = promptSurfaceGuidanceEpochs.resolve(
                sessionId,
                effectiveConfig.prompt_surface,
                promptSurfaceModelKey,
            );

            const result = processSystemPromptForCache({
                sessionId,
                systemPrompt: event.systemPrompt,
                isCacheBusting,
                promptSurfacePreset: promptSurface.preset,
            });

            if (result.hashChanged) {
                systemPromptRefreshSessions.add(sessionId);
            }

            // The handler clears the refresh signal only when the pass-start `isCacheBusting` was true.
            // Using the pass-start value preserves a signal raised during the pass (`result.hashChanged`) for the next prompt.
            if (isCacheBusting) {
                systemPromptRefreshSessions.delete(sessionId);
            }

            return { systemPrompt: result.systemPrompt };
        } catch (error) {
            warn("failed to process system prompt:", error);
            return;
        }
    });
    info("registered before_agent_start system prompt handler");

    pi.on("agent_end", () => {
        log("agent_end: returning synchronously (background work continues)");
    });

    // `tool_execution_start` exposes `event.args` before tool output.
    pi.on("tool_execution_start", async (event, ctx) => {
        try {
            const sessionId = ctx.sessionManager.getSessionId();
            if (event.toolName === "todowrite") {
                const todoArgs = event.args as { todos?: unknown } | undefined;
                const toolCallId =
                    typeof (event as { toolCallId?: unknown }).toolCallId === "string"
                        ? (event as { toolCallId: string }).toolCallId
                        : undefined;
                capturePiTodowriteArgsIfCompatible({
                    sessionId,
                    todos: todoArgs?.todos,
                    todowriteEnabled,
                    todoOverlay,
                    toolCallId,
                });
            }
        } catch (err) {
            // The `tool_execution_start` handler ignores failures to avoid interrupting tool execution.
            log(
                `tool_execution_start hook failed (continuing): ${err instanceof Error ? err.message : String(err)}`,
            );
        }
    });

    pi.on("session_before_compact", async (_event, ctx) =>
        handlePiSessionBeforeCompact({ compactionOff, ctx }),
    );

    // Mutating `event.message` changes the message persisted by `sessionManager.appendMessage`.
    // Unstripped prefixes appear in assistant responses in Pi's UI.
    pi.on("message_end", async (event, ctx) => {
        try {
            const msg = event.message as unknown;
            if (!compactionOff && msg !== null && typeof msg === "object") {
                stripTagPrefixFromAssistantMessage(msg as { role: string; content: unknown });
            }
        } catch (err) {
            warn("message_end: stripTagPrefixFromAssistantMessage threw:", err);
        }

        // `tool_execution_start` fires only for tools registered with Pi.
        // Reading the assistant message at message_end also captures unregistered todowrite-shaped tools.
        try {
            const sm = ctx.sessionManager as
                | { getSessionId?: () => string | undefined }
                | undefined;
            const sessionId = sm?.getSessionId?.();
            if (typeof sessionId !== "string" || sessionId.length === 0) return;
            capturePiTodowriteMessageIfCompatible({
                sessionId,
                message: event.message,
                todowriteEnabled,
                todoOverlay,
            });
        } catch (err) {
            warn("message_end: synthetic todowrite capture failed:", err);
        }
    });

    function sessionIdFromContext(ctx: unknown): string | undefined {
        const sm = (
            ctx as {
                sessionManager?: { getSessionId?: () => string | undefined };
            }
        ).sessionManager;
        const sessionId = typeof sm?.getSessionId === "function" ? sm.getSessionId() : undefined;
        return typeof sessionId === "string" && sessionId.length > 0 ? sessionId : undefined;
    }

    // Clears one session's prompt state and closes its routes on both daemon transports; a closed route reopens on the session's next call, so no durable state is lost. commentlint: allow(JUDGE)
    function releaseSessionResources(sessionId: string): void {
        clearPiSystemPromptSession(sessionId);
        promptSurfaceGuidanceEpochs.clear(sessionId);
        systemPromptRefreshSessions.delete(sessionId);
        moduleClient.closeSession?.(sessionId);
        closeKernelSession(sessionId);
    }

    // `/reload` tears down extensions and re-runs the default export.
    pi.on("session_shutdown", async (_event, ctx) => {
        // Long-lived Pi processes can reinitialize the extension after `session_shutdown`, so the handler clears per-session state.
        try {
            const sessionId = sessionIdFromContext(ctx);
            if (sessionId) releaseSessionResources(sessionId);
        } catch {
            // best-effort cleanup
        }
        // `session_shutdown` with reason `reload` fires before `/reload` re-imports the extension.
        clearPiEidnaraActive();
    });

    // Each session swap releases the outgoing session's prompt state and daemon routes, or the process retains them for its lifetime.
    pi.on("session_before_switch", (_event, ctx) => {
        try {
            const outgoingSessionId = sessionIdFromContext(ctx);
            if (outgoingSessionId) releaseSessionResources(outgoingSessionId);
        } catch {}
    });
    return true;
}

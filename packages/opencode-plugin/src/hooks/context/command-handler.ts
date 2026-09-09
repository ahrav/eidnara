import { randomUUID } from "node:crypto";
import { COMPACTION_ENABLED_PATH } from "../../config/agent-disable";
import type { SidekickConfig } from "../../config/schema/eidnara";
import { runSidekick } from "../../features/context/sidekick/agent";
import type { PluginContext } from "../../plugin/types";
import { sessionLog } from "../../shared";
import type { KernelClientResolver } from "../../shared/kernel-client";
import {
    formatMemoryMarkOutcome,
    MEMORY_MARK_COMMAND,
    parseMemoryMarkArgs,
    runMemoryMarkCommand,
} from "../../shared/memory-mark-command";
import { isTuiConnected, pushNotification } from "../../shared/rpc-notifications";
import {
    formatTailHygiene,
    resolveTailHygieneStatus,
    type WireTailHygieneBaseline,
} from "../../shared/tail-hygiene-status";
import { formatWindowDerivationLine } from "../../shared/window-geometry";
import { TimeoutError } from "../../shared/with-timeout";
import { resolveContextWindowGeometry } from "./event-resolvers";
import { MAX_WRAPUP_REQUEST_BUDGET_MS } from "./module-transport";
import type { RustModeModuleClient } from "./rust-mode-transform";
import type { NotificationParams } from "./send-session-notification";
import { sendUserPrompt } from "./send-session-notification";

export interface PartialRecompRange {
    /** Inclusive raw message ordinal to start rebuilding from. */
    start: number;
    /** Inclusive raw message ordinal to stop rebuilding at. */
    end: number;
}

const RECOMP_USAGE = [
    "Usage:",
    "- `/ctx-recomp` — full rebuild from message 1 to the protected tail",
].join("\n");

const RECOMP_RANGE_UNSUPPORTED =
    "The daemon rebuilds the whole session; `session.recomp` accepts no message range.";

export function parseRecompArgs(
    raw: string,
):
    | { kind: "full" }
    | { kind: "partial"; range: PartialRecompRange }
    | { kind: "error"; message: string } {
    const trimmed = raw.trim();
    if (trimmed === "") return { kind: "full" };

    const match = trimmed.match(/^(\d+)\s*-\s*(\d+)$/);
    if (!match) {
        return {
            kind: "error",
            message: `Invalid /ctx-recomp arguments: \`${trimmed}\`.\n\n${RECOMP_USAGE}`,
        };
    }

    const start = Number.parseInt(match[1], 10);
    const end = Number.parseInt(match[2], 10);
    if (!Number.isFinite(start) || !Number.isFinite(end)) {
        return { kind: "error", message: "Range values must be finite integers." };
    }
    if (start < 1) {
        return { kind: "error", message: `Start must be >= 1 (got ${start}).` };
    }
    if (end < start) {
        return {
            kind: "error",
            message: `End must be >= start (got ${start}-${end}).`,
        };
    }

    return { kind: "partial", range: { start, end } };
}

export function parseWrapupArgs(
    raw: string,
): { ok: true; messagesToKeep: number } | { ok: false; message: string } {
    const trimmed = raw.trim();
    if (trimmed === "") return { ok: true, messagesToKeep: 20 };
    if (!/^\d+$/.test(trimmed)) {
        return {
            ok: false,
            message:
                "Usage: `/ctx-wrapup [messages_to_keep]` where messages_to_keep is a positive integer.",
        };
    }
    const messagesToKeep = Number.parseInt(trimmed, 10);
    if (!Number.isSafeInteger(messagesToKeep) || messagesToKeep <= 0) {
        return { ok: false, message: "messages_to_keep must be a positive integer." };
    }
    return { ok: true, messagesToKeep };
}

export interface CommandExecuteInput {
    command: string;
    sessionID: string;
    arguments: string;
}

export interface CommandExecuteOutput {
    parts: Array<{ type: string; text?: string }>;
}

const SENTINEL_PREFIX = "__CONTEXT_MANAGEMENT_";

// Effect HTTP uses plain-string TypeIds, not Symbols.
// Effect HTTP guards check string keys, so plugin-built responses need no Effect import.
const HTTP_SERVER_RESPONSE_TYPE_ID = "~effect/http/HttpServerResponse";
const HTTP_COOKIES_TYPE_ID = "~effect/http/Cookies";
const HTTP_BODY_TYPE_ID = "~effect/http/HttpBody";
const ERROR_REPORTER_IGNORE = "~effect/ErrorReporter/ignore";

/** The 204 response prevents OpenCode from forwarding the handled command to the LLM.
 *
 * The thrown `Error` preserves `.message` and `.stack` on hosts that do not recognize the Effect HTTP tags.
 * The thrown duck-typed 204 Effect response makes OpenCode treat the command as handled without importing Effect.
 * OpenCode recognizes responses with the `"~effect/http/HttpServerResponse"` TypeId.
 * OpenCode recognizes `"~effect/http/HttpServerResponse" in defect`.
 * The HTTP boundary skips JSON-500 logging when it recognizes the response.
 * The HTTP boundary writes a recognized response as HTTP 204.
 *
 * Response.toWeb's empty-body path requires status, statusText, headers, cookies.cookies, and body._tag.
 *
 * The shim duck-types an Effect HTTP response because the command hook has no handled, cancel, or no-reply result.
 * */
function throwSentinel(command: string): never {
    const sentinel = new Error(`${SENTINEL_PREFIX}${command.toUpperCase()}_HANDLED__`) as Error &
        Record<string, unknown>;
    sentinel[HTTP_SERVER_RESPONSE_TYPE_ID] = HTTP_SERVER_RESPONSE_TYPE_ID;
    sentinel[ERROR_REPORTER_IGNORE] = true;
    sentinel.status = 204;
    sentinel.statusText = undefined;
    sentinel.headers = {};
    sentinel.cookies = { [HTTP_COOKIES_TYPE_ID]: HTTP_COOKIES_TYPE_ID, cookies: {} };
    sentinel.body = { [HTTP_BODY_TYPE_ID]: HTTP_BODY_TYPE_ID, _tag: "Empty" };
    throw sentinel;
}

class SessionDeletedDuringCommandError extends Error {}

function rethrowDeletedCommand(error: unknown, command: string): void {
    if (error instanceof SessionDeletedDuringCommandError) throwSentinel(command);
}

function moduleResponseValue(response: unknown): Record<string, unknown> {
    if (response && typeof response === "object") {
        const value = response as Record<string, unknown>;
        if (value.result && typeof value.result === "object") {
            return value.result as Record<string, unknown>;
        }
        return value;
    }
    return {};
}

function rustCommandId(operation: string): string {
    return `opencode-${operation}-${randomUUID()}`;
}

function formatRustOperationMessage(
    operation: "wrapup" | "recomp",
    value: Record<string, unknown>,
): string {
    const disposition = typeof value.disposition === "string" ? value.disposition : "failed";
    const summary = typeof value.summary === "string" ? value.summary : "";
    const rounds = typeof value.rounds === "number" ? value.rounds : 0;
    if (operation === "wrapup") {
        switch (disposition) {
            case "completed":
                return `## Eidnara Wrapup\n\n${summary || "Wrapup completed."}${summary && rounds > 0 ? ` (${rounds} round${rounds === 1 ? "" : "s"})` : ""}`;
            case "nothing_to_compact":
                return `## Eidnara Wrapup\n\n${summary || "Nothing to compact."}`;
            case "already_in_progress":
                return `## Eidnara Wrapup — Skipped\n\n/ctx-wrapup is already running for this session${rounds > 0 ? ` (${rounds} round${rounds === 1 ? "" : "s"} complete)` : ""}. Wait for it to finish, then run /ctx-wrapup again if more history remains.`;
            case "retryable":
                // A nonterminal disposition indicates that the drain made progress but stopped before the keep watermark for a retryable reason.
                // The orchestrator renders retryable dispositions as Partial rather than Failed.
                // A retryable disposition instructs the user to rerun /ctx-wrapup instead of reporting failure.
                return `## Eidnara Wrapup — Partial\n\n${summary || "Wrapup made progress but stopped before the keep watermark."} Run /ctx-wrapup again to continue.`;
            default:
                return `## Eidnara Wrapup — Failed\n\n${summary || "Wrapup failed; try /ctx-wrapup again."}${rounds > 0 ? ` (${rounds} round${rounds === 1 ? "" : "s"})` : ""}`;
        }
    }
    switch (disposition) {
        case "started":
            return "## Eidnara Recomp\n\nHistorian recomp started. Rebuilding compartments from raw session history now.";
        case "already_in_progress":
            return "## Eidnara Recomp — Skipped\n\nHistorian recomp is already running for this session. Wait for it to finish, then try /ctx-recomp again.";
        case "nothing_to_do":
            return "## Eidnara Recomp\n\nNothing to rebuild: this session has no published compartments.";
        default:
            return `## Eidnara Recomp — Failed\n\n${summary || "Historian recomp failed; try /ctx-recomp again."}`;
    }
}

function statusUsage(value: Record<string, unknown>): Record<string, unknown> {
    return value.usage && typeof value.usage === "object"
        ? (value.usage as Record<string, unknown>)
        : {};
}

function statusInputTokens(value: Record<string, unknown>): number {
    const usage = statusUsage(value);
    return typeof usage.current_total_input_tokens === "number"
        ? usage.current_total_input_tokens
        : 0;
}

function statusCount(value: Record<string, unknown>, key: string): number {
    return typeof value[key] === "number" ? (value[key] as number) : 0;
}

function statusObject(value: Record<string, unknown>, key: string): Record<string, unknown> {
    const nested = value[key];
    return nested && typeof nested === "object" ? (nested as Record<string, unknown>) : {};
}

function plural(count: number, noun: string): string {
    return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

const MAX_STATUS_REJECT_ERROR_CHARS = 160;

function formatRustStatusText(value: Record<string, unknown>): string {
    const usage = statusUsage(value);
    const tokens = statusInputTokens(value);
    const limit = typeof usage.context_limit_tokens === "number" ? usage.context_limit_tokens : 0;
    const coverage = value.coverage_ordinal == null ? "none" : String(value.coverage_ordinal);
    const boundary = value.boundary_present === true ? "present" : "absent";
    const compartments = statusCount(value, "compartment_count");
    const pendingDrops = statusCount(value, "pending_drop_count");
    const tags = statusCount(value, "tag_count");
    const pendingM1 =
        value.pending_m1_delta === true
            ? `pending${typeof value.pending_m1_age_ms === "number" ? ` (${Math.round(value.pending_m1_age_ms / 1000)}s)` : ""}`
            : "none";
    const wrapup =
        value.wrapup_active === true
            ? `running (${plural(statusCount(value, "wrapup_rounds"), "round")} complete)`
            : "idle";
    const historian = statusObject(value, "historian");
    const publishFailures = statusCount(historian, "consecutive_publish_failures");
    const publishHealth =
        historian.publish_health_degraded === true
            ? `degraded (${publishFailures} consecutive publish failures)`
            : `ok (${plural(publishFailures, "consecutive publish failure")})`;
    const passTrace = statusObject(value, "pass_trace");
    const rejectError =
        typeof passTrace.last_reject_error === "string" && passTrace.last_reject_error !== ""
            ? `; last reject: ${passTrace.last_reject_error.slice(0, MAX_STATUS_REJECT_ERROR_CHARS)}`
            : "";
    const lines = [
        "### Module Cache",
        `- Usage: ${tokens.toLocaleString()}${limit > 0 ? ` / ${limit.toLocaleString()} tokens` : " tokens"}`,
        `- Boundary: ${boundary}`,
        `- Coverage ordinal: ${coverage}`,
        `- Compartments: ${compartments}`,
        `- Pending: ${plural(pendingDrops, "drop")}, ${plural(tags, "tag")}, m1 delta ${pendingM1}`,
        `- Wrapup: ${wrapup}`,
        `- Historian publish health: ${publishHealth}`,
    ];
    if (value.pass_trace && typeof value.pass_trace === "object") {
        lines.push(
            `- Passes: ${statusCount(passTrace, "receive_count")} received, ${statusCount(passTrace, "reject_count")} rejected${rejectError}`,
        );
    }
    if (typeof value.summary === "string" && value.summary.trim() !== "") {
        lines.push(`- Daemon: ${value.summary.trim()}`);
    }
    return lines.join("\n");
}

/**
 * /ctx-aug uses Sidekick to augment the user's prompt and sends the result as a user message.
 */
async function executeAugmentation(
    deps: {
        sendNotification: (
            sessionId: string,
            text: string,
            params: NotificationParams,
        ) => Promise<void>;
        sidekick?: {
            config: SidekickConfig;
            projectPath: string;
            /** The Sidekick child runs in the session's own directory, not the plugin launch directory. */
            resolveSessionDirectory?: (sessionId: string) => Promise<string> | string;
            client: PluginContext["client"];
            language?: string;
        };
    },
    sessionId: string,
    userPrompt: string,
    promptContext: NotificationParams,
): Promise<never> {
    if (!deps.sidekick?.config) {
        await deps.sendNotification(
            sessionId,
            "## /ctx-aug\n\nSidekick is not configured. Add sidekick settings to `eidnara.jsonc` to use /ctx-aug.",
            { forcePersist: !isTuiConnected(sessionId) },
        );
        throwSentinel("CTX-AUG");
    }

    const prompt = userPrompt.trim();
    if (prompt.length === 0) {
        await deps.sendNotification(
            sessionId,
            "## /ctx-aug\n\nUsage: `/ctx-aug <your prompt>`\n\nProvide a prompt to augment with project memory context.",
            { forcePersist: !isTuiConnected(sessionId) },
        );
        throwSentinel("CTX-AUG");
    }

    void deps.sendNotification(
        sessionId,
        "🔍 Preparing augmentation… this may take 2-10s depending on your sidekick provider.",
        {},
    );

    sessionLog(sessionId, "/ctx-aug: running sidekick");
    const sidekickResult = await runSidekick({
        client: deps.sidekick.client,
        sessionId,
        projectPath: deps.sidekick.projectPath,
        sessionDirectory: await deps.sidekick.resolveSessionDirectory?.(sessionId),
        userMessage: prompt,
        config: deps.sidekick.config,
        language: deps.sidekick.language,
    });

    let augmentedPrompt: string;
    if (sidekickResult) {
        augmentedPrompt = `${prompt}\n\n<sidekick-augmentation>\n${sidekickResult}\n</sidekick-augmentation>`;
        sessionLog(sessionId, `/ctx-aug: sidekick returned ${sidekickResult.length} chars`);
    } else {
        augmentedPrompt = prompt;
        sessionLog(sessionId, "/ctx-aug: sidekick returned no result, sending prompt as-is");
    }

    try {
        // The replacement turn keeps the agent, model, and variant the intercepted command carried; a bare text prompt would run under the session default.
        await sendUserPrompt(deps.sidekick.client, sessionId, augmentedPrompt, promptContext);
    } catch (error) {
        const reason = error instanceof Error ? error.message : String(error);
        sessionLog(sessionId, `/ctx-aug: failed to send augmented prompt: ${reason}`);
        // A timed-out send may still have enqueued the turn, so the notice asks the user to look before resending instead of telling them the prompt was lost.
        const notice =
            error instanceof TimeoutError
                ? `## /ctx-aug — Delivery unconfirmed\n\nOpenCode did not confirm the augmented prompt in time: ${reason}\n\nThe prompt may still arrive. If it does not appear in this session, send it again, with or without /ctx-aug:\n\n${prompt}`
                : `## /ctx-aug — Failed\n\nThe augmented prompt was not sent to the session: ${reason}\n\nYour original prompt was not sent either. Send it again, with or without /ctx-aug:\n\n${prompt}`;
        await deps.sendNotification(sessionId, notice, { forcePersist: true });
    }

    throwSentinel("CTX-AUG");
}

/** OpenCode's command path has no dialog, so the confirmation is the reply itself: the user re-issues the command with the confirm flag. commentlint: allow(JUDGE) */
export const MEMORY_MARK_ACTOR = "user:opencode";

async function executeMemoryMark(
    deps: {
        kernelClient?: KernelClientResolver;
        isSessionDeleted?: (sessionId: string) => boolean;
        resolveProjectRoot?: (sessionId: string) => Promise<string> | string;
    },
    sessionId: string,
    rawArguments: string,
): Promise<string> {
    const parsed = parseMemoryMarkArgs(rawArguments);
    if (!parsed.ok) return `## Eidnara Memory — Invalid Arguments\n\n${parsed.message}`;
    if (!deps.kernelClient) {
        return formatMemoryMarkOutcome(
            { kind: "refused", step: "preview", state: { kind: "disabled" } },
            parsed.args,
        );
    }
    const projectRoot = (await deps.resolveProjectRoot?.(sessionId)) ?? process.cwd();
    const isSessionDeleted = () => deps.isSessionDeleted?.(sessionId) === true;
    if (isSessionDeleted()) throwSentinel(MEMORY_MARK_COMMAND);
    const outcome = await runMemoryMarkCommand({
        client: deps.kernelClient({ sessionId, projectRoot }),
        sessionId,
        actor: MEMORY_MARK_ACTOR,
        args: parsed.args,
        isCancelled: isSessionDeleted,
    });
    // A session deleted mid-command gets no notification, which would recreate daemon state for it.
    if (isSessionDeleted()) throwSentinel(MEMORY_MARK_COMMAND);
    return formatMemoryMarkOutcome(outcome, parsed.args);
}

export function createEidnaraCommandHandler(deps: {
    /** Command paths use boot-resolved mode and must not reread configuration. */
    compactionOff?: boolean;
    getLiveModelKey?: (sessionId: string) => string | undefined;
    /** The `session.wrapup` request carries no subagent flag, so the handler gates `/ctx-wrapup` on this predicate. */
    isSubagentSession: (sessionId: string) => boolean | Promise<boolean>;
    /** Prevents a command whose route lookup lost to session deletion from recreating daemon state. */
    isSessionDeleted?: (sessionId: string) => boolean;
    onFlush?: (sessionId: string) => void;
    sendNotification: (
        sessionId: string,
        text: string,
        params: NotificationParams,
    ) => Promise<void>;
    moduleClient: RustModeModuleClient;
    /** Resolves the kernel client `/ctx-memory-mark` previews and commits through; the command answers `disabled` when absent. */
    kernelClient?: KernelClientResolver;
    /** The daemon keys session state by `(session, project_root)`; commands route by the same directory the transform resolved for the session. */
    resolveProjectRoot?: (sessionId: string) => Promise<string> | string;
    sidekick?: {
        config: SidekickConfig;
        projectPath: string;
        resolveSessionDirectory?: (sessionId: string) => Promise<string> | string;
        client: PluginContext["client"];
        language?: string;
    };
}) {
    // The notification wrapper swallows delivery failures so `throwSentinel` prevents OpenCode from forwarding the raw command to the LLM.
    const rawSendNotification = deps.sendNotification;
    deps.sendNotification = async (sessionId, text, params) => {
        try {
            await rawSendNotification(sessionId, text, params);
        } catch (err) {
            sessionLog(
                sessionId,
                `command notification delivery failed (continuing to sentinel): ${err instanceof Error ? err.message : String(err)}`,
            );
        }
    };

    const isStatusCommand = (command: string): boolean => command === "ctx-status";
    const isFlushCommand = (command: string): boolean => command === "ctx-flush";
    const isRecompCommand = (command: string): boolean => command === "ctx-recomp";
    const isWrapupCommand = (command: string): boolean => command === "ctx-wrapup";
    const isAugCommand = (command: string): boolean => command === "ctx-aug";
    const isMemoryMarkCommand = (command: string): boolean => command === MEMORY_MARK_COMMAND;
    const callRust = async (
        method: Parameters<RustModeModuleClient["call"]>[0]["method"],
        body: Record<string, unknown>,
        timeoutMs?: number,
        resolvedProjectRoot?: string,
    ): Promise<Record<string, unknown>> => {
        const sessionId = body.session_id as string;
        const projectRoot =
            resolvedProjectRoot ?? (await deps.resolveProjectRoot?.(sessionId)) ?? process.cwd();
        if (deps.isSessionDeleted?.(sessionId)) throw new SessionDeletedDuringCommandError();
        return moduleResponseValue(
            await deps.moduleClient.call({
                sessionId,
                projectRoot,
                method,
                body,
                ...(timeoutMs === undefined ? {} : { timeoutMs }),
            }),
        );
    };

    return {
        "command.execute.before": async (
            input: CommandExecuteInput,
            _output: CommandExecuteOutput,
            params: NotificationParams,
        ): Promise<void> => {
            const isStatus = isStatusCommand(input.command);
            const isFlush = isFlushCommand(input.command);
            const isRecomp = isRecompCommand(input.command);
            const isWrapup = isWrapupCommand(input.command);
            const isAug = isAugCommand(input.command);
            const isMemoryMark = isMemoryMarkCommand(input.command);

            if (!isStatus && !isFlush && !isRecomp && !isWrapup && !isAug && !isMemoryMark) {
                return;
            }

            const sessionId = input.sessionID;
            let result = "";

            if (deps.compactionOff && (isFlush || isRecomp || isWrapup)) {
                const command = `/${input.command}`;
                await deps.sendNotification(
                    sessionId,
                    `Eidnara compaction is disabled (${COMPACTION_ENABLED_PATH}: false) — ${command} manages compacted history and has no effect in this mode.`,
                    { forcePersist: !isTuiConnected(sessionId) },
                );
                throwSentinel(input.command);
            }

            if (isAug) {
                await executeAugmentation(deps, sessionId, input.arguments, params);
                return; // executeAugmentation throws sentinel internally
            }

            if (isMemoryMark) {
                result = await executeMemoryMark(deps, sessionId, input.arguments);
            }

            if (isFlush) {
                try {
                    const value = await callRust("session.flush", {
                        method: "session.flush",
                        v: 1,
                        session_id: sessionId,
                    });
                    result =
                        value.armed === false
                            ? "No pending operations to flush."
                            : "Flushed: Changes take effect on next message.";
                } catch (error) {
                    rethrowDeletedCommand(error, input.command);
                    result = `Error: Failed to flush context operations. ${error instanceof Error ? error.message : String(error)}`;
                }
                deps.onFlush?.(sessionId);
                if (isTuiConnected(sessionId)) {
                    pushNotification(
                        "action",
                        { action: "show-flush-dialog", message: result },
                        sessionId,
                    );
                    sessionLog(sessionId, "command ctx-flush: pushed show-flush-dialog to TUI");
                    throwSentinel(input.command);
                }
            }

            if (isStatus) {
                let rustStatus: Record<string, unknown> | undefined;
                let statusError: string | undefined;
                try {
                    rustStatus = await callRust("session.status", {
                        method: "session.status",
                        v: 1,
                        session_id: sessionId,
                    });
                } catch (error) {
                    rethrowDeletedCommand(error, input.command);
                    sessionLog(sessionId, "rust session.status failed:", error);
                    statusError = error instanceof Error ? error.message : String(error);
                }
                if (isTuiConnected(sessionId)) {
                    pushNotification("action", { action: "show-status-dialog" }, sessionId);
                    sessionLog(sessionId, "command ctx-status: pushed show-status-dialog to TUI");
                    throwSentinel(input.command);
                }
                const liveModelKey = deps.getLiveModelKey?.(sessionId);
                const modelSlash = liveModelKey?.indexOf("/") ?? -1;
                const windowGeometry =
                    liveModelKey && modelSlash > 0
                        ? resolveContextWindowGeometry(
                              liveModelKey.slice(0, modelSlash),
                              liveModelKey.slice(modelSlash + 1),
                          )
                        : undefined;
                const rustTailHygiene = rustStatus?.tail_hygiene;
                const tailHygiene = resolveTailHygieneStatus(
                    rustTailHygiene && typeof rustTailHygiene === "object"
                        ? (rustTailHygiene as WireTailHygieneBaseline)
                        : undefined,
                );
                const lines = ["## Eidnara Status"];
                if (deps.compactionOff) {
                    lines.push(
                        "",
                        `**Compaction:** disabled (${COMPACTION_ENABLED_PATH}: false) — native compaction owns the context window.`,
                    );
                }
                if (rustStatus) {
                    lines.push("", formatRustStatusText(rustStatus));
                    if (windowGeometry) {
                        lines.push(
                            `- ${formatWindowDerivationLine(statusInputTokens(rustStatus), windowGeometry)}`,
                        );
                    }
                } else {
                    lines.push(
                        "",
                        `Session status is unavailable: ${statusError ?? "no response"}`,
                    );
                }
                if (tailHygiene !== undefined) {
                    lines.push(
                        "",
                        "### Tail Hygiene",
                        `- Reclaimable / eligible: ${formatTailHygiene(tailHygiene)}`,
                        "- Reasoning is excluded from both terms.",
                    );
                }
                const combinedStatus = lines.join("\n");
                result += result ? `\n\n${combinedStatus}` : combinedStatus;
            }

            if (isWrapup) {
                const parsed = parseWrapupArgs(input.arguments);
                if (await deps.isSubagentSession(sessionId)) {
                    result =
                        "## Eidnara Wrapup — Skipped\n\n/ctx-wrapup is only available in primary sessions.";
                } else if (!parsed.ok) {
                    result = `## Eidnara Wrapup — Invalid Arguments\n\n${parsed.message}`;
                } else {
                    const keep = parsed.messagesToKeep;
                    const projectRoot =
                        (await deps.resolveProjectRoot?.(sessionId)) ?? process.cwd();
                    if (deps.isSessionDeleted?.(sessionId)) throwSentinel(input.command);
                    void deps.sendNotification(
                        sessionId,
                        "## Eidnara Wrapup\n\nStarting wrapup…",
                        {},
                    );
                    try {
                        const value = await callRust(
                            "session.wrapup",
                            {
                                method: "session.wrapup",
                                v: 1,
                                session_id: sessionId,
                                keep,
                                command_id: rustCommandId("wrapup"),
                            },
                            MAX_WRAPUP_REQUEST_BUDGET_MS,
                            projectRoot,
                        );
                        result = formatRustOperationMessage("wrapup", value);
                    } catch (error) {
                        rethrowDeletedCommand(error, input.command);
                        result = `## Eidnara Wrapup — Failed\n\n${error instanceof Error ? error.message : String(error)}`;
                    }
                }
            }

            if (isRecomp) {
                const parsedArgs = parseRecompArgs(input.arguments);
                if (parsedArgs.kind === "error") {
                    result = `## Eidnara Recomp — Invalid Arguments\n\n${parsedArgs.message}`;
                } else if (parsedArgs.kind === "partial") {
                    result = `## Eidnara Recomp — Unsupported\n\nRequested range: \`${parsedArgs.range.start}-${parsedArgs.range.end}\`. ${RECOMP_RANGE_UNSUPPORTED}\n\n${RECOMP_USAGE}`;
                } else {
                    try {
                        const value = await callRust("session.recomp", {
                            method: "session.recomp",
                            v: 1,
                            session_id: sessionId,
                            command_id: rustCommandId("recomp"),
                        });
                        result = formatRustOperationMessage("recomp", value);
                    } catch (error) {
                        rethrowDeletedCommand(error, input.command);
                        result = `## Eidnara Recomp — Failed\n\n${error instanceof Error ? error.message : String(error)}`;
                    }
                }
            }

            await deps.sendNotification(sessionId, result, {
                forcePersist: !isTuiConnected(sessionId),
            });
            sessionLog(sessionId, `command ${input.command} handled via command.execute.before`);

            throwSentinel(input.command);
        },
    };
}

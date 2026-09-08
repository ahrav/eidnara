import { randomUUID } from "node:crypto";
import { COMPACTION_ENABLED_PATH } from "@eidnara/opencode/config/agent-disable";
import { resolveProjectRootDirectory } from "@eidnara/opencode/features/context/project-identity";
import type { RustModeModuleClient } from "@eidnara/opencode/hooks/context/rust-mode-transform";
import type { CtxStatusLevel } from "./pi-command-utils";

export const COMPACTION_OFF_COMMAND_UNAVAILABLE = `Unavailable: eidnara is in compaction-off mode (${COMPACTION_ENABLED_PATH}=false).`;

export const RECOMP_USAGE = [
    "Usage:",
    "- `/ctx-recomp` — full rebuild from message 1 to the protected tail",
].join("\n");

export const RECOMP_RANGE_UNSUPPORTED =
    "The daemon rebuilds the whole session; `session.recomp` accepts no message range.";

export interface DaemonSessionDeps {
    moduleClient: RustModeModuleClient;
    /** Command paths use boot-resolved mode and must not reread configuration. */
    compactionOff?: boolean;
}

export type DaemonSessionMethod = Parameters<RustModeModuleClient["call"]>[0]["method"];

export function moduleResponseValue(response: unknown): Record<string, unknown> {
    if (response && typeof response === "object") {
        const value = response as Record<string, unknown>;
        if (value.result && typeof value.result === "object") {
            return value.result as Record<string, unknown>;
        }
        return value;
    }
    return {};
}

export function rustCommandId(operation: string): string {
    return `opencode-${operation}-${randomUUID()}`;
}

/**
 * The daemon reads `session_id` from the body; the transport routes on the same id.
 *
 * `projectRoot` follows the invocation cwd: session lineage and `session.wrapup` authority are keyed by `(session, root)`, in the same git-root spelling the kernel memory routes bind, so a Pi `/cd` moves later commands with it. commentlint: allow(JUDGE)
 */
export async function callDaemonSession(
    deps: DaemonSessionDeps,
    ctx: { cwd: string },
    method: DaemonSessionMethod,
    body: Record<string, unknown>,
    timeoutMs?: number,
): Promise<Record<string, unknown>> {
    return moduleResponseValue(
        await deps.moduleClient.call({
            sessionId: body.session_id as string,
            projectRoot: resolveProjectRootDirectory(ctx.cwd),
            method,
            body,
            ...(timeoutMs === undefined ? {} : { timeoutMs }),
        }),
    );
}

export function formatRustOperationMessage(
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

export function statusInputTokens(value: Record<string, unknown>): number {
    const usage = statusUsage(value);
    return typeof usage.current_total_input_tokens === "number"
        ? usage.current_total_input_tokens
        : 0;
}

/** The daemon's context limit, or `undefined` when the status carries none. */
export function statusContextLimitTokens(value: Record<string, unknown>): number | undefined {
    const limit = statusUsage(value).context_limit_tokens;
    return typeof limit === "number" && limit > 0 ? limit : undefined;
}

export function formatRustStatusText(value: Record<string, unknown>): string {
    const tokens = statusInputTokens(value);
    const limit = statusContextLimitTokens(value);
    const coverage = value.coverage_ordinal == null ? "none" : String(value.coverage_ordinal);
    const boundary = value.boundary_present === true ? "present" : "absent";
    const compartments = typeof value.compartment_count === "number" ? value.compartment_count : 0;
    return [
        "### Module Cache",
        `- Usage: ${tokens.toLocaleString()}${limit === undefined ? " tokens" : ` / ${limit.toLocaleString()} tokens`}`,
        `- Boundary: ${boundary}`,
        `- Coverage ordinal: ${coverage}`,
        `- Compartments: ${compartments}`,
    ].join("\n");
}

export function operationMessageLevel(text: string): CtxStatusLevel {
    const heading = text.split("\n", 1)[0] ?? "";
    if (heading.includes("Failed") || heading.includes("Invalid")) return "error";
    if (
        heading.includes("Skipped") ||
        heading.includes("Partial") ||
        heading.includes("Unsupported")
    )
        return "warning";
    return "info";
}

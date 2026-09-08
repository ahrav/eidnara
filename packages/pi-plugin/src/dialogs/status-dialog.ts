import type { ExtensionAPI, ExtensionCommandContext, Theme } from "@earendil-works/pi-coding-agent";
import {
    type Component,
    matchesKey,
    type TUI,
    truncateToWidth,
    visibleWidth,
} from "@earendil-works/pi-tui";
import { resolveProjectRootDirectory } from "@eidnara/opencode/features/context/project-identity";
import {
    MAX_EXECUTE_THRESHOLD,
    resolveExecuteThresholdDetail,
} from "@eidnara/opencode/hooks/context/event-resolvers";
import { estimateTokens } from "@eidnara/opencode/hooks/context/read-session-formatting";
import {
    calibrateBuckets,
    resolveModelCalibration,
} from "@eidnara/opencode/hooks/context/tokenizer-calibration";
import type { RustSessionStatus } from "@eidnara/opencode/plugin/rpc-handlers";
import {
    formatThresholdClampNote,
    formatThresholdPercent,
} from "@eidnara/opencode/shared/format-threshold";
import {
    isServedMemoryDecisionRow,
    type KernelClientResolver,
    type KernelMemorySnapshot,
    kernelMemorySnapshotFrom,
    type StateKey,
    stateKey,
} from "@eidnara/opencode/shared/kernel-client";
import type { TailHygieneStatus } from "@eidnara/opencode/shared/rpc-types";
import {
    formatTailHygiene,
    resolveTailHygieneStatus,
} from "@eidnara/opencode/shared/tail-hygiene-status";
import {
    formatWindowDerivationLine,
    type WindowGeometryResult,
} from "@eidnara/opencode/shared/window-geometry";
import packageJson from "../../package.json";
import { resolveSessionId } from "../commands/pi-command-utils";
import { resolvePiWindowGeometry } from "../pi-context-limit";
import { piSystemPromptStateFor } from "../system-prompt";

// `COLORS` mirrors `packages/plugin/src/tui/slots/sidebar-content.tsx` so Pi and OpenCode use the same category palette.
const COLORS = {
    system: "#c084fc", // Purple
    docs: "#22d3ee", // Cyan — <project-docs>
    compartments: "#60a5fa", // Blue
    memories: "#34d399", // Green
    profile: "#a3e635", // Lime — <user-profile>
    conversation: "#f87171", // Red
    toolCalls: "#fb923c", // Orange
    toolDefs: "#f472b6", // Pink
};

/** The dialog refreshes every 1,000 ms while open. */
const REFRESH_INTERVAL_MS = 1000;

export interface StatusDialogDeps {
    /** Serves the memory count and state the dialog reports. */
    kernelClient: KernelClientResolver;
    projectIdentity: string;
    protectedTags?: number;
    executeThresholdPercentage?: number | { default: number; [modelKey: string]: number };
    historyBudgetPercentage?: number;
    executeThresholdTokens?: {
        default?: number;
        [modelKey: string]: number | undefined;
    };
}

interface StatusDialogDetail {
    sessionId: string;
    usagePercentage: number;
    inputTokens: number;
    systemPromptTokens: number;
    compartmentCount: number;
    /** Rows the kernel serves this project on the `explicit_search` surface. */
    memoryCount: number;
    /** True when the read behind `memoryCount` was truncated by the daemon's per-read bounds, making the count a lower bound. */
    memoryTruncated?: boolean;
    /** The kernel's state for that read, such as `available` or `unavailable:daemon_absent`. */
    memoryState: StateKey;
    memoryBlockCount: number;
    sessionNoteCount: number;
    readySmartNoteCount: number;
    pendingOpsCount: number;
    historianRunning: boolean;
    lastTransformError: string | null;
    isSubagent: boolean;
    contextLimit: number;
    windowGeometry?: WindowGeometryResult;
    executeThreshold: number;
    /** `executeThresholdMode` identifies whether tokens or a percentage produced `executeThreshold`. */
    executeThresholdMode: "percentage" | "tokens";
    /** `executeThresholdClamped` is true when `executeThreshold` is lower than its configured value. */
    executeThresholdClamped?: boolean;
    /** `executeThresholdConfigured` retains the configured value before clamping. */
    executeThresholdConfigured?: number;
    protectedTagCount: number;
    historyBlockTokens: number;
    compressionBudget: number | null;
    compressionUsage: string | null;
    activeTags: number;
    droppedTags: number;
    totalTags: number;
    activeBytes: number;
    compartmentTokens: number;
    factTokens: number;
    memoryTokens: number;
    docsTokens: number;
    profileTokens: number;
    conversationTokens: number;
    toolCallTokens: number;
    toolDefinitionTokens: number;
    tailHygiene?: TailHygieneStatus;
    newWorkTokens: number;
    totalInputTokens: number;
}

export async function showStatusDialog(
    pi: ExtensionAPI,
    ctx: ExtensionCommandContext,
    deps: StatusDialogDeps,
    daemon: DaemonStatusSource | null = null,
): Promise<void> {
    const sessionId = resolveSessionId(ctx);
    if (!sessionId) throw new Error("No active Pi session is available.");
    const memory = await readStatusMemory(deps, sessionId, ctx.cwd);

    await ctx.ui.custom<undefined>(
        (tui, theme, _keybindings, done) =>
            new StatusDialogComponent({
                pi,
                ctx,
                deps,
                daemon,
                sessionId,
                memory,
                theme,
                tui,
                done,
            }),
        {
            overlay: true,
            overlayOptions: { anchor: "center", width: 78 },
        },
    );
}

/** Initial daemon status and the reader that refreshes it. */
export interface DaemonStatusSource {
    initial: RustSessionStatus;
    /** Rejects when the daemon cannot answer; the dialog then keeps the previous snapshot. commentlint: allow(JUDGE) */
    read: () => Promise<RustSessionStatus>;
}

interface StatusDialogProps {
    pi: ExtensionAPI;
    ctx: ExtensionCommandContext;
    deps: StatusDialogDeps;
    daemon: DaemonStatusSource | null;
    sessionId: string;
    /** The memory read taken before the dialog opened; refresh ticks re-read. */
    memory: KernelMemorySnapshot;
    theme: Theme;
    tui: TUI;
    done: (value: undefined) => void;
}

/**
 * The status surface reports what an explicit search would see, lag included,
 * so the dialog shows `stale` when the projector is behind.
 */
export async function readStatusMemory(
    deps: Pick<StatusDialogDeps, "kernelClient">,
    sessionId: string,
    directory: string,
): Promise<KernelMemorySnapshot> {
    const client = deps.kernelClient({
        sessionId,
        projectRoot: resolveProjectRootDirectory(directory),
    });
    return kernelMemorySnapshotFrom(await client.read({ surface: "explicit_search", gated: true }));
}

/**
 * `handleInput` closes the dialog on Escape, Enter, and Ctrl+C.
 */
class StatusDialogComponent implements Component {
    private readonly props: StatusDialogProps;
    private detail: StatusDialogDetail;
    private daemonStatus: RustSessionStatus | null;
    private refreshTimer: ReturnType<typeof setInterval> | null = null;
    private closed = false;
    private refreshing = false;

    constructor(props: StatusDialogProps) {
        this.props = props;
        this.daemonStatus = props.daemon?.initial ?? null;
        this.detail = buildPiStatusDetail(
            props.pi,
            props.ctx,
            props.deps,
            props.sessionId,
            props.memory,
            this.daemonStatus,
        );
        this.refreshTimer = setInterval(() => {
            void this.refresh();
        }, REFRESH_INTERVAL_MS);
    }

    /** One read in flight at a time; a slow daemon never stacks refreshes. */
    private async refresh(): Promise<void> {
        if (this.closed || this.refreshing) return;
        this.refreshing = true;
        try {
            const [memory, daemonStatus] = await Promise.all([
                readStatusMemory(this.props.deps, this.props.sessionId, this.props.ctx.cwd),
                this.readDaemonStatus(),
            ]);
            if (this.closed) return;
            this.daemonStatus = daemonStatus;
            this.detail = buildPiStatusDetail(
                this.props.pi,
                this.props.ctx,
                this.props.deps,
                this.props.sessionId,
                memory,
                daemonStatus,
            );
            this.props.tui.requestRender();
        } catch {
            // On refresh failure, retain the previous detail.
        } finally {
            this.refreshing = false;
        }
    }

    private async readDaemonStatus(): Promise<RustSessionStatus | null> {
        if (!this.props.daemon) return null;
        try {
            return await this.props.daemon.read();
        } catch {
            return this.daemonStatus;
        }
    }

    handleInput(data: string): void {
        if (
            matchesKey(data, "escape") ||
            matchesKey(data, "ctrl+c") ||
            matchesKey(data, "return")
        ) {
            this.close();
        }
    }

    private close(): void {
        if (this.closed) return;
        this.closed = true;
        if (this.refreshTimer) {
            clearInterval(this.refreshTimer);
            this.refreshTimer = null;
        }
        this.props.done(undefined);
    }

    invalidate(): void {
        // Rendering is stateless, so no invalidation is required.
    }

    render(width: number): string[] {
        // `drawBorder` reserves two columns for borders and one for padding.
        // `renderInner` receives the remaining width so the segmented bar fills each row.
        // `renderInner` avoids a fixed 56-character cap so the segmented bar fills the available width.
        const innerWidth = Math.max(20, width - 4);
        const inner = renderInner(this.detail, this.props.theme, innerWidth);
        return drawBorder(inner, width, this.props.theme);
    }

    dispose(): void {
        this.closed = true;
        if (this.refreshTimer) {
            clearInterval(this.refreshTimer);
            this.refreshTimer = null;
        }
    }
}

function renderInner(s: StatusDialogDetail, theme: Theme, innerWidth: number): string[] {
    const pctColor =
        s.usagePercentage >= 80 ? "error" : s.usagePercentage >= 65 ? "warning" : "accent";
    const lines: string[] = [];

    // Header
    lines.push(
        `${theme.fg("accent", theme.bold("⚡ Eidnara Status"))}   ${theme.fg(
            "muted",
            `v${packageJson.version}`,
        )}`,
    );
    lines.push("");

    // Context summary
    lines.push(
        `Context  ${theme.fg(
            pctColor,
            theme.bold(`${s.usagePercentage.toFixed(1)}%`),
        )} · ${fmt(s.inputTokens)} / ${s.contextLimit > 0 ? fmt(s.contextLimit) : "?"} tokens`,
    );
    if (s.windowGeometry) {
        lines.push(
            formatWindowDerivationLine(s.inputTokens, s.windowGeometry).replace(
                /^Context:.* — window /,
                "Window ",
            ),
        );
    }
    if (s.tailHygiene !== undefined) {
        lines.push(`Hygiene ${formatTailHygiene(s.tailHygiene)}`);
    }

    lines.push(renderBar(s, innerWidth));

    // Legend
    for (const seg of breakdownSegments(s)) {
        const pct = ((seg.tokens / (s.inputTokens || 1)) * 100).toFixed(1);
        const left = colorHex(seg.color, `${seg.label}${seg.detail ? ` ${seg.detail}` : ""}`);
        const right = theme.fg("muted", `${fmt(seg.tokens)} (${pct}%)`);
        lines.push(`${left}   ${right}`);
    }
    lines.push("* Conversation includes model Reasoning; hygiene excludes it.");
    lines.push("");

    lines.push(
        `Counts: ${s.compartmentCount} compartments · ${s.memoryCount}${s.memoryTruncated ? "+" : ""} memories (${s.memoryBlockCount} injected, ${s.memoryState}) · ${
            s.sessionNoteCount + s.readySmartNoteCount
        } notes`,
    );
    lines.push(
        `Historian: ${
            s.historianRunning ? theme.fg("warning", "running") : theme.fg("accent", "idle")
        }`,
    );
    lines.push(`Pending drops: ${s.pendingOpsCount}`);
    lines.push("");

    lines.push(theme.fg("muted", "Context"));
    lines.push(
        `Execute threshold ${formatThresholdPercent(s.executeThreshold)}%${formatThresholdClampNote(
            {
                clamped: s.executeThresholdClamped,
                mode: s.executeThresholdMode,
                configuredValue: s.executeThresholdConfigured,
                contextLimit: s.contextLimit,
                maxPercentage: MAX_EXECUTE_THRESHOLD,
            },
        )}`,
    );
    lines.push(
        `Protected tags ${s.protectedTagCount} · Subagent ${s.isSubagent ? "yes" : "no"} · History block ~${fmt(s.historyBlockTokens)} tok${
            s.compressionBudget
                ? ` · Budget ~${fmt(s.compressionBudget)} tok (${s.compressionUsage} used)`
                : ""
        }`,
    );

    if (s.lastTransformError) lines.push(theme.fg("error", `⚠ ${s.lastTransformError}`));

    lines.push("");
    lines.push(theme.fg("muted", "Press Escape to close"));
    return lines;
}

/**
 * The `borderMuted` border distinguishes the overlay from its background.
 */
function drawBorder(inner: string[], width: number, theme: Theme): string[] {
    const innerWidth = Math.max(20, width - 4); // 2 chars border + 1 padding each side
    const border = (s: string) => theme.fg("borderMuted", s);

    const top = border(`╭${"─".repeat(innerWidth + 2)}╮`);
    const bottom = border(`╰${"─".repeat(innerWidth + 2)}╯`);
    const side = border("│");

    const out: string[] = [];
    out.push(top);
    for (const raw of inner) {
        const line = truncateToWidth(raw, innerWidth, "…");
        const visible = visibleWidth(line);
        const pad = " ".repeat(Math.max(0, innerWidth - visible));
        out.push(`${side} ${line}${pad} ${side}`);
    }
    out.push(bottom);
    return out;
}

function positiveNumber(value: unknown): number | undefined {
    return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : undefined;
}

export function buildPiStatusDetail(
    pi: ExtensionAPI,
    ctx: ExtensionCommandContext,
    deps: StatusDialogDeps,
    sessionId: string,
    memory: KernelMemorySnapshot,
    daemonStatus: RustSessionStatus | null = null,
): StatusDialogDetail {
    const usage = ctx.getContextUsage?.();
    const daemonInputTokens = positiveNumber(daemonStatus?.usage?.current_total_input_tokens);
    const daemonContextLimit = positiveNumber(daemonStatus?.usage?.context_limit_tokens);
    const inputTokens = daemonInputTokens ?? (typeof usage?.tokens === "number" ? usage.tokens : 0);
    const windowGeometry = resolvePiWindowGeometry({
        rawContextWindow: usage?.contextWindow ?? ctx.model?.contextWindow,
        model: ctx.model,
    });
    const contextLimit = daemonContextLimit ?? windowGeometry?.usableSoft ?? 0;
    const usagePercentage =
        contextLimit > 0 && inputTokens > 0 ? (inputTokens / contextLimit) * 100 : 0;
    // The derivation line divides by `usableSoft`; a daemon limit that differs from it would put
    // two denominators on one dialog, so the line renders only when both agree.
    const displayedWindowGeometry =
        windowGeometry && windowGeometry.usableSoft === contextLimit ? windowGeometry : undefined;

    const compartmentCount = positiveNumber(daemonStatus?.compartment_count) ?? 0;
    const compartmentTokens = positiveNumber(daemonStatus?.compartment_tokens) ?? 0;
    const pendingOpsCount = positiveNumber(daemonStatus?.pending_drop_count) ?? 0;
    // `wrapup_active` is the daemon's only in-flight signal, so `historianRunning` reads it. commentlint: allow(JUDGE)
    const historianRunning = daemonStatus?.wrapup_active === true;
    const tailHygiene = resolveTailHygieneStatus(daemonStatus?.tail_hygiene);

    let systemPromptTokens = piSystemPromptStateFor(sessionId)?.systemPromptTokens ?? 0;
    try {
        const sysPrompt =
            typeof ctx.getSystemPrompt === "function" ? ctx.getSystemPrompt() : undefined;
        if (typeof sysPrompt === "string" && sysPrompt.length > 0) {
            systemPromptTokens = estimateTokens(sysPrompt);
        }
    } catch {}

    // Provider tool-definition token counts are estimates, not wire-payload counts.
    let toolDefinitionTokens = 0;
    try {
        const tools = pi.getAllTools?.() ?? [];
        for (const tool of tools) {
            toolDefinitionTokens += estimateTokens(
                `${tool.name ?? ""}\n${tool.description ?? ""}\n${safeStringify(tool.parameters)}`,
            );
        }
    } catch {
        // best effort
    }

    const modelKey = ctx.model ? `${ctx.model.provider}/${ctx.model.id}` : undefined;
    const calibrated = calibrateBuckets({
        inputTokens,
        systemLocal: systemPromptTokens,
        toolDefsLocal: toolDefinitionTokens,
        compartmentsLocal: compartmentTokens,
        factsLocal: 0,
        memoriesLocal: 0,
        docsLocal: 0,
        profileLocal: 0,
        conversationLocal: 0,
        toolCallsLocal: 0,
        calibration: resolveModelCalibration(ctx.model?.provider, ctx.model?.id),
    });

    const threshold = resolveExecuteThresholdDetail(
        deps.executeThresholdPercentage ?? 65,
        modelKey,
        65,
        {
            tokensConfig: deps.executeThresholdTokens,
            contextLimit: contextLimit || undefined,
            sessionId,
        },
    );
    const historyBlockTokens = calibrated.compartmentTokens + calibrated.factTokens;
    const historyBudgetPercentage = deps.historyBudgetPercentage ?? 0.15;
    const compressionBudget =
        contextLimit > 0
            ? Math.floor(
                  contextLimit *
                      (Math.min(threshold.percentage, 80) / 100) *
                      historyBudgetPercentage,
              )
            : null;

    return {
        sessionId,
        usagePercentage,
        inputTokens,
        systemPromptTokens: calibrated.systemTokens,
        compartmentCount,
        // Expired anti-memories stay out of the count, matching the surface filter list and search apply.
        memoryCount: memory.rows.filter((row) => isServedMemoryDecisionRow(row, Date.now())).length,
        ...(memory.truncated === true ? { memoryTruncated: true } : {}),
        memoryState: stateKey(memory.state),
        memoryBlockCount: 0,
        sessionNoteCount: 0,
        readySmartNoteCount: 0,
        pendingOpsCount,
        historianRunning,
        lastTransformError: null,
        isSubagent: false,
        contextLimit,
        windowGeometry: displayedWindowGeometry,
        executeThreshold: threshold.percentage,
        executeThresholdMode: threshold.mode,
        executeThresholdClamped: threshold.clamped,
        executeThresholdConfigured: threshold.configuredValue,
        protectedTagCount: deps.protectedTags ?? 20,
        historyBlockTokens,
        compressionBudget,
        compressionUsage:
            compressionBudget && compressionBudget > 0
                ? `${((historyBlockTokens / compressionBudget) * 100).toFixed(0)}%`
                : null,
        activeTags: 0,
        droppedTags: 0,
        totalTags: 0,
        activeBytes: 0,
        compartmentTokens: calibrated.compartmentTokens,
        factTokens: calibrated.factTokens,
        memoryTokens: calibrated.memoryTokens,
        docsTokens: calibrated.docsTokens,
        profileTokens: calibrated.profileTokens,
        conversationTokens: calibrated.conversationTokens,
        toolCallTokens: calibrated.toolCallTokens,
        toolDefinitionTokens: calibrated.toolDefinitionTokens,
        ...(tailHygiene === undefined ? {} : { tailHygiene }),
        newWorkTokens: 0,
        totalInputTokens: 0,
    };
}

function safeStringify(value: unknown): string {
    try {
        if (value === undefined || value === null) return "";
        return typeof value === "string" ? value : JSON.stringify(value);
    } catch {
        return "";
    }
}

function breakdownSegments(s: StatusDialogDetail): Array<{
    label: string;
    tokens: number;
    color: string;
    detail?: string;
}> {
    const segs: Array<{
        label: string;
        tokens: number;
        color: string;
        detail?: string;
    }> = [];
    if (s.systemPromptTokens > 0)
        segs.push({
            label: "System",
            tokens: s.systemPromptTokens,
            color: COLORS.system,
        });
    if (s.docsTokens > 0) segs.push({ label: "Docs", tokens: s.docsTokens, color: COLORS.docs });
    if (s.compartmentTokens > 0)
        segs.push({
            label: "Compartments",
            tokens: s.compartmentTokens,
            color: COLORS.compartments,
            detail: `(${s.compartmentCount})`,
        });
    if (s.memoryTokens > 0)
        segs.push({
            label: "Memories",
            tokens: s.memoryTokens,
            color: COLORS.memories,
            detail: `(${s.memoryBlockCount})`,
        });
    if (s.profileTokens > 0)
        segs.push({
            label: "User Profile",
            tokens: s.profileTokens,
            color: COLORS.profile,
        });
    if (s.conversationTokens > 0)
        segs.push({
            label: "Conversation*",
            tokens: s.conversationTokens,
            color: COLORS.conversation,
        });
    if (s.toolCallTokens > 0)
        segs.push({
            label: "Tool Calls",
            tokens: s.toolCallTokens,
            color: COLORS.toolCalls,
        });
    if (s.toolDefinitionTokens > 0)
        segs.push({
            label: "Tool Defs",
            tokens: s.toolDefinitionTokens,
            color: COLORS.toolDefs,
        });
    return segs;
}

function renderBar(s: StatusDialogDetail, innerWidth: number): string {
    // The 20-column minimum keeps segments visible in narrow terminals.
    const barWidth = Math.max(20, innerWidth);
    const segs = breakdownSegments(s);
    if (segs.length === 0) return "";
    const widths = segs.map((seg) =>
        Math.max(1, Math.round((seg.tokens / (s.inputTokens || 1)) * barWidth)),
    );
    let sum = widths.reduce((a, b) => a + b, 0);
    while (sum > barWidth) {
        const maxIdx = widths.indexOf(Math.max(...widths));
        if ((widths[maxIdx] ?? 0) > 1) {
            widths[maxIdx] -= 1;
            sum--;
        } else break;
    }
    while (sum < barWidth) {
        const maxIdx = widths.indexOf(Math.max(...widths));
        widths[maxIdx] = (widths[maxIdx] ?? 0) + 1;
        sum++;
    }
    return segs.map((seg, i) => colorHex(seg.color, "█".repeat(widths[i] ?? 0))).join("");
}

function fmt(n: number): string {
    const abs = Math.abs(n);
    if (abs >= 1_000_000) return `${trim1(n / 1_000_000)}M`;
    if (abs >= 1_000) return `${trim1(n / 1_000)}K`;
    return String(Math.round(n));
}

function trim1(n: number): string {
    const rounded = n.toFixed(1);
    return rounded.endsWith(".0") ? rounded.slice(0, -2) : rounded;
}

function colorHex(hex: string, text: string): string {
    const clean = hex.replace("#", "");
    const r = Number.parseInt(clean.slice(0, 2), 16);
    const g = Number.parseInt(clean.slice(2, 4), 16);
    const b = Number.parseInt(clean.slice(4, 6), 16);
    return `\x1b[38;2;${r};${g};${b}m${text}\x1b[39m`;
}

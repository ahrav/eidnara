import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { resolvePiWindowGeometry } from "./pi-context-limit";

const STATUS_KEY = "eidnara";
const recompSessions = new Set<string>();

export interface StatusLineDeps {
    projectIdentity: string;
}

const lastRenderedBySession = new Map<string, string>();

/**
 * The daemon applies the whole recomp inside the `session.recomp` call and exposes no progress
 * afterwards, so the active window is exactly the in-flight call. Pi fires no event at either edge
 * of that window, which is why the repaint is forced here rather than left to the event hooks.
 */
export function setEidnaraRecompActive(
    ctx: ExtensionContext,
    sessionId: string,
    active: boolean,
): void {
    if (active) recompSessions.add(sessionId);
    else recompSessions.delete(sessionId);
    paintStatusLine(ctx, sessionId, true);
}

export function registerStatusLine(pi: ExtensionAPI, deps: StatusLineDeps): void {
    void deps.projectIdentity;

    pi.on("session_start", async (_event, ctx) => updateStatusLine(ctx, deps, true));
    pi.on("agent_end", async (_event, ctx) => updateStatusLine(ctx, deps));
    pi.on("session_compact", async (_event, ctx) => updateStatusLine(ctx, deps, true));
    pi.on("tool_execution_end", async (_event, ctx) => updateStatusLine(ctx, deps));
    pi.on("message_end", async (event, ctx) => {
        const role = (event.message as { role?: unknown } | undefined)?.role;
        if (role === "assistant") updateStatusLine(ctx, deps);
    });
    pi.on("session_shutdown", async (_event, ctx) => {
        const sessionId = resolveSessionId(ctx);
        if (sessionId) lastRenderedBySession.delete(sessionId);
        ctx.ui.setStatus(STATUS_KEY, undefined);
    });
}

export function updateStatusLine(ctx: ExtensionContext, deps: StatusLineDeps, force = false): void {
    void deps.projectIdentity;
    const sessionId = resolveSessionId(ctx);
    if (!sessionId) return;
    paintStatusLine(ctx, sessionId, force);
}

function paintStatusLine(ctx: ExtensionContext, sessionId: string, force: boolean): void {
    const text = renderStatusText(ctx, sessionId);
    if (!force && lastRenderedBySession.get(sessionId) === text) return;
    lastRenderedBySession.set(sessionId, text);
    ctx.ui.setStatus(STATUS_KEY, text);
}

export function renderStatusText(ctx: ExtensionContext, sessionId: string): string {
    const usage = ctx.getContextUsage?.();
    const inputTokens =
        typeof usage?.tokens === "number" && Number.isFinite(usage.tokens)
            ? usage.tokens
            : undefined;
    const windowGeometry = resolvePiWindowGeometry({
        rawContextWindow: usage?.contextWindow ?? ctx.model?.contextWindow,
        model: ctx.model,
    });
    const usableSoft = windowGeometry?.usableSoft;
    const pct =
        inputTokens !== undefined && usableSoft !== undefined && usableSoft > 0
            ? (inputTokens / usableSoft) * 100
            : undefined;
    const state = recompSessions.has(sessionId) ? "recomp" : "idle";
    return `eidnara: ${inputTokens === undefined ? "--" : fmt(inputTokens)} (${pct === undefined ? "--" : `${Math.round(pct)}%`}) · ${state}`;
}

function resolveSessionId(ctx: ExtensionContext): string | undefined {
    const getSessionId = (ctx.sessionManager as { getSessionId?: () => string | undefined })
        .getSessionId;
    if (typeof getSessionId !== "function") return undefined;
    try {
        const id = getSessionId.call(ctx.sessionManager);
        return typeof id === "string" && id.length > 0 ? id : undefined;
    } catch {
        return undefined;
    }
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

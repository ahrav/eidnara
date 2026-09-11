import type { PluginContext } from "../../plugin/types";
import { BoundedSessionMap } from "../../shared/bounded-session-map";
import { sha256Hex } from "../../shared/kernel-client";
import { sessionLog } from "../../shared/logger";
import { HOST_SDK_READ_TIMEOUT_MS, withTimeout } from "../../shared/with-timeout";
import { openCodeDbExists, withReadOnlySessionDb } from "./read-session-db";

/**
 *
 * A session's explicit tools map can exclude a globally registered tool.
 * A tools map such as `{"*": false, read: true}` excludes tools not explicitly enabled.
 * Guidance and synthetic calls must not target tools excluded by the session's tools map.
 *
 * The bounded cache retains each tool's first-user-message verdict until eviction.
 * A changing verdict would change provider-visible bytes every turn.
 *
 * The resolver keeps the `ctx_reduce` verdict frozen when live permissions deny the tool because the verdict gates guidance and the system-prompt hash.
 * Changing the ctx_reduce verdict mid-session would invalidate the provider prefix even though permission changes do not alter the prompt.
 * Todowrite permission freshness is independent of the frozen tools map.
 *
 * When the tools map is absent, does not deny the wildcard, or the OpenCode DB is unreadable, availability defaults to true.
 */

/** The verdict records whether its result is final for the session's lifetime. */
export interface ToolAvailabilityVerdict {
    callable: boolean;
    /** `frozen` is true when the verdict comes from the session's first user message.
     * When `frozen` is false, consumers must not persist state derived from the verdict because a later final verdict can change persisted bytes and bust the prompt cache.
     * */
    frozen: boolean;
}

let ctxReduceRegisteredGlobally = true;

/**
 * `resolveCtxReduceAvailability*` returns a frozen `callable: false` verdict when `ctx_reduce` is not registered globally.
 */
export function setCtxReduceRegisteredGlobally(registered: boolean): void {
    ctxReduceRegisteredGlobally = registered;
}

/**
 * */
export function resetCtxReduceRegisteredGloballyForTest(): void {
    ctxReduceRegisteredGlobally = true;
}

export type CtxReduceAvailabilityVerdict = ToolAvailabilityVerdict;

const CTX_REDUCE_TOOL = "ctx_reduce";
const TODOWRITE_TOOL = "todowrite";

/**
 * Verdicts are cached independently for each `(tool, session)` pair.
 * The 1,000-entry cap supports at most 500 sessions when both `ctx_reduce` and `todowrite` have entries.
 */
const availabilityBySession = new BoundedSessionMap<boolean>(1000);

interface PermissionVerdict {
    denied: boolean | undefined;
    expiresAt: number;
    invalidated: boolean;
    pending: Promise<boolean> | undefined;
}

const PERMISSION_TTL_MS = 30_000;
/** The 2,000-entry LRU bounds digest keys and verdict state; concurrent fills share one promise per entry. */
const permissionDeniedBySession = new BoundedSessionMap<PermissionVerdict>(2000);
const ctxReducePermissionDenyLogged = new BoundedSessionMap<boolean>(1000);

type PermissionAction = "ask" | "allow" | "deny";

/* */
export interface PermissionRule {
    permission: string;
    pattern: string;
    action: PermissionAction;
}

function permissionSessionKey(sessionId: string): string {
    return sha256Hex(JSON.stringify(sessionId));
}

function permissionCacheKey(
    toolName: string,
    sessionId: string,
    activeAgent: string | undefined,
): string {
    const identity = JSON.stringify([toolName, activeAgent ?? null]);
    return `${permissionSessionKey(sessionId)}:${sha256Hex(identity)}`;
}

function cacheKey(toolName: string, sessionId: string): string {
    return `${toolName}\u0000${sessionId}`;
}

/** A null result means the tools map carries no signal. */
function verdictFromToolsMap(tools: unknown, toolName: string): boolean | null {
    if (tools === null || typeof tools !== "object" || Array.isArray(tools)) return null;
    const map = tools as Record<string, unknown>;
    if (map[toolName] === true) return true;
    if (map[toolName] === false) return false;
    if (map["*"] === false) return false;
    return null;
}

/**
 * The resolver prefers the in-memory transform message array over the OpenCode DB.
 * The resolver caches verdicts derived from the first user message.
 */
function resolveToolAvailabilityFromMessages(
    sessionId: string,
    toolName: string,
    messages: ReadonlyArray<{ info?: { role?: string; tools?: unknown } }>,
): ToolAvailabilityVerdict {
    if (toolName === CTX_REDUCE_TOOL && !ctxReduceRegisteredGlobally) {
        return { callable: false, frozen: true };
    }
    const key = cacheKey(toolName, sessionId);
    const cached = availabilityBySession.get(key);
    if (cached !== undefined) return { callable: cached, frozen: true };

    for (const message of messages) {
        if (message.info?.role !== "user") continue;
        // First user message decides: explicit signal, or no-signal → available.
        // The first user message always produces a frozen verdict.
        const verdict = verdictFromToolsMap(message.info.tools, toolName) ?? true;
        availabilityBySession.set(key, verdict);
        return { callable: verdict, frozen: true };
    }
    // When no user message exists, the resolver fails open without freezing so the first user message can set the verdict.
    return { callable: true, frozen: false };
}

/**
 * The resolver reads the OpenCode DB and fails open when it is unavailable or unreadable.
 * read fails.
 */
function resolveToolAvailability(sessionId: string, toolName: string): ToolAvailabilityVerdict {
    // Process-global registration override (see resolveToolAvailabilityFromMessages).
    if (toolName === CTX_REDUCE_TOOL && !ctxReduceRegisteredGlobally) {
        return { callable: false, frozen: true };
    }
    const key = cacheKey(toolName, sessionId);
    const cached = availabilityBySession.get(key);
    if (cached !== undefined) return { callable: cached, frozen: true };
    // The resolver freezes the fail-open verdict when no database exists so hash persistence can proceed.
    // Caching the verdict keeps it final: a database that appears later cannot replace a frozen verdict consumers have already persisted.
    if (!openCodeDbExists()) {
        availabilityBySession.set(key, true);
        return { callable: true, frozen: true };
    }
    try {
        const row = withReadOnlySessionDb(
            (db) =>
                db
                    .prepare(
                        `SELECT json_extract(CASE WHEN json_valid(data) THEN data END, '$.tools') AS tools
                          FROM message
                          WHERE session_id = ?
                            AND json_extract(CASE WHEN json_valid(data) THEN data END, '$.role') = 'user'
                          ORDER BY time_created ASC, id ASC LIMIT 1`,
                    )
                    .get(sessionId) as { tools: string | null } | undefined,
        );
        if (!row) return { callable: true, frozen: false }; // session not persisted yet
        const verdict =
            row.tools === null ? null : verdictFromToolsMap(JSON.parse(row.tools), toolName);
        const resolved = verdict ?? true;
        availabilityBySession.set(key, resolved);
        return { callable: resolved, frozen: true };
    } catch (error) {
        sessionLog(sessionId, `${toolName} availability read failed (fail-open):`, error);
        return { callable: true, frozen: false };
    }
}

/** Drop a cached verdict for one tool of one session (test/reset helper). */
function clearToolAvailability(sessionId: string, toolName: string): void {
    availabilityBySession.delete(cacheKey(toolName, sessionId));
}

export function resolveCtxReduceAvailabilityFromMessages(
    sessionId: string,
    messages: ReadonlyArray<{ info?: { role?: string; tools?: unknown } }>,
): CtxReduceAvailabilityVerdict {
    return resolveToolAvailabilityFromMessages(sessionId, CTX_REDUCE_TOOL, messages);
}

export function resolveCtxReduceAvailability(sessionId: string): CtxReduceAvailabilityVerdict {
    return resolveToolAvailability(sessionId, CTX_REDUCE_TOOL);
}

export function clearCtxReduceAvailability(sessionId: string): void {
    clearToolAvailability(sessionId, CTX_REDUCE_TOOL);
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return value !== null && typeof value === "object" && !Array.isArray(value);
}

function responseData(value: unknown): unknown {
    if (isRecord(value) && Object.hasOwn(value, "data")) return value.data;
    return value;
}

function escapeRegExpLiteral(value: string): string {
    return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function permissionNameMatches(rulePermission: string, toolName: string): boolean {
    if (rulePermission === "*" || rulePermission === toolName) return true;
    if (!rulePermission.includes("*")) return false;
    const pattern = rulePermission.split("*").map(escapeRegExpLiteral).join(".*");
    return new RegExp(`^${pattern}$`).test(toolName);
}

/**
 * The last matching `Permission.disabled` rule wins; only a deny of the whole permission pattern disables it.
 */
export function permissionDisabled(toolName: string, rules: readonly PermissionRule[]): boolean {
    let finalRule: PermissionRule | undefined;
    for (let index = rules.length - 1; index >= 0; index -= 1) {
        const rule = rules[index];
        if (rule && permissionNameMatches(rule.permission, toolName)) {
            finalRule = rule;
            break;
        }
    }
    return finalRule?.action === "deny" && finalRule.pattern === "*";
}

function actionOf(value: unknown): PermissionAction | null {
    return value === "ask" || value === "allow" || value === "deny" ? value : null;
}

function appendPermissionRule(
    target: PermissionRule[],
    permission: unknown,
    pattern: unknown,
    action: unknown,
): void {
    if (typeof permission !== "string" || permission.length === 0) {
        throw new Error("OpenCode permission name is malformed");
    }
    const normalizedAction = actionOf(action);
    if (!normalizedAction) throw new Error("OpenCode permission action is malformed");
    const patterns = Array.isArray(pattern) ? pattern : [pattern === undefined ? "*" : pattern];
    for (const candidate of patterns) {
        if (typeof candidate !== "string") {
            throw new Error("OpenCode permission pattern is malformed");
        }
        target.push({ permission, pattern: candidate, action: normalizedAction });
    }
}

/** The normalizer accepts both OpenCode's object shorthand and its already-expanded rules. */
function permissionRules(value: unknown): PermissionRule[] {
    if (value === undefined) return [];
    if (Array.isArray(value)) {
        const result: PermissionRule[] = [];
        for (const item of value) {
            if (!isRecord(item)) throw new Error("OpenCode permission rule is malformed");
            appendPermissionRule(
                result,
                item.permission ?? item.tool ?? item.name,
                item.pattern,
                item.action ?? item.value,
            );
        }
        return result;
    }
    if (!isRecord(value)) throw new Error("OpenCode permission payload is malformed");

    const result: PermissionRule[] = [];
    if (Object.hasOwn(value, "rules")) {
        if (!Array.isArray(value.rules)) {
            throw new Error("OpenCode permission rules are malformed");
        }
        result.push(...permissionRules(value.rules));
    }
    for (const [permission, configured] of Object.entries(value)) {
        if (permission === "rules") continue;
        const simpleAction = actionOf(configured);
        if (simpleAction) {
            // OpenCode interprets a simple string permission as a whole-tool rule.
            appendPermissionRule(result, permission, "*", simpleAction);
            continue;
        }
        if (!isRecord(configured)) {
            throw new Error("OpenCode permission configuration is malformed");
        }
        for (const [pattern, action] of Object.entries(configured)) {
            appendPermissionRule(result, permission, pattern, action);
        }
    }
    return result;
}

/**
 * Session rules follow agent rules, so later session rules override agent rules.
 *
 * The caller supplies the active agent for agent-rule lookup; the SDK `Session` payload carries no agent field.
 * An `undefined` agent skips the agent-list API and evaluates session rules alone.
 */
export async function resolveToolPermissionDenied(
    client: PluginContext["client"] | undefined,
    sessionId: string,
    toolName: string,
    activeAgent: string | undefined,
): Promise<boolean> {
    if (!client?.session?.get || (activeAgent !== undefined && !client.app?.agents)) {
        sessionLog(sessionId, `${toolName} permission APIs are unavailable (fail-closed)`);
        return true;
    }
    const key = permissionCacheKey(toolName, sessionId, activeAgent);
    const cached = permissionDeniedBySession.get(key);
    const startedAt = performance.now();
    if (!cached?.invalidated && cached?.denied !== undefined && startedAt < cached.expiresAt) {
        return cached.denied;
    }

    let entry = cached;
    if (!entry?.pending || entry.invalidated) {
        // Replacement fences invalidated or evicted fills without a separate generation map.
        entry = { denied: cached?.denied, expiresAt: 0, invalidated: false, pending: undefined };
        permissionDeniedBySession.set(key, entry);
        const filling = entry;
        const expiresAt = startedAt + PERMISSION_TTL_MS;
        filling.pending = (async () => {
            try {
                const denied = await withTimeout(
                    readToolPermissionDenied(client, sessionId, toolName, activeAgent),
                    HOST_SDK_READ_TIMEOUT_MS,
                    `${toolName} permission read timed out`,
                );
                if (
                    permissionDeniedBySession.peek(key) !== filling ||
                    filling.invalidated ||
                    performance.now() >= expiresAt
                ) {
                    return true;
                }
                filling.denied = denied;
                // Read-start expiry also rejects results delayed by an event-loop stall.
                filling.expiresAt = expiresAt;
                return denied;
            } catch (error) {
                sessionLog(sessionId, `${toolName} permission read failed (fail-closed):`, error);
                return true;
            } finally {
                filling.pending = undefined;
            }
        })();
    }
    const denied = await entry.pending;
    return permissionDeniedBySession.peek(key) === entry &&
        !entry.invalidated &&
        performance.now() < entry.expiresAt
        ? (denied ?? true)
        : true;
}

async function readToolPermissionDenied(
    client: PluginContext["client"],
    sessionId: string,
    toolName: string,
    activeAgent: string | undefined,
): Promise<boolean> {
    const [agentsResponse, sessionResponse] = await Promise.all([
        activeAgent === undefined ? undefined : client.app.agents(),
        client.session.get({ path: { id: sessionId } }),
    ]);
    const session = responseData(sessionResponse);
    if (!isRecord(session) || (isRecord(sessionResponse) && sessionResponse.error != null)) {
        throw new Error("OpenCode permission response is unavailable");
    }
    let agentRules: PermissionRule[] = [];
    if (activeAgent !== undefined) {
        const agents = responseData(agentsResponse);
        if (!Array.isArray(agents) || (isRecord(agentsResponse) && agentsResponse.error != null)) {
            throw new Error("OpenCode agent permission response is unavailable");
        }
        const agent = agents.find(
            (candidate) => isRecord(candidate) && candidate.name === activeAgent,
        );
        if (!isRecord(agent)) {
            throw new Error("OpenCode active agent permission evidence is unavailable");
        }
        agentRules = permissionRules(agent.permission);
    }
    const sessionRules = permissionRules(
        session.permission === undefined ? session.permissions : session.permission,
    );
    return permissionDisabled(toolName, [...agentRules, ...sessionRules]);
}

export function todowritePermissionDenied(
    client: PluginContext["client"] | undefined,
    sessionId: string,
    activeAgent: string | undefined,
): Promise<boolean> {
    return resolveToolPermissionDenied(client, sessionId, TODOWRITE_TOOL, activeAgent);
}

/** Returns the last successful verdict, including stale entries, without claiming freshness. */
export function peekToolPermissionDeniedForTest(
    sessionId: string,
    toolName: string,
    activeAgent: string | undefined,
): boolean | undefined {
    return permissionDeniedBySession.peek(permissionCacheKey(toolName, sessionId, activeAgent))
        ?.denied;
}

/** Marks one session's cached verdicts stale so the next read fills again instead of serving them. */
export function invalidateToolPermissionDenied(sessionId: string): void {
    const prefix = `${permissionSessionKey(sessionId)}:`;
    for (const [key, entry] of permissionDeniedBySession.entries()) {
        if (key.startsWith(prefix)) entry.invalidated = true;
    }
}

export function clearToolPermissionDenied(sessionId: string): void {
    const prefix = `${permissionSessionKey(sessionId)}:`;
    for (const [key] of permissionDeniedBySession.entries()) {
        if (key.startsWith(prefix)) permissionDeniedBySession.delete(key);
    }
    ctxReducePermissionDenyLogged.delete(sessionId);
}

export function hasLoggedCtxReducePermissionDeny(sessionId: string): boolean {
    return ctxReducePermissionDenyLogged.get(sessionId) === true;
}

export function markCtxReducePermissionDenyLogged(sessionId: string): void {
    ctxReducePermissionDenyLogged.set(sessionId, true);
}

export function resolveTodowriteAvailabilityFromMessages(
    sessionId: string,
    messages: ReadonlyArray<{ info?: { role?: string; tools?: unknown } }>,
): ToolAvailabilityVerdict {
    return resolveToolAvailabilityFromMessages(sessionId, TODOWRITE_TOOL, messages);
}

export function resolveTodowriteAvailability(sessionId: string): ToolAvailabilityVerdict {
    return resolveToolAvailability(sessionId, TODOWRITE_TOOL);
}

export function clearTodowriteAvailability(sessionId: string): void {
    clearToolAvailability(sessionId, TODOWRITE_TOOL);
}

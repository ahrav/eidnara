/**
 *
 *
 * TUI shows a startup dialog; this module handles Desktop.
 */

import { existsSync, readFileSync } from "node:fs";
import { homedir, platform } from "node:os";
import { join } from "node:path";
import { refreshOpenCodeDbPresence, withReadOnlySessionDb } from "../hooks/context/read-session-db";
import {
    dropQueuedIgnoredMessages,
    sendIgnoredMessage,
} from "../hooks/context/send-session-notification";
import {
    CONFLICT_DISABLED_HEADER,
    CONFLICT_WARNING_HEADER,
    type ConflictWarning,
    formatConflictShort,
} from "../shared/conflict-detector";
import { FOLD_AUTHORITY_WARNING_MARKERS } from "../shared/fold-authority-status";
import { log } from "../shared/logger";
import { normalizeSDKResponse } from "../shared/normalize-sdk-response";
import type { SqliteReader } from "../shared/sqlite";
import { jsonField } from "../shared/sqlite-helpers";

const CONFLICT_WARNING_MARKERS = [CONFLICT_DISABLED_HEADER, CONFLICT_WARNING_HEADER];
const ENABLED_MARKER = "✨ Eidnara is now enabled";

function getDesktopStatePath(): string | null {
    const os = platform();
    const home = homedir();

    if (os === "darwin") {
        return join(
            home,
            "Library",
            "Application Support",
            "ai.opencode.desktop",
            "opencode.global.dat",
        );
    }
    if (os === "linux") {
        const xdgConfig = process.env.XDG_CONFIG_HOME || join(home, ".config");
        return join(xdgConfig, "ai.opencode.desktop", "opencode.global.dat");
    }
    if (os === "win32") {
        const appData = process.env.APPDATA || join(home, "AppData", "Roaming");
        return join(appData, "ai.opencode.desktop", "opencode.global.dat");
    }

    return null;
}

interface DesktopState {
    sessionId: string | null;
    sidecarUrl: string | null;
}

function readDesktopState(directory: string): DesktopState {
    const statePath = getDesktopStatePath();
    if (!statePath || !existsSync(statePath)) {
        log(`[eidnara] conflict-warning: Desktop state file not found at ${statePath}`);
        return { sessionId: null, sidecarUrl: null };
    }

    try {
        const raw = readFileSync(statePath, "utf-8");
        const state = JSON.parse(raw) as Record<string, unknown>;

        let sidecarUrl: string | null = null;
        const serverStr = state.server;
        if (typeof serverStr === "string") {
            try {
                const serverState = JSON.parse(serverStr) as Record<string, unknown>;
                if (typeof serverState.currentSidecarUrl === "string") {
                    sidecarUrl = serverState.currentSidecarUrl;
                }
            } catch (error) {
                log(
                    `[eidnara] conflict-warning: Desktop server state is malformed; continuing without a sidecar URL: ${error instanceof Error ? error.message : String(error)}`,
                );
            }
        }

        let sessionId: string | null = null;
        const layoutPage = state["layout.page"];
        if (typeof layoutPage === "string") {
            const parsed = JSON.parse(layoutPage) as Record<string, unknown>;
            const lastProjectSession = parsed.lastProjectSession as
                | Record<string, { id?: string }>
                | undefined;
            if (lastProjectSession) {
                const entry = lastProjectSession[directory];
                sessionId = entry?.id ?? null;
            }
        }

        return { sessionId, sidecarUrl };
    } catch (error) {
        log(
            `[eidnara] conflict-warning: failed to read Desktop state: ${error instanceof Error ? error.message : String(error)}`,
        );
        return { sessionId: null, sidecarUrl: null };
    }
}

async function deleteMessage(
    serverUrl: string,
    sessionId: string,
    messageId: string,
): Promise<boolean> {
    // OpenCode's Session2 wrapper doesn't expose deleteMessage.
    const auth = getServerAuth();
    const url = `${serverUrl}/session/${encodeURIComponent(sessionId)}/message/${encodeURIComponent(messageId)}`;

    try {
        const response = await fetch(url, {
            method: "DELETE",
            headers: auth ? { Authorization: auth } : {},
            signal: AbortSignal.timeout(10_000),
        });

        if (!response.ok) {
            log(`[eidnara] conflict-warning: DELETE failed status=${response.status} url=${url}`);
            return false;
        }
        return true;
    } catch (error) {
        log(
            `[eidnara] conflict-warning: DELETE error (url=${serverUrl}): ${error instanceof Error ? error.message : String(error)}`,
        );
        return false;
    }
}

function getServerAuth(): string | undefined {
    const password = process.env.OPENCODE_SERVER_PASSWORD;
    if (!password) return undefined;
    const username = process.env.OPENCODE_SERVER_USERNAME ?? "opencode";
    return `Basic ${Buffer.from(`${username}:${password}`, "utf8").toString("base64")}`;
}

type SdkMessage = {
    info?: { id?: string; role?: string; sessionID?: string };
    parts?: Array<{ type?: string; text?: string; ignored?: boolean }>;
};

async function getSessionMessages(client: unknown, sessionId: string): Promise<SdkMessage[]> {
    try {
        const c = client as {
            session?: {
                messages?: (input: {
                    path: { id: string };
                    query?: { limit?: number };
                }) => Promise<{ data?: SdkMessage[] } | SdkMessage[]>;
            };
        };

        if (typeof c.session?.messages === "function") {
            // The SDK exposes no cursor, so this window is the bounded fallback behind the database scan.
            const result = await c.session.messages({
                path: { id: sessionId },
                query: { limit: 50 },
            });
            return normalizeSDKResponse(result, [] as SdkMessage[]);
        }
    } catch (error) {
        log(
            `[eidnara] conflict-warning: failed to read messages: ${error instanceof Error ? error.message : String(error)}`,
        );
    }
    return [];
}

/**
 * Search every fetched message because a warning can precede later user and
 * assistant turns.
 */
function findIgnoredMarkerMessageIds(messages: SdkMessage[], marker: string): string[] {
    const ids: string[] = [];
    for (const msg of messages) {
        const msgId = msg.info?.id;
        if (!msgId || msg.info?.role !== "user") continue;

        const parts = msg.parts ?? [];
        const matches =
            parts.length > 0 &&
            parts.every(
                (p) =>
                    p.ignored === true &&
                    p.type === "text" &&
                    typeof p.text === "string" &&
                    p.text.startsWith(marker),
            );
        if (matches) ids.push(msgId);
    }
    return ids;
}

/** The same predicate as `findIgnoredMarkerMessageIds`, evaluated over every row of the session. `substr` compares the marker prefix so marker text is never read as a `LIKE` pattern. */
const MARKER_MESSAGE_IDS_SQL = `
SELECT m.id AS id
FROM message m
WHERE m.session_id = ?
  AND ${jsonField("m.data", "$.role")} = 'user'
  AND EXISTS (SELECT 1 FROM part p WHERE p.message_id = m.id)
  AND NOT EXISTS (
    SELECT 1 FROM part p
    WHERE p.message_id = m.id
      AND NOT (
        COALESCE(${jsonField("p.data", "$.ignored")}, 0) IN (1, 'true')
        AND ${jsonField("p.data", "$.type")} = 'text'
        AND substr(COALESCE(${jsonField("p.data", "$.text")}, ''), 1, length(?)) = ?
      )
  )
ORDER BY m.time_created, m.id`;

export function findIgnoredMarkerMessageIdsFromDb(
    db: SqliteReader,
    sessionId: string,
    marker: string,
): string[] {
    const rows = db.prepare(MARKER_MESSAGE_IDS_SQL).all(sessionId, marker, marker) as Array<{
        id?: unknown;
    }>;
    return rows
        .map((row) => row.id)
        .filter((id): id is string => typeof id === "string" && id.length > 0);
}

/**
 * OpenCode's database sees the whole session, so a warning buried under more
 * than the SDK window of later turns is still found. The SDK fetch is the
 * fallback when the database is unavailable.
 */
async function findMarkerMessageIds(
    client: unknown,
    sessionId: string,
    markers: readonly string[],
): Promise<string[][]> {
    if (refreshOpenCodeDbPresence()) {
        try {
            return withReadOnlySessionDb((db) =>
                markers.map((marker) => findIgnoredMarkerMessageIdsFromDb(db, sessionId, marker)),
            );
        } catch (error) {
            log(
                `[eidnara] conflict-warning: database scan failed, falling back to the SDK window: ${error instanceof Error ? error.message : String(error)}`,
            );
        }
    }
    const messages = await getSessionMessages(client, sessionId);
    return markers.map((marker) => findIgnoredMarkerMessageIds(messages, marker));
}

/**
 * Deletes all messages concurrently so an endpoint that accepts connections but
 * never answers costs one request timeout, not one per message. Returns IDs
 * whose DELETE request failed.
 */
async function deleteMessages(
    serverUrl: string,
    sessionId: string,
    messageIds: string[],
): Promise<string[]> {
    const results = await Promise.all(
        messageIds.map(async (messageId) => ({
            messageId,
            ok: await deleteMessage(serverUrl, sessionId, messageId),
        })),
    );
    return results.filter((r) => !r.ok).map((r) => r.messageId);
}

/**
 */
export async function sendConflictWarning(
    client: unknown,
    directory: string,
    conflictResult: ConflictWarning,
    serverUrl?: string,
): Promise<void> {
    const { sessionId, sidecarUrl } = readDesktopState(directory);
    if (!sessionId) {
        log("[eidnara] conflict-warning: could not find active session for Desktop warning");
        return;
    }

    const disabled = conflictResult.disposition === "disable";
    const [header, otherHeader] = disabled
        ? [CONFLICT_DISABLED_HEADER, CONFLICT_WARNING_HEADER]
        : [CONFLICT_WARNING_HEADER, CONFLICT_DISABLED_HEADER];
    const [found = [], superseded = [], ...live] = await findMarkerMessageIds(client, sessionId, [
        header,
        otherHeader,
        ...FOLD_AUTHORITY_WARNING_MARKERS,
    ]);
    // `reconcileFoldAuthorityWarning` owns the live fold-authority messages under the same header.
    const liveIds = new Set(live.flat());
    const existing = found.filter((id) => !liveIds.has(id));
    // A persisted message under the other header states the other disposition, which no longer holds.
    const deleteUrl = serverUrl ?? sidecarUrl ?? undefined;
    if (superseded.length > 0 && deleteUrl) {
        const failedIds = await deleteMessages(deleteUrl, sessionId, superseded);
        if (failedIds.length > 0) {
            log(
                `[eidnara] conflict-warning: ${failedIds.length} superseded warning message(s) still present in session ${sessionId}`,
            );
        }
    }

    // Conflict detection re-fires on every startup; a warning already in the session is not repeated.
    if (existing.length > 0) {
        log(
            `[eidnara] conflict-warning: session ${sessionId} already carries ${existing.length} warning(s); not sending another`,
        );
        return;
    }

    const warningText = formatConflictShort(conflictResult);

    log(
        `[eidnara] sending conflict warning to session ${sessionId}: ${conflictResult.reasons.join(", ")}`,
    );

    // forcePersist: the warning describes a blocking state the user must act
    // on, and cleanupConflictWarnings deletes the persisted row once the
    // conflict is resolved — a transient toast would satisfy neither. The
    // helper owns the title-safety guard, the mid-turn queue, and prompt-
    // context pinning; conflict detection re-fires on every startup, so a
    // skipped delivery retries on the next launch.
    await sendIgnoredMessage(client, sessionId, warningText, {}, true);
}

/** One in-flight reconciliation per session; a later poll waits for the earlier one. */
const reconciling = new Map<string, Promise<void>>();

/**
 * Keeps at most one live fold-authority message in the session, equal to `warning`. A changed
 * warning replaces the message an earlier poll persisted, a cleared warning deletes it, and startup
 * warnings under the same header stay in place. A failed deletion blocks the replacement, which
 * preserves the one-message bound. Polls of one session reconcile in call order.
 */
export function reconcileFoldAuthorityWarning(
    client: unknown,
    directory: string,
    sessionId: string,
    warning: ConflictWarning | undefined,
    serverUrl?: string,
): Promise<void> {
    const previous = reconciling.get(sessionId) ?? Promise.resolve();
    const run = previous
        .catch(() => {})
        .then(() => reconcileNow(client, directory, sessionId, warning, serverUrl));
    reconciling.set(sessionId, run);
    const settle = () => {
        if (reconciling.get(sessionId) === run) reconciling.delete(sessionId);
    };
    run.then(settle, settle);
    return run;
}

async function reconcileNow(
    client: unknown,
    directory: string,
    sessionId: string,
    warning: ConflictWarning | undefined,
    serverUrl?: string,
): Promise<void> {
    const text = warning === undefined ? undefined : formatConflictShort(warning);
    // Dropping queued fold-authority warnings keeps the idle flush consistent with the warning
    // this poll reports.
    dropQueuedIgnoredMessages(sessionId, FOLD_AUTHORITY_WARNING_MARKERS);
    const markers =
        text === undefined
            ? FOLD_AUTHORITY_WARNING_MARKERS
            : [...FOLD_AUTHORITY_WARNING_MARKERS, text];
    const found = await findMarkerMessageIds(client, sessionId, markers);
    const current =
        text === undefined ? [] : (found[FOLD_AUTHORITY_WARNING_MARKERS.length] ?? []).slice(0, 1);
    const stale = [...new Set(found.slice(0, FOLD_AUTHORITY_WARNING_MARKERS.length).flat())].filter(
        (id) => !current.includes(id),
    );
    if (stale.length > 0) {
        const deleteUrl = serverUrl ?? readDesktopState(directory).sidecarUrl ?? undefined;
        const failed = deleteUrl ? await deleteMessages(deleteUrl, sessionId, stale) : stale;
        if (failed.length > 0) {
            log(
                `[eidnara] fold-authority warning: ${failed.length} outdated message(s) remain in session ${sessionId}; not sending a replacement`,
            );
            return;
        }
    }
    if (text === undefined || current.length > 0) return;
    log(`[eidnara] sending fold-authority warning to session ${sessionId}`);
    await sendIgnoredMessage(client, sessionId, text, {}, true);
}

/**
 * The plugin removes leftover conflict-warning messages from disabled and warning runs.
 * The "enabled" confirmation follows a removed disabled-run message; a warning run
 * kept the plugin enabled, so its message is removed silently.
 */
export async function cleanupConflictWarnings(
    client: unknown,
    directory: string,
    serverUrl?: string,
): Promise<void> {
    const { sessionId, sidecarUrl } = readDesktopState(directory);
    if (!sessionId) {
        log("[eidnara] cleanup: no active Desktop session found");
        return;
    }
    const deleteUrl = serverUrl ?? sidecarUrl ?? undefined;
    const [disabledMessageIds = [], configurationWarningIds = []] = await findMarkerMessageIds(
        client,
        sessionId,
        CONFLICT_WARNING_MARKERS,
    );
    const warningMessageIds = [...disabledMessageIds, ...configurationWarningIds];

    if (warningMessageIds.length === 0) {
        await cleanupEnabledMessages(client, deleteUrl, sessionId);
        return;
    }

    if (!deleteUrl) {
        log("[eidnara] cleanup: no serverUrl provided, cannot delete messages");
        return;
    }

    log(
        `[eidnara] cleaning up ${warningMessageIds.length} conflict warning message(s) from session ${sessionId}`,
    );

    const failedIds = await deleteMessages(deleteUrl, sessionId, warningMessageIds);
    for (const messageId of warningMessageIds) {
        if (!failedIds.includes(messageId)) {
            log(`[eidnara] deleted conflict warning message ${messageId}`);
        }
    }
    if (failedIds.length > 0) {
        log(
            `[eidnara] cleanup: ${failedIds.length} conflict warning message(s) still present; skipping the enabled confirmation until the next startup deletes them`,
        );
        return;
    }
    if (disabledMessageIds.length === 0) return;

    // Send a brief "enabled" confirmation so the user sees the conflict is
    // resolved. The confirmation is transient by design (the timer below
    // removes a persisted copy after a second), so the helper's toast-first
    // path is the right surface when a TUI is connected; the warning cleanup
    // above already ran — only the confirmation is skippable.
    const enabledText = `${ENABLED_MARKER}. Enjoy! ✨`;
    const disposition = await sendIgnoredMessage(client, sessionId, enabledText, {});
    // Schedule the auto-remove only for a confirmed persisted post: a toast
    // leaves no row, and a queued/skipped delivery has no row yet — a copy
    // flushed later is reclaimed by cleanupEnabledMessages on the next
    // startup instead.
    if (disposition !== "sent") return;

    // The plugin removes the "enabled" message after 1 second so it does not persist across restarts.
    // The plugin identifies enabled confirmations by ENABLED_MARKER and the ignored flag to avoid deleting user messages.
    setTimeout(async () => {
        try {
            await cleanupEnabledMessages(client, deleteUrl, sessionId);
        } catch {
            // Best-effort cleanup
        }
    }, 1000);
}

/** The startup cleanup removes enabled messages left by an earlier run. */
async function cleanupEnabledMessages(
    client: unknown,
    serverUrl: string | undefined,
    sessionId: string,
): Promise<void> {
    if (!serverUrl) return;
    const [enabledMessageIds = []] = await findMarkerMessageIds(client, sessionId, [
        ENABLED_MARKER,
    ]);
    if (enabledMessageIds.length === 0) return;
    await deleteMessages(serverUrl, sessionId, enabledMessageIds);
}

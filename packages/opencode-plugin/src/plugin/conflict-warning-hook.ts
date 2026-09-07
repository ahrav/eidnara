/**
 *
 *
 * TUI shows a startup dialog; this module handles Desktop.
 */

import { existsSync, readFileSync } from "node:fs";
import { homedir, platform } from "node:os";
import { join } from "node:path";
import { sendIgnoredMessage } from "../hooks/context/send-session-notification";
import type { ConflictResult } from "../shared/conflict-detector";
import { formatConflictShort } from "../shared/conflict-detector";
import { log } from "../shared/logger";

const CONFLICT_WARNING_MARKER = "⚠️ Eidnara is disabled due to conflicting configuration:";
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
            } catch {}
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

const cachedDesktopStateByDir = new Map<string, DesktopState>();

function getDesktopState(directory: string): DesktopState {
    let cached = cachedDesktopStateByDir.get(directory);
    if (!cached) {
        cached = readDesktopState(directory);
        cachedDesktopStateByDir.set(directory, cached);
    }
    return cached;
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
                }) => Promise<{ data?: SdkMessage[] }>;
            };
        };

        if (typeof c.session?.messages === "function") {
            // Bounded limit prevents loading the entire session into memory.
            const result = await c.session.messages({
                path: { id: sessionId },
                query: { limit: 50 },
            });
            return result?.data ?? [];
        }
    } catch (error) {
        log(
            `[eidnara] conflict-warning: failed to read messages: ${error instanceof Error ? error.message : String(error)}`,
        );
    }
    return [];
}

/**
 */
export async function sendConflictWarning(
    client: unknown,
    directory: string,
    conflictResult: ConflictResult,
): Promise<void> {
    const { sessionId } = getDesktopState(directory);
    if (!sessionId) {
        log("[eidnara] conflict-warning: could not find active session for Desktop warning");
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

/**
 * The plugin removes leftover conflict-warning messages from disabled runs.
 */
export async function cleanupConflictWarnings(
    client: unknown,
    directory: string,
    serverUrl?: string,
): Promise<void> {
    const { sessionId } = getDesktopState(directory);
    if (!sessionId) {
        log("[eidnara] cleanup: no active Desktop session found");
        return;
    }
    const messages = await getSessionMessages(client, sessionId);
    if (messages.length === 0) return;

    const warningMessageIds: string[] = [];
    for (let i = messages.length - 1; i >= 0; i--) {
        const msg = messages[i];
        const msgId = msg.info?.id;
        const msgRole = msg.info?.role;
        if (!msgId || msgRole !== "user") break;

        const parts = msg.parts ?? [];
        const isWarning =
            parts.length > 0 &&
            parts.every(
                (p) =>
                    p.ignored === true &&
                    p.type === "text" &&
                    typeof p.text === "string" &&
                    p.text.startsWith(CONFLICT_WARNING_MARKER),
            );

        if (isWarning) {
            warningMessageIds.push(msgId);
        } else {
            break; // Stop at the first non-warning message from the tail
        }
    }

    if (warningMessageIds.length === 0) {
        await cleanupEnabledMessages(messages, serverUrl, sessionId);
        return;
    }

    if (!serverUrl) {
        log("[eidnara] cleanup: no serverUrl provided, cannot delete messages");
        return;
    }

    log(
        `[eidnara] cleaning up ${warningMessageIds.length} conflict warning message(s) from session ${sessionId}`,
    );

    for (const messageId of warningMessageIds) {
        const ok = await deleteMessage(serverUrl, sessionId, messageId);
        if (ok) {
            log(`[eidnara] deleted conflict warning message ${messageId}`);
        }
    }

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
            const freshMessages = await getSessionMessages(client, sessionId);
            for (let i = freshMessages.length - 1; i >= 0; i--) {
                const msg = freshMessages[i];
                const msgId = msg.info?.id;
                const msgRole = msg.info?.role;
                if (!msgId || msgRole !== "user") break;

                const parts = msg.parts ?? [];
                const isEnabled =
                    parts.length > 0 &&
                    parts.every(
                        (p) =>
                            p.ignored === true &&
                            p.type === "text" &&
                            typeof p.text === "string" &&
                            p.text.startsWith(ENABLED_MARKER),
                    );

                if (isEnabled) {
                    await deleteMessage(serverUrl, sessionId, msgId);
                } else {
                    break;
                }
            }
        } catch {
            // Best-effort cleanup
        }
    }, 1000);
}

/** The startup cleanup removes enabled messages left by an earlier run. */
async function cleanupEnabledMessages(
    messages: SdkMessage[],
    serverUrl: string | undefined,
    sessionId: string,
): Promise<void> {
    if (!serverUrl) return;
    for (let i = messages.length - 1; i >= 0; i--) {
        const msg = messages[i];
        const msgId = msg.info?.id;
        const msgRole = msg.info?.role;
        if (!msgId || msgRole !== "user") break;

        const parts = msg.parts ?? [];
        const isEnabled =
            parts.length > 0 &&
            parts.every(
                (p) =>
                    p.ignored === true &&
                    p.type === "text" &&
                    typeof p.text === "string" &&
                    p.text.startsWith(ENABLED_MARKER),
            );

        if (isEnabled) {
            await deleteMessage(serverUrl, sessionId, msgId);
        } else {
            break;
        }
    }
}

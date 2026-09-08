import { getErrorMessage } from "../../shared/error-message";
import { sessionLog } from "../../shared/logger";
import type { SafeTargetOptions } from "../../shared/safe-notification-target";
import { TimeoutError, withTimeout } from "../../shared/with-timeout";
import { isMidTurn } from "./read-session-db";

export interface NotificationParams {
    agent?: string;
    variant?: string;
    providerId?: string;
    modelId?: string;
    /* */
    toastDurationMs?: number;
    forcePersist?: boolean;
}

export type NotificationDeliveryDisposition = "sent" | "queued" | "skipped" | "failed" | "unknown";

/** A `noReply` prompt only persists a row, so a delivery still pending after this long is aborted and reported with an unknown outcome. */
const NOTIFICATION_SEND_TIMEOUT_MS = 10_000;
let notificationSendTimeoutMs = NOTIFICATION_SEND_TIMEOUT_MS;
let safeTargetOptions: SafeTargetOptions | undefined;

/**
 * Because notifications are status lines rather than user input, the queue keeps only the newest entries.
 * The per-session limit prevents an active turn from accumulating more than 16 queued notifications.
 * The limit caps an idle-boundary backlog at 16 user rows.
 */
export const MAX_QUEUED_IGNORED_NOTIFICATIONS = 16;

/** A queued notification is dropped after this many idle flushes that could not deliver it. */
export const MAX_QUEUED_NOTIFICATION_DELIVERY_ATTEMPTS = 3;

interface IgnoredNotification {
    client: unknown;
    sessionId: string;
    text: string;
    params: NotificationParams;
    forcePersist: boolean;
    /** Idle flushes that ended in a bounded delivery failure for this entry. */
    attempts: number;
    countQueuedAttempt: boolean;
}

const queuedIgnoredNotifications = new Map<string, IgnoredNotification[]>();
const flushingIgnoredNotifications = new Set<string>();
let midTurnDetector = (sessionId: string): boolean => isMidTurn(undefined, sessionId);

function storeQueuedNotifications(sessionId: string, queued: IgnoredNotification[]): void {
    if (queued.length > MAX_QUEUED_IGNORED_NOTIFICATIONS) {
        queued.splice(0, queued.length - MAX_QUEUED_IGNORED_NOTIFICATIONS);
        sessionLog(
            sessionId,
            `ignored notification queue full; dropped oldest entries (kept newest ${MAX_QUEUED_IGNORED_NOTIFICATIONS})`,
        );
    }
    queuedIgnoredNotifications.set(sessionId, queued);
}

function queueIgnoredNotification(notification: IgnoredNotification): void {
    const queued = queuedIgnoredNotifications.get(notification.sessionId) ?? [];
    queued.push(notification);
    storeQueuedNotifications(notification.sessionId, queued);
}

/** Restore the interrupted flush batch ahead of later arrivals to preserve chronological delivery order. */
function requeueFlushBatch(sessionId: string, batch: IgnoredNotification[]): void {
    const arrived = queuedIgnoredNotifications.get(sessionId) ?? [];
    storeQueuedNotifications(sessionId, [...batch, ...arrived]);
}

async function trySendTuiToast(
    sessionId: string,
    text: string,
    params: NotificationParams,
    forcePersist: boolean,
): Promise<boolean> {
    if (forcePersist) return false;

    const title = extractToastTitle(text);
    const message = text.length > 200 ? `${text.slice(0, 200)}…` : text;
    const toastVariant = inferToastVariant(text);
    const { isTuiConnected: checkTui } = await import("../../shared/rpc-notifications");
    if (!checkTui(sessionId)) return false;

    try {
        const { pushNotification } = await import("../../shared/rpc-notifications");
        // The TUI treats a present `duration` as a per-call override; omitting it applies the configured `toast_duration_ms`.
        pushNotification(
            "toast",
            {
                title,
                message,
                variant: toastVariant,
                ...(params.toastDurationMs === undefined
                    ? {}
                    : { duration: params.toastDurationMs }),
            },
            sessionId,
        );
        return true;
    } catch {
        // An RPC enqueue failure falls through to the persisted ignored-message path.
        sessionLog(sessionId, "TUI RPC toast enqueue failed, falling back to ignored message");
        return false;
    }
}

/** Production reads the OpenCode DB signal; tests replace it through __ignoredNotificationTest. */
export const __ignoredNotificationTest = {
    pendingTexts(sessionId: string): string[] {
        return (queuedIgnoredNotifications.get(sessionId) ?? []).map((item) => item.text);
    },
    reset(): void {
        queuedIgnoredNotifications.clear();
        flushingIgnoredNotifications.clear();
        midTurnDetector = (sessionId: string): boolean => isMidTurn(undefined, sessionId);
        notificationSendTimeoutMs = NOTIFICATION_SEND_TIMEOUT_MS;
        safeTargetOptions = undefined;
    },
    setMidTurnDetector(detector: (sessionId: string) => boolean): void {
        midTurnDetector = detector;
    },
    setSendTimeoutMs(timeoutMs: number): void {
        notificationSendTimeoutMs = timeoutMs;
    },
    setSafeTargetOptions(options: SafeTargetOptions): void {
        safeTargetOptions = options;
    },
};

interface NotificationClient {
    session?: {
        prompt?: (opts: unknown) => unknown | Promise<unknown>;
        promptAsync?: (opts: unknown) => Promise<unknown>;
    };
}

function hasNotificationSessionClient(client: unknown): client is NotificationClient {
    if (client === null || typeof client !== "object") return false;
    const candidate = client as Record<string, unknown>;
    if (candidate.session === undefined) return true;
    if (candidate.session === null || typeof candidate.session !== "object") return false;
    const session = candidate.session as Record<string, unknown>;
    return (
        (session.prompt === undefined || typeof session.prompt === "function") &&
        (session.promptAsync === undefined || typeof session.promptAsync === "function")
    );
}

/**
 */
function inferToastVariant(text: string): "success" | "error" | "warning" | "info" {
    const lower = text.toLowerCase();
    if (lower.includes("error") || lower.includes("failed") || lower.includes("alert"))
        return "error";
    if (lower.includes("warning") || lower.includes("⚠")) return "warning";
    if (
        lower.includes("complete") ||
        lower.includes("success") ||
        lower.includes("✓") ||
        lower.includes("finished")
    )
        return "success";
    return "info";
}

/**
 */
function extractToastTitle(text: string): string {
    const headingMatch = text.match(/^#+\s+(.+)/m);
    if (headingMatch) return headingMatch[1].trim();
    const firstLine = text.split("\n")[0].trim();
    if (firstLine.length <= 80) return firstLine;
    return "Eidnara";
}

/** A `"queued"` result leaves the queue untouched; the caller chooses the entry's position. */
async function deliverIgnoredMessage(
    notification: IgnoredNotification,
): Promise<NotificationDeliveryDisposition> {
    const { client, sessionId, text, params, forcePersist } = notification;
    notification.countQueuedAttempt = false;

    // TUI notifications are already out-of-band and do not create a user row.
    if (await trySendTuiToast(sessionId, text, params, forcePersist)) return "sent";

    // `MessageV2.latest` treats an ignored-only user row as the latest user turn.
    // Do not create an ignored-only user row while a run is active.
    if (midTurnDetector(sessionId)) return "queued";

    // Persistence requires a real session title.
    // Ignored messages are hidden from the LLM but are not `synthetic`, so OpenCode counts them as real user messages for title generation.
    // A notification persisted before title generation permanently suppresses that session's title generation.
    const { waitForSafeNotificationTarget } = await import("../../shared/safe-notification-target");
    const target = await waitForSafeNotificationTarget(
        client,
        sessionId,
        forcePersist
            ? { attempts: 1, delayMs: 0, readTimeoutMs: safeTargetOptions?.readTimeoutMs }
            : safeTargetOptions,
    );
    if (target === "skip") {
        if (forcePersist) return "queued";
        sessionLog(sessionId, "notification skipped (session not titled yet)");
        return "skipped";
    }

    // The second active-run check prevents runs that begin during title or prompt-context lookup from receiving a user row.
    // The second check runs after title and prompt-context lookup to close their race window.
    // The second check prevents a newly active run from receiving a user row.
    if (midTurnDetector(sessionId)) return "queued";

    if (!hasNotificationSessionClient(client)) {
        sessionLog(sessionId, "session prompt API unavailable for notification");
        return "failed";
    }
    const c = client;

    // Pin the prompt context (agent + model + variant) to the session's most
    // recent real turn. WHY: even though this is `noReply: true` (no assistant
    // turn fires now), OpenCode's createUserMessage RECORDS prompt context on
    // the appended user message, and THAT becomes the session's active
    // model/agent for the NEXT real turn. Passing nothing makes OpenCode record
    // the DEFAULT agent/model — which then switches the model on the user's
    // next turn and busts the provider prefix cache the prior turn warmed.
    // Mirrors AFT's notifications.ts.
    //
    // Caller-supplied params win; otherwise resolve them from the last assistant message.
    // The code pins only values resolved from real messages; it never pins synthesized defaults.
    // A successful empty result preserves fresh sessions' defaults. A failed read defers delivery
    // because appending without context can change an existing session's active model.
    let agent = params.agent || undefined;
    let variant = params.variant || undefined;
    let model =
        params.providerId && params.modelId
            ? { providerID: params.providerId, modelID: params.modelId }
            : undefined;
    if (!agent || !model || !variant) {
        try {
            const { resolvePromptContext } = await import("../../shared/prompt-context");
            const resolved = await resolvePromptContext(client, sessionId);
            if (resolved) {
                agent = agent ?? resolved.agent;
                // A variant belongs to one model, so it carries over only when the model does too.
                const sameModel =
                    model === undefined ||
                    (resolved.model !== undefined &&
                        model.providerID === resolved.model.providerID &&
                        model.modelID === resolved.model.modelID);
                model = model ?? resolved.model;
                if (sameModel) variant = variant ?? resolved.variant;
            }
        } catch (error: unknown) {
            notification.countQueuedAttempt = true;
            sessionLog(
                sessionId,
                "prompt context unavailable; queued notification:",
                getErrorMessage(error),
            );
            return "queued";
        }
    }

    // Check for an active run immediately before the SDK call to prevent a concurrent run from receiving a user row.
    if (midTurnDetector(sessionId)) return "queued";

    const controller = new AbortController();
    const input = {
        path: { id: sessionId },
        signal: controller.signal,
        body: {
            // noReply prevents this status line from starting a new model loop.
            // noReply does not make appending during an active loop safe; the caller must prevent it.
            // The caller defers while mid-turn; that check is the separate safety gate.
            noReply: true,
            agent,
            model,
            variant,
            parts: [
                {
                    type: "text",
                    text,
                    ignored: true,
                },
            ],
        },
    };

    try {
        if (typeof c.session?.prompt === "function") {
            await withTimeout(
                Promise.resolve(c.session.prompt(input)),
                notificationSendTimeoutMs,
                "notification delivery timed out",
            );
            return "sent";
        }
        if (typeof c.session?.promptAsync === "function") {
            await withTimeout(
                c.session.promptAsync(input),
                notificationSendTimeoutMs,
                "notification delivery timed out",
            );
            return "sent";
        }
        sessionLog(sessionId, "session prompt API unavailable for notification");
        return "failed";
    } catch (error: unknown) {
        if (error instanceof TimeoutError) {
            controller.abort(error);
            sessionLog(sessionId, "notification delivery timed out; outcome unknown");
            return "unknown";
        }
        const msg = getErrorMessage(error);
        sessionLog(sessionId, "failed to send notification:", msg);
        return "failed";
    }
}

export async function sendIgnoredMessage(
    client: unknown,
    sessionId: string,
    text: string,
    params: NotificationParams,
    // forcePersist always persists the notification as an ignored message instead of using the TUI.
    // forcePersist preserves the message in scrollback.
    forcePersist = false,
): Promise<NotificationDeliveryDisposition> {
    const notification: IgnoredNotification = {
        client,
        sessionId,
        text,
        params,
        forcePersist,
        attempts: 0,
        countQueuedAttempt: false,
    };
    const disposition = await deliverIgnoredMessage(notification);
    if (disposition === "queued") queueIgnoredNotification(notification);
    return disposition;
}

/**
 * Flush queued status lines only when the session is idle.
 * midTurnDetector prevents sends while the session is non-idle.
 */
export async function flushIgnoredMessages(sessionId: string): Promise<void> {
    if (flushingIgnoredNotifications.has(sessionId) || midTurnDetector(sessionId)) return;
    const queued = queuedIgnoredNotifications.get(sessionId);
    if (!queued || queued.length === 0) return;

    queuedIgnoredNotifications.delete(sessionId);
    flushingIgnoredNotifications.add(sessionId);
    try {
        let retained: IgnoredNotification[] = [];
        for (const [index, notification] of queued.entries()) {
            const disposition = await deliverIgnoredMessage(notification);
            if (disposition === "queued") {
                if (notification.countQueuedAttempt) {
                    notification.attempts += 1;
                    if (notification.attempts >= MAX_QUEUED_NOTIFICATION_DELIVERY_ATTEMPTS) {
                        sessionLog(
                            sessionId,
                            `dropped queued notification after ${notification.attempts} unavailable context reads`,
                        );
                        continue;
                    }
                }
                retained = queued.slice(index);
                break;
            }
            if (disposition === "unknown") {
                sessionLog(sessionId, "dropped queued notification with unknown delivery outcome");
                continue;
            }
            if (disposition === "failed" || disposition === "skipped") {
                notification.attempts += 1;
                if (notification.attempts < MAX_QUEUED_NOTIFICATION_DELIVERY_ATTEMPTS) {
                    retained = queued.slice(index);
                    break;
                }
                sessionLog(
                    sessionId,
                    `dropped queued notification after ${notification.attempts} undelivered flushes (last: ${disposition})`,
                );
            }
        }
        if (retained.length > 0) requeueFlushBatch(sessionId, retained);
    } finally {
        flushingIgnoredNotifications.delete(sessionId);
    }
}

export function clearIgnoredMessages(sessionId: string): void {
    queuedIgnoredNotifications.delete(sessionId);
    flushingIgnoredNotifications.delete(sessionId);
}

/** Propagates session prompt failures so callers replacing user input can report the loss. */
export async function sendUserPrompt(
    client: unknown,
    sessionId: string,
    text: string,
    promptContext: NotificationParams = {},
): Promise<void> {
    if (!hasNotificationSessionClient(client)) {
        throw new Error("session prompt API unavailable for user prompt");
    }
    const c = client as NotificationClient;

    const model =
        promptContext.providerId && promptContext.modelId
            ? { providerID: promptContext.providerId, modelID: promptContext.modelId }
            : undefined;
    const input = {
        path: { id: sessionId },
        body: {
            ...(promptContext.agent ? { agent: promptContext.agent } : {}),
            ...(model ? { model } : {}),
            ...(promptContext.variant ? { variant: promptContext.variant } : {}),
            parts: [{ type: "text", text }],
        },
    };

    if (typeof c.session?.promptAsync === "function") {
        // `promptAsync` only enqueues the turn, so a call still pending after the deadline is a stuck endpoint, not a long turn.
        await withTimeout(
            c.session.promptAsync(input),
            notificationSendTimeoutMs,
            "user prompt delivery timed out",
        );
    } else if (typeof c.session?.prompt === "function") {
        // `prompt` returns after the model turn completes; a deadline here would report a slow turn as an undelivered prompt.
        await Promise.resolve(c.session.prompt(input));
    } else {
        throw new Error("session prompt API unavailable for user prompt");
    }
}

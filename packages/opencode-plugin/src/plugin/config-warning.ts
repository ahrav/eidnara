import { sendIgnoredMessage } from "../hooks/context/send-session-notification";
import { log } from "../shared/logger";

/** Every attempt costs one host SDK read, so an undeliverable warning stops retrying after this many. */
const MAX_DELIVERY_ATTEMPTS = 10;

export function formatConfigWarning(warnings: readonly string[]): string {
    return [
        "## ⚠️ Eidnara Config Warning",
        "",
        "Some configuration values are invalid and were replaced with defaults:",
        "",
        ...warnings.map((warning) => `- ${warning}`),
        "",
        "Check your `eidnara.jsonc` to fix these values.",
    ].join("\n");
}

type SessionListResult = { data?: Array<{ id?: string }> } | Array<{ id?: string }> | null;

export interface ConfigWarningDelivery {
    /** True until one attempt reports `sent` or `queued`, or the attempt budget is spent. */
    readonly pending: boolean;
    /** A project whose `session.list()` is empty leaves the warning pending for `deliverTo`. */
    deliverToFirstSession(): Promise<void>;
    deliverTo(sessionId: string): Promise<void>;
}

/** Concurrent callers share one in-flight attempt, so the startup timer and a `chat.message` hook cannot persist the warning twice. */
export function createConfigWarningDelivery(client: unknown, text: string): ConfigWarningDelivery {
    let delivered = false;
    let attempts = 0;
    let inFlight: Promise<void> | null = null;

    const attempt = async (sessionId: string): Promise<void> => {
        attempts += 1;
        try {
            const disposition = await sendIgnoredMessage(client, sessionId, text, {});
            if (disposition === "sent" || disposition === "queued") delivered = true;
        } catch (error) {
            log(
                `[eidnara] config warning delivery failed: ${error instanceof Error ? error.message : String(error)}`,
            );
        }
    };

    const delivery: ConfigWarningDelivery = {
        get pending() {
            return !delivered && attempts < MAX_DELIVERY_ATTEMPTS;
        },
        async deliverTo(sessionId) {
            if (inFlight) {
                await inFlight;
                return;
            }
            if (!delivery.pending) return;
            inFlight = attempt(sessionId).finally(() => {
                inFlight = null;
            });
            await inFlight;
        },
        async deliverToFirstSession() {
            const sessions = client as { session?: { list?: () => Promise<SessionListResult> } };
            const listed = await Promise.resolve(sessions.session?.list?.()).catch(() => null);
            const sessionList = Array.isArray(listed) ? listed : listed?.data;
            const sessionId = sessionList?.[0]?.id;
            if (sessionId) await delivery.deliverTo(sessionId);
        },
    };
    return delivery;
}

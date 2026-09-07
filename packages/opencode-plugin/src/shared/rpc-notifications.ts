/**
 * The server keeps notifications in memory for TUI push.
 *
 * The server plugin cannot use `process.env.OPENCODE_CLIENT` to detect TUI
 * The server runs in a separate process from the TUI client.
 */

import type { RpcNotificationMessage } from "./rpc-types";

export type RpcNotification = RpcNotificationMessage;

let queue: RpcNotification[] = [];
let nextNotificationId = 1;

/** `GLOBAL_SCOPE` identifies session-less clients when recording global acknowledgements. */
const GLOBAL_SCOPE = "\0global";

/**
 * Global notifications belong to every scope, so one client's acknowledgement cannot remove them.
 * Acknowledging a global notification does not remove it from `queue`.
 */
const globalAcknowledgements = new Map<number, Set<string>>();

function scopeKey(sessionId: string | undefined): string {
    return sessionId ?? GLOBAL_SCOPE;
}

function isGlobal(notification: RpcNotification): boolean {
    return notification.sessionId === undefined;
}

function acknowledgedBy(notification: RpcNotification, sessionId: string | undefined): boolean {
    return globalAcknowledgements.get(notification.id)?.has(scopeKey(sessionId)) ?? false;
}

function acknowledgeGlobal(notification: RpcNotification, sessionId: string | undefined): void {
    let scopes = globalAcknowledgements.get(notification.id);
    if (scopes === undefined) {
        scopes = new Set<string>();
        globalAcknowledgements.set(notification.id, scopes);
    }
    scopes.add(scopeKey(sessionId));
}

/** Removes matching notifications and their acknowledgement records together. */
function removeFromQueue(shouldRemove: (notification: RpcNotification) => boolean): void {
    queue = queue.filter((notification) => {
        if (!shouldRemove(notification)) return true;
        globalAcknowledgements.delete(notification.id);
        return false;
    });
}

function applyCursors(
    sessionId: string | undefined,
    sessionCursor: number,
    globalCursor: number,
): void {
    for (const notification of queue) {
        if (isGlobal(notification) && notification.id <= globalCursor) {
            acknowledgeGlobal(notification, sessionId);
        }
    }
    removeFromQueue(
        (notification) =>
            !isGlobal(notification) &&
            notification.id <= sessionCursor &&
            (sessionId === undefined || notification.sessionId === sessionId),
    );
}

/**
 * Each authenticated TUI WebSocket registers one `NotificationSink`.
 * The server registers a sink when a TUI socket authenticates and removes the sink when the socket closes.
 * The server owns the WebSocket, so `send` is sink-agnostic.
 * `send` accepts no WebSocket type, so this module has no Bun/WS dependency.
 */
export interface NotificationSink {
    /** `sessionId` records the TUI's active session when the TUI connects. */
    sessionId?: string;
    /** Protocol 2 and later clients use strict session scoping; absent or below 2 means legacy behavior. */
    protocol?: number;
    /* */
    send: (notification: RpcNotification) => void;
}

// Protocol 2 sinks with `sessionId` receive scoped notifications only for that `sessionId`.
const sinks = new Set<NotificationSink>();

/** Call the returned function when the socket closes. */
export function registerNotificationSink(sink: NotificationSink): () => void {
    sinks.add(sink);
    return () => {
        sinks.delete(sink);
    };
}

/** Strict scoping is the default for any protocol from 2 on, so an unknown newer protocol cannot receive other sessions' notifications. */
function isLegacySink(sink: NotificationSink): boolean {
    return sink.protocol === undefined || sink.protocol < 2;
}

/**
 * Protocol 2 sinks without `sessionId` receive only global notifications; legacy sinks also receive scoped notifications. */
function notificationMatchesSink(notification: RpcNotification, sink: NotificationSink): boolean {
    if (notification.sessionId === undefined) return true;
    if (sink.sessionId !== undefined) return notification.sessionId === sink.sessionId;
    return isLegacySink(sink);
}

/**
 * The queue retains notifications for backlog delivery until acknowledgement or capacity eviction.
 * A reconnecting TUI can receive retained notifications during its next hello.
 * A failed live send leaves the notification queued unless capacity eviction removes it.
 * */
export function pushNotification(
    type: string,
    payload: Record<string, unknown>,
    sessionId?: string,
): void {
    const notification: RpcNotification = { id: nextNotificationId++, type, payload, sessionId };
    queue.push(notification);
    // A thrown `send` call must not block other sinks or the caller.
    for (const sink of sinks) {
        if (!notificationMatchesSink(notification, sink)) continue;
        try {
            sink.send(notification);
        } catch {
            // Ignore `send` failures so the notification remains queued for backlog delivery.
        }
    }
    // `queue` reserves one notification for each of up to 25 recent scopes during capacity eviction.
    if (queue.length > 100) {
        const reservedIds = new Set<number>();
        const reservedScopes = new Set<string>();
        for (let i = queue.length - 1; i >= 0 && reservedScopes.size < 25; i -= 1) {
            const candidate = queue[i];
            const scope = candidate.sessionId ?? "\0global";
            if (reservedScopes.has(scope)) continue;
            reservedScopes.add(scope);
            reservedIds.add(candidate.id);
        }
        const evictionIndex = queue.findIndex((candidate) => !reservedIds.has(candidate.id));
        const [evicted] = queue.splice(evictionIndex >= 0 ? evictionIndex : 0, 1);
        if (evicted !== undefined) globalAcknowledgements.delete(evicted.id);
    }
}

/** Global notifications remain queued for sessions that have not acknowledged them. */
export function acknowledgeNotifications(ids: readonly number[], sessionId?: string): void {
    // `ids` arrives from an RPC payload, so a non-array is a malformed request rather than a crash.
    if (!Array.isArray(ids)) return;
    const acknowledged = new Set(ids.filter((id) => Number.isSafeInteger(id) && id > 0));
    if (acknowledged.size === 0) return;
    for (const notification of queue) {
        if (isGlobal(notification) && acknowledged.has(notification.id)) {
            acknowledgeGlobal(notification, sessionId);
        }
    }
    // Acknowledging specific IDs prevents an out-of-order handler from removing an earlier notification.
    removeFromQueue(
        (notification) =>
            !isGlobal(notification) &&
            acknowledged.has(notification.id) &&
            (sessionId === undefined || notification.sessionId === sessionId),
    );
}

/** `__resetNotificationStateForTests` simulates a fresh server module by clearing process-local notification state. */
export function __resetNotificationStateForTests(): void {
    queue = [];
    nextNotificationId = 1;
    sinks.clear();
    globalAcknowledgements.clear();
}

export interface DrainNotificationsOptions {
    /**
     * `globalLastReceivedId` tracks global notifications independently from the session cursor.
     */
    globalLastReceivedId?: number;
    /* */
    sessionOnly?: boolean;
    /* */
    globalOnly?: boolean;
}

function cursor(value: number | undefined): number {
    return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : 0;
}

/** `drainNotifications` prunes only the scopes acknowledged by the client's cursors.
 *
 * When `globalLastReceivedId` is set with a `sessionId`, session-scoped and global notifications use separate cursors.
 * A session cursor removes that session's notifications; a global cursor records acknowledgement per client, preserving global notifications for other sessions.
 *
 * Returned notifications remain queued until acknowledgement, so reconnecting clients can receive them again.
 * */
export function drainNotifications(
    lastReceivedId = 0,
    sessionId?: string,
    options: DrainNotificationsOptions = {},
): RpcNotification[] {
    const sessionCursor = cursor(lastReceivedId);
    const visibleGlobal = (notification: RpcNotification, globalCursor: number): boolean =>
        isGlobal(notification) &&
        notification.id > globalCursor &&
        !acknowledgedBy(notification, sessionId);
    const visibleOwn = (notification: RpcNotification): boolean =>
        !isGlobal(notification) &&
        notification.id > sessionCursor &&
        (sessionId === undefined || notification.sessionId === sessionId);

    if (options.globalOnly) {
        // A client that tracks global items separately sends `globalLastReceivedId`; a single-cursor client sends `lastReceivedId`.
        const globalCursor = cursor(options.globalLastReceivedId ?? lastReceivedId);
        applyCursors(sessionId, 0, globalCursor);
        return queue.filter((notification) => visibleGlobal(notification, globalCursor));
    }

    if (options.sessionOnly) {
        if (sessionId === undefined) return [];
        applyCursors(sessionId, sessionCursor, 0);
        return queue.filter(visibleOwn);
    }

    if (sessionId !== undefined && options.globalLastReceivedId !== undefined) {
        const globalCursor = cursor(options.globalLastReceivedId);
        applyCursors(sessionId, sessionCursor, globalCursor);
        return queue.filter(
            (notification) => visibleGlobal(notification, globalCursor) || visibleOwn(notification),
        );
    }

    applyCursors(sessionId, sessionCursor, sessionCursor);
    return queue.filter(
        (notification) => visibleGlobal(notification, sessionCursor) || visibleOwn(notification),
    );
}

/** A TUI is connected only when a live notification sink is registered.
 * A TUI connection requires a registered notification sink; draining notifications does not establish one.
 *
 * `sessionId` scopes the connection check to that session.
 * A session-less legacy sink counts as connected for every session.
 * */
export function isTuiConnected(sessionId?: string): boolean {
    if (sinks.size === 0) return false;
    if (sessionId === undefined) return true;
    for (const sink of sinks) {
        if (sink.sessionId === sessionId) return true;
        if (sink.sessionId === undefined && isLegacySink(sink)) return true;
    }
    return false;
}

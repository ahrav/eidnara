import { beforeEach, describe, expect, test } from "bun:test";
import {
    __resetNotificationStateForTests,
    acknowledgeNotifications,
    drainNotifications,
    isTuiConnected,
    type NotificationSink,
    pushNotification,
    registerNotificationSink,
    scopeSeesSession,
} from "./rpc-notifications";

describe("rpc notifications", () => {
    beforeEach(() => {
        __resetNotificationStateForTests();
    });

    test("keeps messages queued until the client acks their id", () => {
        expect(drainNotifications(Number.MAX_SAFE_INTEGER)).toEqual([]);

        pushNotification("one", { ok: true }, "ses_1");
        const firstPoll = drainNotifications();
        expect(firstPoll).toHaveLength(1);
        expect(firstPoll[0].type).toBe("one");

        const retryPoll = drainNotifications();
        expect(retryPoll.map((m) => m.id)).toEqual(firstPoll.map((m) => m.id));

        const lastReceivedId = Math.max(...firstPoll.map((m) => m.id));
        expect(drainNotifications(lastReceivedId)).toEqual([]);
    });

    test("scopes drain to the requesting session; other sessions' items survive", () => {
        pushNotification("for-a", { action: "show-upgrade-dialog" }, "ses_A");
        pushNotification("for-b", { action: "show-upgrade-dialog" }, "ses_B");
        pushNotification("global", { action: "show-status-dialog" });

        // Session A sees only its own item + the global one, never ses_B's.
        const aPoll = drainNotifications(0, "ses_A");
        expect(aPoll.map((m) => m.type).sort()).toEqual(["for-a", "global"]);

        // Acking session A must NOT prune session B's still-unseen notification, nor the shared global one.
        const ackId = Math.max(...aPoll.map((m) => m.id));
        drainNotifications(ackId, "ses_A");
        const bPoll = drainNotifications(0, "ses_B");
        expect(bPoll.map((m) => m.type).sort()).toEqual(["for-b", "global"]);
    });

    test("one session's single-cursor ack keeps a global notification for another session", () => {
        pushNotification("global-upgrade", { action: "show-upgrade-dialog" });
        pushNotification("for-b", { ok: true }, "ses_B");

        const aPoll = drainNotifications(0, "ses_A");
        expect(aPoll.map((m) => m.type)).toEqual(["global-upgrade"]);
        const ackId = Math.max(...aPoll.map((m) => m.id));
        expect(drainNotifications(ackId, "ses_A")).toEqual([]);

        // Session B never polled, so the global broadcast must still be waiting for it.
        expect(
            drainNotifications(0, "ses_B")
                .map((m) => m.type)
                .sort(),
        ).toEqual(["for-b", "global-upgrade"]);
    });

    test("one session's dual-cursor ack keeps a global notification for another session", () => {
        pushNotification("global-status", { action: "show-status-dialog" });
        const globalId = drainNotifications(0, "ses_A", { globalLastReceivedId: 0 })[0].id;

        expect(
            drainNotifications(0, "ses_A", { globalLastReceivedId: globalId }).map((m) => m.type),
        ).toEqual([]);
        expect(
            drainNotifications(0, "ses_B", { globalLastReceivedId: 0 }).map((m) => m.type),
        ).toEqual(["global-status"]);
    });

    test("acknowledgeNotifications is scoped to the acknowledging session", () => {
        pushNotification("global-status", { action: "show-status-dialog" });
        pushNotification("for-b", { ok: true }, "ses_B");
        const [globalId, forBId] = drainNotifications(0).map((m) => m.id);

        // Session A acknowledges both ids; only its own view of the global item changes.
        acknowledgeNotifications([globalId, forBId], { sessionId: "ses_A" });
        expect(drainNotifications(0, "ses_A").map((m) => m.type)).toEqual([]);
        expect(
            drainNotifications(0, "ses_B")
                .map((m) => m.type)
                .sort(),
        ).toEqual(["for-b", "global-status"]);

        // Session B acknowledging its own item removes it.
        acknowledgeNotifications([forBId], { sessionId: "ses_B" });
        expect(drainNotifications(0, "ses_B").map((m) => m.type)).toEqual(["global-status"]);
    });

    test("a scoped acknowledgement removes only session notifications that scope could receive", () => {
        pushNotification("for-a", { ok: true }, "ses_A");
        pushNotification("for-b", { ok: true }, "ses_B");
        const ids = drainNotifications(0).map((m) => m.id);

        // A protocol 2 socket bound to ses_A removes its own entry, never ses_B's.
        acknowledgeNotifications(ids, { sessionId: "ses_A", protocol: 2 });
        expect(drainNotifications(0).map((m) => m.type)).toEqual(["for-b"]);

        // A session-less protocol 2 socket receives only global entries, so it removes nothing here.
        acknowledgeNotifications(ids, { sessionId: undefined, protocol: 2 });
        expect(drainNotifications(0).map((m) => m.type)).toEqual(["for-b"]);

        // A session-less legacy socket receives every session's entries.
        acknowledgeNotifications(ids, { sessionId: undefined });
        expect(drainNotifications(0)).toEqual([]);
    });

    test("an unscoped acknowledgement removes every listed session notification", () => {
        pushNotification("for-a", { ok: true }, "ses_A");
        pushNotification("for-b", { ok: true }, "ses_B");
        acknowledgeNotifications(drainNotifications(0).map((m) => m.id));
        expect(drainNotifications(0)).toEqual([]);
    });

    test("scopeSeesSession follows sink visibility", () => {
        expect(scopeSeesSession({ sessionId: "ses_A" }, "ses_A")).toBe(true);
        expect(scopeSeesSession({ sessionId: "ses_A" }, "ses_B")).toBe(false);
        expect(scopeSeesSession({ sessionId: undefined }, "ses_B")).toBe(true);
        expect(scopeSeesSession({ sessionId: undefined, protocol: 1 }, "ses_B")).toBe(true);
        expect(scopeSeesSession({ sessionId: undefined, protocol: 2 }, "ses_B")).toBe(false);
        expect(scopeSeesSession({ sessionId: undefined, protocol: 3 }, "ses_B")).toBe(false);
    });

    test("acknowledgeNotifications ignores a malformed ids payload instead of throwing", () => {
        pushNotification("for-a", { ok: true }, "ses_A");
        for (const malformed of [null, undefined, 42, "1", { length: 1 }]) {
            expect(() =>
                acknowledgeNotifications(malformed as unknown as number[], { sessionId: "ses_A" }),
            ).not.toThrow();
        }
        expect(drainNotifications(0, "ses_A").map((m) => m.type)).toEqual(["for-a"]);
    });

    test("globalOnly honors globalLastReceivedId over the session cursor", () => {
        pushNotification("global-1", { ok: true });
        pushNotification("global-2", { ok: true });
        pushNotification("for-a", { ok: true }, "ses_A");
        const [firstGlobalId] = drainNotifications(0, "ses_A", { globalOnly: true }).map(
            (m) => m.id,
        );

        // `globalLastReceivedId` overrides `lastReceivedId` for global-only polls.
        expect(
            drainNotifications(0, "ses_A", {
                globalOnly: true,
                globalLastReceivedId: firstGlobalId,
            }).map((m) => m.type),
        ).toEqual(["global-2"]);

        // A single-cursor client still advances through `lastReceivedId`.
        expect(
            drainNotifications(firstGlobalId, "ses_A", { globalOnly: true }).map((m) => m.type),
        ).toEqual(["global-2"]);

        // Neither global poll touches the session-scoped item.
        expect(drainNotifications(0, "ses_A", { sessionOnly: true }).map((m) => m.type)).toEqual([
            "for-a",
        ]);
    });

    test("isTuiConnected reflects live WS sinks per-session", () => {
        // isTuiConnected returns false for every session and globally when no sinks are registered.
        expect(isTuiConnected("ses_anything")).toBe(false);
        expect(isTuiConnected()).toBe(false);

        // A live sink for session A makes isTuiConnected true only for session A and globally.
        const unregister = registerNotificationSink({ sessionId: "ses_A", send: () => {} });
        expect(isTuiConnected("ses_A")).toBe(true);
        expect(isTuiConnected("ses_B")).toBe(false);
        expect(isTuiConnected()).toBe(true);

        // Unregistering the sink makes isTuiConnected return false for session A and globally.
        unregister();
        expect(isTuiConnected("ses_A")).toBe(false);
        expect(isTuiConnected()).toBe(false);
    });

    test("a session-less sink's visibility follows its protocol: legacy sees every session, 2 and newer are global-only", () => {
        const cases: Array<{ protocol?: number; seesSessions: boolean; expected: string[] }> = [
            { protocol: undefined, seesSessions: true, expected: ["scoped", "global"] },
            { protocol: 2, seesSessions: false, expected: ["global"] },
            // An unknown newer protocol keeps strict scoping instead of falling back to legacy.
            { protocol: 3, seesSessions: false, expected: ["global"] },
        ];
        for (const { protocol, seesSessions, expected } of cases) {
            __resetNotificationStateForTests();
            const received: string[] = [];
            const unregister = registerNotificationSink({
                sessionId: undefined,
                protocol,
                send: (notification) => received.push(notification.type),
            });
            expect(isTuiConnected("ses_whatever"), `protocol ${protocol}`).toBe(seesSessions);
            expect(isTuiConnected()).toBe(true);

            pushNotification("scoped", { ok: true }, "ses_whatever");
            pushNotification("global", { ok: true });
            expect(received, `protocol ${protocol}`).toEqual(expected);
            unregister();
        }
    });

    test("pushNotification fans out live to a matching sink and skips a foreign session", () => {
        const received: string[] = [];
        const sink: NotificationSink = {
            sessionId: "ses_live",
            send: (n) => received.push(n.type),
        };
        const unregister = registerNotificationSink(sink);

        pushNotification("for-live", { action: "show-status-dialog" }, "ses_live");
        pushNotification("for-other", { action: "show-status-dialog" }, "ses_other");
        pushNotification("global", { action: "show-status-dialog" });

        // The sink sees its own session + global, never the foreign session.
        expect(received.sort()).toEqual(["for-live", "global"]);
        unregister();
    });

    test("a dead sink (throwing send) does not block delivery to other sinks", () => {
        const live: string[] = [];
        const unregDead = registerNotificationSink({
            sessionId: undefined,
            send: () => {
                throw new Error("socket dead");
            },
        });
        const unregLive = registerNotificationSink({
            sessionId: undefined,
            send: (n) => live.push(n.type),
        });
        // pushNotification must not throw, and the live sink must receive the notification.
        expect(() => pushNotification("resilient", { ok: true })).not.toThrow();
        expect(live).toEqual(["resilient"]);
        unregDead();
        unregLive();
    });

    test("queue-cap eviction is session-fair: a noisy session cannot evict another session's newest unseen item", () => {
        pushNotification("quiet-dialog", { action: "show-upgrade-dialog" }, "ses_quiet");
        for (let i = 0; i < 200; i += 1) {
            pushNotification("noise", { i }, "ses_noisy");
        }
        const quietPoll = drainNotifications(0, "ses_quiet");
        expect(quietPoll.some((m) => m.type === "quiet-dialog")).toBe(true);
        expect(drainNotifications(0)).toHaveLength(100);
    });

    test("queue cap remains global across more than one hundred sessions", () => {
        for (let i = 0; i < 250; i += 1) {
            pushNotification("one-per-session", { i }, `ses_${i}`);
        }
        expect(drainNotifications(0)).toHaveLength(100);
    });
});

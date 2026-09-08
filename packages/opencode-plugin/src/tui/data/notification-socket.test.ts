import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import {
    __resetNotificationStateForTests,
    drainNotifications,
    isTuiConnected,
    pushNotification,
} from "../../shared/rpc-notifications";
import { EidnaraRpcServer } from "../../shared/rpc-server";
import { rpcPortFilePath } from "../../shared/rpc-utils";
import {
    _resetNotificationSocketStateForTesting,
    type SocketNotification,
    startNotificationSocket,
    stopNotificationSocket,
} from "./notification-socket";
import { closeRpc, initRpcClient } from "./session-rpc";

const originalXdgDataHome = process.env.XDG_DATA_HOME;
const tempDirs: string[] = [];
const servers: EidnaraRpcServer[] = [];
const legacyServers: Array<ReturnType<typeof Bun.serve>> = [];

afterEach(() => {
    _resetNotificationSocketStateForTesting();
    closeRpc();
    for (const server of servers.splice(0)) {
        server.stop();
    }
    for (const server of legacyServers.splice(0)) server.stop(true);
    __resetNotificationStateForTests();
    for (const dir of tempDirs.splice(0)) {
        rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    }
    if (originalXdgDataHome === undefined) {
        delete process.env.XDG_DATA_HOME;
    } else {
        process.env.XDG_DATA_HOME = originalXdgDataHome;
    }
});

function makeDataHome(): string {
    const dir = mkdtempSync(join(tmpdir(), "eidnara-notification-socket-"));
    tempDirs.push(dir);
    process.env.XDG_DATA_HOME = dir;
    return dir;
}

function storageDir(dataHome: string): string {
    return join(dataHome, "eidnara", "context");
}

async function startServer(dataHome: string, directory: string): Promise<EidnaraRpcServer> {
    const server = new EidnaraRpcServer(storageDir(dataHome), directory);
    await server.start();
    servers.push(server);
    return server;
}

function startLegacyServer(
    dataHome: string,
    directory: string,
    notifications: SocketNotification[],
): {
    sockets: Set<{ close(): void }>;
    helloCursors: number[];
    ackCursors: number[];
    stop: () => void;
} {
    const token = "v032-query-token";
    const sockets = new Set<{ close(): void }>();
    const helloCursors: number[] = [];
    const ackCursors: number[] = [];
    const server = Bun.serve({
        hostname: "127.0.0.1",
        port: 0,
        fetch(req, bunServer) {
            const url = new URL(req.url);
            if (url.pathname === "/health") {
                return Response.json({ ok: true, pid: process.pid });
            }
            if (
                url.pathname === "/ws" &&
                req.headers.get("authorization") === `Bearer ${token}` &&
                bunServer.upgrade(req, { data: {} })
            ) {
                return undefined;
            }
            return new Response("Not Found", { status: 404 });
        },
        websocket: {
            open(ws) {
                sockets.add(ws);
            },
            message(ws, raw) {
                const msg = JSON.parse(String(raw)) as {
                    type?: string;
                    lastReceivedId?: number;
                    cursor?: number;
                };
                if (msg.type === "hello") {
                    const cursor = Number(msg.lastReceivedId ?? 0);
                    helloCursors.push(cursor);
                    for (const notification of notifications) {
                        if (notification.id > cursor) {
                            ws.send(JSON.stringify({ type: "notification", notification }));
                        }
                    }
                    ws.send(JSON.stringify({ type: "hello-ack" }));
                } else if (msg.type === "ack" && typeof msg.cursor === "number") {
                    ackCursors.push(msg.cursor);
                }
            },
            close(ws) {
                sockets.delete(ws);
            },
        },
    });
    legacyServers.push(server);

    const portFile = rpcPortFilePath(storageDir(dataHome), directory, process.pid);
    mkdirSync(dirname(portFile), { recursive: true });
    writeFileSync(
        portFile,
        JSON.stringify({ port: server.port, pid: process.pid, started_at: Date.now(), token }),
    );
    const stop = () => {
        for (const ws of sockets) ws.close();
        server.stop(true);
        const index = legacyServers.indexOf(server);
        if (index >= 0) legacyServers.splice(index, 1);
    };
    return { sockets, helloCursors, ackCursors, stop };
}

async function waitFor(condition: () => boolean, label: string, timeoutMs = 4_000): Promise<void> {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
        if (condition()) return;
        await new Promise((resolve) => setTimeout(resolve, 25));
    }
    throw new Error(`Timed out waiting for ${label}`);
}

describe("notification socket", () => {
    test("a home-screen socket receives globals but leaves scoped notifications pending", async () => {
        drainNotifications(Number.MAX_SAFE_INTEGER);
        const dataHome = makeDataHome();
        const directory = "/repo-home-screen";
        await startServer(dataHome, directory);
        initRpcClient(directory);

        const received: SocketNotification[] = [];
        startNotificationSocket({
            getSessionId: () => null,
            onNotification: (notification) => {
                received.push(notification);
                return true;
            },
        });
        await waitFor(() => isTuiConnected(), "home-screen socket connection");
        expect(isTuiConnected("ses_hidden")).toBe(false);

        pushNotification("scoped", { action: "show-status-dialog" }, "ses_hidden");
        pushNotification("global", { action: "show-status-dialog" });
        await waitFor(
            () => received.some((notification) => notification.type === "global"),
            "global home-screen notification",
        );
        expect(received.some((notification) => notification.type === "scoped")).toBe(false);
        expect(drainNotifications(0, "ses_hidden", { sessionOnly: true })).toHaveLength(1);
    });

    test("overlapping starts create one socket and one delivery", async () => {
        drainNotifications(Number.MAX_SAFE_INTEGER);
        const dataHome = makeDataHome();
        const directory = "/repo-overlapping-start";
        await startServer(dataHome, directory);
        initRpcClient(directory);

        let deliveries = 0;
        const options = {
            getSessionId: () => "ses_once",
            onNotification: () => {
                deliveries += 1;
                return true;
            },
        };
        startNotificationSocket(options);
        startNotificationSocket(options);
        await waitFor(() => isTuiConnected("ses_once"), "single overlapping connection");
        pushNotification("once", { ok: true }, "ses_once");
        await waitFor(() => deliveries === 1, "single notification delivery");
        await new Promise((resolve) => setTimeout(resolve, 75));
        expect(deliveries).toBe(1);
    });

    test("onConnected runs once the socket is open so RPC-backed preferences load from a reachable server", async () => {
        drainNotifications(Number.MAX_SAFE_INTEGER);
        const dataHome = makeDataHome();
        const directory = "/repo-on-connected";
        await startServer(dataHome, directory);
        initRpcClient(directory);

        let connections = 0;
        startNotificationSocket({
            getSessionId: () => "ses_connected",
            onNotification: () => true,
            onConnected: () => {
                connections += 1;
            },
        });

        await waitFor(() => connections === 1, "onConnected after the socket opened");
        expect(isTuiConnected("ses_connected")).toBe(true);
    });

    test("the hello backlog is handled only after a pending onConnected settles", async () => {
        drainNotifications(Number.MAX_SAFE_INTEGER);
        const dataHome = makeDataHome();
        const directory = "/repo-on-connected-backlog";
        await startServer(dataHome, directory);
        // Queued before the socket exists, so the server hands it over as hello backlog.
        pushNotification("backlog", { ok: true }, "ses_backlog");
        initRpcClient(directory);

        let releaseConnected: () => void = () => {};
        const connected = new Promise<void>((resolve) => {
            releaseConnected = resolve;
        });
        const deliveries: SocketNotification[] = [];
        startNotificationSocket({
            getSessionId: () => "ses_backlog",
            onNotification: (notification) => {
                deliveries.push(notification);
                return true;
            },
            onConnected: () => connected,
        });

        await waitFor(() => isTuiConnected("ses_backlog"), "backlog socket connection");
        await new Promise((resolve) => setTimeout(resolve, 150));
        expect(deliveries).toHaveLength(0);

        releaseConnected();
        await waitFor(() => deliveries.length === 1, "backlog delivery after onConnected settled");
        expect(deliveries[0]?.type).toBe("backlog");
    });

    test("an RPC client replaced during endpoint lookup still gets a socket", async () => {
        drainNotifications(Number.MAX_SAFE_INTEGER);
        const dataHome = makeDataHome();
        const directory = "/repo-replaced-client";
        await startServer(dataHome, directory);
        initRpcClient(directory);

        let deliveries = 0;
        startNotificationSocket({
            getSessionId: () => "ses_replaced",
            onNotification: () => {
                deliveries += 1;
                return true;
            },
        });
        // The lookup for the first client is in flight; replacing the client bumps the generation.
        initRpcClient(directory);

        await waitFor(() => isTuiConnected("ses_replaced"), "connection for the replaced client");
        pushNotification("after-replace", { ok: true }, "ses_replaced");
        await waitFor(() => deliveries === 1, "delivery after client replacement");
    });

    test("an RPC client replaced after the socket opened reconnects on the new client", async () => {
        drainNotifications(Number.MAX_SAFE_INTEGER);
        const dataHome = makeDataHome();
        const directory = "/repo-replaced-open-client";
        await startServer(dataHome, directory);
        initRpcClient(directory);

        const received: string[] = [];
        startNotificationSocket({
            getSessionId: () => "ses_reopen",
            onNotification: (notification) => {
                received.push(notification.type);
                return true;
            },
        });
        await waitFor(() => isTuiConnected("ses_reopen"), "first connection");
        pushNotification("before-swap", { ok: true }, "ses_reopen");
        await waitFor(() => received.includes("before-swap"), "delivery on the first client");

        initRpcClient(directory);
        pushNotification("after-swap", { ok: true }, "ses_reopen");
        await waitFor(() => received.includes("after-swap"), "delivery after client replacement");
        expect(received).toEqual(["before-swap", "after-swap"]);
    });

    test("uses the active session cursor when switching sessions", async () => {
        drainNotifications(Number.MAX_SAFE_INTEGER);
        const dataHome = makeDataHome();
        const directory = "/repo-session-cursors";
        await startServer(dataHome, directory);
        initRpcClient(directory);

        let activeSession = "ses_A";
        const received: SocketNotification[] = [];
        startNotificationSocket({
            getSessionId: () => activeSession,
            onNotification: (notification) => {
                received.push(notification);
                return true;
            },
        });

        await waitFor(() => isTuiConnected("ses_A"), "socket connected to session A");
        pushNotification("for-b", { action: "show-status-dialog" }, "ses_B");
        pushNotification("for-a", { action: "show-status-dialog" }, "ses_A");
        await waitFor(
            () => received.some((notification) => notification.type === "for-a"),
            "session A notification",
        );

        activeSession = "ses_B";
        await waitFor(
            () => received.some((notification) => notification.type === "for-b"),
            "session B backlog after switching sessions",
        );

        expect(received.map((notification) => notification.type)).toContain("for-a");
        expect(received.map((notification) => notification.type)).toContain("for-b");
    });

    test("clears cursors and deduplication when a fresh server reuses notification ids", async () => {
        __resetNotificationStateForTests();
        const dataHome = makeDataHome();
        const directory = "/repo-reconnect";
        const first = await startServer(dataHome, directory);
        initRpcClient(directory);

        const received: SocketNotification[] = [];
        startNotificationSocket({
            getSessionId: () => "ses_R",
            onNotification: (notification) => {
                received.push(notification);
                return true;
            },
        });
        await waitFor(() => isTuiConnected("ses_R"), "initial websocket connection");

        pushNotification("before-restart", { action: "show-status-dialog" }, "ses_R");
        await waitFor(() => received.length === 1, "first notification acknowledgement");
        await waitFor(
            () => drainNotifications(0, "ses_R").length === 0,
            "first server queue to empty",
        );
        expect(received[0].id).toBe(1);

        first.stop();
        await waitFor(() => !isTuiConnected("ses_R"), "old websocket sink removed");
        __resetNotificationStateForTests();
        pushNotification("queued-after-restart", { action: "show-status-dialog" }, "ses_R");
        const second = await startServer(dataHome, directory);
        expect(second).toBeDefined();

        await waitFor(
            () => received.some((notification) => notification.type === "queued-after-restart"),
            "reused id from replacement server backlog",
        );
        const restarted = received.find(
            (notification) => notification.type === "queued-after-restart",
        );
        expect(restarted?.id).toBe(received[0].id);
    });

    test("acknowledges only consumed ids and redelivers a failed earlier notification", async () => {
        __resetNotificationStateForTests();
        const dataHome = makeDataHome();
        const directory = "/repo-exact-ack";
        await startServer(dataHome, directory);
        initRpcClient(directory);

        let releaseFirst: ((consumed: boolean) => void) | undefined;
        const firstResult = new Promise<boolean>((resolve) => {
            releaseFirst = resolve;
        });
        let slowCalls = 0;
        let fastCalls = 0;
        startNotificationSocket({
            getSessionId: () => "ses_exact",
            onNotification: (notification) => {
                if (notification.type === "slow") {
                    slowCalls += 1;
                    return slowCalls === 1 ? firstResult : true;
                }
                fastCalls += 1;
                return true;
            },
        });
        await waitFor(() => isTuiConnected("ses_exact"), "exact ack socket connection");

        pushNotification("slow", { action: "show-status-dialog" }, "ses_exact");
        pushNotification("fast", { action: "show-toast" }, "ses_exact");
        await waitFor(() => slowCalls === 1, "blocked first handler");
        expect(fastCalls).toBe(0);
        releaseFirst?.(false);
        await waitFor(() => fastCalls === 1, "later handler completion");
        await waitFor(() => {
            const pending = drainNotifications(0, "ses_exact");
            return pending.length === 1 && pending[0].type === "slow";
        }, "only failed notification to remain queued");

        _resetNotificationSocketStateForTesting();
        await waitFor(() => !isTuiConnected("ses_exact"), "socket disconnect before redelivery");
        startNotificationSocket({
            getSessionId: () => "ses_exact",
            onNotification: (notification) => {
                if (notification.type === "slow") slowCalls += 1;
                if (notification.type === "fast") fastCalls += 1;
                return true;
            },
        });
        await waitFor(() => slowCalls === 2, "failed notification redelivery");
        expect(fastCalls).toBe(1);
    });

    test("legacy mode keeps its watermark behind a declined notification across reconnect", async () => {
        const dataHome = makeDataHome();
        const directory = "/repo-v032-notifications";
        const sessionId = "ses_v032";
        const legacy = startLegacyServer(dataHome, directory, [
            { id: 1, type: "declined", payload: {}, sessionId },
            { id: 2, type: "consumed", payload: {}, sessionId },
        ]);
        initRpcClient(directory);

        let declinedCalls = 0;
        let consumedCalls = 0;
        startNotificationSocket({
            getSessionId: () => sessionId,
            onNotification: (notification) => {
                if (notification.type === "declined") {
                    declinedCalls += 1;
                    return declinedCalls > 1;
                }
                consumedCalls += 1;
                return true;
            },
        });

        await waitFor(() => declinedCalls === 1 && consumedCalls === 1, "legacy backlog handling");
        expect(Math.max(0, ...legacy.ackCursors)).toBe(0);
        for (const ws of legacy.sockets) ws.close();

        await waitFor(() => legacy.helloCursors.length >= 2, "legacy reconnect hello");
        await waitFor(() => declinedCalls === 2, "declined notification redelivery");
        expect(legacy.helloCursors[1]).toBe(0);
        expect(consumedCalls).toBe(1);
        await waitFor(() => legacy.ackCursors.includes(2), "gap-safe legacy watermark advance");
    });

    test("a restarted legacy server gets fresh cursors even though its ids restart at 1", async () => {
        const dataHome = makeDataHome();
        const directory = "/repo-v032-restart";
        const sessionId = "ses_v032_restart";
        const first = startLegacyServer(dataHome, directory, [
            { id: 1, type: "first-run", payload: {}, sessionId },
        ]);
        initRpcClient(directory);

        const received: string[] = [];
        startNotificationSocket({
            getSessionId: () => sessionId,
            onNotification: (notification) => {
                received.push(notification.type);
                return true;
            },
        });
        await waitFor(() => received.includes("first-run"), "first legacy delivery");
        await waitFor(() => first.ackCursors.includes(1), "first legacy ack");

        // The restarted server reuses id 1 for an unrelated notification and writes a new started_at.
        await new Promise((resolve) => setTimeout(resolve, 5));
        first.stop();
        const second = startLegacyServer(dataHome, directory, [
            { id: 1, type: "second-run", payload: {}, sessionId },
        ]);

        await waitFor(() => received.includes("second-run"), "delivery after legacy restart");
        expect(second.helloCursors[0]).toBe(0);
    });

    test("serializes back-to-back dialog notification handlers", async () => {
        __resetNotificationStateForTests();
        const dataHome = makeDataHome();
        const directory = "/repo-serialized-dialogs";
        await startServer(dataHome, directory);
        initRpcClient(directory);

        let releaseFirst: (() => void) | undefined;
        const firstSettled = new Promise<void>((resolve) => {
            releaseFirst = resolve;
        });
        const events: string[] = [];
        startNotificationSocket({
            getSessionId: () => "ses_dialogs",
            onNotification: async (notification) => {
                events.push(`start:${notification.type}`);
                if (notification.type === "dialog-one") await firstSettled;
                events.push(`finish:${notification.type}`);
                return true;
            },
        });
        await waitFor(() => isTuiConnected("ses_dialogs"), "dialog socket connection");

        pushNotification("dialog-one", { action: "show-status-dialog" }, "ses_dialogs");
        pushNotification("dialog-two", { action: "show-upgrade-dialog" }, "ses_dialogs");
        await waitFor(() => events.includes("start:dialog-one"), "first dialog handler start");
        await new Promise((resolve) => setTimeout(resolve, 75));
        expect(events).toEqual(["start:dialog-one"]);

        releaseFirst?.();
        await waitFor(() => events.includes("finish:dialog-two"), "second dialog handler finish");
        expect(events).toEqual([
            "start:dialog-one",
            "finish:dialog-one",
            "start:dialog-two",
            "finish:dialog-two",
        ]);
    });

    test("a dialog handler pending across a socket restart still serializes the replacement socket's handler", async () => {
        __resetNotificationStateForTests();
        const dataHome = makeDataHome();
        const directory = "/repo-restart-dialogs";
        await startServer(dataHome, directory);
        initRpcClient(directory);

        let releaseFirst: (() => void) | undefined;
        const firstSettled = new Promise<void>((resolve) => {
            releaseFirst = resolve;
        });
        const events: string[] = [];
        const options = {
            getSessionId: () => "ses_restart_dialogs",
            onNotification: async (notification: SocketNotification) => {
                events.push(`start:${notification.type}`);
                if (notification.type === "dialog-one") await firstSettled;
                events.push(`finish:${notification.type}`);
                return true;
            },
        };
        startNotificationSocket(options);
        await waitFor(() => isTuiConnected("ses_restart_dialogs"), "dialog socket connection");

        pushNotification("dialog-one", { action: "show-status-dialog" }, "ses_restart_dialogs");
        await waitFor(() => events.includes("start:dialog-one"), "first dialog handler start");

        // Restart the socket while the first dialog handler is pending.
        stopNotificationSocket();
        await waitFor(() => !isTuiConnected("ses_restart_dialogs"), "socket disconnect");
        startNotificationSocket(options);
        await waitFor(() => isTuiConnected("ses_restart_dialogs"), "replacement socket connection");
        pushNotification("dialog-two", { action: "show-upgrade-dialog" }, "ses_restart_dialogs");
        await new Promise((resolve) => setTimeout(resolve, 75));
        expect(events).toEqual(["start:dialog-one"]);

        releaseFirst?.();
        await waitFor(() => events.includes("finish:dialog-two"), "second dialog handler finish");
        // The unacknowledged first notification is redelivered on the replacement socket; every handler still runs one at a time.
        expect(events).toEqual([
            "start:dialog-one",
            "finish:dialog-one",
            "start:dialog-one",
            "finish:dialog-one",
            "start:dialog-two",
            "finish:dialog-two",
        ]);
    });
});

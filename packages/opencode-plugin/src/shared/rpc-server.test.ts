import { afterEach, describe, expect, spyOn, test } from "bun:test";
import {
    existsSync,
    mkdirSync,
    mkdtempSync,
    readdirSync,
    readFileSync,
    rmSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { openRpcSocket, waitFor, waitForJsonMessage } from "../testing/rpc-websocket";
import * as logger from "./logger";
import { EidnaraRpcClient } from "./rpc-client";
import {
    __resetNotificationStateForTests,
    drainNotifications,
    isTuiConnected,
    pushNotification,
} from "./rpc-notifications";
import { EidnaraRpcServer } from "./rpc-server";
import {
    __resetRpcIdentityTestHooks,
    __setRpcIdentityTestHooks,
    parseRpcPortFile,
    type RpcPortFileRecord,
    rpcPortDir,
    rpcPortFilePath,
} from "./rpc-utils";

const tempDirs: string[] = [];
const servers: EidnaraRpcServer[] = [];

afterEach(() => {
    for (const server of servers.splice(0)) server.stop();
    __resetNotificationStateForTests();
    __resetRpcIdentityTestHooks();
    for (const dir of tempDirs.splice(0)) {
        rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    }
});

function makeTempDir(): string {
    const dir = mkdtempSync(join(tmpdir(), "eidnara-rpc-server-"));
    tempDirs.push(dir);
    return dir;
}

function makeServer(storageDir: string, directory: string): EidnaraRpcServer {
    const server = new EidnaraRpcServer(storageDir, directory);
    servers.push(server);
    return server;
}

function seedPortFile(storageDir: string, directory: string, pid: number, port: number): string {
    const path = rpcPortFilePath(storageDir, directory, pid, "seeded");
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, JSON.stringify({ port, pid, started_at: Date.now() - 60_000 }), "utf-8");
    return path;
}

/** Make `isPidAlive` report `state` for `pid` while leaving every other PID's probe untouched. */
function stubPidLiveness(pid: number, state: "alive" | "dead" | "inconclusive"): void {
    __setRpcIdentityTestHooks({
        processKill: ((target: number, signal?: number | string) => {
            if (target !== pid) return process.kill(target, signal as never);
            if (state === "alive") return true;
            const error = new Error(state) as NodeJS.ErrnoException;
            error.code = state === "dead" ? "ESRCH" : "EPERM";
            throw error;
        }) as typeof process.kill,
    });
}

const OTHER_INSTANCE_LOG = "another Eidnara RPC server is active";

function readPortRecords(storageDir: string, directory: string): RpcPortFileRecord[] {
    const records: RpcPortFileRecord[] = [];
    for (const entry of readdirSync(rpcPortDir(storageDir, directory))) {
        if (!entry.startsWith("port-") || !entry.endsWith(".json")) continue;
        const record = parseRpcPortFile(
            readFileSync(join(rpcPortDir(storageDir, directory), entry), "utf-8"),
        );
        if (record) records.push(record);
    }
    return records;
}

function readToken(storageDir: string, directory: string): string {
    for (const record of readPortRecords(storageDir, directory)) {
        if (record.pid === process.pid && typeof record.token === "string") return record.token;
    }
    throw new Error("no port file for this process");
}

function readNewestPortRecord(storageDir: string, directory: string): RpcPortFileRecord | null {
    const records = readPortRecords(storageDir, directory);
    records.sort((a, b) => b.started_at - a.started_at);
    return records[0] ?? null;
}

describe("EidnaraRpcServer port-file directory scan", () => {
    test("a port file for a dead PID does not trigger the other-instance warning", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-dead-pid";
        const deadPid = 4_000_001;
        seedPortFile(storageDir, directory, deadPid, 65_000);
        stubPidLiveness(deadPid, "dead");
        const logSpy = spyOn(logger, "log");

        try {
            await makeServer(storageDir, directory).start();
            const warnings = logSpy.mock.calls.filter(([message]) =>
                String(message).includes(OTHER_INSTANCE_LOG),
            );
            expect(warnings).toHaveLength(0);
        } finally {
            logSpy.mockRestore();
        }
    });

    test("a port file for a live foreign PID triggers the other-instance warning", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-live-pid";
        const livePid = 4_000_002;
        seedPortFile(storageDir, directory, livePid, 65_001);
        stubPidLiveness(livePid, "alive");
        const logSpy = spyOn(logger, "log");

        try {
            await makeServer(storageDir, directory).start();
            const warnings = logSpy.mock.calls.filter(([message]) =>
                String(message).includes(OTHER_INSTANCE_LOG),
            );
            expect(warnings).toHaveLength(1);
            expect(String(warnings[0][0])).toContain(`pid ${livePid}`);
        } finally {
            logSpy.mockRestore();
        }
    });

    test("start() removes port files whose PID is dead and keeps the rest", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-prune";
        const deadPid = 4_000_003;
        const inconclusivePid = 4_000_004;
        const deadPath = seedPortFile(storageDir, directory, deadPid, 65_002);
        const inconclusivePath = rpcPortFilePath(storageDir, directory, inconclusivePid, "held");
        writeFileSync(
            inconclusivePath,
            JSON.stringify({ port: 65_003, pid: inconclusivePid, started_at: Date.now() }),
            "utf-8",
        );
        __setRpcIdentityTestHooks({
            processKill: ((target: number, signal?: number | string) => {
                if (target === deadPid) {
                    const error = new Error("dead") as NodeJS.ErrnoException;
                    error.code = "ESRCH";
                    throw error;
                }
                if (target === inconclusivePid) {
                    const error = new Error("denied") as NodeJS.ErrnoException;
                    error.code = "EPERM";
                    throw error;
                }
                return process.kill(target, signal as never);
            }) as typeof process.kill,
        });

        const server = makeServer(storageDir, directory);
        await server.start();

        expect(existsSync(deadPath)).toBe(false);
        // A denied probe is not proof of death, so the record survives for the client's health check.
        expect(existsSync(inconclusivePath)).toBe(true);
    });
});

describe("EidnaraRpcServer start()", () => {
    test("reports failure and does not leave a listener when the port file cannot be written", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-unwritable";
        // A regular file where the `rpc/` directory must be created makes mkdirSync throw.
        writeFileSync(join(storageDir, "rpc"), "not a directory", "utf-8");
        const server = makeServer(storageDir, directory);
        server.handle("ping", async () => ({ pong: true }));

        const port = await server.start();

        expect(port).toBe(0);
        expect(existsSync(rpcPortFilePath(storageDir, directory))).toBe(false);
    });

    test("a second start() reuses the running listener instead of binding another port", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-double-start";
        const server = makeServer(storageDir, directory);

        const first = await server.start();
        const second = await server.start();

        expect(second).toBe(first);
        const portFiles = readdirSync(rpcPortDir(storageDir, directory)).filter(
            (entry) => entry.startsWith("port-") && entry.endsWith(".json"),
        );
        expect(portFiles).toHaveLength(1);

        server.stop();
        await expect(fetch(`http://127.0.0.1:${first}/health`)).rejects.toThrow();
    });
});

describe("EidnaraRpcServer request body limit", () => {
    test("rejects a body over 1 MiB of bytes even when it is under 1 Mi UTF-16 code units", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-body-limit";
        const server = makeServer(storageDir, directory);
        server.handle("echo", async (params) => ({ received: Object.keys(params).length }));
        const port = await server.start();
        const token = readToken(storageDir, directory);

        // 400k three-byte characters: ~400k code units but ~1.2 MiB of bytes.
        const body = JSON.stringify({ s: "€".repeat(400_000) });
        expect(body.length).toBeLessThan(1_048_576);
        expect(Buffer.byteLength(body, "utf8")).toBeGreaterThan(1_048_576);

        const res = await fetch(`http://127.0.0.1:${port}/rpc/echo`, {
            method: "POST",
            headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
            body,
        });
        expect(res.status).toBe(413);
    });
});

describe("EidnaraRpcServer RPC params", () => {
    test.each([
        "null",
        "[1, 2]",
        '"text"',
        "42",
    ])("rejects the valid JSON body %s with 400 before invoking the handler", async (body) => {
        const storageDir = makeTempDir();
        const directory = "/repo-params";
        const server = makeServer(storageDir, directory);
        let invoked = 0;
        server.handle("echo", async (params) => {
            invoked += 1;
            return { keys: Object.keys(params) };
        });
        const port = await server.start();
        const token = readToken(storageDir, directory);

        const res = await fetch(`http://127.0.0.1:${port}/rpc/echo`, {
            method: "POST",
            headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
            body,
        });

        expect(res.status).toBe(400);
        expect(invoked).toBe(0);
    });
});

async function helloSocket(
    port: number,
    token: string,
    hello: Record<string, unknown>,
): Promise<WebSocket> {
    const ws = await openRpcSocket(port, token);
    const helloAck = waitForJsonMessage(ws, (message) => message.type === "hello-ack");
    ws.send(JSON.stringify({ type: "hello", token, ...hello }));
    await helloAck;
    return ws;
}

describe("EidnaraRpcServer WebSocket frames", () => {
    test("ignores valid JSON frames that are not objects and keeps the socket open", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-ws-null-frame";
        const server = makeServer(storageDir, directory);
        const port = await server.start();
        const token = readToken(storageDir, directory);
        const ws = await helloSocket(port, token, { sessionId: "ses_A", protocol: 2 });

        try {
            ws.send("null");
            ws.send("[]");
            ws.send('"hello"');
            const pushed = waitForJsonMessage<{ type?: string; notification?: { type: string } }>(
                ws,
                (message) => message.type === "notification",
            );
            pushNotification("after-ignored-frames", { ok: true }, "ses_A");
            expect((await pushed).notification?.type).toBe("after-ignored-frames");
            expect(ws.readyState).toBe(WebSocket.OPEN);
        } finally {
            ws.close();
        }
    });
});

describe("EidnaraRpcServer acknowledgement scope", () => {
    test("a protocol 2 socket cannot acknowledge another session's notification", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-ack-scope-p2";
        const server = makeServer(storageDir, directory);
        const port = await server.start();
        const token = readToken(storageDir, directory);

        pushNotification("for-a", { ok: true }, "ses_A");
        pushNotification("for-b", { ok: true }, "ses_B");
        pushNotification("for-everyone", { ok: true });
        const [forA] = drainNotifications(0, "ses_A", { sessionOnly: true });
        const [forB] = drainNotifications(0, "ses_B", { sessionOnly: true });
        const [forEveryone] = drainNotifications(0, undefined, { globalOnly: true });

        const ws = await helloSocket(port, token, { sessionId: "ses_A", protocol: 2 });
        try {
            ws.send(JSON.stringify({ type: "ack", ids: [forA.id, forB.id, forEveryone.id] }));
            await waitFor(
                () => drainNotifications(0, "ses_A", { sessionOnly: true }).length === 0,
                "own-session acknowledgement",
            );
            expect(drainNotifications(0, undefined, { globalOnly: true })).toHaveLength(0);
            expect(drainNotifications(0, "ses_B", { sessionOnly: true }).map((n) => n.id)).toEqual([
                forB.id,
            ]);
        } finally {
            ws.close();
        }
    });

    test("a legacy cursor acknowledgement naming another session is ignored", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-ack-scope-legacy";
        const server = makeServer(storageDir, directory);
        const port = await server.start();
        const token = readToken(storageDir, directory);

        pushNotification("for-a", { ok: true }, "ses_A");
        pushNotification("for-b", { ok: true }, "ses_B");
        const [forA] = drainNotifications(0, "ses_A", { sessionOnly: true });
        const [forB] = drainNotifications(0, "ses_B", { sessionOnly: true });

        const ws = await helloSocket(port, token, { sessionId: "ses_A" });
        try {
            ws.send(JSON.stringify({ type: "ack", cursor: forB.id, sessionId: "ses_B" }));
            // Frames arrive in order, so the ses_B frame has been handled once ses_A is pruned.
            ws.send(JSON.stringify({ type: "ack", cursor: forA.id, sessionId: "ses_A" }));
            await waitFor(
                () => drainNotifications(0, "ses_A", { sessionOnly: true }).length === 0,
                "own-session acknowledgement",
            );
            expect(drainNotifications(0, "ses_B", { sessionOnly: true }).map((n) => n.id)).toEqual([
                forB.id,
            ]);
        } finally {
            ws.close();
        }
    });

    test("a session-less legacy socket sees every session and may acknowledge any of them", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-ack-scope-legacy-global";
        const server = makeServer(storageDir, directory);
        const port = await server.start();
        const token = readToken(storageDir, directory);

        pushNotification("for-b", { ok: true }, "ses_B");
        const [forB] = drainNotifications(0, "ses_B", { sessionOnly: true });

        const ws = await helloSocket(port, token, {});
        try {
            ws.send(JSON.stringify({ type: "ack", cursor: forB.id, sessionId: "ses_B" }));
            await waitFor(
                () => drainNotifications(0, "ses_B", { sessionOnly: true }).length === 0,
                "session acknowledgement from a session-less legacy socket",
            );
        } finally {
            ws.close();
        }
    });
});

describe("EidnaraRpcServer HTTP authentication", () => {
    test("authenticates against a real server with the published token", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-auth";
        const server = makeServer(storageDir, directory);
        server.handle("ping", async () => ({ pong: true }));
        await server.start();

        const client = new EidnaraRpcClient(storageDir, directory);
        expect(await client.call<{ pong: boolean }>("ping")).toEqual({ pong: true });
    });

    test("a request without the token is rejected 401 by the server", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-noauth";
        const server = makeServer(storageDir, directory);
        server.handle("ping", async () => ({ pong: true }));
        const port = await server.start();
        const record = readNewestPortRecord(storageDir, directory);
        expect(typeof record?.token).toBe("string");
        expect((record?.token ?? "").length).toBeGreaterThan(0);

        const res = await fetch(`http://127.0.0.1:${port}/rpc/ping`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: "{}",
        });
        expect(res.status).toBe(401);

        // The /health endpoint requires no token.
        const health = await fetch(`http://127.0.0.1:${port}/health`);
        expect(health.status).toBe(200);
    });

    test("same-process servers keep distinct port files during overlap", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-port-collision";
        const first = makeServer(storageDir, directory);
        const second = makeServer(storageDir, directory);
        await first.start();
        const secondPort = await second.start();

        const files = readdirSync(rpcPortDir(storageDir, directory)).filter(
            (entry) => entry.startsWith("port-") && entry.endsWith(".json"),
        );
        expect(files.length).toBeGreaterThanOrEqual(2);

        first.stop();
        const remaining = readNewestPortRecord(storageDir, directory);
        expect(remaining?.port).toBe(secondPort);

        const client = new EidnaraRpcClient(storageDir, directory);
        const endpoint = await client.resolveEndpoint();
        expect(endpoint?.port).toBe(secondPort);
        expect(endpoint?.instanceId).toBe(remaining?.instance_id);
    });
});

describe("EidnaraRpcServer WebSocket handshake", () => {
    test("accepts a frozen v0.32 websocket upgrade with query-token auth", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-ws-v032";
        const server = makeServer(storageDir, directory);
        const port = await server.start();
        const token = readToken(storageDir, directory);

        const ws = await openRpcSocket(port, token, true);
        try {
            const helloAck = waitForJsonMessage(ws, (message) => message.type === "hello-ack");
            ws.send(JSON.stringify({ type: "hello", token, sessionId: "ses_v032" }));
            expect((await helloAck).type).toBe("hello-ack");
        } finally {
            ws.close();
        }
    });

    test("websocket upgrade rejects missing bearer token before a socket is created", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-ws-auth";
        const server = makeServer(storageDir, directory);
        const port = await server.start();

        const res = await fetch(`http://127.0.0.1:${port}/ws`);
        expect(res.status).toBe(401);
        expect(isTuiConnected()).toBe(false);
    });

    test("re-hello replaces the previous websocket notification sink", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-ws-rehello";
        const server = makeServer(storageDir, directory);
        const port = await server.start();
        const token = readToken(storageDir, directory);

        const ws = await openRpcSocket(port, token);
        const notifications: unknown[] = [];
        ws.addEventListener("message", (event) => {
            const message = JSON.parse(String(event.data)) as {
                type?: string;
                notification?: unknown;
            };
            if (message.type === "notification") notifications.push(message.notification);
        });

        try {
            ws.send(JSON.stringify({ type: "hello", token, sessionId: "ses_A" }));
            await waitForJsonMessage(ws, (message) => message.type === "hello-ack");
            expect(isTuiConnected("ses_A")).toBe(true);

            ws.send(JSON.stringify({ type: "hello", token, sessionId: "ses_B" }));
            await waitForJsonMessage(ws, (message) => message.type === "hello-ack");
            expect(isTuiConnected("ses_A")).toBe(false);
            expect(isTuiConnected("ses_B")).toBe(true);

            ws.send(JSON.stringify({ type: "hello", token, sessionId: "ses_B" }));
            await waitForJsonMessage(ws, (message) => message.type === "hello-ack");
            pushNotification("live", { ok: true }, "ses_B");
            await waitFor(() => notifications.length >= 1, "one live notification");
            await new Promise((resolve) => setTimeout(resolve, 50));
            expect(notifications).toHaveLength(1);

            ws.close();
            await waitFor(() => !isTuiConnected(), "socket sink cleanup");
        } finally {
            ws.close();
        }
    });

    test("accepts legacy cursor acknowledgements during protocol skew", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-legacy-ack";
        const server = makeServer(storageDir, directory);
        const port = await server.start();
        const record = readNewestPortRecord(storageDir, directory);
        const token = record?.token ?? "";
        expect(token.length).toBeGreaterThan(0);

        pushNotification("legacy-one", { ok: true }, "ses_legacy");
        pushNotification("legacy-two", { ok: true }, "ses_legacy");
        const queued = drainNotifications(0, "ses_legacy", { sessionOnly: true });
        expect(queued).toHaveLength(2);

        const ws = await openRpcSocket(port, token);
        try {
            const helloAck = waitForJsonMessage<{ type?: string; instanceId?: string }>(
                ws,
                (message) => message.type === "hello-ack",
            );
            ws.send(JSON.stringify({ type: "hello", token, sessionId: "ses_legacy" }));
            expect((await helloAck).instanceId).toBe(record?.instance_id);

            ws.send(JSON.stringify({ type: "ack", cursor: queued[0].id, sessionId: "ses_legacy" }));
            await waitFor(() => {
                const pending = drainNotifications(0, "ses_legacy", { sessionOnly: true });
                return pending.length === 1 && pending[0].id === queued[1].id;
            }, "legacy cursor acknowledgement pruning");
        } finally {
            ws.close();
        }
    });
});

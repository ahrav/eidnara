import { randomBytes, timingSafeEqual } from "node:crypto";
import {
    chmodSync,
    mkdirSync,
    readdirSync,
    readFileSync,
    renameSync,
    rmSync,
    unlinkSync,
    writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import type { Server, ServerWebSocket } from "bun";
import { log } from "./logger";
import {
    acknowledgeNotifications,
    drainNotifications,
    isLegacySink,
    type NotificationSink,
    registerNotificationSink,
    scopeSeesSession,
} from "./rpc-notifications";
import { isPidAlive, parseRpcPortFile, rpcPortDir, rpcPortFilePath } from "./rpc-utils";

type RpcHandler = (params: Record<string, unknown>) => Promise<Record<string, unknown>>;

/** Upper bound on a request body in bytes; `Bun.serve` rejects larger bodies with 413 before buffering. */
const MAX_BODY_BYTES = 1_048_576;
/** The server closes a WS client that does not authenticate within 5,000 ms. */
const WS_AUTH_TIMEOUT_MS = 5_000;
/** The server closes WebSocket authentication failures with code 4401.
 * */
const WS_CLOSE_UNAUTHORIZED = 4401;
/** `ws.send` returns this when the frame was dropped because the connection is unusable. */
const WS_SEND_DROPPED = 0;

/** `WsData` stores per-socket state in `ServerWebSocket.data`. */
interface WsData {
    authed: boolean;
    sessionId?: string;
    /** The protocol the client announced in its hello; absent for legacy clients. */
    protocol?: number;
    /** `unregister` removes this socket's sink from the notification registry. */
    unregister?: () => void;
    /** The auth timer fires if the client never sends a valid hello. */
    authTimer?: ReturnType<typeof setTimeout>;
}

/**
 * `tokensMatch` checks buffer lengths before calling `timingSafeEqual`, which throws for unequal-length buffers.
 * Token length is not secret, but token bytes are.
 * The comparison avoids leaking token bytes through loopback-auth response timing.
 */
function tokensMatch(presented: string, expected: string): boolean {
    const a = Buffer.from(presented, "utf8");
    const b = Buffer.from(expected, "utf8");
    if (a.length !== b.length) return false;
    return timingSafeEqual(a, b);
}

function bearerToken(req: Request): string {
    const auth = req.headers.get("authorization");
    return typeof auth === "string" ? auth.replace(/^Bearer\s+/i, "") : "";
}

function websocketToken(req: Request): string {
    const headerToken = bearerToken(req);
    if (headerToken) return headerToken;
    return new URL(req.url).searchParams.get("token") ?? "";
}

/**
 * `EidnaraRpcServer` provides localhost RPC communication between the TUI and server plugin.
 *
 * The TUI uses `/health` and `/rpc/<method>` for event-driven snapshot and dialog-result requests.
 * The TUI makes snapshot and dialog-result calls through HTTP routes rather than an idle connection.
 * The TUI uses `/ws` for a persistent WebSocket connection.
 * The server pushes dialog and toast actions over `/ws`.
 */
export class EidnaraRpcServer {
    private server: Server<WsData> | null = null;
    private port = 0;
    private handlers = new Map<string, RpcHandler>();
    private portFilePath: string;
    private portDir: string;
    private startedAt = Date.now();
    private readonly instanceId = randomBytes(8).toString("hex");
    /** `sockets` tracks authenticated WebSocket sockets so `dispose` can close them. */
    private sockets = new Set<ServerWebSocket<WsData>>();
    // Each server instance publishes its bearer token in the user-private port file.
    // The server requires the token on every non-health RPC call and in the WebSocket hello.
    // The token protects recompilation, upgrade, dismissal, and push-channel endpoints.
    // The token blocks local processes and browser scripts that discover or guess the port from accessing protected endpoints.
    private readonly token = randomBytes(32).toString("hex");

    constructor(storageDir: string, directory: string) {
        this.portFilePath = rpcPortFilePath(storageDir, directory, process.pid, this.instanceId);
        this.portDir = rpcPortDir(storageDir, directory);
    }

    /* */
    handle(method: string, handler: RpcHandler): void {
        this.handlers.set(method, handler);
    }

    /** A second `start()` on a running instance returns the existing port instead of binding a second listener. */
    async start(): Promise<number> {
        if (typeof Bun === "undefined") {
            // The terminal-TUI sidebar is unavailable on Node/Electron.
            // On Node/Electron, no RPC consumer exists, so start returns without calling Bun.serve.
            log("rpc server skipped: Bun runtime not available (no TUI consumer)");
            return 0;
        }
        if (this.server) return this.port;
        this.startedAt = Date.now();
        const self = this;
        const server = Bun.serve<WsData>({
            port: 0,
            hostname: "127.0.0.1",
            // The runtime enforces the byte bound before the body is buffered and answers 413 itself.
            maxRequestBodySize: MAX_BODY_BYTES,
            fetch(req, srv) {
                return self.handleFetch(req, srv);
            },
            websocket: {
                // A client that stops reading is closed when its send buffer fills.
                closeOnBackpressureLimit: true,
                open(ws) {
                    // The server closes unauthenticated sockets after `WS_AUTH_TIMEOUT_MS`.
                    ws.data.authTimer = setTimeout(() => {
                        if (!ws.data.authed) ws.close(WS_CLOSE_UNAUTHORIZED, "auth timeout");
                    }, WS_AUTH_TIMEOUT_MS);
                },
                message(ws, raw) {
                    self.handleWsMessage(ws, raw);
                },
                close(ws) {
                    if (ws.data.authTimer) clearTimeout(ws.data.authTimer);
                    ws.data.unregister?.();
                    self.sockets.delete(ws);
                },
            },
        });

        this.server = server;
        this.port = server.port ?? 0;

        // The port-file writer writes each instance's port file atomically so readers never observe a partial file.
        try {
            this.reconcileSiblingPortFiles();
            const dir = dirname(this.portFilePath);
            // The port file carries the bearer token, so the directory and file are owner-only.
            mkdirSync(dir, { recursive: true, mode: 0o700 });
            try {
                chmodSync(dir, 0o700);
            } catch {}
            const tmpPath = `${this.portFilePath}.tmp`;
            // The port-file writer must not reuse a stale temporary file with loose permissions after a crashed write.
            // `writeFileSync` applies `mode` only when creating a file, so remove the stale temporary file first.
            try {
                rmSync(tmpPath, { force: true });
            } catch {
                // best-effort
            }
            writeFileSync(
                tmpPath,
                JSON.stringify({
                    port: this.port,
                    pid: process.pid,
                    started_at: this.startedAt,
                    kind: "OpenCode server",
                    token: this.token,
                    instance_id: this.instanceId,
                }),
                { encoding: "utf-8", mode: 0o600 },
            );
            renameSync(tmpPath, this.portFilePath);
            try {
                chmodSync(this.portFilePath, 0o600);
            } catch {}
            log(`[rpc] server listening on 127.0.0.1:${this.port}`);
        } catch (err) {
            // A listener without a port file is unreachable by every client, so the
            // server is torn down and start() reports the same 0 as a skipped start.
            log(`[rpc] failed to write port file; stopping server: ${err}`);
            void server.stop(true);
            this.server = null;
            this.port = 0;
        }

        return this.port;
    }

    /** Bun 1.3.14 never settles the stop promise when a WebSocket closed by the server remains in `pendingWebSockets`. */
    stop(): void {
        for (const ws of this.sockets) {
            try {
                if (ws.data.authTimer) clearTimeout(ws.data.authTimer);
                ws.data.unregister?.();
                ws.close();
            } catch {
                // best-effort
            }
        }
        this.sockets.clear();
        if (this.server) {
            void this.server.stop(true);
            this.server = null;
        }
        try {
            unlinkSync(this.portFilePath);
        } catch {
            // The port file may already be gone.
        }
    }

    /**
     * A record with a confirmed-dead PID is stale and is unlinked to bound directory growth.
     * A denied or failed probe does not prove death, so the record remains.
     */
    private reconcileSiblingPortFiles(): void {
        let liveSibling: { pid: number; port: number } | null = null;
        try {
            for (const entry of readdirSync(this.portDir)) {
                if (!entry.startsWith("port-") || !entry.endsWith(".json")) continue;
                const entryPath = join(this.portDir, entry);
                const record = parseRpcPortFile(readFileSync(entryPath, "utf-8"));
                if (!record || record.pid === process.pid) continue;
                const liveness = isPidAlive(record.pid);
                if (liveness === "dead") {
                    try {
                        unlinkSync(entryPath);
                    } catch {
                        // Another instance may have removed it first.
                    }
                    continue;
                }
                if (liveness === "alive" && liveSibling === null) {
                    liveSibling = { pid: record.pid, port: record.port };
                }
            }
        } catch {}
        if (liveSibling) {
            log(
                `[rpc] another Eidnara RPC server is active for this project (pid ${liveSibling.pid}, port ${liveSibling.port}); starting separate instance on a new port`,
            );
        }
    }

    /** Bun fetch returns undefined after upgrading a request to a WebSocket.
     * */
    private async handleFetch(req: Request, srv: Server<WsData>): Promise<Response | undefined> {
        const url = new URL(req.url);

        // The handler authenticates the WebSocket request before srv.upgrade so unauthorized requests never become live sockets.
        if (url.pathname === "/ws") {
            if (!tokensMatch(websocketToken(req), this.token)) {
                return new Response("Unauthorized", { status: 401 });
            }
            const ok = srv.upgrade(req, { data: { authed: false } });
            if (ok) return undefined;
            return new Response("upgrade failed", { status: 400 });
        }

        if (req.method === "GET" && url.pathname === "/health") {
            return json({ ok: true, pid: process.pid, instance_id: this.instanceId });
        }

        if (req.method !== "POST" || !url.pathname.startsWith("/rpc/")) {
            return new Response("Not Found", { status: 404 });
        }

        // Every side-effecting call requires the per-process bearer token.
        if (!tokensMatch(bearerToken(req), this.token)) {
            return json({ error: "Unauthorized" }, 401);
        }

        const method = url.pathname.slice(5); // strip "/rpc/"
        const handler = this.handlers.get(method);
        if (!handler) {
            return json({ error: `Unknown method: ${method}` }, 404);
        }

        const bodyText = await req.text();
        let params: Record<string, unknown> = {};
        if (bodyText.length > 0) {
            let decoded: unknown;
            try {
                decoded = JSON.parse(bodyText);
            } catch {
                return json({ error: "Invalid JSON" }, 400);
            }
            // Handlers receive `Record<string, unknown>`, so `null`, arrays, and primitives are rejected here.
            if (!isPlainObject(decoded)) {
                return json({ error: "Params must be a JSON object" }, 400);
            }
            params = decoded;
        }

        try {
            const result = await handler(params);
            return json(result);
        } catch (err) {
            log(`[rpc] handler error: ${method} => ${err}`);
            return json({ error: String(err) }, 500);
        }
    }

    /**
     * */
    private handleWsMessage(ws: ServerWebSocket<WsData>, raw: string | Buffer): void {
        let msg: {
            type?: string;
            token?: string;
            sessionId?: string;
            lastReceivedId?: number;
            globalLastReceivedId?: number;
            ackScope?: string;
            protocol?: number;
            instanceId?: string;
            ids?: unknown;
            cursor?: number;
        };
        try {
            const decoded: unknown = JSON.parse(
                typeof raw === "string" ? raw : raw.toString("utf8"),
            );
            // Valid JSON such as `null` or `[]` is not a frame; property reads on it are skipped.
            if (!isPlainObject(decoded)) return;
            msg = decoded;
        } catch {
            return;
        }

        if (msg.type !== "hello" && !ws.data.authed) return;

        if (msg.type === "hello") {
            if (!tokensMatch(typeof msg.token === "string" ? msg.token : "", this.token)) {
                this.sendFrame(ws, { type: "error", error: "unauthorized" });
                ws.close(WS_CLOSE_UNAUTHORIZED, "bad token");
                return;
            }
            if (ws.data.authTimer) {
                clearTimeout(ws.data.authTimer);
                ws.data.authTimer = undefined;
            }
            ws.data.authed = true;
            ws.data.sessionId =
                typeof msg.sessionId === "string" && msg.sessionId.length > 0
                    ? msg.sessionId
                    : undefined;
            ws.data.protocol = typeof msg.protocol === "number" ? msg.protocol : undefined;

            // Before sending another hello, `handleWsMessage` removes the old sink so each socket has exactly one live sink.
            // `handleWsMessage` registers the replacement sink before sending hello so each socket has exactly one live sink.
            ws.data.unregister?.();
            ws.data.unregister = undefined;

            // `handleWsMessage` registers a live sink so future pushes reach this socket immediately.
            const sink: NotificationSink = {
                sessionId: ws.data.sessionId,
                protocol: ws.data.protocol,
                send: (notification) => {
                    this.sendFrame(ws, { type: "notification", notification });
                },
            };
            ws.data.unregister = registerNotificationSink(sink);
            this.sockets.add(ws);

            const usesExactAcknowledgements = !isLegacySink(sink);
            // The server sends the epoch before backlog frames so the client discards cursors and deduplication entries from a replaced server first.
            this.sendFrame(ws, {
                type: "hello-ack",
                protocol: 2,
                instanceId: this.instanceId,
            });

            let backlog: ReturnType<typeof drainNotifications>;
            if (usesExactAcknowledgements) {
                // Protocol 2 never treats a high handled ID as proof that lower IDs were consumed.
                // Only exact acknowledgements remove entries, so declined or interrupted dialogs survive reconnects.
                backlog =
                    ws.data.sessionId === undefined
                        ? drainNotifications(0, undefined, { globalOnly: true })
                        : drainNotifications(0, ws.data.sessionId, {
                              globalLastReceivedId: 0,
                          });
            } else {
                // Legacy clients use independent session and global watermarks.
                const lastReceivedId = Number(msg.lastReceivedId ?? 0);
                const sessionCursor = Number.isFinite(lastReceivedId) ? lastReceivedId : 0;
                const hasGlobalCursor = typeof msg.globalLastReceivedId === "number";
                const globalLastReceivedId = hasGlobalCursor
                    ? Number.isFinite(msg.globalLastReceivedId)
                        ? msg.globalLastReceivedId
                        : 0
                    : 0;
                backlog =
                    ws.data.sessionId === undefined && hasGlobalCursor
                        ? drainNotifications(globalLastReceivedId, undefined, { globalOnly: true })
                        : drainNotifications(
                              sessionCursor,
                              ws.data.sessionId,
                              hasGlobalCursor
                                  ? { globalLastReceivedId: globalLastReceivedId }
                                  : undefined,
                          );
            }
            for (const notification of backlog) {
                if (!this.sendFrame(ws, { type: "notification", notification })) break;
            }
            return;
        }

        if (msg.type === "ack") {
            // Acknowledgements use this socket's session and protocol scope, so one session cannot remove another's queued notifications.
            const scope = { sessionId: ws.data.sessionId, protocol: ws.data.protocol };
            if (Array.isArray(msg.ids)) {
                acknowledgeNotifications(
                    msg.ids.filter((id): id is number => typeof id === "number"),
                    scope,
                );
                return;
            }
            // A strict-protocol socket only acknowledges exact ids; a watermark from it would remove lower entries it never handled.
            if (!isLegacySink(scope)) return;

            // Legacy clients require watermark acknowledgements.
            // Legacy acknowledgements apply only to the current socket scope.
            const lastReceivedId = Number(msg.cursor ?? msg.lastReceivedId ?? 0);
            if (Number.isFinite(lastReceivedId) && lastReceivedId > 0) {
                if (msg.ackScope === "global") {
                    // The socket's session identifies the scope that handled the global entries.
                    drainNotifications(lastReceivedId, ws.data.sessionId, { globalOnly: true });
                } else if (typeof msg.sessionId === "string" && msg.sessionId.length > 0) {
                    if (scopeSeesSession(scope, msg.sessionId)) {
                        drainNotifications(lastReceivedId, msg.sessionId, { sessionOnly: true });
                    }
                } else {
                    // Older clients use one cursor for their current socket scope.
                    drainNotifications(lastReceivedId, ws.data.sessionId);
                }
            }
        }
    }

    /** Returns false after Bun drops a frame on an unusable connection and the socket is closed. */
    private sendFrame(ws: ServerWebSocket<WsData>, frame: Record<string, unknown>): boolean {
        if (ws.send(JSON.stringify(frame)) !== WS_SEND_DROPPED) return true;
        try {
            ws.close();
        } catch {
            // The socket is already closing.
        }
        return false;
    }
}

/** Narrows decoded JSON to the object shape handlers and frame readers expect. */
function isPlainObject(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

/* */
function json(body: unknown, status = 200): Response {
    return new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
    });
}

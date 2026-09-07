import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { EidnaraRpcClient } from "./rpc-client";
import { rpcPortFilePath } from "./rpc-utils";

interface TestServer {
    port: number;
    close: () => Promise<void>;
}

const tempDirs: string[] = [];
let servers: TestServer[] = [];

afterEach(async () => {
    for (const server of servers.splice(0)) {
        await server.close();
    }
    for (const dir of tempDirs.splice(0)) {
        try {
            rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
        } catch {
            /* */
        }
    }
});

function makeTempDir(): string {
    const dir = mkdtempSync(join(tmpdir(), "eidnara-rpc-discovery-"));
    tempDirs.push(dir);
    return dir;
}

function writePortFile(storageDir: string, directory: string, port: number): void {
    const portFile = rpcPortFilePath(storageDir, directory);
    mkdirSync(dirname(portFile), { recursive: true });
    writeFileSync(
        portFile,
        JSON.stringify({ port, pid: process.pid, started_at: Date.now() }),
        "utf-8",
    );
}

function writePortFileForPid(
    storageDir: string,
    directory: string,
    port: number,
    pid: number,
    startedAt: number,
    instanceId?: string,
    token?: string,
): string {
    const portFile = rpcPortFilePath(storageDir, directory, pid, instanceId);
    mkdirSync(dirname(portFile), { recursive: true });
    writeFileSync(
        portFile,
        JSON.stringify({
            port,
            pid,
            started_at: startedAt,
            instance_id: instanceId,
            token,
        }),
        "utf-8",
    );
    return portFile;
}

/** Each fixture answers `/health` with the identity a test assigns it, so one test can stand up several candidates. */
async function startRpcServer(
    handler: (method: string) => Response | object,
    health: { pid: number; instanceId?: string } = { pid: process.pid },
): Promise<TestServer> {
    const server = createServer(async (req, res) => {
        if (req.method === "GET" && req.url === "/health") {
            res.writeHead(200, { "Content-Type": "application/json" });
            res.end(JSON.stringify({ ok: true, pid: health.pid, instance_id: health.instanceId }));
            return;
        }

        if (req.method === "POST" && req.url?.startsWith("/rpc/")) {
            const method = req.url.slice("/rpc/".length);
            const result = handler(method);
            if (result instanceof Response) {
                res.writeHead(result.status, { "Content-Type": "application/json" });
                res.end(await result.text());
                return;
            }
            res.writeHead(200, { "Content-Type": "application/json" });
            res.end(JSON.stringify(result));
            return;
        }

        res.writeHead(404);
        res.end("Not Found");
    });

    await new Promise<void>((resolve, reject) => {
        server.once("error", reject);
        server.listen(0, "127.0.0.1", () => resolve());
    });
    const addr = server.address();
    if (!addr || typeof addr === "string") throw new Error("failed to bind test server");

    const testServer = {
        port: addr.port,
        close: () =>
            new Promise<void>((resolve, reject) => {
                server.close((err) => (err ? reject(err) : resolve()));
            }),
    };
    servers.push(testServer);
    return testServer;
}

async function closeServer(server: TestServer): Promise<void> {
    servers = servers.filter((s) => s !== server);
    await server.close();
}

describe("EidnaraRpcClient discovery", () => {
    test("gives up when the port file points at a dead server", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo";
        const dead = await startRpcServer(() => ({ ok: true }));
        const port = dead.port;
        await closeServer(dead);
        writePortFile(storageDir, directory, port);

        const client = new EidnaraRpcClient(storageDir, directory);
        await expect(client.call("value")).rejects.toThrow("Eidnara RPC server not available");
    }, 20_000);

    test("ignores newer stale pid files and discovers the latest live instance", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo";
        const live = await startRpcServer(() => ({ value: "live" }));
        writePortFileForPid(storageDir, directory, 65535, 999_999_999, Date.now() + 10_000);
        writePortFileForPid(storageDir, directory, live.port, process.pid, Date.now());

        const client = new EidnaraRpcClient(storageDir, directory);
        expect(await client.call<{ value: string }>("value")).toEqual({ value: "live" });
    });

    test("discovers a frozen v0.32 health response without an instance id", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-v032-health";
        const legacy = await startRpcServer(() => ({ value: "legacy" }), {
            pid: process.pid,
        });
        writePortFileForPid(
            storageDir,
            directory,
            legacy.port,
            process.pid,
            Date.now(),
            "new-client-record",
        );

        const client = new EidnaraRpcClient(storageDir, directory);
        expect(await client.call<{ value: string }>("value")).toEqual({ value: "legacy" });
    });

    test("prefers this process and validates every discovery candidate identity", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-affinity";
        const foreign = await startRpcServer(() => ({ value: "foreign" }), {
            pid: process.ppid,
            instanceId: "foreign",
        });
        const unrelated = await startRpcServer(() => ({ value: "unrelated" }), {
            pid: process.pid,
            instanceId: "different-service",
        });
        const local = await startRpcServer(() => ({ value: "local" }), {
            pid: process.pid,
            instanceId: "local-healthy",
        });

        writePortFileForPid(
            storageDir,
            directory,
            foreign.port,
            process.ppid,
            Date.now() + 20_000,
            "foreign",
        );
        writePortFileForPid(
            storageDir,
            directory,
            unrelated.port,
            process.pid,
            Date.now() + 10_000,
            "local-stale",
        );
        writePortFileForPid(
            storageDir,
            directory,
            local.port,
            process.pid,
            Date.now(),
            "local-healthy",
        );

        const client = new EidnaraRpcClient(storageDir, directory);
        expect(await client.call<{ value: string }>("value")).toEqual({ value: "local" });
    });

    test("resets discovery after a 401 response", async () => {
        const storageDir = makeTempDir();
        const directory = "/repo-reauth";
        let unauthorizedRecord = "";
        const unauthorized = await startRpcServer(
            () => {
                rmSync(unauthorizedRecord, { force: true });
                return new Response("stale token", { status: 401 });
            },
            { pid: process.pid, instanceId: "unauthorized" },
        );
        const healthy = await startRpcServer(() => ({ value: "healthy" }), {
            pid: process.pid,
            instanceId: "healthy",
        });
        unauthorizedRecord = writePortFileForPid(
            storageDir,
            directory,
            unauthorized.port,
            process.pid,
            Date.now() + 10_000,
            "unauthorized",
            "stale",
        );
        writePortFileForPid(
            storageDir,
            directory,
            healthy.port,
            process.pid,
            Date.now(),
            "healthy",
        );

        const client = new EidnaraRpcClient(storageDir, directory);
        expect(await client.call<{ value: string }>("value")).toEqual({ value: "healthy" });
    });
});

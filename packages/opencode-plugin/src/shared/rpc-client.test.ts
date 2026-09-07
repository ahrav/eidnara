import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { EidnaraRpcClient } from "./rpc-client";
import { legacyRpcPortFilePath, rpcPortDir, rpcPortFilePath } from "./rpc-utils";

const DIRECTORY = "/workspace/project";
const MAX_RETRIES = 10;

interface Fixture {
    port: number;
    hits: Map<string, number>;
    stop: () => void;
}

function serve(handler: (request: Request, path: string) => Response | Promise<Response>): Fixture {
    const hits = new Map<string, number>();
    const server = Bun.serve({
        hostname: "127.0.0.1",
        port: 0,
        fetch(request) {
            const path = new URL(request.url).pathname;
            hits.set(path, (hits.get(path) ?? 0) + 1);
            return handler(request, path);
        },
    });
    return { port: server.port, hits, stop: () => server.stop(true) };
}

function json(body: unknown): Response {
    return new Response(JSON.stringify(body), {
        headers: { "Content-Type": "application/json" },
    });
}

/** A response whose headers arrive immediately and whose body never completes. */
function stalledBody(): Response {
    return new Response(new ReadableStream({ start() {} }), {
        headers: { "Content-Type": "application/json" },
    });
}

function writePortFile(
    storageDir: string,
    record: { port: number; pid: number; started_at: number; token?: string },
): void {
    mkdirSync(rpcPortDir(storageDir, DIRECTORY), { recursive: true });
    writeFileSync(rpcPortFilePath(storageDir, DIRECTORY, record.pid), JSON.stringify(record));
}

function writeLegacyPortFile(storageDir: string, content: string): void {
    mkdirSync(rpcPortDir(storageDir, DIRECTORY), { recursive: true });
    writeFileSync(legacyRpcPortFilePath(storageDir, DIRECTORY), content);
}

function sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

const cleanups: Array<() => void> = [];
let storageDir = "";

function client(options?: { requestTimeoutMs?: number }): EidnaraRpcClient {
    return new EidnaraRpcClient(storageDir, DIRECTORY, {
        requestTimeoutMs: options?.requestTimeoutMs ?? 1_000,
        retryDelayMs: 0,
    });
}

afterEach(() => {
    for (const cleanup of cleanups.splice(0)) cleanup();
    if (storageDir) rmSync(storageDir, { recursive: true, force: true });
    storageDir = "";
});

function freshStorageDir(): void {
    storageDir = mkdtempSync(`${tmpdir()}/eidnara-rpc-client-`);
}

describe("EidnaraRpcClient", () => {
    test("call() posts the port-file token to a health-checked server", async () => {
        freshStorageDir();
        let authorization: string | null = null;
        const fixture = serve((request, path) => {
            if (path === "/health") return json({ pid: process.pid });
            authorization = request.headers.get("authorization");
            return json({ ok: true });
        });
        cleanups.push(fixture.stop);
        writePortFile(storageDir, {
            port: fixture.port,
            pid: process.pid,
            started_at: Date.now(),
            token: "secret",
        });

        await expect(client().call("ping")).resolves.toEqual({ ok: true });
        expect(authorization).toBe("Bearer secret");
    });

    test("call() fails instead of hanging when the server stalls after sending headers", async () => {
        freshStorageDir();
        const fixture = serve((_request, path) =>
            path === "/health" ? json({ pid: process.pid }) : stalledBody(),
        );
        cleanups.push(fixture.stop);
        writePortFile(storageDir, { port: fixture.port, pid: process.pid, started_at: Date.now() });

        const outcome = await Promise.race([
            client({ requestTimeoutMs: 50 })
                .call("ping")
                .then(
                    () => "resolved",
                    (error: unknown) => error,
                ),
            sleep(1_500).then(() => "hung"),
        ]);

        expect(outcome).toBeInstanceOf(Error);
    });

    test("call() runs the discovery ladder once when no server passes the health check", async () => {
        freshStorageDir();
        const fixture = serve(() => json({ pid: 1 }));
        cleanups.push(fixture.stop);
        writePortFile(storageDir, { port: fixture.port, pid: process.pid, started_at: Date.now() });

        await expect(client().call("ping")).rejects.toThrow("not available");
        expect(fixture.hits.get("/health")).toBe(MAX_RETRIES);
    });

    test("ignores a legacy port file that carries no pid", async () => {
        freshStorageDir();
        const fixture = serve(() => json({ pid: process.pid }));
        cleanups.push(fixture.stop);
        writeLegacyPortFile(storageDir, String(fixture.port));

        await expect(client().isAvailable()).resolves.toBe(false);
        expect(fixture.hits.get("/health")).toBeUndefined();
    });

    test("uses a legacy port file that records a live pid", async () => {
        freshStorageDir();
        const fixture = serve(() => json({ pid: process.pid }));
        cleanups.push(fixture.stop);
        writeLegacyPortFile(
            storageDir,
            JSON.stringify({ port: fixture.port, pid: process.pid, started_at: Date.now() }),
        );

        await expect(client().isAvailable()).resolves.toBe(true);
    });

    test("skips a port file whose pid cannot have written it", async () => {
        freshStorageDir();
        const fixture = serve(() => json({ pid: process.pid }));
        cleanups.push(fixture.stop);
        // A `started_at` far earlier than this process's own start time cannot belong to this pid.
        writePortFile(storageDir, { port: fixture.port, pid: process.pid, started_at: 1 });

        await expect(client().isAvailable()).resolves.toBe(false);
        expect(fixture.hits.get("/health")).toBeUndefined();
    });

    test("stops sending RPC calls to a cached port once the server's process exits", async () => {
        freshStorageDir();
        const child = Bun.spawn(["sleep", "60"], { stdio: ["ignore", "ignore", "ignore"] });
        cleanups.push(() => child.kill());
        const fixture = serve((_request, path) =>
            path === "/health" ? json({ pid: child.pid }) : json({ ok: true }),
        );
        cleanups.push(fixture.stop);
        writePortFile(storageDir, { port: fixture.port, pid: child.pid, started_at: Date.now() });

        const rpc = client();
        await expect(rpc.call("ping")).resolves.toEqual({ ok: true });
        expect(fixture.hits.get("/rpc/ping")).toBe(1);

        child.kill();
        await child.exited;

        await expect(rpc.call("ping")).rejects.toThrow("not available");
        expect(fixture.hits.get("/rpc/ping")).toBe(1);
    });
});

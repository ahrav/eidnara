import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EidnaraRpcClient } from "./rpc-client";
import {
    __resetRpcIdentityTestHooks,
    __setRpcIdentityTestHooks,
    legacyRpcPortFilePath,
    rpcPortDir,
    rpcPortFilePath,
} from "./rpc-utils";

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
    record: {
        port: number;
        pid: number;
        started_at: number;
        token?: string;
        instance_id?: string;
    },
): void {
    mkdirSync(rpcPortDir(storageDir, DIRECTORY), { recursive: true });
    writeFileSync(
        rpcPortFilePath(storageDir, DIRECTORY, record.pid, record.instance_id),
        JSON.stringify(record),
    );
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
    __resetRpcIdentityTestHooks();
    for (const cleanup of cleanups.splice(0)) cleanup();
    if (storageDir) rmSync(storageDir, { recursive: true, force: true });
    storageDir = "";
});

function freshStorageDir(): void {
    storageDir = mkdtempSync(`${tmpdir()}/eidnara-rpc-client-`);
}

/** Makes every `kill(pid, 0)` probe report EPERM, the result a process owned by another user produces. */
function makeLivenessInconclusive(): void {
    __setRpcIdentityTestHooks({
        processKill: (() => {
            throw Object.assign(new Error("EPERM"), { code: "EPERM" });
        }) as typeof process.kill,
    });
}

/** Makes `/proc/<pid>/stat` report a start time after the current wall clock. */
function makeStartTimeImplausible(): void {
    __setRpcIdentityTestHooks({
        platform: "linux",
        readFileSync: ((path: Parameters<typeof readFileSync>[0], ...rest: unknown[]) => {
            if (String(path) === "/proc/uptime") return "100.00 100.00";
            if (/^\/proc\/\d+\/stat$/.test(String(path))) {
                // Field 22 (start time in clock ticks) is index 19 after the `(comm)` field: 1e9 ticks
                // puts the start ~115 days after boot, while uptime says boot was 100 s ago.
                const fields = Array.from({ length: 30 }, () => "0");
                fields[19] = "1000000000";
                return `1 (bun) ${fields.join(" ")}`;
            }
            return readFileSync(path, ...(rest as [BufferEncoding]));
        }) as typeof readFileSync,
    });
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
        // The handler may have run before the body stalled, so the request is not replayed.
        expect(fixture.hits.get("/rpc/ping")).toBe(1);
    });

    test("call() does not replay a request the server answered with a 5xx", async () => {
        freshStorageDir();
        const fixture = serve((_request, path) =>
            path === "/health" ? json({ pid: process.pid }) : new Response("boom", { status: 500 }),
        );
        cleanups.push(fixture.stop);
        writePortFile(storageDir, { port: fixture.port, pid: process.pid, started_at: Date.now() });

        await expect(client().call("recomp")).rejects.toThrow("RPC recomp failed (500): boom");
        expect(fixture.hits.get("/rpc/recomp")).toBe(1);
    });

    test("call() re-reads the port file and retries after a 401", async () => {
        freshStorageDir();
        const seen: Array<string | null> = [];
        const fixture = serve((request, path) => {
            if (path === "/health") return json({ pid: process.pid });
            const authorization = request.headers.get("authorization");
            seen.push(authorization);
            if (authorization !== "Bearer rotated")
                return new Response("bad token", { status: 401 });
            return json({ ok: true });
        });
        cleanups.push(fixture.stop);
        writePortFile(storageDir, {
            port: fixture.port,
            pid: process.pid,
            started_at: Date.now(),
            token: "stale",
        });

        const rpc = client();
        await expect(rpc.call("ping")).rejects.toThrow("(401)");
        expect(seen).toEqual(["Bearer stale", "Bearer stale", "Bearer stale"]);

        writePortFile(storageDir, {
            port: fixture.port,
            pid: process.pid,
            started_at: Date.now(),
            token: "rotated",
        });
        await expect(rpc.call("ping")).resolves.toEqual({ ok: true });
        expect(seen.at(-1)).toBe("Bearer rotated");
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

    test("discovery skips a port file that vanishes after enumeration and keeps scanning", async () => {
        freshStorageDir();
        const fixture = serve(() => json({ pid: process.pid }));
        cleanups.push(fixture.stop);
        writePortFile(storageDir, { port: fixture.port, pid: process.pid, started_at: Date.now() });
        // A dangling symlink is listed by `readdirSync` but fails `readFileSync` with ENOENT,
        // the same shape as a file removed between the two calls.
        const portDir = rpcPortDir(storageDir, DIRECTORY);
        for (const name of ["port-1.json", "port-2.json", "port-3.json", "port-4.json"]) {
            symlinkSync(join(portDir, "missing"), join(portDir, name));
        }

        await expect(client().isAvailable()).resolves.toBe(true);
        expect(fixture.hits.get("/health")).toBe(1);
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

    test("a cached server is re-probed on every call without rescanning the port directory", async () => {
        freshStorageDir();
        const fixture = serve((_request, path) =>
            path === "/health" ? json({ pid: process.pid }) : json({ ok: true }),
        );
        cleanups.push(fixture.stop);
        writePortFile(storageDir, { port: fixture.port, pid: process.pid, started_at: Date.now() });

        const rpc = client();
        await expect(rpc.call("ping")).resolves.toEqual({ ok: true });
        expect(fixture.hits.get("/health")).toBe(1);

        rmSync(rpcPortFilePath(storageDir, DIRECTORY, process.pid));
        makeLivenessInconclusive();
        await expect(rpc.call("ping")).resolves.toEqual({ ok: true });
        expect(fixture.hits.get("/health")).toBe(2);
    });

    test("a cached server whose pid start time refutes the record is dropped without a request", async () => {
        freshStorageDir();
        const fixture = serve((_request, path) =>
            path === "/health" ? json({ pid: process.pid }) : json({ ok: true }),
        );
        cleanups.push(fixture.stop);
        writePortFile(storageDir, { port: fixture.port, pid: process.pid, started_at: Date.now() });

        const rpc = client();
        await expect(rpc.call("ping")).resolves.toEqual({ ok: true });
        expect(fixture.hits.get("/rpc/ping")).toBe(1);
        expect(fixture.hits.get("/health")).toBe(1);

        makeStartTimeImplausible();
        await expect(rpc.call("ping")).rejects.toThrow("not available");
        expect(fixture.hits.get("/rpc/ping")).toBe(1);
        expect(fixture.hits.get("/health")).toBe(1);
    });

    test("a server replaced inside the same process is rediscovered through its new port file", async () => {
        freshStorageDir();
        const first = serve((_request, path) =>
            path === "/health" ? json({ pid: process.pid, instance_id: "aaaa" }) : json({ ok: 1 }),
        );
        cleanups.push(first.stop);
        writePortFile(storageDir, {
            port: first.port,
            pid: process.pid,
            started_at: Date.now(),
            instance_id: "aaaa",
        });

        const rpc = client();
        await expect(rpc.call("ping")).resolves.toEqual({ ok: 1 });

        first.stop();
        rmSync(rpcPortFilePath(storageDir, DIRECTORY, process.pid, "aaaa"));
        const second = serve((_request, path) =>
            path === "/health" ? json({ pid: process.pid, instance_id: "bbbb" }) : json({ ok: 2 }),
        );
        cleanups.push(second.stop);
        const secondStartedAt = Date.now();
        writePortFile(storageDir, {
            port: second.port,
            pid: process.pid,
            started_at: secondStartedAt,
            instance_id: "bbbb",
        });

        await expect(rpc.call("ping")).resolves.toEqual({ ok: 2 });
        expect(first.hits.get("/rpc/ping")).toBe(1);
        expect(second.hits.get("/rpc/ping")).toBe(1);
        await expect(rpc.resolveEndpoint()).resolves.toEqual({
            port: second.port,
            token: null,
            instanceId: "bbbb",
            startedAt: secondStartedAt,
        });
    });
});

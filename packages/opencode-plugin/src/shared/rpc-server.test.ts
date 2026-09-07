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
import * as logger from "./logger";
import { __resetNotificationStateForTests } from "./rpc-notifications";
import { EidnaraRpcServer } from "./rpc-server";
import {
    __resetRpcIdentityTestHooks,
    __setRpcIdentityTestHooks,
    parseRpcPortFile,
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

function readToken(storageDir: string, directory: string): string {
    for (const entry of readdirSync(rpcPortDir(storageDir, directory))) {
        if (!entry.startsWith("port-") || !entry.endsWith(".json")) continue;
        const record = parseRpcPortFile(
            readFileSync(join(rpcPortDir(storageDir, directory), entry), "utf-8"),
        );
        if (record?.pid === process.pid && typeof record.token === "string") return record.token;
    }
    throw new Error("no port file for this process");
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

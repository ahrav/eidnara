import { afterEach, describe, expect, it } from "bun:test";
import { type ChildProcess, spawn } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { probeCapabilities } from "@eidnara/shm-native";
import { IDENTITY } from "./__tests__/fake-daemon";
import { HostClient } from "./client";
import { MAX_FRAME_BODY_LEN } from "./protocol";

/**
 * Cross-process witness for the TypeScript client over the native addon: a real host
 * process (`crates/host-runtime/examples/local_embeddings_host.rs`, whose primary component
 * echoes each body) serves the sole payload-pool profile to `HostClient`.
 *
 * `EIDNARA_REAL_HOST_BIN` names the built example. With `EIDNARA_REQUIRE_REAL_HOST=1` a
 * missing binary or an unavailable channel fails the suite instead of skipping it.
 */
const HOST_BIN = process.env.EIDNARA_REAL_HOST_BIN;
const REQUIRED = process.env.EIDNARA_REQUIRE_REAL_HOST === "1";
const CHANNEL = probeCapabilities();
const RUNNABLE = HOST_BIN !== undefined && HOST_BIN !== "" && CHANNEL.available;
const ECHO = { kind: "tool_provider", module_id: "context" } as const;

interface RunningHost {
    child: ChildProcess;
    connectionFile: string;
}

const hosts: RunningHost[] = [];
const clients: HostClient[] = [];
const dataDirs: string[] = [];

async function startHost(dataDir: string): Promise<RunningHost> {
    const child = spawn(HOST_BIN as string, [dataDir, "-"], {
        stdio: ["ignore", "pipe", "inherit"],
    });
    const lines = createInterface({ input: child.stdout as NodeJS.ReadableStream });
    const connectionFile = await new Promise<string>((resolve, reject) => {
        child.once("error", reject);
        child.once("exit", (code) => reject(new Error(`host exited before READY: ${code}`)));
        lines.on("line", (line) => {
            if (line.startsWith("READY ")) resolve(line.slice("READY ".length));
        });
    });
    const host = { child, connectionFile };
    hosts.push(host);
    return host;
}

async function stopHost(host: RunningHost): Promise<void> {
    if (host.child.exitCode !== null || host.child.signalCode !== null) return;
    const exited = new Promise<void>((resolve) => host.child.once("exit", () => resolve()));
    // A host a test stopped with SIGSTOP acts on SIGTERM only once continued.
    host.child.kill("SIGCONT");
    host.child.kill("SIGTERM");
    await exited;
}

/** Waits until every thread of `pid` is in the stopped state, so the host cannot run. */
async function waitUntilStopped(pid: number): Promise<void> {
    const deadline = Date.now() + 5_000;
    for (;;) {
        const tasks = readdirSync(`/proc/${pid}/task`);
        const states = tasks.map(
            (task) => readFileSync(`/proc/${pid}/task/${task}/stat`, "utf8").split(") ")[1]?.[0],
        );
        if (states.every((state) => state === "T" || state === "t")) return;
        if (Date.now() > deadline) throw new Error(`host ${pid} did not stop: ${states.join("")}`);
        await new Promise((resolve) => setTimeout(resolve, 5));
    }
}

async function connect(host: RunningHost): Promise<HostClient> {
    const client = await HostClient.connect({ connectionFile: host.connectionFile });
    clients.push(client);
    return client;
}

/** Byte `index` is `(index * 131 + 7) & 0xff`, which repeats every 256 bytes. Prefix copies
 * preserve the 256-byte pattern, including when the final copy is truncated at `length`. */
function patterned(length: number): Uint8Array {
    const bytes = new Uint8Array(length);
    const period = Math.min(length, 256);
    for (let index = 0; index < period; index++) bytes[index] = (index * 131 + 7) & 0xff;
    for (let filled = period; filled < length; filled *= 2) bytes.copyWithin(filled, 0, filled);
    return bytes;
}

function leaseEquals(
    lease: { segmentCount: number; segment(index: number): Uint8Array },
    expected: Uint8Array,
): boolean {
    let offset = 0;
    for (let index = 0; index < lease.segmentCount; index++) {
        const segment = lease.segment(index);
        const end = offset + segment.byteLength;
        if (end > expected.byteLength) return false;
        const view = Buffer.from(segment.buffer, segment.byteOffset, segment.byteLength);
        if (!view.equals(expected.subarray(offset, end))) return false;
        offset = end;
    }
    return offset === expected.byteLength;
}

function leaseBytes(lease: { segmentCount: number; segment(index: number): Uint8Array }): Buffer {
    const parts: Uint8Array[] = [];
    for (let index = 0; index < lease.segmentCount; index++) parts.push(lease.segment(index));
    return Buffer.concat(parts);
}

function sharedMemory(status: { sharedMemory?: Record<string, unknown> }): Record<string, any> {
    expect(status.sharedMemory).toBeDefined();
    return status.sharedMemory as Record<string, any>;
}

afterEach(async () => {
    for (const client of clients.splice(0)) client.close();
    for (const host of hosts.splice(0)) await stopHost(host);
    for (const dir of dataDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("TypeScript client against a real host process", () => {
    it("names why it skips", () => {
        if (RUNNABLE) return;
        const reason =
            HOST_BIN === undefined || HOST_BIN === ""
                ? "EIDNARA_REAL_HOST_BIN is unset"
                : `shared-memory channel unavailable: ${CHANNEL.reason}`;
        console.warn(`real-host witness skipped: ${reason}`);
        expect(REQUIRED).toBe(false);
    });

    it.skipIf(!RUNNABLE)(
        "activates the sole profile, echoes the maximum body both ways, refuses one byte over before publication, and recovers across close and restart",
        async () => {
            const dataDir = mkdtempSync(join(tmpdir(), "eidnara-real-host-"));
            dataDirs.push(dataDir);
            const host = await startHost(dataDir);
            const client = await connect(host);
            const first = sharedMemory(await client.hostStatus());
            expect(first.artifact).toEqual({ profile: "host-payload-pool" });
            expect(first.activation.completed).toBe(1);
            expect(first.returns.live_backings).toBe(2);

            const route = await client.routeOpen(ECHO, IDENTITY);
            const body = patterned(MAX_FRAME_BODY_LEN);
            const held = await client.requestBinary(route, body, { timeoutMs: 120_000 });
            expect(held.byteLength).toBe(MAX_FRAME_BODY_LEN);
            expect(leaseEquals(held, body)).toBe(true);

            const over = client.requestBinary(route, new Uint8Array(MAX_FRAME_BODY_LEN + 1));
            await expect(over).rejects.toMatchObject({
                kind: "not_sent",
                code: "frame_body_too_large",
            });

            // The held maximum response stays readable after its connection closes and is
            // returned exactly once.
            client.close();
            expect(leaseEquals(held, body)).toBe(true);
            expect(held.release()).toBe(true);
            expect(held.release()).toBe(false);

            // A second client on the same host attaches fresh backing; the first generation
            // retires and its backing is released.
            const second = await connect(host);
            const reroute = await second.routeOpen(ECHO, IDENTITY);
            const echoed = await second.requestBinary(reroute, Uint8Array.of(1, 2, 3));
            expect([...leaseBytes(echoed)]).toEqual([1, 2, 3]);
            echoed.release();
            const deadline = Date.now() + 10_000;
            for (;;) {
                const shm = sharedMemory(await second.hostStatus());
                expect(shm.activation.completed).toBe(2);
                if (shm.reclamation.completed === 1 && shm.returns.released_backings === 2) break;
                expect(Date.now()).toBeLessThan(deadline);
                await new Promise((resolve) => setTimeout(resolve, 20));
            }
            second.close();

            // A restarted host publishes a new connection file; a new client completes a
            // request against it.
            await stopHost(host);
            const restarted = await startHost(dataDir);
            const third = await connect(restarted);
            const restartedRoute = await third.routeOpen(ECHO, IDENTITY);
            const after = await third.requestBinary(restartedRoute, Uint8Array.of(9));
            expect([...leaseBytes(after)]).toEqual([9]);
            after.release();
            expect(sharedMemory(await third.hostStatus()).activation.completed).toBe(1);
        },
        240_000,
    );

    it.skipIf(!RUNNABLE)(
        "a host that stops after a request was published reports outcome_unknown, and a fresh client recovers on a restarted host",
        async () => {
            const dataDir = mkdtempSync(join(tmpdir(), "eidnara-real-host-stop-"));
            dataDirs.push(dataDir);
            const host = await startHost(dataDir);
            const client = await connect(host);
            const route = await client.routeOpen(ECHO, IDENTITY);
            (await client.requestBinary(route, Uint8Array.of(1))).release();

            // A frozen host cannot consume or answer; killing it after publication leaves the
            // request's outcome unknown to the client.
            const pid = host.child.pid as number;
            process.kill(pid, "SIGSTOP");
            await waitUntilStopped(pid);
            const published = client.requestBinary(route, Uint8Array.of(2), { timeoutMs: 30_000 });
            const rejected = published.then(
                () => null,
                (error: unknown) => error as { kind?: string },
            );
            await new Promise((resolve) => setTimeout(resolve, 200));
            const exited = new Promise<void>((resolve) => host.child.once("exit", () => resolve()));
            process.kill(pid, "SIGKILL");
            await exited;
            const failure = await rejected;
            expect(failure?.kind).toBe("outcome_unknown");
            // A request issued after the connection is gone is refused before publication:
            // its route belonged to the lost connection.
            await expect(client.requestBinary(route, Uint8Array.of(4))).rejects.toMatchObject({
                name: "StaleRouteHandleError",
            });

            const restarted = await startHost(dataDir);
            const fresh = await connect(restarted);
            const freshRoute = await fresh.routeOpen(ECHO, IDENTITY);
            const echoed = await fresh.requestBinary(freshRoute, Uint8Array.of(3));
            expect([...leaseBytes(echoed)]).toEqual([3]);
            echoed.release();
        },
        120_000,
    );
});

import { afterAll, describe, expect, test } from "bun:test";
import {
    existsSync,
    mkdtempSync,
    readFileSync,
    rmSync,
    writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import {
    assertUint32Argument,
    DESCRIPTOR_SCHEMA_VERSION,
    NativeChannel,
    nativeWireConstants,
    probeCapabilities,
    QUALIFIED_TEST_PROFILE,
} from "../index.ts";

const scratch = mkdtempSync(join(tmpdir(), "shm-native-"));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

describe("native mechanism gate", () => {
    test("proves every required runtime mechanism or omits capability", () => {
        const result = probeCapabilities();
        expect(result.napiVersion === null || result.napiVersion >= 1).toBe(
            true,
        );
        if (result.available) {
            expect(result.napiVersion).toBeGreaterThanOrEqual(8);
            expect(result.externalArrayBuffer).toBe(true);
            expect(result.exactBounds).toBe(true);
            expect(result.detachment).toBe(true);
            expect(result.transferPrevention).toBe(true);
            expect(result.cleanupHooks).toBe(true);
        } else {
            expect(typeof result.reason).toBe("string");
            expect(result.reason?.length).toBeGreaterThan(0);
        }
    });

    test("exported wire constants match the addon", () => {
        const constants = nativeWireConstants();
        if (constants === null) {
            if (process.env.EIDNARA_SHM_NATIVE_CLAIMED_TARGET === "1") {
                throw new Error("claimed native addon is missing");
            }
            return;
        }
        expect(constants.descriptorSchemaVersion).toBe(DESCRIPTOR_SCHEMA_VERSION);
        expect(constants.qualifiedTestProfile).toBe(QUALIFIED_TEST_PROFILE);
    });

    test("environment cleanup hook runs at runtime exit when addon loads", () => {
        const marker = join(scratch, "cleanup.marker");
        const script = join(scratch, "cleanup.mjs");
        const addon = resolve(
            dirname(fileURLToPath(import.meta.url)),
            "../shm_native.node",
        );
        // The first test tolerates an unbuilt addon via probeCapabilities();
        // this one requires the artifact, so skip rather than fail without it.
        if (!existsSync(addon)) return;
        writeFileSync(
            script,
            `import { createRequire } from "node:module";\n` +
                `const addon = createRequire(import.meta.url)(${JSON.stringify(addon)});\n` +
                `addon.registerCleanupProbe(${JSON.stringify(marker)});\n` +
                `addon.createTestPair();\n`,
        );
        const child = spawnSync(process.execPath, [script], {
            encoding: "utf8",
        });
        expect(child.stderr).toBe("");
        expect(child.status).toBe(0);
        expect(readFileSync(marker, "utf8")).toBe("clean");
    });
});

interface RawAttachAddon {
    attach(descriptor: unknown): number;
    activeChannelCount(): number;
    activeExternalRefCount(): number;
    nativeLeakDiagnostics(): number;
    createTestPair(): { first: number; second: number };
    produce(
        channel: number,
        header: Uint8Array,
        capacity: number,
        timeoutMs: number,
        fill: (segments: Uint8Array[]) => number,
        beforePublish: () => void,
    ): void;
    poll(
        channel: number,
        deliver: (token: number, header: Uint8Array, segments: Uint8Array[]) => void,
    ): boolean;
    watch(channel: number, callback: () => void): void;
    readinessHandled(): boolean;
    release(channel: number, token: number): void;
    close(channel: number): void;
}

function loadRawAddon(): RawAttachAddon | null {
    const path = resolve(
        dirname(fileURLToPath(import.meta.url)),
        "../shm_native.node",
    );
    if (!existsSync(path)) {
        if (process.env.EIDNARA_SHM_NATIVE_CLAIMED_TARGET === "1") {
            throw new Error("claimed native addon is missing");
        }
        return null;
    }
    return createRequire(import.meta.url)(path) as RawAttachAddon;
}

function supportsMechanismTests(addon: RawAttachAddon | null): addon is RawAttachAddon {
    return addon !== null && process.platform === "linux";
}

/** Geometry of the `host-test-ring-v1` profile (`host_test_ring_profile`). */
const GRANT_DESCRIPTOR_DEPTH = 8n;
/** `MIN_ARENA_BYTES` == `MAX_FRAME_BYTES` == 64 MiB. */
const GRANT_ARENA_BYTES = 67_108_864n;
const GRANT_MAX_LEASES = 8n;
/**
 * Bytes the ring layout adds around a page-aligned arena: the control
 * region that precedes it (producer, consumer, and reclaim cache lines
 * plus `descriptor_depth` slots, rounded up to a page) and the trailing
 * lifecycle page.
 *
 * `RingGrant::decode` recomputes the layout and rejects any grant whose
 * `total_bytes` disagrees, so this value is not decoration: it must track
 * `Layout::new(GRANT_DESCRIPTOR_DEPTH, GRANT_ARENA_BYTES).total`. Growing
 * a control-region struct past a page boundary changes it, and a stale
 * value surfaces as `invalid shared-memory descriptor` from whichever
 * test needs the grant to be *valid* — see the unresolvable-descriptor
 * test below, which is the only case that gets past decoding.
 */
const GRANT_LAYOUT_OVERHEAD_BYTES = 8_192n;

/**
 * Encodes one RingGrant wire image (layout version 3) as lowercase hex:
 * layout_version u16, incarnation [16], lane u32, descriptor_depth u64,
 * arena_bytes u64, max_leases u64, total_bytes u64, reserved u32 zero —
 * all little-endian.
 */
function testGrantHex(lane: number, incarnation: number): string {
    const bytes = new Uint8Array(58);
    const view = new DataView(bytes.buffer);
    view.setUint16(0, 3, true);
    bytes[2] = incarnation;
    view.setUint32(18, lane, true);
    view.setBigUint64(22, GRANT_DESCRIPTOR_DEPTH, true);
    view.setBigUint64(30, GRANT_ARENA_BYTES, true);
    view.setBigUint64(38, GRANT_MAX_LEASES, true);
    view.setBigUint64(
        46,
        GRANT_ARENA_BYTES + GRANT_LAYOUT_OVERHEAD_BYTES,
        true,
    );
    view.setUint32(54, 0, true);
    return [...bytes]
        .map((byte) => byte.toString(16).padStart(2, "0"))
        .join("");
}

function validRawDescriptor(): Record<string, unknown> {
    return {
        profile: QUALIFIED_TEST_PROFILE,
        hostToPeerFd: 10,
        hostToPeerDataReadyFd: 11,
        hostToPeerCapacityReadyFd: 12,
        hostToPeerGrant: testGrantHex(0, 0xab),
        peerToHostFd: 13,
        peerToHostDataReadyFd: 14,
        peerToHostCapacityReadyFd: 15,
        peerToHostGrant: testGrantHex(1, 0xcd),
    };
}

describe("readiness dispatch", () => {
    test("a dead peer ends dispatch for its channel and reports peerClosed", async () => {
        if (!probeCapabilities().available) return;
        const pair = NativeChannel.createTestPair();
        let dispatches = 0;
        let receiveErrors = 0;
        pair.second.startReadiness(() => {
            dispatches += 1;
            try {
                while (pair.second.drainOne((lease) => lease.release())) {}
            } catch {
                receiveErrors += 1;
            }
        });
        try {
            expect(pair.second.peerClosed()).toBe(false);
            // Closing `pair.first` closes `pair.second`'s doorbell peer end.
            pair.first.close();
            const deadline = Date.now() + 1_000;
            while (!pair.second.peerClosed() && Date.now() < deadline) {
                await new Promise((resolve) => setTimeout(resolve, 1));
            }
            expect(pair.second.peerClosed()).toBe(true);
            const settled = dispatches;
            // No further dispatch arrives after peer closure while timers still run.
            await new Promise((resolve) => setTimeout(resolve, 100));
            expect(dispatches).toBe(settled);
            expect(receiveErrors).toBeGreaterThan(0);
            expect(() => pair.second.drainOne(() => {})).toThrow(/receive failed/);
        } finally {
            pair.second.close();
        }
        expect(pair.second.peerClosed()).toBe(true);
    });

    test("one channel handler failure does not starve later channels", async () => {
        if (!probeCapabilities().available) return;
        const first = NativeChannel.createTestPair();
        const second = NativeChannel.createTestPair();
        let firstCalls = 0;
        let delivered = false;
        first.first.startReadiness(() => {
            firstCalls += 1;
            throw new Error("first handler failed");
        });
        second.first.startReadiness(() => {
            while (second.first.drainOne((lease) => {
                delivered = true;
                lease.release();
            })) {}
        });
        try {
            const header = new Uint8Array(21);
            const view = new DataView(header.buffer);
            view.setUint32(0, 0, true);
            view.setUint8(4, 2);
            view.setUint8(5, 3);
            view.setUint16(7, 1, true);
            view.setUint32(9, 1, true);
            second.second.produce(header, 0, () => {});
            const deadline = Date.now() + 1_000;
            while (!delivered && Date.now() < deadline) {
                await new Promise((resolve) => setTimeout(resolve, 1));
            }
            expect(firstCalls).toBeGreaterThan(0);
            expect(delivered).toBe(true);
        } finally {
            first.first.close();
            first.second.close();
            second.first.close();
            second.second.close();
        }
    });

    test("out-of-range u32 arguments are rejected before reaching the addon", () => {
        for (const value of [-1, 0.5, 2 ** 32, Number.NaN, Number.POSITIVE_INFINITY]) {
            expect(() => assertUint32Argument("timeoutMs", value)).toThrow(RangeError);
        }
        for (const value of [0, 1, 0xffff_ffff]) {
            expect(assertUint32Argument("timeoutMs", value)).toBe(value);
        }
    });
});

describe("raw N-API descriptor boundary", () => {
    const DESCRIPTOR_ERROR = /invalid shared-memory descriptor/;

    test("a dead peer leaves the reactor instead of redispatching forever", () => {
        const addon = loadRawAddon();
        if (!supportsMechanismTests(addon)) return;
        const addonPath = resolve(
            dirname(fileURLToPath(import.meta.url)),
            "../shm_native.node",
        );
        // Runs in a child so this process's shared reactor (which keeps its first callback)
        // is not the one under test, and so a livelock surfaces as a timeout, not a hang.
        const script = join(scratch, "dead-peer.mjs");
        writeFileSync(
            script,
            `import { createRequire } from "node:module";\n` +
                `const addon = createRequire(import.meta.url)(${JSON.stringify(addonPath)});\n` +
                `const pair = addon.createTestPair();\n` +
                `let dispatches = 0;\n` +
                `let receiveErrors = 0;\n` +
                `const onReady = () => {\n` +
                `  dispatches += 1;\n` +
                `  try { while (addon.poll(pair.second, (token) => addon.release(pair.second, token))) {} }\n` +
                `  catch { receiveErrors += 1; }\n` +
                `  if (addon.readinessHandled()) queueMicrotask(onReady);\n` +
                `};\n` +
                `addon.watch(pair.second, onReady);\n` +
                `const before = addon.peerClosed(pair.second);\n` +
                `addon.close(pair.first);\n` +
                `const deadline = Date.now() + 2000;\n` +
                `while (!addon.peerClosed(pair.second) && Date.now() < deadline) {\n` +
                `  await new Promise((resolve) => setTimeout(resolve, 1));\n` +
                `}\n` +
                `const settled = dispatches;\n` +
                `await new Promise((resolve) => setTimeout(resolve, 100));\n` +
                `console.log(JSON.stringify({ before, after: addon.peerClosed(pair.second), settled, dispatches, receiveErrors }));\n` +
                `addon.close(pair.second);\n`,
        );
        const child = spawnSync(process.execPath, [script], {
            encoding: "utf8",
            timeout: 10_000,
        });
        expect(child.signal).toBeNull();
        expect(child.stderr).toBe("");
        expect(child.status).toBe(0);
        const report = JSON.parse(child.stdout.trim()) as {
            before: boolean;
            after: boolean;
            settled: number;
            dispatches: number;
            receiveErrors: number;
        };
        expect(report.before).toBe(false);
        expect(report.after).toBe(true);
        expect(report.dispatches).toBe(report.settled);
        expect(report.receiveErrors).toBeGreaterThan(0);
    });

    test("a handler that throws over an undrained frame does not liveloop the event loop", () => {
        const addon = loadRawAddon();
        if (!supportsMechanismTests(addon)) return;
        const addonPath = resolve(
            dirname(fileURLToPath(import.meta.url)),
            "../shm_native.node",
        );
        // Runs in a child so event-loop starvation surfaces as the child timing out.
        const script = join(scratch, "throwing-handler.mjs");
        writeFileSync(
            script,
            `import { createRequire } from "node:module";\n` +
                `const addon = createRequire(import.meta.url)(${JSON.stringify(addonPath)});\n` +
                `const pair = addon.createTestPair();\n` +
                `let dispatches = 0;\n` +
                `const onReady = () => {\n` +
                `  try { dispatches += 1; throw new Error("handler failed before draining"); }\n` +
                `  catch {}\n` +
                `  finally { if (addon.readinessHandled()) queueMicrotask(onReady); }\n` +
                `};\n` +
                `addon.watch(pair.second, onReady);\n` +
                `const header = new Uint8Array(21);\n` +
                `const view = new DataView(header.buffer);\n` +
                `view.setUint32(0, 1, true); view.setUint8(4, 2); view.setUint8(5, 3);\n` +
                `view.setUint16(7, 1, true); view.setUint32(9, 1, true); view.setBigUint64(13, 7n, true);\n` +
                `addon.produce(pair.first, header, 1, 0, (segments) => { segments[0][0] = 7; return 1; }, () => {});\n` +
                `const deadline = Date.now() + 2000;\n` +
                `while (dispatches === 0 && Date.now() < deadline) {\n` +
                `  await new Promise((resolve) => setTimeout(resolve, 1));\n` +
                `}\n` +
                `const settled = dispatches;\n` +
                `const timerFired = await new Promise((resolve) => setTimeout(() => resolve(true), 100));\n` +
                `let drained = false;\n` +
                `addon.poll(pair.second, (token) => { drained = true; addon.release(pair.second, token); });\n` +
                `console.log(JSON.stringify({ settled, dispatches, timerFired, drained }));\n` +
                `addon.close(pair.first);\n` +
                `addon.close(pair.second);\n`,
        );
        const child = spawnSync(process.execPath, [script], {
            encoding: "utf8",
            timeout: 10_000,
        });
        expect(child.signal).toBeNull();
        expect(child.stderr).toBe("");
        expect(child.status).toBe(0);
        const report = JSON.parse(child.stdout.trim()) as {
            settled: number;
            dispatches: number;
            timerFired: boolean;
            drained: boolean;
        };
        expect(report.settled).toBeGreaterThan(0);
        expect(report.settled).toBeLessThanOrEqual(2);
        expect(report.dispatches).toBe(report.settled);
        expect(report.timerFired).toBe(true);
        expect(report.drained).toBe(true);
    });

    test("a dead channel's handler is pruned once the reactor drops it", () => {
        const addon = loadRawAddon();
        if (!supportsMechanismTests(addon)) return;
        const addonPath = resolve(
            dirname(fileURLToPath(import.meta.url)),
            "../shm_native.node",
        );
        const script = join(scratch, "pruned-handler.mjs");
        writeFileSync(
            script,
            `import { createRequire } from "node:module";\n` +
                `const addon = createRequire(import.meta.url)(${JSON.stringify(addonPath)});\n` +
                `const dead = addon.createTestPair();\n` +
                `const live = addon.createTestPair();\n` +
                `const handlers = new Map();\n` +
                `let deadCalls = 0; let liveDrained = 0;\n` +
                `const dispatch = () => {\n` +
                `  try {\n` +
                `    for (const [id, handler] of [...handlers]) {\n` +
                `      if (addon.isWatching(id) === false) { handlers.delete(id); continue; }\n` +
                `      try { handler(); } catch {}\n` +
                `    }\n` +
                `  } finally { if (addon.readinessHandled()) queueMicrotask(dispatch); }\n` +
                `};\n` +
                `handlers.set(dead.second, () => { deadCalls += 1; while (addon.poll(dead.second, (t) => addon.release(dead.second, t))) {} });\n` +
                `handlers.set(live.second, () => { while (addon.poll(live.second, (t) => { liveDrained += 1; addon.release(live.second, t); })) {} });\n` +
                `addon.watch(dead.second, dispatch);\n` +
                `addon.watch(live.second, dispatch);\n` +
                `const header = new Uint8Array(21);\n` +
                `const view = new DataView(header.buffer);\n` +
                `view.setUint32(0, 1, true); view.setUint8(4, 2); view.setUint8(5, 3);\n` +
                `view.setUint16(7, 1, true); view.setUint32(9, 1, true);\n` +
                `const publish = (channel, value) => { view.setBigUint64(13, BigInt(value), true); addon.produce(channel, header, 1, 0, (s) => { s[0][0] = value; return 1; }, () => {}); };\n` +
                `const until = async (ready) => { const deadline = Date.now() + 2000; while (!ready() && Date.now() < deadline) await new Promise((r) => setTimeout(r, 1)); };\n` +
                `addon.close(dead.first);\n` +
                `await until(() => !addon.isWatching(dead.second));\n` +
                `const deadCallsAfterDeath = deadCalls;\n` +
                `for (let value = 1; value <= 3; value += 1) { publish(live.first, value); await until(() => liveDrained >= value); }\n` +
                `console.log(JSON.stringify({ unwatched: !addon.isWatching(dead.second), deadCallsAfterDeath, deadCalls, liveDrained, handlers: handlers.size, deadPeerClosed: addon.peerClosed(dead.second) }));\n` +
                `addon.close(dead.second); addon.close(live.first); addon.close(live.second);\n`,
        );
        const child = spawnSync(process.execPath, [script], {
            encoding: "utf8",
            timeout: 10_000,
        });
        expect(child.signal).toBeNull();
        expect(child.stderr).toBe("");
        expect(child.status).toBe(0);
        const report = JSON.parse(child.stdout.trim()) as {
            unwatched: boolean;
            deadCallsAfterDeath: number;
            deadCalls: number;
            liveDrained: number;
            handlers: number;
            deadPeerClosed: boolean;
        };
        expect(report.unwatched).toBe(true);
        expect(report.deadPeerClosed).toBe(true);
        expect(report.liveDrained).toBe(3);
        // Three live wakes ran after the dead channel left the reactor; none reached its handler.
        expect(report.deadCalls).toBe(report.deadCallsAfterDeath);
        expect(report.handlers).toBe(1);
    });

    test("readiness acknowledgement preserves a frame published during callback", async () => {
        const addon = loadRawAddon();
        if (!supportsMechanismTests(addon)) return;
        const pair = addon.createTestPair();
        const received: number[] = [];
        let callbacks = 0;
        let complete!: () => void;
        const completed = new Promise<void>((resolve) => (complete = resolve));
        const publishTo = (channel: number, value: number): void => {
            const header = new Uint8Array(21);
            const view = new DataView(header.buffer);
            view.setUint32(0, 1, true);
            view.setUint8(4, 2);
            view.setUint8(5, 3);
            view.setUint16(7, 1, true);
            view.setUint32(9, 1, true);
            view.setBigUint64(13, BigInt(value), true);
            addon.produce(
                channel,
                header,
                1,
                0,
                (segments) => {
                    segments[0]![0] = value;
                    return 1;
                },
                () => {},
            );
        };
        const publish = (value: number): void => publishTo(pair.first, value);
        let closed = false;
        let later: { first: number; second: number } | null = null;
        let trailingWake: (() => void) | undefined;
        let laterDelivered: (() => void) | undefined;
        const onReady = (): void => {
            // A peer's exit wakes its consumer, so the reactor may deliver one
            // more readiness callback after both ends have been closed. Acknowledge
            // it, or the process-wide reactor stays parked on this callback.
            if (closed) {
                // The closed pair's own trailing wake and any later channel's wake
                // both land here. Drain the later channel so its frame proves the
                // reactor was released by the acknowledgement below.
                trailingWake?.();
                if (later !== null) {
                    addon.poll(later.second, (token) => {
                        addon.release(later.second, token);
                        laterDelivered?.();
                    });
                }
                addon.readinessHandled();
                return;
            }
            try {
                callbacks += 1;
                addon.poll(pair.second, (token, _header, segments) => {
                    received.push(segments[0]![0] ?? 0);
                    addon.release(pair.second, token);
                });
                if (callbacks === 1) {
                    publish(2);
                } else {
                    expect(addon.poll(pair.second, () => {})).toBe(false);
                    complete();
                }
            } finally {
                if (addon.readinessHandled()) queueMicrotask(onReady);
            }
        };
        addon.watch(pair.second, onReady);

        let timeout: ReturnType<typeof setTimeout>;
        try {
            publish(1);
            await Promise.race([
                completed,
                new Promise<never>((_, reject) => {
                    timeout = setTimeout(
                        () => reject(new Error("readiness callback timed out")),
                        5_000,
                    );
                }),
            ]);
        } finally {
            clearTimeout(timeout!);
            closed = true;
            addon.close(pair.first);
            addon.close(pair.second);
        }
        expect(received).toEqual([1, 2]);
        expect(callbacks).toBe(2);

        // Wait for the closed pair's trailing wake before opening another channel,
        // so the later channel's frame can only be drained by a second wake.
        await new Promise<void>((resolve) => {
            trailingWake = resolve;
            setTimeout(resolve, 200);
        });

        // The reactor is shared by every channel in the process and keeps the first
        // registered callback, so a later channel's publish must still reach it.
        later = addon.createTestPair();
        const laterPair = later;
        const delivered = new Promise<void>((resolve) => (laterDelivered = resolve));
        addon.watch(laterPair.second, () => {});
        let laterTimeout: ReturnType<typeof setTimeout>;
        try {
            publishTo(laterPair.first, 7);
            await Promise.race([
                delivered,
                new Promise<never>((_, reject) => {
                    laterTimeout = setTimeout(
                        () => reject(new Error("reactor stayed parked after the closed pair")),
                        5_000,
                    );
                }),
            ]);
        } finally {
            clearTimeout(laterTimeout!);
            // The later pair's own trailing wake must find nothing to poll.
            later = null;
            addon.close(laterPair.first);
            addon.close(laterPair.second);
        }
    });

    test("releasing a lease returns its slot; an unreleased ring fills", () => {
        const addon = loadRawAddon();
        if (!supportsMechanismTests(addon)) return;
        const depth = 8; // HOST_TEST_RING_DEPTH
        const header = new Uint8Array(21);
        const view = new DataView(header.buffer);
        view.setUint32(0, 1, true);
        view.setUint8(4, 2);
        view.setUint8(5, 3);
        view.setUint16(7, 1, true);
        view.setUint32(9, 1, true);
        const publish = (pair: { first: number }, value: number): void => {
            view.setBigUint64(13, BigInt(value), true);
            addon.produce(pair.first, header, 1, 0, (segments) => {
                segments[0]![0] = value;
                return 1;
            }, () => {});
        };

        const released = addon.createTestPair();
        const held = addon.createTestPair();
        try {
            // Releasing after every receive keeps the ring from filling at any frame count.
            for (let value = 1; value <= depth * 3; value += 1) {
                publish(released, value);
                let token = -1;
                expect(addon.poll(released.second, (t) => { token = t; })).toBe(true);
                addon.release(released.second, token);
            }
            expect(addon.poll(released.second, () => {})).toBe(false);

            // Holding every lease exhausts the ring on the depth-th publish.
            const tokens: number[] = [];
            for (let value = 1; value <= depth; value += 1) {
                publish(held, value);
                expect(addon.poll(held.second, (t) => { tokens.push(t); })).toBe(true);
            }
            expect(() => publish(held, depth + 1)).toThrow(/ring is full/);
            addon.release(held.second, tokens.shift()!);
            publish(held, depth + 1);
            expect(addon.poll(held.second, (t) => { tokens.push(t); })).toBe(true);
            expect(() => addon.release(held.second, 999)).toThrow(/already released/);
            for (const token of tokens) addon.release(held.second, token);
        } finally {
            addon.close(released.first);
            addon.close(released.second);
            addon.close(held.first);
            addon.close(held.second);
        }
    });

    function expectRejectedWithoutEffects(
        addon: RawAttachAddon,
        descriptor: unknown,
        pattern: RegExp = DESCRIPTOR_ERROR,
    ): void {
        const channels = addon.activeChannelCount();
        const refs = addon.activeExternalRefCount();
        const leaks = addon.nativeLeakDiagnostics();
        expect(() => addon.attach(descriptor)).toThrow(pattern);
        expect(addon.activeChannelCount()).toBe(channels);
        expect(addon.activeExternalRefCount()).toBe(refs);
        expect(addon.nativeLeakDiagnostics()).toBe(leaks);
    }

    test("rejects non-object and structurally hostile arguments", () => {
        const addon = loadRawAddon();
        if (!addon || !["linux", "darwin"].includes(process.platform)) return;
        for (const hostile of [
            null,
            undefined,
            42,
            "descriptor",
            true,
            [],
            () => {},
        ]) {
            expectRejectedWithoutEffects(addon, hostile);
        }
        // A missing field and an explicit undefined are both absent.
        const { hostToPeerFd: _fd, ...missingFd } = validRawDescriptor();
        expectRejectedWithoutEffects(addon, missingFd);
        expectRejectedWithoutEffects(addon, {
            ...validRawDescriptor(),
            hostToPeerFd: undefined,
        });
    });

    test("rejects every unsafe numeric representation before narrowing", () => {
        const addon = loadRawAddon();
        if (!addon || !["linux", "darwin"].includes(process.platform)) return;
        const hostileFds = [-1, -0, 2 ** 31, 3.5, Number.NaN, "10"];
        const fields = [
            "hostToPeerFd",
            "hostToPeerDataReadyFd",
            "hostToPeerCapacityReadyFd",
            "peerToHostFd",
            "peerToHostDataReadyFd",
            "peerToHostCapacityReadyFd",
        ];
        for (const fd of hostileFds) {
            for (const field of fields) {
                expectRejectedWithoutEffects(addon, {
                    ...validRawDescriptor(),
                    [field]: fd,
                });
            }
        }
    });

    test("rejects malformed, non-ASCII, and aliased grant text", () => {
        const addon = loadRawAddon();
        if (!addon || !["linux", "darwin"].includes(process.platform)) return;
        const valid = validRawDescriptor();
        const hostileGrants = [
            "\u00e9".repeat(58), // UTF-8 length 116, non-ASCII
            testGrantHex(0, 0xab).toUpperCase(),
            testGrantHex(0, 0xab).slice(0, 115), // truncation
            `${testGrantHex(0, 0xab)}0`, // trailing digit
            `${testGrantHex(0, 0xab).slice(0, 114)}g0`, // non-hex tail
            "SENTINEL_GRANT_TEXT".padEnd(116, "0"),
            "",
            42,
        ];
        for (const grant of hostileGrants) {
            expectRejectedWithoutEffects(addon, {
                ...valid,
                hostToPeerGrant: grant,
            });
        }
        expectRejectedWithoutEffects(addon, {
            ...validRawDescriptor(),
            peerToHostCapacityReadyFd: 10,
        });
        expectRejectedWithoutEffects(addon, {
            ...validRawDescriptor(),
            peerToHostGrant: testGrantHex(0, 0xab),
        });
    });

    test("accessor objects and proxies get one bounded redacted error", () => {
        const addon = loadRawAddon();
        if (!addon || !["linux", "darwin"].includes(process.platform)) return;
        let reads = 0;
        const accessor = {
            ...validRawDescriptor(),
            get hostToPeerFd(): number {
                reads += 1;
                throw new Error("SENTINEL_ACCESSOR_THROW");
            },
        };
        try {
            addon.attach(accessor);
            throw new Error("attach unexpectedly succeeded");
        } catch (error) {
            const message =
                error instanceof Error ? error.message : String(error);
            expect(message).toBe("invalid shared-memory descriptor");
            expect(message).not.toContain("SENTINEL");
            expect((error as { cause?: unknown }).cause).toBeUndefined();
        }
        expect(reads).toBe(1);
        expect(addon.activeChannelCount()).toBe(0);

        const flipping = new Proxy(validRawDescriptor(), {
            get(target, property, receiver) {
                if (property === "hostToPeerFd") return Number.NaN;
                return Reflect.get(target, property, receiver);
            },
        });
        expectRejectedWithoutEffects(addon, flipping);
    });

    test("a wrong profile is refused before any attachment effect", () => {
        const addon = loadRawAddon();
        if (!addon || !["linux", "darwin"].includes(process.platform)) return;
        expectRejectedWithoutEffects(
            addon,
            { ...validRawDescriptor(), profile: "SENTINEL_PROFILE" },
            /shared-memory profile is unavailable/,
        );
    });

    test("a well-formed but unresolvable descriptor fails without registry effects", () => {
        const addon = loadRawAddon();
        if (!addon || !["linux", "darwin"].includes(process.platform)) return;
        expectRejectedWithoutEffects(
            addon,
            validRawDescriptor(),
            /shared-memory attachment failed/,
        );
    });
});

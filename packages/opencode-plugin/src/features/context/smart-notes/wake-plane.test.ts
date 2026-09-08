import { afterEach, describe, expect, test } from "bun:test";
import { __wakePlaneTest, wakePlaneStatus } from "./wake-plane";

const WAKE_CATALOG = [{ control_ops: ["wake.create"] }];
const BARE_CATALOG = [{ control_ops: ["other.op"] }];

function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((done) => {
        resolve = done;
    });
    return { promise, resolve };
}

afterEach(() => {
    __wakePlaneTest.reset();
});

describe("wakePlaneStatus", () => {
    test("coalesces concurrent callers under one publication onto a single probe", async () => {
        let probes = 0;
        const gate = deferred<typeof WAKE_CATALOG>();
        __wakePlaneTest.setPublicationReader(() => "pub-1");
        __wakePlaneTest.setCatalogProbe(() => {
            probes += 1;
            return gate.promise;
        });

        const first = wakePlaneStatus();
        const second = wakePlaneStatus();
        gate.resolve(WAKE_CATALOG);

        expect(await Promise.all([first, second])).toEqual(["present", "present"]);
        expect(probes).toBe(1);
        expect(await wakePlaneStatus()).toBe("present");
        expect(probes).toBe(1);
    });

    test("a caller under a replaced publication starts its own probe instead of sharing a stale one", async () => {
        let publication = "pub-1";
        const gates: Array<ReturnType<typeof deferred<typeof WAKE_CATALOG>>> = [];
        __wakePlaneTest.setPublicationReader(() => publication);
        __wakePlaneTest.setCatalogProbe(() => {
            const gate = deferred<typeof WAKE_CATALOG>();
            gates.push(gate);
            return gate.promise;
        });

        const stale = wakePlaneStatus();
        expect(gates).toHaveLength(1);

        // The daemon is replaced while the first probe is in flight.
        publication = "pub-2";
        const fresh = wakePlaneStatus();
        expect(gates).toHaveLength(2);

        // The stale probe settles after the fresh one; it must not clobber the fresh answer.
        gates[1]?.resolve(WAKE_CATALOG);
        expect(await fresh).toBe("present");
        gates[0]?.resolve(BARE_CATALOG);
        expect(await stale).toBe("unknown");

        expect(await wakePlaneStatus()).toBe("present");
        expect(gates).toHaveLength(2);
    });

    test("a probe whose publication changes before it settles answers unknown and is not cached", async () => {
        let publication = "pub-1";
        const gate = deferred<typeof WAKE_CATALOG>();
        __wakePlaneTest.setPublicationReader(() => publication);
        __wakePlaneTest.setCatalogProbe(() => gate.promise);

        const pending = wakePlaneStatus();
        publication = "pub-2";
        gate.resolve(WAKE_CATALOG);
        expect(await pending).toBe("unknown");

        let probes = 0;
        __wakePlaneTest.setCatalogProbe(async () => {
            probes += 1;
            return BARE_CATALOG;
        });
        expect(await wakePlaneStatus()).toBe("absent");
        expect(probes).toBe(1);
    });

    test("probes and caches per connection file", async () => {
        const probed: string[] = [];
        __wakePlaneTest.setPublicationReader((file) => `pub:${file}`);
        __wakePlaneTest.setCatalogProbe(async (file) => {
            probed.push(file);
            return file === "/configured/daemon" ? WAKE_CATALOG : BARE_CATALOG;
        });

        expect(await wakePlaneStatus({ connectionFile: "/configured/daemon" })).toBe("present");
        expect(await wakePlaneStatus()).toBe("absent");
        expect(probed).toEqual(["/configured/daemon", __wakePlaneTest.connectionFile()]);

        expect(await wakePlaneStatus({ connectionFile: "/configured/daemon" })).toBe("present");
        expect(probed).toHaveLength(3);
    });
});

import { describe, expect, it } from "bun:test";
import { disposeAll } from "./harness-primitives";

function recorder(log: string[], name: string, failure?: Error) {
    return {
        dispose: async () => {
            log.push(name);
            if (failure) throw failure;
        },
    };
}

describe("disposeAll", () => {
    it("disposes every resource in order, skipping absent ones", async () => {
        const log: string[] = [];
        await disposeAll([recorder(log, "pi"), undefined, recorder(log, "oc")]);
        expect(log).toEqual(["pi", "oc"]);
    });

    it("disposes later resources after an earlier disposal throws, then rethrows that failure", async () => {
        const log: string[] = [];
        const piFailure = new Error("Pi RPC process did not exit");
        await expect(
            disposeAll([recorder(log, "pi", piFailure), recorder(log, "oc")]),
        ).rejects.toBe(piFailure);
        expect(log).toEqual(["pi", "oc"]);
    });

    it("rethrows the first failure when several disposals throw", async () => {
        const log: string[] = [];
        const first = new Error("first");
        await expect(
            disposeAll([
                recorder(log, "pi", first),
                recorder(log, "oc", new Error("second")),
                recorder(log, "host"),
            ]),
        ).rejects.toBe(first);
        expect(log).toEqual(["pi", "oc", "host"]);
    });
});

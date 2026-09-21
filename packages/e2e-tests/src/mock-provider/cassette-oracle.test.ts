import { afterEach, describe, expect, test } from "bun:test";
import { CassetteOracle, CassetteRefused } from "./cassette-oracle";

/** Stands in for `eval_runner cassette-oracle`: a Bun script answering each stdin line as scripted. */
function scriptedOracle(script: string): Promise<CassetteOracle> {
    return CassetteOracle.start({ command: { binary: process.execPath, args: ["-e", script] } });
}

const REQUEST = { path: "/messages", headers: {}, body_text: "{}" };
const SETTLE_WINDOW_MS = 1_500;

async function outcome(call: Promise<unknown>): Promise<unknown> {
    return Promise.race([
        call.then(
            (value) => ({ resolved: value }),
            (error: unknown) => error,
        ),
        Bun.sleep(SETTLE_WINDOW_MS).then(() => "unsettled"),
    ]);
}

describe("CassetteOracle client", () => {
    const oracles: CassetteOracle[] = [];
    afterEach(async () => {
        for (const oracle of oracles.splice(0)) await oracle.stop();
    });

    async function started(script: string): Promise<CassetteOracle> {
        const oracle = await scriptedOracle(script);
        oracles.push(oracle);
        return oracle;
    }

    test("a well-formed error reply is a typed refusal and the client stays usable", async () => {
        const oracle = await started(
            'for await (const line of console) console.log(JSON.stringify({ error: { kind: "WrongNamespace", detail: "" } }));',
        );
        const first = await outcome(oracle.lookup("ns", REQUEST));
        expect(first).toBeInstanceOf(CassetteRefused);
        expect((first as CassetteRefused).kind).toBe("WrongNamespace");
        const second = await outcome(oracle.lookup("ns", REQUEST));
        expect(second).toBeInstanceOf(CassetteRefused);
    });

    test("an unreadable reply line fails the in-flight call instead of leaving it pending", async () => {
        const oracle = await started('for await (const line of console) console.log("not json");');
        const first = await outcome(oracle.lookup("ns", REQUEST));
        expect(first).toBeInstanceOf(Error);
        expect(String(first)).toContain("cassette oracle reply unreadable");
        // CassetteOracle rejects subsequent calls with the same latched error.
        const later = await outcome(oracle.open("replay", "ns", "/nowhere.json"));
        expect(later).toBe(first);
    });

    test("a reply that is neither ok nor error fails the in-flight call", async () => {
        const oracle = await started(
            "for await (const line of console) console.log(JSON.stringify({ neither: 1 }));",
        );
        const first = await outcome(
            oracle.record("ns", REQUEST, {
                status: 200,
                content_type: "application/json",
                frames: ["{}"],
                aborted: false,
            }),
        );
        expect(first).toBeInstanceOf(Error);
        expect(String(first)).toContain("neither ok nor error");
    });

    test("a malformed operation section latches the failure like a malformed envelope", async () => {
        const oracle = await started(
            "for await (const line of console) console.log(JSON.stringify({ ok: { record: {} } }));",
        );
        const first = await outcome(
            oracle.record("ns", REQUEST, {
                status: 200,
                content_type: "application/json",
                frames: ["{}"],
                aborted: false,
            }),
        );
        expect(first).toBeInstanceOf(Error);
        expect(String(first)).toContain("record.request_digest is malformed");
        // The process is protocol-incompatible; every later call fails with the same latched error.
        const later = await outcome(oracle.lookup("ns", REQUEST));
        expect(later).toBe(first);
    });

    test("a child that dies mid-reply fails the in-flight call", async () => {
        // A partial line with no newline is flushed by the reader when stdout ends.
        const oracle = await started(
            'process.stdin.once("data", () => process.stdout.write(\'{"ok":{"lookup\', () => process.exit(0)));',
        );
        const first = await outcome(oracle.lookup("ns", REQUEST));
        expect(first).toBeInstanceOf(Error);
        expect(first).not.toBe("unsettled");
    });
});

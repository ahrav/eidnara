import { Database } from "bun:sqlite";
import { describe, expect, it } from "bun:test";
import { COMPRESSION_FIDELITY_CORPUS_SHA256 } from "./compression-fidelity/corpus";
import { MockProvider } from "./mock-provider/server";
import {
    CaptureLedger,
    deleteMessagesAfter,
    parseRustPassLine,
    RustTestHarness,
    requestSessionId,
} from "./rust-harness";

// One line in the exact shape `transform-session-client.ts` logs, so a format drift fails here
// instead of silently zeroing a timing the perf suite bounds.
const PASS_LINE =
    "[eidnara] rust pass: decision=DEFER reason=steady served_from=transform in=12 out=12 " +
    "applied=true row_version=7 emergency_wait=1234.5 rediscovered=false admission=shrinks " +
    "invocation_bytes=9000 invocation_charged=3215 history_budget=750 elapsed=41.7 ms module=23.4 ms " +
    "stages=prefix_guard:6.2 clone:0.4 wire_build:3.9 wire_messages:3 transport:9.8 " +
    "transport_pages:1 transport_bytes:20480 apply:1.2 other:0.8 work=scanned:3 charged:4096 retained:512";

describe("RustTestHarness.create", () => {
    it("refuses a scripted default beside forwarding before starting anything", async () => {
        const forward = {
            upstreamURL: "https://provider.invalid/v1/messages",
            model: "m",
            contextLimit: 100_000,
            corpusSha256: COMPRESSION_FIDELITY_CORPUS_SHA256,
            pricing: { inputPerMTok: 3, outputPerMTok: 15 },
            limits: { maxCalls: 1, maxOutputTokens: 1024, timeoutMs: 1_000, spendCapUsd: 1 },
            credentials: () => ({}),
        };
        await expect(
            RustTestHarness.create({ forward, mockDefault: { text: "scripted" } }),
        ).rejects.toThrow("forwarding serves no scripted default");
    });
});

describe("parseRustPassLine", () => {
    it("reads top-level fields and every stage timing, including the first stage after stages=", () => {
        const pass = parseRustPassLine(PASS_LINE);
        expect(pass).not.toBeNull();
        expect(pass).toMatchObject({
            decision: "DEFER",
            reason: "steady",
            emergencyWaitMs: 1234.5,
            admission: "shrinks",
            invocationBytes: 9000,
            invocationCharged: 3215,
            historyBudget: 750,
            servedFrom: "transform",
            inputCount: 12,
            outputCount: 12,
            applied: true,
            rowVersion: 7,
            elapsedMs: 41.7,
            moduleElapsedMs: 23.4,
            prefixGuardMs: 6.2,
            wireBuildMs: 3.9,
            wireMessages: 3,
            transportMs: 9.8,
            transportPages: 1,
            transportBytes: 20480,
        });
        expect(pass?.adapterElapsedMs).toBeCloseTo(18.3, 5);
    });

    it("ignores lines without the marker", () => {
        expect(parseRustPassLine("[eidnara] something else entirely")).toBeNull();
    });
});

describe("deleteMessagesAfter", () => {
    // The plugin orders session history by `(time_created, id)`, so a revert must remove a
    // later message that shares the anchor's millisecond, and must leave other sessions alone.
    it("removes every message after the anchor in (time_created, id) order within the session", () => {
        const db = new Database(":memory:");
        db.exec(
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT)",
        );
        db.exec(
            "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, data TEXT)",
        );
        const insert = db.prepare(
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?, ?, ?, '{}')",
        );
        insert.run("msg_a", "ses_1", 100);
        insert.run("msg_b", "ses_1", 200);
        insert.run("msg_c", "ses_1", 200);
        insert.run("msg_d", "ses_1", 300);
        insert.run("msg_z", "ses_2", 900);
        const part = db.prepare(
            "INSERT INTO part (id, message_id, session_id, data) VALUES (?, ?, ?, '{}')",
        );
        part.run("prt_b", "msg_b", "ses_1");
        part.run("prt_c", "msg_c", "ses_1");
        part.run("prt_z", "msg_z", "ses_2");

        expect(deleteMessagesAfter(db, "ses_1", "msg_b")).toBe(2);
        const remaining = db
            .prepare("SELECT id FROM message ORDER BY time_created, id")
            .all() as Array<{ id: string }>;
        expect(remaining.map((row) => row.id)).toEqual(["msg_a", "msg_b", "msg_z"]);
        const parts = db.prepare("SELECT id FROM part ORDER BY id").all() as Array<{ id: string }>;
        expect(parts.map((row) => row.id)).toEqual(["prt_b", "prt_z"]);
        db.close();
    });
});

describe("CaptureLedger", () => {
    async function post(baseURL: string, session: string): Promise<void> {
        await fetch(`${baseURL}/v1/messages`, {
            method: "POST",
            headers: { "content-type": "application/json", "x-session-id": session },
            body: JSON.stringify({ model: "m", messages: [{ role: "user", content: session }] }),
        }).then((response) => response.text());
    }

    it("keeps tagged captures across a mock reset and files only the tagged session", async () => {
        const mock = new MockProvider();
        const { baseURL } = await mock.start();
        try {
            mock.setDefault({ text: "ok", usage: { input_tokens: 1, output_tokens: 1 } });
            const ledger = new CaptureLedger(() => mock.requests());
            ledger.tag({ sessionId: "ses_a", caseId: "C1", scenarioId: "C1.S1" });
            await post(baseURL, "ses_a");
            await post(baseURL, "ses_b");
            ledger.retain();
            mock.reset();
            expect(mock.requests()).toEqual([]);
            mock.setDefault({ text: "ok", usage: { input_tokens: 1, output_tokens: 1 } });
            // Arrives before the next tag, so it is filed under the identity it was driven under.
            await post(baseURL, "ses_a");
            ledger.tag({ sessionId: "ses_a", caseId: "C1", scenarioId: "C1.S2" });
            await post(baseURL, "ses_a");

            const retained = ledger.captures({ sessionId: "ses_a" });
            expect(retained.map((capture) => capture.scenarioId)).toEqual([
                "C1.S1",
                "C1.S1",
                "C1.S2",
            ]);
            expect(retained[0]?.request.headers["x-session-id"]).toBe("ses_a");
            // ses_b's requests arrived before ses_b was tagged, so none is filed under it.
            ledger.tag({ sessionId: "ses_b", caseId: "C2", scenarioId: "C2.S1" });
            expect(ledger.captures({ sessionId: "ses_b" })).toEqual([]);
            await post(baseURL, "ses_b");
            expect(ledger.captures({ caseId: "C2" })).toHaveLength(1);
        } finally {
            await mock.stop();
        }
    });

    it("files requests by the session headers OpenCode sends a non-OpenCode provider", async () => {
        const mock = new MockProvider();
        const { baseURL } = await mock.start();
        try {
            mock.setDefault({ text: "ok", usage: { input_tokens: 1, output_tokens: 1 } });
            const ledger = new CaptureLedger(() => mock.requests());
            ledger.tag({ sessionId: "ses_a", caseId: "C1", scenarioId: "C1.S1" });
            const sends: Record<string, string>[] = [
                { "x-session-affinity": "ses_a", "X-Session-Id": "ses_a" },
                { "X-Session-Id": "ses_a" },
                { "x-session-affinity": "ses_b", "X-Session-Id": "ses_b" },
            ];
            for (const headers of sends) {
                await fetch(`${baseURL}/v1/messages`, {
                    method: "POST",
                    headers: { "content-type": "application/json", ...headers },
                    body: JSON.stringify({
                        model: "m",
                        messages: [{ role: "user", content: "x" }],
                    }),
                }).then((response) => response.text());
            }
            const retained = ledger.captures({ sessionId: "ses_a" });
            expect(retained).toHaveLength(2);
            expect(retained.map((capture) => requestSessionId(capture.request))).toEqual([
                "ses_a",
                "ses_a",
            ]);
        } finally {
            await mock.stop();
        }
    });
});

import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { COMPRESSION_FIDELITY_CORPUS_SHA256 } from "../src/compression-fidelity/corpus";
import type { ForwardConfig } from "../src/mock-provider/forward";
import { scriptedResponse } from "../src/mock-provider/server";
import { RustTestHarness } from "../src/rust-harness";
import { findToolResultText, publishedToolName } from "../src/scripted-tool-call";

const prereqs = RustTestHarness.detectPrereqs();
const MODEL = "claude-forwarding-double";
const CANARY = "sk-ant-api03-FORWARDING-CANARY";
const CALL_ID = "toolu_forwarded_read";
const USAGE = { input_tokens: 1_200, output_tokens: 30 };

interface Sent {
    body: string;
    apiKey: string | null;
}

/**
 * Stands in for the HTTPS provider: a turn that can call `read` gets a `read` call on the
 * fixture file, the turn carrying its result gets the final answer, and any other turn gets
 * plain text.
 */
function providerDouble(file: string) {
    const sent: Sent[] = [];
    const send = async (_url: string, init: RequestInit): Promise<Response> => {
        const body = new TextDecoder().decode(init.body as Uint8Array);
        sent.push({ body, apiKey: new Headers(init.headers).get("x-api-key") });
        const parsed = JSON.parse(body) as Record<string, unknown>;
        if (body.includes(CALL_ID)) {
            return scriptedResponse({ text: "forwarded final answer", usage: USAGE }, parsed);
        }
        const read = publishedToolName(parsed, "read");
        if (read) {
            return scriptedResponse(
                {
                    content: [
                        { type: "tool_use", id: CALL_ID, name: read, input: { filePath: file } },
                    ],
                    stop_reason: "tool_use",
                    usage: USAGE,
                },
                parsed,
            );
        }
        return scriptedResponse({ text: "forwarded title", usage: USAGE }, parsed);
    };
    return { sent, send };
}

describe.skipIf(!prereqs.ok)("record-and-forward through OpenCode", () => {
    let h: RustTestHarness;
    let double: ReturnType<typeof providerDouble>;
    const file = "forwarding-fixture.txt";

    beforeAll(async () => {
        double = providerDouble(file);
        const forward: ForwardConfig = {
            upstreamURL: "https://provider.test/v1/messages",
            model: MODEL,
            corpusSha256: COMPRESSION_FIDELITY_CORPUS_SHA256,
            pricing: { inputPerMTok: 3, outputPerMTok: 15 },
            limits: { maxCalls: 8, maxOutputTokens: 8_192, timeoutMs: 30_000, spendCapUsd: 1 },
            credentials: () => ({ "x-api-key": CANARY }),
            fetch: double.send,
        };
        h = await RustTestHarness.create({ forward, modelContextLimit: 100_000 });
        writeFileSync(join(h.env.workdir, file), "forwarding fixture contents\n");
    });

    afterAll(async () => {
        await h?.dispose();
    });

    it("forwards OpenCode's bytes, returns the provider's answer, and carries the tool result", async () => {
        const session = await h.createSession();
        await h.sendPrompt(session, `Read ${file} and tell me what it says.`);

        const report = h.mock.forwardingReport();
        expect(report.mode).toBe("forward");
        expect(report.stopped).toBeNull();
        expect(report.refusals).toEqual([]);
        expect(report.exchanges.map((e) => e.request.body_text)).toEqual(
            double.sent.map((s) => s.body),
        );
        expect(double.sent.every((s) => s.apiKey === CANARY)).toBe(true);
        expect(report.exchanges.every((e) => JSON.parse(e.request.body_text).model === MODEL)).toBe(
            true,
        );
        expect(findToolResultText(h, CALL_ID)).toContain("forwarding fixture contents");
        expect(report.exchanges.at(-1)?.response?.stop_reason).toBe("end_turn");
        expect(report.complete).toBe(true);

        const messages = (await h.client.session.messages({ path: { id: session } })).data;
        expect(JSON.stringify(messages)).toContain("forwarded final answer");
        expect(JSON.stringify(report)).not.toContain(CANARY);
        expect(JSON.stringify(h.mock.requests())).not.toContain(CANARY);
        expect(h.opencode.stderr()).not.toContain(CANARY);
    }, 300_000);
});

describe.skipIf(prereqs.ok)("record-and-forward skip visibility", () => {
    it("prints a skip reason when prerequisites are unmet", () => {
        console.log(`[rust-e2e] SKIPPED: ${prereqs.skipReason ?? "unknown reason"}`);
        expect(prereqs.skipReason && prereqs.skipReason.length > 0).toBe(true);
    });
});

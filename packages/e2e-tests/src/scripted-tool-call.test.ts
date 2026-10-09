import { describe, expect, it } from "bun:test";
import { MockProvider } from "./mock-provider/server";
import { CaptureLedger, type RustTestHarness } from "./rust-harness";
import { runScriptedToolCall } from "./scripted-tool-call";

const SESSION = "ses_scripted";

/** The harness surface `runScriptedToolCall` drives, over a real mock and capture ledger. */
function fakeHarness(mock: MockProvider, baseURL: string, ledger: CaptureLedger) {
    const post = async (body: Record<string, unknown>) =>
        (await fetch(`${baseURL}/v1/messages`, {
            method: "POST",
            headers: { "content-type": "application/json", "x-session-id": SESSION },
            body: JSON.stringify({ model: "m", stream: false, ...body }),
        }).then((response) => response.json())) as { content?: { type: string; id?: string }[] };
    return {
        mock,
        resetMock: () => {
            ledger.retain();
            mock.reset();
        },
        sendPrompt: async (_session: string, text: string) => {
            const first = await post({
                messages: [{ role: "user", content: text }],
                tools: [{ name: "eidnara_memory" }],
            });
            const callId = first.content?.find((block) => block.type === "tool_use")?.id;
            await post({
                messages: [
                    {
                        role: "user",
                        content: [{ type: "tool_result", tool_use_id: callId, content: "result" }],
                    },
                ],
            });
            return { data: { info: { id: "msg_scripted" } } };
        },
    };
}

describe("runScriptedToolCall", () => {
    it("retains the tagged session's earlier captures before its mock reset", async () => {
        const mock = new MockProvider();
        const { baseURL } = await mock.start();
        try {
            const ledger = new CaptureLedger(() => mock.requests());
            ledger.tag({ sessionId: SESSION, caseId: "C3", scenarioId: "C3.S6" });
            mock.setDefault({ text: "ok", usage: { input_tokens: 1, output_tokens: 1 } });
            await fetch(`${baseURL}/v1/messages`, {
                method: "POST",
                headers: { "content-type": "application/json", "x-session-id": SESSION },
                body: JSON.stringify({ model: "m", stream: false, messages: [] }),
            }).then((response) => response.text());

            const harness = fakeHarness(mock, baseURL, ledger) as unknown as RustTestHarness;
            const call = await runScriptedToolCall(harness, SESSION, {
                tool: "eidnara_memory",
                input: { query: "q" },
                prompt: "recall",
            });

            expect(call.resultText).toBe("result");
            const retained = ledger.captures({ sessionId: SESSION, scenarioId: "C3.S6" });
            expect(retained).toHaveLength(3);
            expect(retained[0]?.request.body.messages).toEqual([]);
        } finally {
            await mock.stop();
        }
    });
});

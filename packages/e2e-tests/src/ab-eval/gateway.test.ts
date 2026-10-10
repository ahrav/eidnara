import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { type ConverseBody, classify, sendWithRetry } from "./gateway";

const SUMMARIZER_SYSTEM_PROMPT = readFileSync(
    resolve(
        import.meta.dir,
        "../../../../crates/daemon/testdata/history_summarizer-system-prompt.txt",
    ),
    "utf8",
);

const text = (value: string) => [{ text: value }];

function throttled(): Error {
    return Object.assign(new Error("Too many requests"), {
        name: "ThrottlingException",
        $metadata: { httpStatusCode: 429 },
    });
}

describe("gateway caller classification", () => {
    it("labels a tool-free history summarizer request by its Eidnara identity", () => {
        const body: ConverseBody = {
            system: text(SUMMARIZER_SYSTEM_PROMPT),
            messages: [
                { role: "user", content: text("<new_messages>\n[1] U: hello\n</new_messages>") },
            ],
        };
        expect(classify(body)).toBe("history_summarizer");
    });

    it("labels a tool-free harness summary request as native compaction", () => {
        const body: ConverseBody = {
            system: text("You are a helpful AI assistant tasked with summarizing conversations."),
            messages: [{ role: "user", content: text("Summarize the conversation above.") }],
        };
        expect(classify(body)).toBe("native_compaction");
    });

    it("labels a memory capture request by its system prompt and payload", () => {
        const body: ConverseBody = {
            system: text("Extract durable project memory from the new messages."),
            messages: [{ role: "user", content: text('{"messages":[],"existing_memories":[]}') }],
        };
        expect(classify(body)).toBe("memory_capture");
    });

    it("labels a request that offers the bash tool as the main agent", () => {
        const body: ConverseBody = {
            system: text("You are a coding agent."),
            messages: [{ role: "user", content: text("Run the tests.") }],
            toolConfig: { tools: [{ toolSpec: { name: "bash" } }] },
        };
        expect(classify(body)).toBe("main");
    });
});

describe("gateway upstream retries", () => {
    it("retries throttling until the call succeeds and reports each retry", async () => {
        let calls = 0;
        let retries = 0;
        const value = await sendWithRetry(
            async () => {
                calls++;
                if (calls < 3) throw throttled();
                return "ok";
            },
            {
                deadlineAt: Date.now() + 10_000,
                signal: new AbortController().signal,
                onRetry: () => retries++,
                backoffMs: () => 1,
            },
        );
        expect(value).toBe("ok");
        expect(calls).toBe(3);
        expect(retries).toBe(2);
    });

    it("does not retry a validation error", async () => {
        let calls = 0;
        const validation = Object.assign(new Error("bad"), { $metadata: { httpStatusCode: 400 } });
        await expect(
            sendWithRetry(
                async () => {
                    calls++;
                    throw validation;
                },
                {
                    deadlineAt: Date.now() + 10_000,
                    signal: new AbortController().signal,
                    onRetry: () => undefined,
                    backoffMs: () => 1,
                },
            ),
        ).rejects.toBe(validation);
        expect(calls).toBe(1);
    });

    it("stops retrying once the next backoff would end past the deadline", async () => {
        let calls = 0;
        const started = Date.now();
        await expect(
            sendWithRetry(
                async () => {
                    calls++;
                    throw throttled();
                },
                {
                    deadlineAt: started + 100,
                    signal: new AbortController().signal,
                    onRetry: () => undefined,
                    backoffMs: () => 40,
                },
            ),
        ).rejects.toMatchObject({ name: "ThrottlingException" });
        expect(calls).toBeLessThanOrEqual(3);
        expect(Date.now() - started).toBeLessThan(150);
    });

    it("abandons a pending backoff when the caller disconnects", async () => {
        const controller = new AbortController();
        const started = Date.now();
        setTimeout(() => controller.abort(), 20);
        await expect(
            sendWithRetry(
                async () => {
                    throw throttled();
                },
                {
                    deadlineAt: started + 60_000,
                    signal: controller.signal,
                    onRetry: () => undefined,
                    backoffMs: () => 30_000,
                },
            ),
        ).rejects.toThrow();
        expect(Date.now() - started).toBeLessThan(1_000);
    });
});

import { afterEach, describe, expect, it } from "bun:test";

import { measureAnthropic } from "./anthropic";

const originalFetch = globalThis.fetch;

afterEach(() => {
    globalThis.fetch = originalFetch;
});

const TEST = {
    label: "anthropic/claude-opus-4-7",
    provider: "anthropic",
    modelId: "claude-opus-4-7",
};
const TOOLS = [{ name: "tool", description: "Tool", input_schema: { type: "object" } }];

/** Captures the headers of every count_tokens call and answers system, tools, then baseline. */
function captureHeaders(): Array<Record<string, string>> {
    const seen: Array<Record<string, string>> = [];
    const totals = [30, 50, 10];
    globalThis.fetch = (async (_input: string | URL | Request, init?: RequestInit) => {
        seen.push(Object.fromEntries(new Headers(init?.headers).entries()));
        return new Response(JSON.stringify({ input_tokens: totals[seen.length - 1] }), {
            status: 200,
            headers: { "content-type": "application/json" },
        });
    }) as typeof fetch;
    return seen;
}

describe("measureAnthropic", () => {
    it("sends an OAuth access token as a bearer with the OAuth beta flag", async () => {
        const seen = captureHeaders();
        const result = await measureAnthropic(
            TEST,
            { type: "oauth", access: "tok" },
            "system",
            TOOLS,
        );
        expect(seen).toHaveLength(3);
        for (const headers of seen) {
            expect(headers.authorization).toBe("Bearer tok");
            expect(headers["anthropic-beta"]).toBe("oauth-2025-04-20");
            expect(headers["x-api-key"]).toBeUndefined();
        }
        expect(result).toEqual({ systemApi: 20, toolsApi: 40 });
    });

    it("sends an API key in x-api-key without the OAuth beta flag", async () => {
        const seen = captureHeaders();
        const result = await measureAnthropic(
            TEST,
            { type: "api", key: "sk-ant-test" },
            "system",
            TOOLS,
        );
        expect(seen).toHaveLength(3);
        for (const headers of seen) {
            expect(headers["x-api-key"]).toBe("sk-ant-test");
            expect(headers.authorization).toBeUndefined();
            expect(headers["anthropic-beta"]).toBeUndefined();
        }
        expect(result).toEqual({ systemApi: 20, toolsApi: 40 });
    });

    it("rejects an entry that carries neither an access token nor a key before any request", async () => {
        const seen = captureHeaders();
        await expect(measureAnthropic(TEST, { type: "api" }, "system", TOOLS)).rejects.toThrow(
            /OAuth entry with access or an api entry with key/,
        );
        expect(seen).toHaveLength(0);
    });
});

import { describe, expect, it } from "bun:test";
import { KernelClient, renderToolStateText } from "@eidnara/opencode/shared/kernel-client";
import { CTX_SEARCH_DESCRIPTION } from "@eidnara/opencode/tools/ctx-search/constants";
import { fakeContext, fakeKernelResolver } from "../__tests__/test-utils";
import { createCtxSearchTool } from "./ctx-search";

const OBJECT_A = `mem_${"a".repeat(32)}`;
const OBJECT_B = `mem_${"b".repeat(32)}`;

function harness(fake = fakeKernelResolver()) {
    const tool = createCtxSearchTool({
        kernelClient: fake.kernelClient,
        resolveProjectPath: () => "git:test",
    });
    const execute = (args: Record<string, unknown>, callId = "call-search") =>
        tool.execute(
            callId,
            args as never,
            new AbortController().signal,
            undefined,
            fakeContext("ses-search", process.cwd()) as never,
        );
    return { tool, execute, ...fake };
}

type ExecuteResult = Awaited<ReturnType<ReturnType<typeof harness>["execute"]>>;

function textOf(result: ExecuteResult): string {
    return result.content[0]?.text ?? "";
}

describe("createCtxSearchTool", () => {
    it("describes the memory-only tool and exposes only the memory source", () => {
        const { tool } = harness();
        expect(tool.description).toBe(CTX_SEARCH_DESCRIPTION);
        const sources = (
            tool.parameters as {
                properties: { sources: { items: { anyOf: Array<{ const: string }> } } };
            }
        ).properties.sources.items.anyOf.map((literal) => literal.const);
        expect(sources).toEqual(["memory"]);
    });

    it("rejects an over-cap query with the native isError envelope before any work", async () => {
        const tool = harness();
        const byteResult = await tool.execute({ query: "a".repeat(16 * 1024 + 1) }, "call-overcap");
        expect(byteResult.isError).toBe(true);
        expect(textOf(byteResult)).toStartWith("Error: query is too large:");

        const atomResult = await tool.execute(
            { query: Array.from({ length: 65 }, (_, index) => `a${index}`).join(" ") },
            "call-overcap-atoms",
        );
        expect(atomResult.isError).toBe(true);
        expect(textOf(atomResult)).toStartWith("Error: query is too complex:");
        expect(tool.transport.calls).toHaveLength(0);
    });

    it("ranks text queries over memory rows served by the fake kernel", async () => {
        const tool = harness();
        tool.kernel.seedDecision({
            object_id: OBJECT_A,
            decision_kind: "CONSTRAINTS",
            summary: "The cache must stay offline.",
        });
        tool.kernel.seedDecision({
            object_id: OBJECT_B,
            decision_kind: "NAMING",
            summary: "Handlers end in Handler.",
        });
        const result = await tool.execute({ query: "offline cache" });
        expect(result.isError).toBeUndefined();
        const text = textOf(result);
        expect(text).toContain("[1] [memory]");
        expect(text).toContain(`id=${OBJECT_A}`);
        expect(text).toContain("category=CONSTRAINTS");
        expect(text).not.toContain(`id=${OBJECT_B}`);
        expect(tool.transport.methods()).toEqual(["kernel.read"]);
    });

    it("resolves an object-id query directly from the kernel", async () => {
        const tool = harness();
        tool.kernel.seedDecision({
            object_id: OBJECT_A,
            decision_kind: "USER_DIRECTIVES",
            summary: "Direct id hit for the short-circuit.",
        });
        const result = await tool.execute({ query: OBJECT_A }, "call-id");
        const text = textOf(result);
        expect(text).toContain("[1] [memory]");
        expect(text).toContain(OBJECT_A);
        expect(text).toContain("Direct id hit for the short-circuit.");
        const read = tool.transport.calls[0]?.body as { object_ids?: string[] };
        expect(read.object_ids).toEqual([OBJECT_A]);
    });

    it("rejects unknown sources instead of searching nothing", async () => {
        const tool = harness();
        const result = await tool.execute({ query: "appear", sources: ["message"] });
        expect(result.isError).toBe(true);
        expect(textOf(result)).toContain('Error: unknown source: "message"');
        expect(textOf(result)).toContain("Supported sources: memory.");
        expect(tool.transport.calls).toHaveLength(0);
    });

    it("renders the disabled state from a disabled kernel client", async () => {
        const fake = fakeKernelResolver();
        const tool = createCtxSearchTool({
            kernelClient: ({ sessionId, projectRoot }) =>
                new KernelClient({
                    transport: fake.transport,
                    enabled: false,
                    sessionId,
                    projectRoot,
                    tokens: fake.tokens,
                }),
            resolveProjectPath: () => "git:test",
        });
        const result = await tool.execute(
            "call-disabled",
            { query: "anything" } as never,
            new AbortController().signal,
            undefined,
            fakeContext("ses-search", process.cwd()) as never,
        );
        expect(result.isError).toBe(true);
        expect(textOf(result)).toBe(`Error: ${renderToolStateText({ kind: "disabled" })}`);
        expect(fake.transport.calls).toHaveLength(0);
    });

    it("renders the unavailable state when the daemon is absent", async () => {
        const fake = fakeKernelResolver();
        fake.transport.fileExists = false;
        const tool = harness(fake);
        const memoryOnly = await tool.execute(
            { query: "anything", sources: ["memory"] },
            "call-absent",
        );
        expect(memoryOnly.isError).toBe(true);
        expect(textOf(memoryOnly)).toBe(
            `Error: ${renderToolStateText({ kind: "unavailable", reason: "daemon_absent" })}`,
        );
        const implicit = await tool.execute({ query: "anything" }, "call-absent-implicit");
        expect(textOf(implicit)).toBe(textOf(memoryOnly));
        expect(fake.transport.calls).toHaveLength(0);
    });
});

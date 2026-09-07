import { describe, expect, test } from "bun:test";
import { KernelClient } from "../../shared/kernel-client";
import { FakeKernel, FakeKernelTransport } from "../../shared/kernel-client-testing/fake-kernel";
import { CTX_MEMORY_ACTOR, type CtxMemoryWriteIdentity, executeCtxMemory } from "./execute";
import type { CtxMemoryAction, CtxMemoryArgs } from "./types";

const SESSION = "ses-exec";
const PROJECT = "/tmp/exec";
const CONTENT = "Use the daemon";

function harness(enabled = true) {
    const transport = new FakeKernelTransport(new FakeKernel());
    const client = new KernelClient({
        transport,
        enabled,
        sessionId: SESSION,
        projectRoot: PROJECT,
    });
    return { transport, client };
}

function identityFor(toolCallId: string): CtxMemoryWriteIdentity {
    return { sessionId: SESSION, toolCallId };
}

function run(
    client: KernelClient,
    action: CtxMemoryAction,
    args: CtxMemoryArgs,
    toolCallId: string,
): Promise<string> {
    return executeCtxMemory({
        client,
        args: { ...args, action },
        action,
        identity: identityFor(toolCallId),
        actor: CTX_MEMORY_ACTOR,
    });
}

async function createOne(client: KernelClient): Promise<string> {
    const text = await run(
        client,
        "create",
        { category: "ARCHITECTURE", content: CONTENT },
        "call-create-1",
    );
    const reply = JSON.parse(text) as { outcome: string; objectId: string };
    expect(reply.outcome).toBe("applied");
    expect(reply.objectId.startsWith("mem_")).toBe(true);
    return reply.objectId;
}

describe("executeCtxMemory", () => {
    test("create commits a memory and renders the derived object id", async () => {
        const { client } = harness();
        await createOne(client);
    });

    test("get by explicit id returns the created content", async () => {
        const { client } = harness();
        const objectId = await createOne(client);
        const text = await run(client, "get", { objectIds: [objectId] }, "call-get-1");
        const reply = JSON.parse(text) as {
            memories: { objectId: string; content: string }[];
            missingObjectIds: string[];
        };
        expect(reply.memories).toHaveLength(1);
        expect(reply.memories[0]?.objectId).toBe(objectId);
        expect(reply.memories[0]?.content).toBe(CONTENT);
        expect(reply.missingObjectIds).toEqual([]);
    });

    test("a disabled client renders the state as tool error text", async () => {
        const { client } = harness(false);
        const text = await run(
            client,
            "create",
            { category: "ARCHITECTURE", content: CONTENT },
            "call-create-disabled",
        );
        expect(text.startsWith("Error:")).toBe(true);
    });
});

import { describe, expect, test } from "bun:test";
import { KernelClient } from "../../shared/kernel-client";
import { ClaimOperationInputError } from "../../shared/kernel-client/anti-memory";
import { FakeKernel, FakeKernelTransport } from "../../shared/kernel-client-testing/fake-kernel";
import { CTX_MEMORY_RESPONSE_BUDGET_BYTES } from "./constants";
import { CTX_MEMORY_ACTOR, type CtxMemoryWriteIdentity, executeCtxMemory } from "./execute";
import type { CtxMemoryAction, CtxMemoryArgs } from "./types";

const SESSION = "ses-exec";
const PROJECT = "/tmp/exec";
const CONTENT = "Use the daemon";

interface CommitReply {
    outcome: string;
    objectId: string;
    objects: string[];
}

function harness(enabled = true) {
    const kernel = new FakeKernel();
    const transport = new FakeKernelTransport(kernel);
    const client = new KernelClient({
        transport,
        enabled,
        sessionId: SESSION,
        projectRoot: PROJECT,
    });
    return { kernel, transport, client };
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

    test("a redelivered revise lists the retired predecessor alongside the successor", async () => {
        const { kernel, client } = harness();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        const args: CtxMemoryArgs = {
            objectId: "mem_a",
            category: "ARCHITECTURE",
            content: "A, revised.",
            reason: "clarified",
        };
        const first = JSON.parse(
            await run(client, "revise", args, "call-revise-replay"),
        ) as CommitReply;
        const second = JSON.parse(
            await run(client, "revise", args, "call-revise-replay"),
        ) as CommitReply;
        expect(first.outcome).toBe("applied");
        expect(first.objects).toEqual(["mem_a", first.objectId].sort());
        expect(second).toMatchObject({
            outcome: "already applied",
            objectId: first.objectId,
            objects: first.objects,
        });
        expect(kernel.liveRows()).toHaveLength(1);
    });

    test("a redelivered merge lists every retired target alongside the survivor", async () => {
        const { kernel, client } = harness();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "NAMING", summary: "A." });
        kernel.seedDecision({ object_id: "mem_b", decision_kind: "NAMING", summary: "B." });
        const args: CtxMemoryArgs = {
            objectIds: ["mem_a", "mem_b"],
            category: "NAMING",
            content: "A and B.",
            reason: "folded",
        };
        const first = JSON.parse(
            await run(client, "merge", args, "call-merge-replay"),
        ) as CommitReply;
        const second = JSON.parse(
            await run(client, "merge", args, "call-merge-replay"),
        ) as CommitReply;
        expect(first.outcome).toBe("applied");
        expect(first.objects).toEqual(["mem_a", "mem_b", first.objectId].sort());
        expect(second).toMatchObject({
            outcome: "already applied",
            objectId: first.objectId,
            objects: first.objects,
        });
        expect(kernel.liveRows()).toHaveLength(1);
    });

    test("a first row whose bounded view still overflows is bounded until it fits the response budget", async () => {
        const { client } = harness();
        // Each control character serializes as six bytes, so a raw 1,024-byte field cap still lets ten anti-memory fields plus the duplicated summary exceed the budget. commentlint: allow(JUDGE)
        const heavy = "\u0001".repeat(3_000);
        const antiMemory = {
            trigger: heavy,
            rejectedStrategy: heavy,
            rejectionReason: heavy,
            saferAlternative: heavy,
            preconditions: heavy,
            attemptedApproach: heavy,
            observedFailure: heavy,
            rootCause: heavy,
            recovery: heavy,
            nonApplicableWhen: heavy,
        };
        const created = JSON.parse(
            await run(
                client,
                "create",
                { category: "REJECTED_APPROACH", antiMemory, reason: heavy },
                "call-heavy-create",
            ),
        ) as CommitReply;
        const text = await run(client, "get", { objectIds: [created.objectId] }, "call-heavy-get");
        const reply = JSON.parse(text) as {
            memories: Record<string, unknown>[];
            elidedObjectIds?: string[];
        };
        expect(reply.memories).toHaveLength(1);
        expect(reply.elidedObjectIds).toBeUndefined();
        const view = reply.memories[0] as Record<string, unknown>;
        expect(Buffer.byteLength(JSON.stringify(view), "utf8")).toBeLessThanOrEqual(
            CTX_MEMORY_RESPONSE_BUDGET_BYTES,
        );
        expect(String(view.content).endsWith("… [truncated]")).toBe(true);
    });

    test("create rejects an anti-memory payload under a positive category", async () => {
        const { kernel, client } = harness();
        await expect(
            run(
                client,
                "create",
                {
                    category: "ARCHITECTURE",
                    content: CONTENT,
                    antiMemory: {
                        trigger: "caching",
                        rejectedStrategy: "Redis",
                        rejectionReason: "split ownership",
                    },
                },
                "call-create-mixed",
            ),
        ).rejects.toBeInstanceOf(ClaimOperationInputError);
        expect(kernel.liveRows()).toHaveLength(0);
    });

    test("create rejects a category outside the writable taxonomy", async () => {
        const { kernel, client } = harness();
        await expect(
            run(
                client,
                "create",
                { category: "NOT_A_CATEGORY", content: CONTENT },
                "call-create-bogus",
            ),
        ).rejects.toBeInstanceOf(ClaimOperationInputError);
        expect(kernel.liveRows()).toHaveLength(0);
    });
});

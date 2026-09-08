import { describe, expect, setSystemTime, test } from "bun:test";
import { KernelClient } from "../../shared/kernel-client";
import {
    ClaimOperationInputError,
    renderAntiMemoryContent,
} from "../../shared/kernel-client/anti-memory";
import { FakeKernel, FakeKernelTransport } from "../../shared/kernel-client-testing/fake-kernel";
import { CTX_MEMORY_RESPONSE_BUDGET_BYTES } from "./constants";
import { CTX_MEMORY_ACTOR, type CtxMemoryWriteIdentity, executeCtxMemory } from "./execute";
import type { CtxMemoryAction, CtxMemoryArgs } from "./types";

const SESSION = "ses-exec";
const PROJECT = "/tmp/exec";
const CONTENT = "Use the daemon";
const ANTI_MEMORY = {
    trigger: "session caching",
    rejectedStrategy: "Redis",
    rejectionReason: "it creates split ownership",
};

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

/** The wrappers pass raw arguments through when schema parsing fails, so the executor can receive `null` where the type says `string | undefined`. commentlint: allow(JUDGE) */
function rawArgs(value: Record<string, unknown>): CtxMemoryArgs {
    return value as unknown as CtxMemoryArgs;
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

    test("a create replay recovered from the row reports the creating commit as knownAsOf", async () => {
        const { client } = harness();
        const args: CtxMemoryArgs = { category: "REJECTED_APPROACH", antiMemory: ANTI_MEMORY };
        try {
            setSystemTime(new Date("2026-01-01T12:00:00Z"));
            const first = JSON.parse(await run(client, "create", args, "call-anti-replay")) as {
                commitSeq: number;
            };
            await run(
                client,
                "create",
                { category: "ARCHITECTURE", content: "unrelated one" },
                "call-unrelated-1",
            );
            await run(
                client,
                "create",
                { category: "ARCHITECTURE", content: "unrelated two" },
                "call-unrelated-2",
            );
            // The generated expiry re-renders two days later, so the daemon answers `operation_key_reused` and the executor recovers the replay from the stored row instead of a receipt. commentlint: allow(JUDGE)
            setSystemTime(new Date("2026-01-03T12:00:00Z"));
            const second = JSON.parse(await run(client, "create", args, "call-anti-replay")) as {
                outcome: string;
                commitSeq: number;
                knownAsOf: number;
            };
            expect(second.outcome).toBe("already applied");
            expect(second.commitSeq).toBe(first.commitSeq);
            expect(second.knownAsOf).toBe(first.commitSeq);
        } finally {
            setSystemTime();
        }
    });

    test("a null reason on revise inherits the predecessor's rationale", async () => {
        const { kernel, client } = harness();
        kernel.seedDecision({
            object_id: "mem_a",
            decision_kind: "ARCHITECTURE",
            summary: "A.",
            rationale: "keep me",
        });
        const text = await run(
            client,
            "revise",
            rawArgs({ objectId: "mem_a", content: "A, revised.", reason: null }),
            "call-revise-null-reason",
        );
        expect((JSON.parse(text) as CommitReply).outcome).toBe("applied");
        expect(kernel.liveRows()[0]?.decision?.payload.rationale).toBe("keep me");
    });

    test("a redelivered revise carrying null fields reaches the visibility error, not a TypeError", async () => {
        const { kernel, client } = harness();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        const first = JSON.parse(
            await run(
                client,
                "revise",
                { objectId: "mem_a", category: "ARCHITECTURE", content: "A2.", reason: "r" },
                "call-revise-null-probe",
            ),
        ) as CommitReply;
        expect(first.outcome).toBe("applied");
        // A null reason counts as omitted, and an omitted field makes the reconstructed spec unverifiable, so the probe declines the replay and the ordinary path reports the retired target. commentlint: allow(JUDGE)
        await expect(
            run(
                client,
                "revise",
                rawArgs({
                    objectId: "mem_a",
                    category: "ARCHITECTURE",
                    content: "A2.",
                    reason: null,
                }),
                "call-revise-null-probe",
            ),
        ).rejects.toBeInstanceOf(ClaimOperationInputError);
        await expect(
            run(
                client,
                "revise",
                rawArgs({
                    objectId: "mem_a",
                    category: "ARCHITECTURE",
                    content: null,
                    reason: "r",
                }),
                "call-revise-null-probe",
            ),
        ).rejects.toBeInstanceOf(ClaimOperationInputError);
    });

    test("a create under an identity already spent on a revise surfaces the daemon's rejection", async () => {
        const { kernel, client } = harness();
        const seeded = JSON.parse(
            await run(
                client,
                "create",
                { category: "REJECTED_APPROACH", antiMemory: ANTI_MEMORY, reason: "seed" },
                "call-seed",
            ),
        ) as CommitReply;
        const revised = JSON.parse(
            await run(
                client,
                "revise",
                {
                    objectId: seeded.objectId,
                    category: "REJECTED_APPROACH",
                    antiMemory: ANTI_MEMORY,
                    reason: "same",
                },
                "call-shared-identity",
            ),
        ) as CommitReply;
        expect(revised.outcome).toBe("applied");
        // The successor carries the object id a create under this identity derives, with the same category, rationale, and payload, but its stored operation was a supersede at revision 2, so the create must not report it as already applied. commentlint: allow(JUDGE)
        const text = await run(
            client,
            "create",
            { category: "REJECTED_APPROACH", antiMemory: ANTI_MEMORY, reason: "same" },
            "call-shared-identity",
        );
        expect(text).toBe("Error: The operation key was reused with a different request digest.");
        expect(kernel.liveRows()).toHaveLength(1);
    });

    test("get bounds the ids it echoes back as missing", async () => {
        const { client } = harness();
        const bogus = ["x".repeat(50_000), "y".repeat(50_000)];
        const text = await run(client, "get", { objectIds: bogus }, "call-get-bogus");
        const reply = JSON.parse(text) as { memories: unknown[]; missingObjectIds: string[] };
        expect(reply.memories).toEqual([]);
        expect(reply.missingObjectIds).toHaveLength(2);
        for (const id of reply.missingObjectIds) {
            expect(id.endsWith("… [truncated]")).toBe(true);
            expect(Buffer.byteLength(id, "utf8")).toBeLessThan(1_100);
        }
        expect(Buffer.byteLength(text, "utf8")).toBeLessThan(4_096);
    });

    test("archive rejects a non-string reason as an input error, not a TypeError", async () => {
        const { kernel, client } = harness();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        await expect(
            run(client, "archive", rawArgs({ objectId: "mem_a", reason: 5 }), "call-archive-num"),
        ).rejects.toBeInstanceOf(ClaimOperationInputError);
        expect(kernel.liveRows()).toHaveLength(1);
    });

    test("an anti-memory create carrying null content commits as a valid anti-memory", async () => {
        const { kernel, client } = harness();
        const text = await run(
            client,
            "create",
            rawArgs({ category: "REJECTED_APPROACH", antiMemory: ANTI_MEMORY, content: null }),
            "call-create-anti-null-content",
        );
        expect((JSON.parse(text) as CommitReply).outcome).toBe("applied");
        expect(kernel.liveRows()[0]?.decision?.decision_kind).toBe("REJECTED_APPROACH");
    });

    test("merge counts blank entries toward the raw list cap", async () => {
        const { client } = harness();
        const objectIds = [...Array.from({ length: 20 }, (_, index) => `mem_over_${index}`), " "];
        await expect(run(client, "merge", { objectIds }, "call-merge-over")).rejects.toThrow(
            "merge accepts at most 20 objectIds; 21 were given. Merge in smaller batches.",
        );
    });

    test("merge without survivor content is rejected before any target is retired", async () => {
        const { kernel, client } = harness();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "NAMING", summary: "A." });
        kernel.seedDecision({ object_id: "mem_b", decision_kind: "NAMING", summary: "B." });
        await expect(
            run(client, "merge", { objectIds: ["mem_a", "mem_b"] }, "call-merge-no-content"),
        ).rejects.toThrow("merge requires content (with category) or antiMemory for the survivor");
        expect(kernel.liveRows()).toHaveLength(2);
    });

    test("get bounds the echoed missing ids to the response budget", async () => {
        const { client } = harness();
        const longIds = Array.from(
            { length: 20 },
            (_, index) => `mem_${index}_${"x".repeat(2_000)}`,
        );
        const text = await run(client, "get", { objectIds: longIds }, "call-get-long-missing");
        const reply = JSON.parse(text) as {
            memories: unknown[];
            missingObjectIds: string[];
            elidedRequestedIdCount?: number;
        };
        expect(Buffer.byteLength(text, "utf8")).toBeLessThan(
            CTX_MEMORY_RESPONSE_BUDGET_BYTES + 512,
        );
        expect(reply.memories).toEqual([]);
        expect(reply.missingObjectIds.length).toBeGreaterThan(0);
        expect(reply.missingObjectIds.length + (reply.elidedRequestedIdCount ?? 0)).toBe(20);
        expect(reply.elidedRequestedIdCount).toBeGreaterThan(0);
    });
});

describe("executeCtxMemory get expiry", () => {
    test("get reads an expired anti-memory as missing, like list and search do", async () => {
        const { kernel, client } = harness();
        try {
            setSystemTime(new Date("2026-03-01T12:00:00Z"));
            const expired = renderAntiMemoryContent({
                ...ANTI_MEMORY,
                expiresAt: Date.parse("2026-02-01T00:00:00Z"),
            });
            const live = renderAntiMemoryContent({
                ...ANTI_MEMORY,
                expiresAt: Date.parse("2026-04-01T00:00:00Z"),
            });
            kernel.seedDecision({
                object_id: "mem_expired",
                decision_kind: "REJECTED_APPROACH",
                summary: expired,
            });
            kernel.seedDecision({
                object_id: "mem_live",
                decision_kind: "REJECTED_APPROACH",
                summary: live,
            });
            const text = await run(
                client,
                "get",
                { objectIds: ["mem_expired", "mem_live"] },
                "call-get-expired",
            );
            const reply = JSON.parse(text) as {
                memories: Array<{ objectId: string }>;
                missingObjectIds: string[];
            };
            expect(reply.memories.map((view) => view.objectId)).toEqual(["mem_live"]);
            expect(reply.missingObjectIds).toEqual(["mem_expired"]);
        } finally {
            setSystemTime();
        }
    });
});

describe("executeCtxMemory get on a truncated read", () => {
    test("an expired anti-memory the daemon returned reads as missing, not unresolved", async () => {
        const { kernel, client } = harness();
        try {
            setSystemTime(new Date("2026-03-01T12:00:00Z"));
            kernel.seedDecision({
                object_id: "mem_expired",
                decision_kind: "REJECTED_APPROACH",
                summary: renderAntiMemoryContent({
                    ...ANTI_MEMORY,
                    expiresAt: Date.parse("2026-02-01T00:00:00Z"),
                }),
            });
            kernel.seedDecision({ object_id: "mem_live", decision_kind: "NAMING", summary: "A." });
            kernel.readTruncated = true;
            const text = await run(
                client,
                "get",
                { objectIds: ["mem_expired", "mem_live", "mem_absent"] },
                "call-get-truncated-expired",
            );
            const reply = JSON.parse(text) as {
                truncated?: boolean;
                memories: Array<{ objectId: string }>;
                missingObjectIds: string[];
                unresolvedObjectIds: string[];
            };
            expect(reply.truncated).toBe(true);
            expect(reply.memories.map((view) => view.objectId)).toEqual(["mem_live"]);
            expect(reply.missingObjectIds).toEqual(["mem_expired"]);
            expect(reply.unresolvedObjectIds).toEqual(["mem_absent"]);
        } finally {
            setSystemTime();
        }
    });
});

import { describe, expect, it } from "bun:test";
import {
    KernelClient,
    type KernelTransportCall,
    MAX_COMMIT_OPERATIONS,
    MAX_COMMIT_TOKENS,
} from "../kernel-client";
import { FakeKernel, FakeKernelTransport } from "./fake-kernel";

function commitCall(operations: unknown[]): KernelTransportCall {
    return {
        sessionId: "session",
        projectRoot: "/repo",
        method: "kernel.commit",
        body: {
            intent: { operation_key: "op-key", request_digest: "digest" },
            tokens: [],
            operations,
            source_kind: "assistant",
        },
    };
}

function decisionSpec(objectId: string): Record<string, unknown> {
    return {
        object_id: objectId,
        domain_id: "memory",
        source_id: "ctx_memory",
        source_revision: 2,
        decision_kind: "ARCHITECTURE",
        payload: { summary: "summary", rationale: "rationale" },
    };
}

describe("FakeKernel commit prevalidation", () => {
    it("rejects two inserts of one id in a single envelope and applies nothing", () => {
        const kernel = new FakeKernel();
        const reply = kernel.reply(
            commitCall([
                { op: "insert_decision", spec: decisionSpec("mem_dup") },
                { op: "insert_decision", spec: decisionSpec("mem_dup") },
            ]),
        );
        expect(reply).toEqual({ state: { kind: "invalid", reason: "already_exists" } });
        expect(kernel.objects.size).toBe(0);
        expect(kernel.tip).toBe(0);
    });

    it("accepts several supersessions sharing one survivor id", () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_a",
            decision_kind: "ARCHITECTURE",
            summary: "first",
        });
        kernel.seedDecision({
            object_id: "mem_b",
            decision_kind: "ARCHITECTURE",
            summary: "second",
        });
        const reply = kernel.reply(
            commitCall([
                {
                    op: "supersede_decision",
                    replaced_object_id: "mem_a",
                    spec: decisionSpec("mem_survivor"),
                },
                {
                    op: "supersede_decision",
                    replaced_object_id: "mem_b",
                    spec: decisionSpec("mem_survivor"),
                },
            ]),
        ) as { state: { kind: string } };
        expect(reply.state).toEqual({ kind: "available" });
        expect(kernel.objects.get("mem_a")?.superseded_by).toBe("mem_survivor");
        expect(kernel.objects.get("mem_b")?.superseded_by).toBe("mem_survivor");
        expect(kernel.objects.get("mem_survivor")?.invalidated_commit_seq).toBeNull();
    });

    it("answers not_found for a replacement id another project holds", () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_a",
            decision_kind: "ARCHITECTURE",
            summary: "ours",
            projectRoot: "/repo",
        });
        kernel.seedDecision({
            object_id: "mem_theirs",
            decision_kind: "ARCHITECTURE",
            summary: "theirs",
            projectRoot: "/elsewhere",
        });
        const reply = kernel.reply(
            commitCall([
                {
                    op: "supersede_decision",
                    replaced_object_id: "mem_a",
                    spec: decisionSpec("mem_theirs"),
                },
            ]),
        );
        expect(reply).toEqual({ state: { kind: "invalid", reason: "not_found" } });
        expect(kernel.objects.get("mem_a")?.invalidated_commit_seq).toBeNull();
    });

    it("floors a non-fold successor's sensitivity at its predecessor's", () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_a",
            decision_kind: "ARCHITECTURE",
            summary: "guarded",
            sensitivity: "sensitive",
        });
        const reply = kernel.reply(
            commitCall([
                {
                    op: "supersede_decision",
                    replaced_object_id: "mem_a",
                    spec: { ...decisionSpec("mem_b"), sensitivity: "normal" },
                },
            ]),
        ) as { state: { kind: string }; merged: string[] };
        expect(reply.state).toEqual({ kind: "available" });
        expect(reply.merged).toEqual([]);
        expect(kernel.objects.get("mem_b")?.sensitivity).toBe("sensitive");
    });
});

function readCall(body: Record<string, unknown>): KernelTransportCall {
    return {
        sessionId: "session",
        projectRoot: "/repo",
        method: "kernel.read",
        body: { surface: "explicit_search", gated: false, ...body },
    };
}

interface ReadReply {
    truncated: boolean;
    rows: { object: { object_id: string } }[];
}

function seedThree(kernel: FakeKernel): void {
    kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "oldest" });
    kernel.seedDecision({ object_id: "mem_b", decision_kind: "ARCHITECTURE", summary: "middle" });
    kernel.seedDecision({ object_id: "mem_c", decision_kind: "ARCHITECTURE", summary: "newest" });
}

describe("FakeKernel read filtering and row cap", () => {
    it("scopes rows to the object_ids filter", () => {
        const kernel = new FakeKernel();
        seedThree(kernel);
        const reply = kernel.reply(readCall({ object_ids: ["mem_b"] })) as ReadReply;
        expect(reply.rows.map((row) => row.object.object_id)).toEqual(["mem_b"]);
        expect(reply.truncated).toBe(false);
    });

    it("caps unfiltered reads to the newest rows and flags truncation", () => {
        const kernel = new FakeKernel();
        seedThree(kernel);
        kernel.readRowCap = 2;
        const reply = kernel.reply(readCall({})) as ReadReply;
        expect(reply.rows.map((row) => row.object.object_id)).toEqual(["mem_b", "mem_c"]);
        expect(reply.truncated).toBe(true);
    });

    it("applies the id filter before the row cap, so a filtered read reaches a dropped row", () => {
        const kernel = new FakeKernel();
        seedThree(kernel);
        kernel.readRowCap = 2;
        const reply = kernel.reply(readCall({ object_ids: ["mem_a"] })) as ReadReply;
        expect(reply.rows.map((row) => row.object.object_id)).toEqual(["mem_a"]);
        expect(reply.truncated).toBe(false);
    });
});

function commitCallWith(
    operations: unknown[],
    fields: Record<string, unknown>,
    operationKey = "op-key",
): KernelTransportCall {
    const call = commitCall(operations);
    const body = call.body as Record<string, unknown>;
    const intent = body.intent as Record<string, unknown>;
    return {
        ...call,
        body: { ...body, intent: { ...intent, operation_key: operationKey }, ...fields },
    };
}

describe("FakeKernel admission classes", () => {
    const insert = [{ op: "insert_decision", spec: decisionSpec("mem_new") }];

    it("refuses a source class above the derived one as class_over_declared and applies nothing", () => {
        const kernel = new FakeKernel();
        const reply = kernel.reply(
            commitCallWith(insert, { asserted_source_class: "explicit_user" }),
        );
        expect(reply).toEqual({ state: { kind: "invalid", reason: "class_over_declared" } });
        expect(kernel.objects.size).toBe(0);
        expect(kernel.tip).toBe(0);
    });

    it("refuses a taint class above the derived one as class_over_declared", () => {
        const kernel = new FakeKernel();
        const reply = kernel.reply(
            commitCallWith(insert, { asserted_taint_class: "current_code" }),
        );
        expect(reply).toEqual({ state: { kind: "invalid", reason: "class_over_declared" } });
        expect(kernel.tip).toBe(0);
    });

    it("refuses an unknown source_kind as invalid_input", () => {
        const kernel = new FakeKernel();
        const reply = kernel.reply(commitCallWith(insert, { source_kind: "oracle" }));
        expect(reply).toEqual({ state: { kind: "invalid", reason: "invalid_input" } });
        expect(kernel.tip).toBe(0);
    });

    it("refuses an unknown class name as invalid_input", () => {
        const kernel = new FakeKernel();
        for (const fields of [
            { asserted_source_class: "oracle" },
            { asserted_taint_class: "oracle" },
            { asserted_source_class: 7 },
        ]) {
            const reply = kernel.reply(commitCallWith(insert, fields));
            expect(reply).toEqual({ state: { kind: "invalid", reason: "invalid_input" } });
        }
        expect(kernel.tip).toBe(0);
    });

    it("refuses a taint the derived source does not admit as invalid_input", () => {
        // A `user` write derives `user_inferred`; `repo_untrusted_text` ranks below it but is not a taint `model_inference` admits. commentlint: allow(JUDGE)
        const kernel = new FakeKernel();
        const reply = kernel.reply(
            commitCallWith(insert, {
                source_kind: "user",
                asserted_taint_class: "repo_untrusted_text",
            }),
        );
        expect(reply).toEqual({ state: { kind: "invalid", reason: "invalid_input" } });
        expect(kernel.tip).toBe(0);
    });

    it("admits the derived pair asserted explicitly and a taint at or below it", () => {
        const kernel = new FakeKernel();
        const derived = kernel.reply(
            commitCallWith(
                insert,
                {
                    asserted_source_class: "model_inference",
                    asserted_taint_class: "assistant_inference",
                },
                "derived",
            ),
        ) as { state: { kind: string } };
        expect(derived.state).toEqual({ kind: "available" });
        const lowered = kernel.reply(
            commitCallWith(
                [{ op: "insert_decision", spec: decisionSpec("mem_personal") }],
                { source_kind: "user", asserted_taint_class: "personal" },
                "lowered",
            ),
        ) as { state: { kind: string } };
        expect(lowered.state).toEqual({ kind: "available" });
        expect(kernel.objects.get("mem_personal")?.source_kind).toBe("user");
        expect(kernel.tip).toBe(2);
    });

    it("refuses an over-declared class before consulting the receipt for a replayed key", () => {
        const kernel = new FakeKernel();
        const first = kernel.reply(commitCall(insert)) as { state: { kind: string } };
        expect(first.state).toEqual({ kind: "available" });
        const replay = kernel.reply(
            commitCallWith(insert, { asserted_source_class: "explicit_user" }),
        );
        expect(replay).toEqual({ state: { kind: "invalid", reason: "class_over_declared" } });
    });
});

describe("FakeKernel envelope limits", () => {
    it("refuses more operations than the daemon carries with the invalid_params transport error", () => {
        const kernel = new FakeKernel();
        const operations = Array.from({ length: MAX_COMMIT_OPERATIONS + 1 }, (_, i) => ({
            op: "insert_decision",
            spec: decisionSpec(`mem_${i}`),
        }));
        expect(() => kernel.reply(commitCall(operations))).toThrow(
            expect.objectContaining({ kind: "terminal", code: "invalid_params" }),
        );
        expect(kernel.tip).toBe(0);
        expect(kernel.objects.size).toBe(0);
    });

    it("refuses more tokens than the daemon carries with the invalid_params transport error", () => {
        const kernel = new FakeKernel();
        const tokens = Array.from({ length: MAX_COMMIT_TOKENS + 1 }, (_, i) => ({
            object_id: `mem_${i}`,
            known_as_of: 0,
        }));
        expect(() =>
            kernel.reply(
                commitCallWith([{ op: "insert_decision", spec: decisionSpec("mem_new") }], {
                    tokens,
                }),
            ),
        ).toThrow(expect.objectContaining({ kind: "terminal", code: "invalid_params" }));
        expect(kernel.tip).toBe(0);
    });

    it("refuses a missing source_kind with the invalid_params transport error", () => {
        const kernel = new FakeKernel();
        expect(() =>
            kernel.reply(
                commitCallWith([{ op: "insert_decision", spec: decisionSpec("mem_new") }], {
                    source_kind: undefined,
                }),
            ),
        ).toThrow(expect.objectContaining({ kind: "terminal", code: "invalid_params" }));
        expect(kernel.tip).toBe(0);
    });

    it("surfaces the transport refusal through the client as invalid_input", async () => {
        const transport = new FakeKernelTransport();
        const client = new KernelClient({
            transport,
            enabled: true,
            sessionId: "session",
            projectRoot: "/repo",
        });
        const tokens = Array.from({ length: MAX_COMMIT_TOKENS + 1 }, (_, i) => ({
            object_id: `mem_${i}`,
            known_as_of: 0,
        }));
        // The client refuses the envelope itself before any transport call; a transport-driven reply exercises the fake's own refusal. commentlint: allow(JUDGE)
        const refusedByClient = await client.commit({
            actor: "assistant",
            operationId: "op-1",
            cause: "ctx_memory",
            operations: [{ op: "insert_decision", spec: decisionSpec("mem_new") as never }],
            tokens,
        });
        expect(refusedByClient.state).toEqual({ kind: "invalid", reason: "invalid_input" });
        expect(transport.calls).toHaveLength(0);
        await expect(
            transport.call(
                commitCallWith([{ op: "insert_decision", spec: decisionSpec("mem_new") }], {
                    tokens,
                }),
            ),
        ).rejects.toMatchObject({ kind: "terminal", code: "invalid_params" });
    });
});

import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { renderToolStateText } from "@eidnara/opencode/shared/kernel-client";
import { FakeKernel } from "@eidnara/opencode/shared/kernel-client-testing/fake-kernel";
import {
    MEMORY_STATE_TABLE,
    stubKernelClient,
} from "@eidnara/opencode/shared/kernel-client-testing/state-table";
import { createCtxMemoryTools } from "@eidnara/opencode/tools/ctx-memory/tools";
import { fakeKernelResolver } from "../__tests__/test-utils";
import { createCtxMemoryTool } from "./ctx-memory";

const PROJECT = "git:kernel-pi";
const CWD = "/tmp/kernel-pi";
const SESSION = "ses-kernel-pi";

interface CommitJson {
    action: string;
    outcome: string;
    commitSeq: number;
    knownAsOf: number;
    objects: string[];
}

interface ReadJson {
    action: string;
    knownAsOf: number;
    memories: Array<{ objectId: string; category: string; content: string }>;
    missingObjectIds?: string[];
}

function harness(
    kernel = new FakeKernel(),
    options: {
        kernelClient?: ReturnType<typeof fakeKernelResolver>["kernelClient"];
    } = {},
) {
    const fake = fakeKernelResolver(kernel);
    const tool = createCtxMemoryTool({
        kernelClient: options.kernelClient ?? fake.kernelClient,
        resolveProjectPath: () => PROJECT,
    });
    const execute = async (args: Record<string, unknown>, callId: string) =>
        tool.execute(callId, args as never, new AbortController().signal, undefined, {
            cwd: CWD,
            sessionManager: { getSessionId: () => SESSION },
        } as never);
    return { tool, execute, ...fake };
}

type ExecuteResult = Awaited<ReturnType<ReturnType<typeof harness>["execute"]>>;

function textOf(result: ExecuteResult): string {
    return result.content[0]?.text ?? "";
}

function parseResult<T>(result: ExecuteResult): T {
    expect(result.isError).toBeUndefined();
    return JSON.parse(textOf(result)) as T;
}

function createArgs(content: string) {
    return { action: "create", category: "ARCHITECTURE", content };
}

function reduced(inner: Record<string, unknown>) {
    return { reduced: true, summary: JSON.stringify(inner) };
}

describe("Pi ctx_memory create", () => {
    it("returns a commit receipt and stores the decision in the kernel", async () => {
        const tool = harness();
        const created = parseResult<CommitJson>(
            await tool.execute(createArgs("Pi uses the kernel."), "call-create"),
        );
        expect(created).toMatchObject({
            action: "create",
            outcome: "applied",
            commitSeq: 1,
        });
        expect(created.objects).toHaveLength(1);
        const objectId = created.objects[0] as string;
        expect(objectId).toMatch(/^mem_[0-9a-f]{32}$/);
        expect(tool.kernel.objects.get(objectId)?.decision).toEqual({
            decision_kind: "ARCHITECTURE",
            payload: { summary: "Pi uses the kernel.", rationale: "" },
        });
        const commit = tool.transport.calls[0]?.body as {
            intent: { actor: string };
        };
        expect(commit.intent.actor).toBe("agent:opencode");
    });

    it("replays a duplicate call and creates no second object", async () => {
        const tool = harness();
        const args = createArgs("Replay exact bytes.");
        const first = parseResult<CommitJson>(await tool.execute(args, "call-replay"));
        const second = parseResult<CommitJson>(await tool.execute(args, "call-replay"));
        expect(second).toMatchObject({
            outcome: "already applied",
            objects: first.objects,
        });
        expect(tool.kernel.liveRows()).toHaveLength(1);
    });

    it("an aborted tool call answers cancelled without committing", async () => {
        const tool = harness();
        const controller = new AbortController();
        controller.abort();
        const result = await tool.tool.execute(
            "call-abort",
            createArgs("never lands") as never,
            controller.signal,
            undefined,
            {
                cwd: CWD,
                sessionManager: { getSessionId: () => SESSION },
            } as never,
        );
        expect(result.isError).toBe(true);
        expect(textOf(result)).toBe("Error: The memory request was cancelled before it completed.");
        expect(tool.kernel.objects.size).toBe(0);
    });
});

describe("Pi ctx_memory revise", () => {
    it("revises by object id through the cached token and supersedes the target", async () => {
        const tool = harness();
        const created = parseResult<CommitJson>(
            await tool.execute(createArgs("Pi uses the kernel."), "call-create"),
        );
        const objectId = created.objects[0] as string;
        const revised = parseResult<CommitJson>(
            await tool.execute(
                { action: "revise", objectId, content: "Pi uses the kernel routes." },
                "call-revise",
            ),
        );
        expect(revised.outcome).toBe("applied");
        const survivor = revised.objects.find((id) => id !== objectId) as string;
        expect(tool.kernel.objects.get(objectId)?.superseded_by).toBe(survivor);
        expect(tool.kernel.objects.get(survivor)?.decision?.payload.summary).toBe(
            "Pi uses the kernel routes.",
        );
        expect(tool.transport.methods()).toEqual(["kernel.commit", "kernel.read", "kernel.commit"]);
    });

    it("renders the same conflict text as OpenCode after a concurrent change", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_seeded",
            decision_kind: "CONSTRAINTS",
            summary: "Seeded.",
        });
        const pi = harness(kernel);
        parseResult<ReadJson>(
            await pi.execute({ action: "get", objectIds: ["mem_seeded"] }, "call-get"),
        );
        kernel.beforeCommit = () => kernel.touch("mem_seeded");
        const piResult = await pi.execute(
            { action: "revise", objectId: "mem_seeded", content: "Moved." },
            "call-revise-conflict",
        );
        expect(piResult.isError).toBe(true);
        const conflictText = renderToolStateText({
            kind: "conflict",
            reason: "known_as_of_advanced",
        });
        expect(textOf(piResult)).toBe(
            `Error: ${conflictText} Re-read mem_seeded with ctx_memory get, then retry.`,
        );

        const openCodeKernel = new FakeKernel();
        openCodeKernel.seedDecision({
            object_id: "mem_seeded",
            decision_kind: "CONSTRAINTS",
            summary: "Seeded.",
        });
        const openCodeFake = fakeKernelResolver(openCodeKernel);
        const openCode = createCtxMemoryTools({
            kernelClient: openCodeFake.kernelClient,
            resolveProjectPath: () => PROJECT,
        }).ctx_memory;
        const openCodeExecute = (args: Record<string, unknown>, callID: string) =>
            openCode.execute(
                args as never,
                {
                    sessionID: SESSION,
                    directory: CWD,
                    callID,
                    agent: "primary",
                } as never,
            ) as Promise<string>;
        await openCodeExecute({ action: "get", objectIds: ["mem_seeded"] }, "call-get");
        openCodeKernel.beforeCommit = () => openCodeKernel.touch("mem_seeded");
        expect(
            await openCodeExecute(
                { action: "revise", objectId: "mem_seeded", content: "Moved." },
                "call-revise-conflict",
            ),
        ).toBe(textOf(piResult));
    });
});

describe("Pi ctx_memory archive and merge", () => {
    it("archives by object id and retires the target", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_old", decision_kind: "NAMING", summary: "Old." });
        const tool = harness(kernel);
        const archived = parseResult<CommitJson>(
            await tool.execute(
                { action: "archive", objectId: "mem_old", reason: "obsolete" },
                "call-archive",
            ),
        );
        expect(archived).toMatchObject({ action: "archive", outcome: "applied" });
        expect(kernel.objects.get("mem_old")?.invalidated_commit_seq).not.toBeNull();
    });

    it("merges two objects into one survivor", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_x", decision_kind: "NAMING", summary: "X." });
        kernel.seedDecision({ object_id: "mem_y", decision_kind: "NAMING", summary: "Y." });
        const tool = harness(kernel);
        const merged = parseResult<CommitJson>(
            await tool.execute(
                { action: "merge", objectIds: ["mem_x", "mem_y"], content: "XY." },
                "call-merge",
            ),
        );
        expect(merged.outcome).toBe("applied");
        const survivor = merged.objects.find((id) => id !== "mem_x" && id !== "mem_y") as string;
        expect(kernel.objects.get("mem_x")?.superseded_by).toBe(survivor);
        expect(kernel.objects.get("mem_y")?.superseded_by).toBe(survivor);
        expect(kernel.objects.get(survivor)?.decision?.payload.summary).toBe("XY.");
    });
});

describe("Pi ctx_memory reads and action gates", () => {
    it("reads by object id and reports missing ids", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_a",
            decision_kind: "NAMING",
            summary: "A.",
        });
        const tool = harness(kernel);
        const read = parseResult<ReadJson>(
            await tool.execute({ action: "get", objectIds: ["mem_a", "mem_missing"] }, "call-get"),
        );
        expect(read.memories.map((memory) => memory.objectId)).toEqual(["mem_a"]);
        expect(read.missingObjectIds).toEqual(["mem_missing"]);
    });

    it("rejects actions outside the agent set", async () => {
        const tool = harness();
        const denied = await tool.execute({ action: "list" }, "call-list");
        expect(denied.isError).toBe(true);
        expect(textOf(denied)).toBe("Error: Action 'list' is not allowed in this context.");
        expect(tool.transport.calls).toHaveLength(0);
    });

    it("rejects agent approve and enforce", async () => {
        const tool = harness();
        for (const action of ["approve", "enforce"]) {
            const result = await tool.execute({ action }, `call-${action}`);
            expect(result.isError).toBe(true);
            expect(textOf(result)).toBe(
                "Error: approve and enforce are human-host-owned commands, not agent actions.",
            );
        }
        expect(tool.transport.calls).toHaveLength(0);
    });

    it("maps a write-shape input error to the Error prefix", async () => {
        const tool = harness();
        const result = await tool.execute(
            { action: "create", category: "ARCHITECTURE" },
            "call-shape",
        );
        expect(result.isError).toBe(true);
        expect(textOf(result)).toBe("Error: create requires non-empty content and category");
        expect(tool.transport.calls).toHaveLength(0);
    });
});

describe("Pi ctx_memory imitated reduced arguments", () => {
    it("decodes a reduced revise that names its object id", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_reduced",
            decision_kind: "NAMING",
            summary: "Before.",
        });
        const tool = harness(kernel);
        const revised = parseResult<CommitJson>(
            await tool.execute(
                reduced({
                    action: "revise",
                    objectId: "mem_reduced",
                    content: "After.",
                }),
                "call-reduced-revise",
            ),
        );
        expect(revised.outcome).toBe("applied");
        expect(tool.kernel.objects.get("mem_reduced")?.superseded_by).toBeString();
    });

    it("decodes a reduced merge that names its object ids", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_x", decision_kind: "NAMING", summary: "X." });
        kernel.seedDecision({ object_id: "mem_y", decision_kind: "NAMING", summary: "Y." });
        const tool = harness(kernel);
        const merged = parseResult<CommitJson>(
            await tool.execute(
                reduced({
                    action: "merge",
                    objectIds: ["mem_x", "mem_y"],
                    content: "XY.",
                }),
                "call-reduced-merge",
            ),
        );
        expect(merged.outcome).toBe("applied");
        expect(tool.kernel.objects.get("mem_x")?.superseded_by).toBe(
            tool.kernel.objects.get("mem_y")?.superseded_by ?? "",
        );
    });
});

describe("Pi ctx_memory memory state table", () => {
    it.each(
        MEMORY_STATE_TABLE,
    )("a stubbed client answering %s drives the tool text", async (_key, state) => {
        const tool = harness(new FakeKernel(), {
            kernelClient: () => stubKernelClient(state),
        });
        const result = await tool.execute({ action: "get", objectIds: ["mem_a"] }, "call-get");
        if (state.kind === "available") {
            expect(parseResult<ReadJson>(result)).toMatchObject({
                action: "get",
                memories: [],
                missingObjectIds: ["mem_a"],
            });
        } else {
            expect(result.isError).toBe(true);
            expect(textOf(result)).toBe(`Error: ${renderToolStateText(state)}`);
        }
    });
});

describe("Pi ctx_memory source", () => {
    it("contains no local-store IDs, embeddings, or mutation-log writes", () => {
        const source = readFileSync(resolve(import.meta.dir, "ctx-memory.ts"), "utf8");
        for (const forbidden of [
            "memory_embeddings",
            "memory_mutation_log",
            "storage-memory-claims",
            'storage-memory"',
            "memoryId",
            "publicClaimId",
            "mutationToken",
        ]) {
            expect(source).not.toContain(forbidden);
        }
    });
});

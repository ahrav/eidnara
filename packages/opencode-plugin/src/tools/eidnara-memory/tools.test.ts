import { describe, expect, setSystemTime, test } from "bun:test";
import { CONTEXT_RESEARCHER_AGENT } from "../../agents/context-researcher";
import { resolveSessionDirectory } from "../../hooks/context/session-directory";
import { HostCallError } from "../../shared/host-client/errors";
import { KernelClient, TokenCache } from "../../shared/kernel-client";
import { FakeKernel, FakeKernelTransport } from "../../shared/kernel-client-testing/fake-kernel";
import { createEidnaraMemoryTools } from "./tools";
import {
    EIDNARA_MEMORY_ACTIONS,
    EIDNARA_MEMORY_DEFAULT_ACTIONS,
    type EidnaraMemoryAction,
} from "./types";

const PROJECT = "git:kernel-opencode";
const ROOT = "/tmp/kernel-opencode";
const SESSION = "ses-kernel-opencode";

interface CommitJson {
    action: string;
    outcome: string;
    commitSeq: number;
    knownAsOf: number;
    objectId?: string;
    objects: string[];
}

interface ReadJson {
    action: string;
    knownAsOf: number;
    memories: Array<{
        objectId: string;
        category: string;
        content: string;
        antiMemory?: Record<string, string | null>;
        labeled: boolean;
    }>;
    missingObjectIds?: string[];
    truncated?: boolean;
}

function harness(kernel = new FakeKernel(), enabled = true) {
    const transport = new FakeKernelTransport(kernel);
    const tokens = new TokenCache();
    const definition = createEidnaraMemoryTools({
        kernelClient: ({ sessionId, projectRoot }) =>
            new KernelClient({ transport, enabled, sessionId, projectRoot, tokens }),
        resolveProjectPath: () => PROJECT,
        allowedActions: ["create", "get", "revise", "archive", "merge"],
    }).eidnara_memory;
    const execute = async (
        args: Record<string, unknown>,
        callID: string,
        agent = "primary",
    ): Promise<string> =>
        definition.execute(
            args as never,
            { sessionID: SESSION, directory: ROOT, callID, agent } as never,
        ) as Promise<string>;
    return { definition, execute, transport, kernel, tokens };
}

function parseJson<T>(text: string): T {
    expect(text).not.toStartWith("Error:");
    return JSON.parse(text) as T;
}

function createArgs(content: string) {
    return { action: "create", category: "ARCHITECTURE", content };
}

function reduced(inner: Record<string, unknown>) {
    return { reduced: true, summary: JSON.stringify(inner) };
}

describe("eidnara_memory without a daemon", () => {
    test("routes a pinned session independently of the tool context directory", async () => {
        const transport = new FakeKernelTransport(new FakeKernel());
        const routedRoots: string[] = [];
        const identityDirectories: string[] = [];
        const pinnedRoot = "/tmp/kernel-opencode-pinned";
        const sessionDirectories = new Map([[SESSION, pinnedRoot]]);
        let metadataReads = 0;
        const definition = createEidnaraMemoryTools({
            kernelClient: ({ sessionId, projectRoot }) => {
                routedRoots.push(projectRoot);
                return new KernelClient({
                    transport,
                    enabled: true,
                    sessionId,
                    projectRoot,
                    tokens: new TokenCache(),
                });
            },
            resolveProjectPath: (directory) => {
                identityDirectories.push(directory);
                return PROJECT;
            },
            resolveSessionDirectory: (sessionId, fallbackDirectory) =>
                resolveSessionDirectory(
                    {
                        client: {
                            session: {
                                get: async () => {
                                    metadataReads += 1;
                                    throw new Error("session metadata unavailable");
                                },
                            },
                        } as never,
                        directory: pinnedRoot,
                        sessionDirectoryBySession: sessionDirectories,
                    },
                    sessionId,
                    fallbackDirectory,
                ),
        }).eidnara_memory;

        const text = (await definition.execute(createArgs("Pinned route."), {
            sessionID: SESSION,
            directory: "/tmp/divergent-tool-context",
            callID: "call-pinned-route",
            agent: "primary",
        } as never)) as string;

        expect(text).not.toStartWith("Error:");
        expect(metadataReads).toBe(0);
        expect(sessionDirectories.get(SESSION)).toBe(pinnedRoot);
        expect(identityDirectories).toEqual([pinnedRoot]);
        expect(routedRoots).toEqual([pinnedRoot]);
    });

    test("get answers with the unavailable text, no retry wording, and no kernel call", async () => {
        const tool = harness();
        tool.transport.fileExists = false;
        const text = await tool.execute({ action: "get", objectIds: ["mem_a"] }, "call-get");
        expect(text).toBe("Error: Memory is unavailable because the daemon is not running.");
        expect(text.toLowerCase()).not.toContain("retry");
        expect(tool.transport.calls).toHaveLength(0);
    });

    test("a disabled client answers with the disabled text", async () => {
        const tool = harness(new FakeKernel(), false);
        expect(await tool.execute(createArgs("x"), "call-disabled")).toBe(
            "Error: Memory is disabled by configuration (memory.enabled = false).",
        );
    });

    test("an aborted tool call answers the cancelled text without committing", async () => {
        const tool = harness();
        const controller = new AbortController();
        controller.abort();
        const text = (await tool.definition.execute(
            createArgs("never lands") as never,
            {
                sessionID: SESSION,
                directory: ROOT,
                callID: "call-abort",
                agent: "primary",
                abort: controller.signal,
            } as never,
        )) as string;
        expect(text).toBe("Error: The memory request was cancelled before it completed.");
        expect(tool.kernel.objects.size).toBe(0);
    });

    test("cancellation between the read and the commit never sends the mutation", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        const tool = harness(kernel);
        const controller = new AbortController();
        const call = tool.transport.call.bind(tool.transport);
        // The abort lands after the archive's visibility read is served; the
        // client's pre-send gate keeps the commit envelope off the wire.
        tool.transport.call = async (args) => {
            const reply = await call(args);
            if (args.method === "kernel.read") controller.abort();
            return reply;
        };
        const text = (await tool.definition.execute(
            { action: "archive", objectId: "mem_a" } as never,
            {
                sessionID: SESSION,
                directory: ROOT,
                callID: "call-abort-late",
                agent: "primary",
                abort: controller.signal,
            } as never,
        )) as string;
        expect(text).toBe("Error: The memory request was cancelled before it completed.");
        expect(tool.transport.methods()).toEqual(["kernel.read"]);
        expect(kernel.objects.get("mem_a")?.invalidated_commit_seq).toBeNull();
    });
});

describe("eidnara_memory create and revise through the cached token", () => {
    test("write ids pin to the session and tool-call derivation byte-for-byte", async () => {
        // The literals detect changes to hash inputs, separator, field order, or slice.
        const tool = harness();
        const created = parseJson<CommitJson>(
            await tool.execute(createArgs("Pinned derivation."), "call-pin-id"),
        );
        expect(created.objectId).toBe("mem_eb23fc2e926e16c6b2fbb47e96cd5615");
        const commit = tool.transport.calls[0]?.body as {
            operations: Array<{ spec: { decision_id: string } }>;
        };
        expect(commit.operations[0]?.spec.decision_id).toBe("dec_021727e5fc3f540175b4a9c91f9dcc55");
    });

    test("create then revise by object id issues one read for the token and supersedes", async () => {
        const tool = harness();
        const created = parseJson<CommitJson>(
            await tool.execute(createArgs("OpenCode uses the kernel."), "call-create"),
        );
        expect(created).toMatchObject({ action: "create", outcome: "applied", commitSeq: 1 });
        expect(created.objects).toHaveLength(1);
        const objectId = created.objects[0] as string;
        expect(created.objectId).toBe(objectId);
        expect(objectId).toMatch(/^mem_[0-9a-f]{32}$/);
        expect(tool.kernel.objects.get(objectId)?.decision).toEqual({
            decision_kind: "ARCHITECTURE",
            payload: { summary: "OpenCode uses the kernel.", rationale: "" },
        });

        const revised = parseJson<CommitJson>(
            await tool.execute(
                { action: "revise", objectId, content: "OpenCode uses the kernel routes." },
                "call-revise",
            ),
        );
        expect(revised.outcome).toBe("applied");
        expect(revised.objects).toContain(objectId);
        const survivor = revised.objects.find((id) => id !== objectId) as string;
        // The tool result names the survivor apart from the retired predecessor.
        expect(revised.objectId).toBe(survivor);
        expect(tool.kernel.objects.get(objectId)?.superseded_by).toBe(survivor);
        expect(tool.kernel.objects.get(survivor)?.decision?.payload.summary).toBe(
            "OpenCode uses the kernel routes.",
        );
        expect(tool.kernel.objects.get(survivor)?.decision?.decision_kind).toBe("ARCHITECTURE");
        // The tool's own read supplies the token; the client does not read a second time.
        expect(tool.transport.methods()).toEqual(["kernel.commit", "kernel.read", "kernel.commit"]);
        const commit = tool.transport.calls[2]?.body as { tokens: unknown[] };
        expect(commit.tokens).toEqual([{ object_id: objectId, known_as_of: 1 }]);
    });

    test("revise after a concurrent change renders the conflict directive", async () => {
        const tool = harness();
        const seeded = tool.kernel.seedDecision({
            object_id: "mem_seeded",
            decision_kind: "CONSTRAINTS",
            summary: "Seeded.",
        });
        // Prime the token cache from a read, then let the object move underneath it.
        parseJson<ReadJson>(
            await tool.execute({ action: "get", objectIds: [seeded.object_id] }, "call-get"),
        );
        tool.kernel.beforeCommit = () => tool.kernel.touch(seeded.object_id);
        const text = await tool.execute(
            { action: "revise", objectId: seeded.object_id, content: "Moved." },
            "call-revise-conflict",
        );
        expect(text).toBe(
            "Error: The object changed since it was read; read it again before writing. Re-read mem_seeded with eidnara_memory get, then retry.",
        );
    });

    test("revise of an object seen only in m[0] re-reads then succeeds", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_injected", decision_kind: "NAMING", summary: "N." });
        const tool = harness(kernel);
        // No prior tool read: the token cache is empty for this object.
        expect(tool.tokens.get(ROOT, "mem_injected")).toBeUndefined();
        const revised = parseJson<CommitJson>(
            await tool.execute(
                { action: "revise", objectId: "mem_injected", content: "Renamed." },
                "call-revise-m0",
            ),
        );
        expect(revised.outcome).toBe("applied");
        expect(tool.transport.methods()).toEqual(["kernel.read", "kernel.commit"]);
    });

    test("a duplicate create on the same tool call replays and creates no second object", async () => {
        const tool = harness();
        const args = createArgs("Replay exact bytes.");
        const first = parseJson<CommitJson>(await tool.execute(args, "call-replay"));
        const second = parseJson<CommitJson>(await tool.execute(args, "call-replay"));
        expect(first.outcome).toBe("applied");
        expect(second).toMatchObject({ outcome: "already applied", objects: first.objects });
        expect(tool.kernel.liveRows()).toHaveLength(1);
        // Changed arguments under the same tool call retain the operation key
        // and change the request digest, so the daemon rejects the redelivery
        // instead of committing a second operation.
        expect(await tool.execute(createArgs("Changed args."), "call-replay")).toBe(
            "Error: The operation key was reused with a different request digest.",
        );
        expect(tool.kernel.liveRows()).toHaveLength(1);
    });

    test("a redelivered revise that inherited category and reason replays as already applied", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        const tool = harness(kernel);
        const args = { action: "revise", objectId: "mem_a", content: "A, revised." };
        const first = parseJson<CommitJson>(await tool.execute(args, "call-revise-inherit"));
        expect(first.outcome).toBe("applied");
        // The omitted fields inherited from the retired predecessor, which no read serves; the probe compares only the stated content and lets the daemon's digest prove the rest.
        const second = parseJson<CommitJson>(await tool.execute(args, "call-revise-inherit"));
        expect(second).toMatchObject({
            outcome: "already applied",
            objectId: first.objectId,
            objects: first.objects,
        });
        expect(kernel.liveRows()).toHaveLength(1);
        expect(kernel.liveRows()[0]?.decision?.payload.summary).toBe("A, revised.");
    });

    test("a revise reusing an identity with different explicit content errors instead of replaying", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        const tool = harness(kernel);
        const first = parseJson<CommitJson>(
            await tool.execute(
                { action: "revise", objectId: "mem_a", content: "A, revised." },
                "call-revise-differs",
            ),
        );
        expect(first.outcome).toBe("applied");
        // An explicit field that differs from the successor is not a redelivery, so the probe declines and the ordinary path reports the retired target.
        const text = await tool.execute(
            { action: "revise", objectId: "mem_a", content: "A, revised differently." },
            "call-revise-differs",
        );
        expect(text).toBe("Error: memory not found or not visible from this project: mem_a");
        expect(kernel.liveRows()).toHaveLength(1);
        expect(kernel.liveRows()[0]?.decision?.payload.summary).toBe("A, revised.");
    });

    test("a retry that drops the original's explicit expiry keeps its digest conflict", async () => {
        const digestError = "Error: The operation key was reused with a different request digest.";
        const antiMemory = {
            trigger: "session caching",
            rejectedStrategy: "Redis",
            rejectionReason: "it creates split ownership",
        };
        try {
            setSystemTime(new Date("2026-01-01T12:00:00Z"));
            // The stored expiry is not day-aligned, so it cannot have been generated; the probe must not substitute it.
            const misaligned = harness();
            const first = parseJson<CommitJson>(
                await misaligned.execute(
                    {
                        action: "create",
                        category: "REJECTED_APPROACH",
                        antiMemory: {
                            ...antiMemory,
                            expiresAt: Date.parse("2026-02-15T07:30:00Z"),
                        },
                    },
                    "call-anti-explicit",
                ),
            );
            expect(first.outcome).toBe("applied");
            expect(
                await misaligned.execute(
                    { action: "create", category: "REJECTED_APPROACH", antiMemory },
                    "call-anti-explicit",
                ),
            ).toBe(digestError);
            // A day-aligned explicit expiry beyond the retry's own generated horizon fails the substitution bound.
            const farFuture = harness();
            const second = parseJson<CommitJson>(
                await farFuture.execute(
                    {
                        action: "create",
                        category: "REJECTED_APPROACH",
                        antiMemory: {
                            ...antiMemory,
                            expiresAt: Date.parse("2027-06-01T00:00:00Z"),
                        },
                    },
                    "call-anti-explicit-far",
                ),
            );
            expect(second.outcome).toBe("applied");
            expect(
                await farFuture.execute(
                    { action: "create", category: "REJECTED_APPROACH", antiMemory },
                    "call-anti-explicit-far",
                ),
            ).toBe(digestError);
        } finally {
            setSystemTime();
        }
    });

    test("a revise retry reusing its identity against an absent target errors instead of replaying", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        const tool = harness(kernel);
        const args = {
            action: "revise",
            category: "ARCHITECTURE",
            content: "A, revised.",
            reason: "clarified",
        };
        const first = parseJson<CommitJson>(
            await tool.execute({ ...args, objectId: "mem_a" }, "call-revise-retarget"),
        );
        expect(first.outcome).toBe("applied");
        // The retry names a target that never existed: absence from the read cannot prove it was the committed predecessor, and the probe's digest names the new target, so the daemon rejects the reuse and the ordinary visibility error surfaces.
        expect(await tool.execute({ ...args, objectId: "mem_gone" }, "call-revise-retarget")).toBe(
            "Error: memory not found or not visible from this project: mem_gone",
        );
        expect(kernel.liveRows()).toHaveLength(1);
        expect(kernel.liveRows()[0]?.object_id).toBe(first.objectId as string);
    });
});

describe("eidnara_memory reads", () => {
    test("get returns typed memories complete with no elision fields and reports missing ids", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_a",
            decision_kind: "ARCHITECTURE",
            summary: "A.",
            labeled: true,
        });
        kernel.seedDecision({ object_id: "mem_b", decision_kind: "NAMING", summary: "B." });
        const tool = harness(kernel);
        const got = parseJson<ReadJson & { elidedObjectIds?: string[] }>(
            await tool.execute({ action: "get", objectIds: ["mem_a", "mem_missing"] }, "call-get"),
        );
        expect(got.memories).toEqual([
            expect.objectContaining({
                objectId: "mem_a",
                category: "ARCHITECTURE",
                content: "A.",
                labeled: true,
            }),
        ]);
        expect(got.missingObjectIds).toEqual(["mem_missing"]);
        expect(got.elidedObjectIds).toBeUndefined();
        const read = tool.transport.calls[0]?.body as { surface: string; gated: boolean };
        expect(read).toMatchObject({ surface: "explicit_search", gated: false });
        expect(await tool.execute({ action: "list" }, "call-list-primary")).toContain(
            "not allowed",
        );
    });

    test("get with more than 20 unique ids is rejected naming the limit", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        const tool = harness(kernel);
        const objectIds = Array.from({ length: 21 }, (_, index) => `mem_over_${index}`);
        expect(await tool.execute({ action: "get", objectIds }, "call-get-over")).toBe(
            "Error: get accepts at most 20 objectIds; 21 were given. Split the request.",
        );
        // Duplicates collapse before the limit check, so 21 entries with 20 unique ids pass.
        const got = parseJson<ReadJson>(
            await tool.execute(
                {
                    action: "get",
                    objectIds: [...objectIds.slice(0, 20), "mem_over_0"],
                },
                "call-get-dedup",
            ),
        );
        expect(got.missingObjectIds).toHaveLength(20);
    });

    test("a stale read renders the state text", async () => {
        const kernel = new FakeKernel();
        kernel.surfaceStates.set("explicit_search", {
            kind: "stale",
            lag_positions: 3,
            oldest_unconsumed_age_ms: 10,
        });
        const tool = harness(kernel);
        expect(await tool.execute({ action: "get", objectIds: ["mem_a"] }, "call-get")).toBe(
            "Error: Memory results may lag recent changes; the projector has not caught up.",
        );
    });
});

describe("eidnara_memory reads beyond the daemon row cap", () => {
    test("get and revise resolve a row the capped unfiltered snapshot drops", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        kernel.seedDecision({ object_id: "mem_b", decision_kind: "ARCHITECTURE", summary: "B." });
        kernel.seedDecision({ object_id: "mem_c", decision_kind: "ARCHITECTURE", summary: "C." });
        kernel.readRowCap = 2;
        const tool = harness(kernel);
        const got = parseJson<ReadJson>(
            await tool.execute({ action: "get", objectIds: ["mem_a"] }, "call-get-capped"),
        );
        expect(got.truncated).toBeUndefined();
        expect(got.memories.map((memory) => memory.objectId)).toEqual(["mem_a"]);
        const readBodies = tool.transport.calls
            .filter((call) => call.method === "kernel.read")
            .map((call) => call.body as Record<string, unknown>);
        expect(readBodies[0]?.object_ids).toEqual(["mem_a"]);
        const revised = parseJson<CommitJson>(
            await tool.execute(
                { action: "revise", objectId: "mem_a", content: "A, revised." },
                "call-revise-capped",
            ),
        );
        expect(revised.outcome).toBe("applied");
        expect(kernel.objects.get("mem_a")?.superseded_by).toBe(revised.objectId ?? "");
    });

    test("a merge whose targeted preflight is byte-truncated resolves every target through chunked reads", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        kernel.seedDecision({ object_id: "mem_b", decision_kind: "ARCHITECTURE", summary: "B." });
        kernel.seedDecision({ object_id: "mem_c", decision_kind: "ARCHITECTURE", summary: "C." });
        // The id filter bypasses the daemon's row cap but not its byte budget; the cap stands in for a budget that holds two rows per filtered read.
        kernel.filteredReadRowCap = 2;
        const tool = harness(kernel);
        const merged = parseJson<CommitJson>(
            await tool.execute(
                {
                    action: "merge",
                    objectIds: ["mem_a", "mem_b", "mem_c"],
                    content: "A, B, and C.",
                },
                "call-merge-byte-truncated",
            ),
        );
        expect(merged.outcome).toBe("applied");
        expect(merged.objects).toContain("mem_a");
        expect(merged.objects).toContain("mem_b");
        expect(merged.objects).toContain("mem_c");
        expect(kernel.liveRows().map((row) => row.decision?.payload.summary)).toEqual([
            "A, B, and C.",
        ]);
        // Every follow-up read after the truncated preflight names a strict subset of the ids.
        const idReads = tool.transport.calls
            .filter((call) => call.method === "kernel.read")
            .map((call) => (call.body as { object_ids?: string[] }).object_ids)
            .filter((ids): ids is string[] => Array.isArray(ids));
        expect(idReads[0]).toHaveLength(4);
        for (const ids of idReads.slice(1)) {
            expect(ids.length).toBeLessThanOrEqual(2);
        }
    });
});

describe("eidnara_memory lifecycle and merge", () => {
    test("archive retires and merge folds two into one", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        kernel.seedDecision({ object_id: "mem_b", decision_kind: "ARCHITECTURE", summary: "B." });
        kernel.seedDecision({ object_id: "mem_c", decision_kind: "ARCHITECTURE", summary: "C." });
        const tool = harness(kernel);
        const archived = parseJson<CommitJson>(
            await tool.execute(
                { action: "archive", objectId: "mem_a", reason: "obsolete" },
                "call-archive",
            ),
        );
        expect(archived.outcome).toBe("applied");
        expect(archived.objectId).toBeUndefined();
        expect(kernel.objects.get("mem_a")?.invalidated_commit_seq).not.toBeNull();

        const merged = parseJson<CommitJson>(
            await tool.execute(
                { action: "merge", objectIds: ["mem_b", "mem_c"], content: "B and C." },
                "call-merge",
            ),
        );
        expect(merged.outcome).toBe("applied");
        expect(merged.objects).toContain("mem_b");
        expect(merged.objects).toContain("mem_c");
        // The tool result names the merge survivor apart from the retired predecessors.
        expect(merged.objectId).toMatch(/^mem_[0-9a-f]{32}$/);
        expect(merged.objectId).not.toBe("mem_b");
        expect(merged.objectId).not.toBe("mem_c");
        expect(kernel.objects.get(merged.objectId as string)?.decision?.payload.summary).toBe(
            "B and C.",
        );
        expect(kernel.liveRows().map((row) => row.decision?.payload.summary)).toEqual(["B and C."]);
        expect(await tool.execute({ action: "merge", objectIds: ["only"] }, "call-merge-one")).toBe(
            "Error: merge requires at least two objectIds",
        );
    });

    test("archive persists the supplied reason in the commit cause", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        kernel.seedDecision({ object_id: "mem_b", decision_kind: "ARCHITECTURE", summary: "B." });
        const tool = harness(kernel);
        await tool.execute(
            { action: "archive", objectId: "mem_a", reason: "superseded by ADR-7" },
            "call-archive-reason",
        );
        const withReason = tool.transport.calls.find((call) => call.method === "kernel.commit")
            ?.body as { intent: { cause: string } };
        expect(withReason.intent.cause).toBe("call-archive-reason reason: superseded by ADR-7");

        await tool.execute({ action: "archive", objectId: "mem_b" }, "call-archive-bare");
        const bare = tool.transport.calls.filter((call) => call.method === "kernel.commit")[1]
            ?.body as { intent: { cause: string } };
        expect(bare.intent.cause).toBe("call-archive-bare");
    });

    test("an archive redelivered with another target keeps its key regardless of the reason text", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        kernel.seedDecision({ object_id: "mem_b", decision_kind: "ARCHITECTURE", summary: "B." });
        const tool = harness(kernel);
        const first = parseJson<CommitJson>(
            await tool.execute(
                { action: "archive", objectId: "mem_a", reason: "first delivery" },
                "call-archive-reason-swap",
            ),
        );
        expect(first.outcome).toBe("applied");
        // The reason is caller-controlled audit text and never enters the operation key, so a fresh reason cannot mint a fresh key.
        expect(
            await tool.execute(
                { action: "archive", objectId: "mem_b", reason: "second delivery" },
                "call-archive-reason-swap",
            ),
        ).toBe("Error: The operation key was reused with a different request digest.");
        expect(kernel.objects.get("mem_b")?.invalidated_commit_seq).toBeNull();
    });

    test("two sessions sharing a tool-call id commit as distinct operations", async () => {
        const tool = harness();
        const args = createArgs("Session-scoped operation keys.");
        const first = parseJson<CommitJson>(await tool.execute(args, "call-shared"));
        expect(first.outcome).toBe("applied");
        const otherSession = parseJson<CommitJson>(
            (await tool.definition.execute(
                { ...args, content: "A different fact from another session." } as never,
                {
                    sessionID: "ses-kernel-opencode-2",
                    directory: ROOT,
                    callID: "call-shared",
                    agent: "primary",
                } as never,
            )) as string,
        );
        expect(otherSession.outcome).toBe("applied");
        expect(otherSession.objectId).not.toBe(first.objectId);
        expect(tool.kernel.liveRows()).toHaveLength(2);
    });

    test("a target the project cannot read is refused before any commit", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        kernel.seedDecision({
            object_id: "mem_secret",
            decision_kind: "ARCHITECTURE",
            summary: "S.",
            sensitivity: "secret",
        });
        const tool = harness(kernel);
        const refused = "Error: memory not found or not visible from this project: mem_secret";
        expect(
            await tool.execute(
                { action: "revise", objectId: "mem_secret", content: "S2." },
                "call-revise-x",
            ),
        ).toBe(refused);
        expect(
            await tool.execute(
                { action: "merge", objectIds: ["mem_a", "mem_secret"], content: "AS." },
                "call-merge-x",
            ),
        ).toBe(refused);
        expect(tool.transport.methods()).not.toContain("kernel.commit");
        // Archive's replay probe reaches the daemon, but its snapshot-0 token trips the token conflict before the envelope can mutate the hidden row.
        expect(
            await tool.execute({ action: "archive", objectId: "mem_secret" }, "call-archive-x"),
        ).toBe(refused);
        const probe = tool.transport.calls.find((call) => call.method === "kernel.commit");
        expect(probe?.body).toMatchObject({
            tokens: [{ object_id: "mem_secret", known_as_of: 0 }],
        });
        expect(kernel.objects.get("mem_secret")?.invalidated_commit_seq).toBeNull();
    });
});

describe("eidnara_memory merge category and sensitivity fences", () => {
    test("merge rejects a duplicated target naming the id, before any read or commit", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        kernel.seedDecision({ object_id: "mem_b", decision_kind: "ARCHITECTURE", summary: "B." });
        const tool = harness(kernel);
        expect(
            await tool.execute(
                { action: "merge", objectIds: ["mem_a", "mem_a", "mem_b"], content: "AB." },
                "call-merge-duplicate",
            ),
        ).toBe("Error: merge requires distinct objectIds; duplicated: mem_a");
        expect(tool.transport.methods()).not.toContain("kernel.commit");
        expect(kernel.objects.get("mem_a")?.invalidated_commit_seq).toBeNull();
        expect(kernel.objects.get("mem_b")?.invalidated_commit_seq).toBeNull();
    });

    test("merge refuses predecessors from different categories before any commit", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        kernel.seedDecision({ object_id: "mem_b", decision_kind: "CONSTRAINTS", summary: "B." });
        const tool = harness(kernel);
        expect(
            await tool.execute(
                { action: "merge", objectIds: ["mem_a", "mem_b"], content: "AB." },
                "call-merge-cross-category",
            ),
        ).toBe(
            "Error: merge targets span categories (ARCHITECTURE, CONSTRAINTS); one survivor cannot replace facts from different categories. Merge same-category memories only.",
        );
        expect(tool.transport.methods()).not.toContain("kernel.commit");
        expect(kernel.objects.get("mem_a")?.invalidated_commit_seq).toBeNull();
        expect(kernel.objects.get("mem_b")?.invalidated_commit_seq).toBeNull();
    });

    test("merge refuses folding anti-memories into a positive survivor", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_r1",
            decision_kind: "REJECTED_APPROACH",
            summary: "Trigger: retries. Rejected strategy: unbounded retry loop.",
        });
        kernel.seedDecision({
            object_id: "mem_r2",
            decision_kind: "REJECTED_APPROACH",
            summary: "Trigger: caching. Rejected strategy: cache without invalidation.",
        });
        const tool = harness(kernel);
        expect(
            await tool.execute(
                {
                    action: "merge",
                    objectIds: ["mem_r1", "mem_r2"],
                    category: "ARCHITECTURE",
                    content: "Do retries and caching.",
                },
                "call-merge-anti-to-positive",
            ),
        ).toBe(
            "Error: merge cannot fold REJECTED_APPROACH memories into a positive survivor; the negation would be lost",
        );
        expect(tool.transport.methods()).not.toContain("kernel.commit");
    });

    test("merge refuses folding positive memories into an anti-memory survivor", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        kernel.seedDecision({ object_id: "mem_b", decision_kind: "ARCHITECTURE", summary: "B." });
        const tool = harness(kernel);
        expect(
            await tool.execute(
                {
                    action: "merge",
                    objectIds: ["mem_a", "mem_b"],
                    category: "REJECTED_APPROACH",
                    antiMemory: {
                        trigger: "merging",
                        rejectedStrategy: "merging positives",
                        rejectionReason: "positives are not rejections",
                    },
                },
                "call-merge-positive-to-anti",
            ),
        ).toBe("Error: merge cannot fold positive memories into an anti-memory survivor");
        expect(tool.transport.methods()).not.toContain("kernel.commit");
    });

    test("the merge survivor carries the strictest predecessor sensitivity", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "ARCHITECTURE", summary: "A." });
        kernel.seedDecision({
            object_id: "mem_b",
            decision_kind: "ARCHITECTURE",
            summary: "B.",
            sensitivity: "sensitive",
        });
        const tool = harness(kernel);
        const merged = parseJson<CommitJson>(
            await tool.execute(
                { action: "merge", objectIds: ["mem_a", "mem_b"], content: "A and B." },
                "call-merge-sensitive",
            ),
        );
        expect(merged.outcome).toBe("applied");
        expect(kernel.objects.get(merged.objectId as string)?.sensitivity).toBe("sensitive");
        const commit = tool.transport.calls.find((call) => call.method === "kernel.commit")
            ?.body as { operations: Array<{ spec: { sensitivity?: string } }> };
        expect(commit.operations.every((op) => op.spec.sensitivity === "sensitive")).toBeTrue();
    });

    test("revise inherits the predecessor's sensitivity instead of resetting to normal", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_s",
            decision_kind: "ARCHITECTURE",
            summary: "S.",
            sensitivity: "sensitive",
        });
        const tool = harness(kernel);
        const revised = parseJson<CommitJson>(
            await tool.execute(
                { action: "revise", objectId: "mem_s", content: "S2." },
                "call-revise-sensitive",
            ),
        );
        expect(revised.outcome).toBe("applied");
        expect(kernel.objects.get(revised.objectId as string)?.sensitivity).toBe("sensitive");
    });
});

describe("eidnara_memory domain fence and lineage", () => {
    test("a decision outside the memory domain is invisible and cannot be archived", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_notes",
            decision_kind: "ARCHITECTURE",
            summary: "Notes.",
            domain_id: "notes",
        });
        const tool = harness(kernel);
        const got = parseJson<ReadJson>(
            await tool.execute({ action: "get", objectIds: ["mem_notes"] }, "call-get-domain"),
        );
        expect(got.memories).toEqual([]);
        expect(got.missingObjectIds).toEqual(["mem_notes"]);
        expect(
            await tool.execute({ action: "archive", objectId: "mem_notes" }, "call-archive-domain"),
        ).toBe("Error: memory not found or not visible from this project: mem_notes");
        // Archive's replay probe reaches the daemon, but its snapshot-0 token trips the token conflict before the envelope can mutate the out-of-domain row.
        const probe = tool.transport.calls.find((call) => call.method === "kernel.commit");
        expect(probe?.body).toMatchObject({
            tokens: [{ object_id: "mem_notes", known_as_of: 0 }],
        });
        expect(kernel.objects.get("mem_notes")?.invalidated_commit_seq).toBeNull();
    });

    test("a memory-domain row with a legacy category reads and archives", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_legacy",
            decision_kind: "USER_DIRECTIVES",
            summary: "Legacy.",
            source_id: "claim-lane-import",
            source_kind: "model",
        });
        const tool = harness(kernel);
        const got = parseJson<ReadJson>(
            await tool.execute({ action: "get", objectIds: ["mem_legacy"] }, "call-get-legacy"),
        );
        expect(got.memories).toEqual([
            expect.objectContaining({ objectId: "mem_legacy", category: "USER_DIRECTIVES" }),
        ]);
        const archived = parseJson<CommitJson>(
            await tool.execute(
                { action: "archive", objectId: "mem_legacy" },
                "call-archive-legacy",
            ),
        );
        expect(archived.outcome).toBe("applied");
        expect(kernel.objects.get("mem_legacy")?.invalidated_commit_seq).not.toBeNull();
    });

    test("revise of a history_summarizer-promoted memory sends the predecessor's lineage", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_hist",
            decision_kind: "ARCHITECTURE",
            summary: "H.",
            source_id: "history_summarizer",
            source_kind: "model",
        });
        const tool = harness(kernel);
        const revised = parseJson<CommitJson>(
            await tool.execute(
                { action: "revise", objectId: "mem_hist", content: "H2." },
                "call-revise-hist",
            ),
        );
        expect(revised.outcome).toBe("applied");
        const survivor = revised.objects.find((id) => id !== "mem_hist") as string;
        const commit = tool.transport.calls.find((call) => call.method === "kernel.commit")
            ?.body as { source_kind?: string };
        expect(commit.source_kind).toBe("model");
        expect(kernel.objects.get(survivor)).toMatchObject({
            source_id: "history_summarizer",
            source_kind: "model",
            source_revision: 2,
        });
    });

    test("merge across differing lineages is refused before any commit", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_own",
            decision_kind: "ARCHITECTURE",
            summary: "O.",
        });
        kernel.seedDecision({
            object_id: "mem_hist",
            decision_kind: "ARCHITECTURE",
            summary: "H.",
            source_id: "history_summarizer",
            source_kind: "model",
        });
        const tool = harness(kernel);
        const text = await tool.execute(
            { action: "merge", objectIds: ["mem_own", "mem_hist"], content: "OH." },
            "call-merge-mixed",
        );
        expect(text).toContain("Merge same-lineage memories only");
        expect(tool.transport.methods()).not.toContain("kernel.commit");
        expect(kernel.liveRows()).toHaveLength(2);
    });
});

describe("eidnara_memory anti-memory", () => {
    test("an omitted or explicit null expiry defaults the anti-memory horizon, and the omitted create replays byte-identically", async () => {
        const payload = {
            trigger: "Choosing a cache backend",
            rejectedStrategy: "Use Redis",
            rejectionReason: "The project must work offline",
        };
        const ninetyDays = 90 * 24 * 60 * 60 * 1_000;
        const day = 24 * 60 * 60 * 1_000;
        const tool = harness();
        for (const [callId, antiMemory] of [
            ["call-anti-ttl", payload],
            ["call-anti-null-ttl", { ...payload, expiresAt: null }],
        ] as const) {
            const created = parseJson<CommitJson>(
                await tool.execute(
                    { action: "create", category: "REJECTED_APPROACH", antiMemory },
                    callId,
                ),
            );
            const objectId = created.objects[0] as string;
            const summary = tool.kernel.objects.get(objectId)?.decision?.payload.summary ?? "";
            const match = summary.match(/^Expires at: (\d+)$/m);
            expect(match, callId).not.toBeNull();
            const expiresAt = Number(match?.[1]);
            expect(expiresAt, callId).toBeGreaterThanOrEqual(Date.now() + ninetyDays - day);
            expect(expiresAt, callId).toBeLessThanOrEqual(Date.now() + ninetyDays + day);
        }

        const replayed = parseJson<CommitJson>(
            await tool.execute(
                { action: "create", category: "REJECTED_APPROACH", antiMemory: payload },
                "call-anti-ttl",
            ),
        );
        expect(replayed.outcome).toBe("already applied");
        expect(tool.kernel.liveRows()).toHaveLength(2);
    });

    test("get reads an expired anti-memory as missing, like search does", async () => {
        const tool = harness();
        const expired = {
            trigger: "Choosing a cache backend",
            rejectedStrategy: "Use Redis",
            rejectionReason: "The project must work offline",
            expiresAt: Date.now() - 1_000,
        };
        const created = parseJson<CommitJson>(
            await tool.execute(
                reduced({ action: "create", category: "REJECTED_APPROACH", antiMemory: expired }),
                "call-anti-expired",
            ),
        );
        const objectId = created.objects[0] as string;
        const got = parseJson<ReadJson>(
            await tool.execute({ action: "get", objectIds: [objectId] }, "call-anti-expired-get"),
        );
        expect(got.memories).toHaveLength(0);
        expect(got.missingObjectIds).toEqual([objectId]);
    });

    test("creates typed anti-memory, reads it back parsed, and rejects cross-arm shapes", async () => {
        const tool = harness();
        const payload = {
            trigger: "Choosing a cache backend",
            rejectedStrategy: "Use Redis",
            rejectionReason: "The project must work offline",
            saferAlternative: "Use SQLite",
        };
        const created = parseJson<CommitJson>(
            await tool.execute(
                reduced({ action: "create", category: "REJECTED_APPROACH", antiMemory: payload }),
                "call-anti-create",
            ),
        );
        const objectId = created.objects[0] as string;
        const got = parseJson<ReadJson>(
            await tool.execute({ action: "get", objectIds: [objectId] }, "call-anti-get"),
        );
        expect(got.memories[0]).toMatchObject({
            category: "REJECTED_APPROACH",
            antiMemory: expect.objectContaining(payload),
        });
        for (const [callId, args] of [
            ["call-anti-missing", { action: "create", category: "REJECTED_APPROACH" }],
            [
                "call-positive-payload",
                { action: "create", category: "ARCHITECTURE", content: "p", antiMemory: payload },
            ],
            ["call-bad-category", { action: "create", category: "arbitrary", content: "x" }],
        ] as const) {
            expect(await tool.execute(args, callId)).toContain("Error:");
        }
        expect(tool.kernel.liveRows()).toHaveLength(1);
    });

    test("revise of an anti-memory without a replacement inherits the parsed payload", async () => {
        const tool = harness();
        const payload = {
            trigger: "Choosing a cache backend",
            rejectedStrategy: "Use Redis",
            rejectionReason: "The project must work offline",
        };
        const created = parseJson<CommitJson>(
            await tool.execute(
                { action: "create", category: "REJECTED_APPROACH", antiMemory: payload },
                "call-anti-create-revise",
            ),
        );
        const objectId = created.objects[0] as string;
        const revised = parseJson<CommitJson>(
            await tool.execute(
                { action: "revise", objectId, reason: "still true" },
                "call-anti-revise",
            ),
        );
        expect(revised.outcome).toBe("applied");
        const survivor = revised.objectId as string;
        const got = parseJson<ReadJson>(
            await tool.execute({ action: "get", objectIds: [survivor] }, "call-anti-revise-get"),
        );
        expect(got.memories[0]).toMatchObject({
            category: "REJECTED_APPROACH",
            antiMemory: expect.objectContaining(payload),
        });
    });

    test("a redelivered anti-memory revise without an expiry replays across a day boundary", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_anti_predecessor",
            decision_kind: "REJECTED_APPROACH",
            summary: "Trigger: caching\nRejected strategy: Redis\nWhy rejected: offline builds",
        });
        const tool = harness(kernel);
        const args = {
            action: "revise",
            objectId: "mem_anti_predecessor",
            category: "REJECTED_APPROACH",
            reason: "sharpened",
            antiMemory: {
                trigger: "Choosing a cache backend",
                rejectedStrategy: "Use Redis",
                rejectionReason: "The project must work offline",
            },
        };
        try {
            setSystemTime(new Date("2026-01-01T12:00:00Z"));
            const first = parseJson<CommitJson>(await tool.execute(args, "call-anti-revise-day"));
            expect(first.outcome).toBe("applied");
            // The generated expiry re-renders day-aligned two days later, so only the stored-expiry comparison recognizes the committed successor.
            setSystemTime(new Date("2026-01-03T12:00:00Z"));
            const second = parseJson<CommitJson>(await tool.execute(args, "call-anti-revise-day"));
            expect(second).toMatchObject({
                outcome: "already applied",
                objectId: first.objectId,
            });
            expect(kernel.liveRows()).toHaveLength(1);
        } finally {
            setSystemTime();
        }
    });

    test("revise of an anti-memory whose summary does not parse names the input error", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_anti_broken",
            decision_kind: "REJECTED_APPROACH",
            summary: "free prose that is not an anti-memory payload",
        });
        const tool = harness(kernel);
        const text = await tool.execute(
            { action: "revise", objectId: "mem_anti_broken" },
            "call-anti-broken",
        );
        expect(text).toBe(
            "Error: the anti-memory being replaced has an unparseable stored payload; pass a full antiMemory payload to replace it",
        );
        expect(kernel.objects.get("mem_anti_broken")?.invalidated_commit_seq).toBeNull();
    });

    test("merge inherits category and reason from the caller's first target, not store order", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_first_in_store",
            decision_kind: "ARCHITECTURE",
            summary: "Stored earlier.",
            rationale: "earlier why",
        });
        kernel.seedDecision({
            object_id: "mem_second_in_store",
            decision_kind: "ARCHITECTURE",
            summary: "Stored later.",
            rationale: "why",
        });
        const tool = harness(kernel);
        const merged = parseJson<CommitJson>(
            await tool.execute(
                {
                    action: "merge",
                    objectIds: ["mem_second_in_store", "mem_first_in_store"],
                    content: "Stored, combined.",
                },
                "call-merge-order",
            ),
        );
        const survivor = kernel.objects.get(merged.objectId as string);
        expect(survivor?.decision?.decision_kind).toBe("ARCHITECTURE");
        expect(survivor?.decision?.payload.summary).toBe("Stored, combined.");
        expect(survivor?.decision?.payload.rationale).toBe("why");
    });

    test("merge without survivor content is rejected before any target is retired", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({ object_id: "mem_a", decision_kind: "NAMING", summary: "A." });
        kernel.seedDecision({ object_id: "mem_b", decision_kind: "NAMING", summary: "B." });
        const tool = harness(kernel);
        const text = await tool.execute(
            { action: "merge", objectIds: ["mem_a", "mem_b"] },
            "call-merge-no-content",
        );
        expect(text).toStartWith(
            "Error: merge requires content (with category) or antiMemory for the survivor",
        );
        expect(kernel.liveRows()).toHaveLength(2);
        expect(tool.transport.calls.some((call) => call.method === "kernel.commit")).toBeFalse();
    });
});

describe("eidnara_memory human authority", () => {
    test("agent approve and enforce actions are rejected", async () => {
        const tool = harness();
        expect(await tool.execute({ action: "approve" }, "call-approve")).toContain(
            "human-host-owned",
        );
        expect(await tool.execute({ action: "enforce" }, "call-enforce")).toContain(
            "human-host-owned",
        );
        expect(await tool.execute({ action: "delete" }, "call-delete")).toContain("not allowed");
    });
});

describe("eidnara_memory action allowlist", () => {
    function toolWith(allowedActions: EidnaraMemoryAction[] | undefined) {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_allow",
            decision_kind: "ARCHITECTURE",
            summary: "Allowlist probe.",
        });
        const transport = new FakeKernelTransport(kernel);
        const definition = createEidnaraMemoryTools({
            kernelClient: ({ sessionId, projectRoot }) =>
                new KernelClient({ transport, enabled: true, sessionId, projectRoot }),
            resolveProjectPath: () => PROJECT,
            ...(allowedActions === undefined ? {} : { allowedActions }),
        }).eidnara_memory;
        const execute = (args: Record<string, unknown>, callID: string) =>
            definition.execute(
                args as never,
                { sessionID: SESSION, directory: ROOT, callID, agent: "primary" } as never,
            ) as Promise<string>;
        return { execute, transport };
    }

    test("an explicitly empty allowlist admits no action and sends nothing to the daemon", async () => {
        const tool = toolWith([]);
        for (const action of EIDNARA_MEMORY_ACTIONS) {
            const text = await tool.execute(
                { action, objectIds: ["mem_allow"], category: "ARCHITECTURE", content: "x" },
                `call-${action}`,
            );
            expect(text).toBe(`Error: Action '${action}' is not allowed in this context.`);
        }
        expect(tool.transport.calls).toHaveLength(0);
    });

    test("an omitted allowlist admits the default actions, which exclude dispute", async () => {
        const tool = toolWith(undefined);
        expect(
            await tool.execute(
                {
                    action: "dispute",
                    objectId: "mem_allow",
                    event: "mark_disputed",
                    evidenceId: "e",
                },
                "call-dispute-default",
            ),
        ).toBe("Error: Action 'dispute' is not allowed in this context.");
        expect(tool.transport.calls).toHaveLength(0);
    });

    test("an omitted allowlist admits every default action", async () => {
        const tool = toolWith(undefined);
        const text = await tool.execute(
            { action: "get", objectIds: ["mem_allow"] },
            "call-get-default",
        );
        expect(text.startsWith("Error:")).toBeFalse();
        expect(text).toContain("mem_allow");
    });

    test("a partial allowlist admits only the listed actions", async () => {
        const tool = toolWith(["get"]);
        expect(
            await tool.execute(
                { action: "create", category: "ARCHITECTURE", content: "x" },
                "call-create-denied",
            ),
        ).toBe("Error: Action 'create' is not allowed in this context.");
        const got = await tool.execute({ action: "get", objectIds: ["mem_allow"] }, "call-get-ok");
        expect(got.startsWith("Error:")).toBeFalse();
    });
});

describe("eidnara_memory response byte budget", () => {
    const bigContent = (seed: string) => seed.repeat(8 * 1024);

    test("get keeps leading ids complete and elides trailing ids by name", async () => {
        const kernel = new FakeKernel();
        for (const suffix of ["a", "b", "c"]) {
            kernel.seedDecision({
                object_id: `mem_big_${suffix}`,
                decision_kind: "ARCHITECTURE",
                summary: bigContent(suffix),
            });
        }
        const tool = harness(kernel);
        const got = parseJson<ReadJson & { elidedObjectIds?: string[]; elisionNote?: string }>(
            await tool.execute(
                { action: "get", objectIds: ["mem_big_a", "mem_big_b", "mem_big_c"] },
                "call-get-big",
            ),
        );
        // The daemon serves rows newest first, so `mem_big_c` leads and the older two are elided by name.
        expect(got.memories).toEqual([
            expect.objectContaining({ objectId: "mem_big_c", content: bigContent("c") }),
        ]);
        expect(got.elidedObjectIds).toEqual(["mem_big_b", "mem_big_a"]);
        expect(got.elisionNote).toContain("2 elided ids");
        expect(got.missingObjectIds).toEqual([]);
    });

    test("get bounds a single id whose row alone exceeds the budget", async () => {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_huge",
            decision_kind: "ARCHITECTURE",
            summary: "x".repeat(64 * 1024),
        });
        const tool = harness(kernel);
        const got = parseJson<ReadJson & { elidedObjectIds?: string[] }>(
            await tool.execute({ action: "get", objectIds: ["mem_huge"] }, "call-get-huge"),
        );
        expect(got.memories).toHaveLength(1);
        expect(got.memories[0].content.endsWith("… [truncated]")).toBeTrue();
        expect(got.memories[0].content.length).toBeLessThan(2 * 1024);
        expect(got.elidedObjectIds).toBeUndefined();
    });
});

describe("eidnara_memory dispute", () => {
    const OID = "0123456789abcdef0123456789abcdef01234567";

    function disputeHarness(
        allowedActions: readonly EidnaraMemoryAction[] = EIDNARA_MEMORY_ACTIONS,
        transportFor: (kernel: FakeKernel) => FakeKernelTransport = (kernel) =>
            new FakeKernelTransport(kernel),
    ) {
        const kernel = new FakeKernel();
        kernel.seedDecision({
            object_id: "mem_labeled",
            decision_kind: "ARCHITECTURE",
            summary: "l",
        });
        kernel.seedDecision({
            object_id: "mem_verified",
            decision_kind: "ARCHITECTURE",
            summary: "v",
            labeled: false,
        });
        kernel.seedDecision({
            object_id: "mem_quarantined",
            decision_kind: "ARCHITECTURE",
            summary: "q",
            disposition: "quarantined",
        });
        const transport = transportFor(kernel);
        const definition = createEidnaraMemoryTools({
            kernelClient: ({ sessionId, projectRoot }) =>
                new KernelClient({ transport, enabled: true, sessionId, projectRoot }),
            resolveProjectPath: () => PROJECT,
            allowedActions: [...allowedActions],
        }).eidnara_memory;
        const execute = (
            args: Record<string, unknown>,
            callID: string | undefined,
            agent = "primary",
        ) =>
            definition.execute(
                args as never,
                { sessionID: SESSION, directory: ROOT, callID, agent } as never,
            ) as Promise<string>;
        return { definition, execute, transport, kernel };
    }

    const byId = {
        action: "dispute",
        objectId: "mem_labeled",
        event: "mark_disputed",
        evidenceId: "ev-1",
    };

    function commits(transport: FakeKernelTransport) {
        return transport.calls
            .filter((call) => call.method === "kernel.commit")
            .map((call) => call.body as Record<string, any>);
    }

    test("is a gated mutation: tool-call identity, agent, and allowlist", async () => {
        const h = disputeHarness();
        expect(await h.execute(byId, undefined)).toContain("requires a stable tool-call identity");
        expect(await h.execute(byId, "call-r", CONTEXT_RESEARCHER_AGENT)).toContain(
            "not available to the context_researcher agent",
        );
        const narrowed = disputeHarness(["get"]);
        expect(await narrowed.execute(byId, "call-n")).toBe(
            "Error: Action 'dispute' is not allowed in this context.",
        );
        expect(h.transport.calls).toHaveLength(0);
        expect(narrowed.transport.calls).toHaveLength(0);
        expect(EIDNARA_MEMORY_DEFAULT_ACTIONS).not.toContain("dispute");
        for (const forbidden of ["approve", "enforce"]) {
            expect(EIDNARA_MEMORY_ACTIONS as readonly string[]).not.toContain(forbidden);
        }
    });

    test("is in the exposed schema exactly when the allowlist names it", () => {
        const shapeOf = (actions: readonly EidnaraMemoryAction[]) =>
            disputeHarness(actions).definition.args as Record<string, any>;
        const allowed = shapeOf(EIDNARA_MEMORY_ACTIONS);
        const defaulted = shapeOf(EIDNARA_MEMORY_DEFAULT_ACTIONS);
        expect(allowed.action.safeParse("dispute").success).toBeTrue();
        expect(defaulted.action.safeParse("dispute").success).toBeFalse();
        for (const field of ["event", "evidenceId", "path", "gitObjectId"]) {
            expect(Object.hasOwn(allowed, field)).toBeTrue();
            expect(Object.hasOwn(defaulted, field)).toBeFalse();
        }
        expect(disputeHarness(EIDNARA_MEMORY_ACTIONS).definition.description).toContain("dispute:");
        expect(disputeHarness(EIDNARA_MEMORY_DEFAULT_ACTIONS).definition.description).not.toContain(
            "dispute:",
        );
    });

    test("sends the evidence under the tool identity, actor, cause, and assistant source kind", async () => {
        const h = disputeHarness();
        const text = await h.execute(byId, "call-1");
        expect(JSON.parse(text)).toEqual({
            action: "dispute",
            outcome: "applied",
            objectId: "mem_labeled",
            event: "mark_disputed",
            previousDisposition: "active",
            disposition: "disputed",
            commitSeq: expect.any(Number),
            replayed: false,
            visibilityChanged: false,
        });
        const [preview, commit] = commits(h.transport);
        for (const body of [preview, commit]) {
            expect(body.operations).toEqual([
                {
                    op: "disposition",
                    object_id: "mem_labeled",
                    event: "mark_disputed",
                    evidence_id: "ev-1",
                },
            ]);
            expect(body.source_kind).toBe("assistant");
            expect(body.asserted_source_class).toBeUndefined();
            expect(body.asserted_taint_class).toBeUndefined();
            expect(body.intent).toMatchObject({
                actor: "agent:opencode",
                cause: "eidnara_memory dispute call-1",
            });
        }
        expect(preview.preview).toBeTrue();

        const byFile = await h.execute(
            {
                action: "dispute",
                objectId: "mem_verified",
                event: "contradict",
                path: "src/a.rs",
                gitObjectId: OID,
            },
            "call-2",
        );
        expect(JSON.parse(byFile)).toEqual({
            action: "dispute",
            outcome: "applied",
            objectId: "mem_verified",
            event: "contradict",
            previousDisposition: "active",
            disposition: "contradicted",
            commitSeq: expect.any(Number),
            replayed: false,
            visibilityChanged: true,
        });
        expect(commits(h.transport).at(-1)?.operations[0].evidence_ref).toEqual({
            path: "src/a.rs",
            git_object_id: OID,
        });
    });

    function callWithAbort(
        h: ReturnType<typeof disputeHarness>,
        signal: AbortSignal,
        callID: string,
    ) {
        return h.definition.execute(
            byId as never,
            {
                sessionID: SESSION,
                directory: ROOT,
                callID,
                agent: "primary",
                abort: signal,
            } as never,
        ) as Promise<string>;
    }

    test("an aborted tool call sends nothing", async () => {
        const h = disputeHarness();
        const controller = new AbortController();
        controller.abort();
        const text = await callWithAbort(h, controller.signal, "call-abort");
        expect(text).toStartWith("Error: dispute refused at preview");
        expect(h.transport.calls).toHaveLength(0);
        expect(h.kernel.receipts.size).toBe(0);
    });

    test("an abort while the commit is unbound stops the rebind and the resend", async () => {
        const controller = new AbortController();
        class UnboundCommitTransport extends FakeKernelTransport {
            private unbound = false;
            override async call(args: Parameters<FakeKernelTransport["call"]>[0]) {
                const body = args.body as { preview?: boolean };
                if (args.method === "kernel.commit" && body.preview !== true && !this.unbound) {
                    this.unbound = true;
                    this.calls.push(args);
                    controller.abort();
                    throw new HostCallError("terminal", "no binding", "route_unbound");
                }
                return super.call(args);
            }
        }
        const h = disputeHarness(
            EIDNARA_MEMORY_ACTIONS,
            (kernel) => new UnboundCommitTransport(kernel),
        );
        const text = await callWithAbort(h, controller.signal, "call-unbound");
        expect(text).toStartWith("Error: dispute refused at commit");
        expect(h.transport.rebinds).toBe(0);
        expect(commits(h.transport).filter((body) => body.preview !== true)).toHaveLength(1);
        expect(h.kernel.receipts.size).toBe(0);
        expect(h.kernel.objects.get("mem_labeled")?.disposition).toBe("active");
    });

    test("never relaxes a disposition through the memory's stored approval", async () => {
        const h = disputeHarness();
        h.kernel.seedApproval("approval-1");
        const held = h.kernel.seedDecision({
            object_id: "mem_held",
            decision_kind: "ARCHITECTURE",
            summary: "h",
            disposition: "quarantined",
        });
        held.approval_object_id = "approval-1";
        const text = await h.execute({ ...byId, objectId: "mem_held" }, "call-held");
        expect(JSON.parse(text)).toEqual({
            action: "dispute",
            outcome: "denied",
            objectId: "mem_held",
            event: "mark_disputed",
            previousDisposition: "quarantined",
            disposition: "quarantined",
        });
        expect(commits(h.transport).filter((body) => body.preview !== true)).toHaveLength(0);
        expect(h.kernel.objects.get("mem_held")?.disposition).toBe("quarantined");
    });

    test("a retry with the same tool-call id replays; a new tool-call id is a second write", async () => {
        const h = disputeHarness();
        const first = JSON.parse(await h.execute(byId, "call-a"));
        const again = JSON.parse(await h.execute(byId, "call-a"));
        expect(again).toEqual({
            action: "dispute",
            outcome: "applied",
            objectId: "mem_labeled",
            event: "mark_disputed",
            previousDisposition: first.previousDisposition,
            disposition: first.disposition,
            commitSeq: first.commitSeq,
            replayed: true,
        });
        const receipts = h.kernel.receipts.size;
        const second = JSON.parse(await h.execute(byId, "call-b"));
        expect(second.replayed).toBeFalse();
        expect(h.kernel.receipts.size).toBe(receipts + 1);
    });

    test("refuses missing evidence and unsupported events before sending; renders denials and refusals", async () => {
        const h = disputeHarness();
        for (const args of [
            { action: "dispute", objectId: "mem_labeled", event: "mark_disputed" },
            { action: "dispute", objectId: "mem_labeled", event: "mark_disputed", evidenceId: "" },
            {
                action: "dispute",
                objectId: "mem_labeled",
                event: "mark_disputed",
                path: "",
                gitObjectId: "",
            },
            {
                action: "dispute",
                objectId: "mem_labeled",
                event: "mark_disputed",
                path: "src/a.rs",
            },
            { action: "dispute", event: "mark_disputed", evidenceId: "ev" },
        ]) {
            expect(await h.execute(args, "call-usage")).toStartWith(
                "Error: dispute needs objectId",
            );
        }
        expect(await h.execute({ ...byId, event: "quarantine" }, "call-event")).toStartWith(
            "Error: dispute event 'quarantine' is not supported",
        );
        expect(h.transport.calls).toHaveLength(0);

        const denied = JSON.parse(
            await h.execute({ ...byId, objectId: "mem_quarantined" }, "call-denied"),
        );
        expect(denied).toMatchObject({ outcome: "denied", disposition: "quarantined" });
        const refused = await h.execute(
            {
                action: "dispute",
                objectId: "mem_labeled",
                event: "contradict",
                path: "src/a.rs",
                gitObjectId: OID.toUpperCase(),
            },
            "call-refused",
        );
        expect(refused).toStartWith("Error: dispute refused at preview: ");
        expect(refused).toContain("(invalid:invalid_input)");
    });
});

describe("eidnara_memory candidates", () => {
    const ITEM = {
        evidence_id: "srcev:abc",
        decision_object_id: "mem_labeled",
        decision_id: "dec_labeled",
        matched_terms: 3,
        decision_terms: 4,
        score: 7500,
        publish_tip: 41,
        created_at_ms: 1_700_000_000_000,
    };

    function candidatesHarness(
        replies: unknown[],
        allowedActions?: readonly EidnaraMemoryAction[],
    ) {
        const calls: { method: string; body: Record<string, unknown> }[] = [];
        const transport = {
            connectionFileExists: () => true,
            ensureRoute: async () => {},
            call: async (call: { method: string; body: unknown }) => {
                calls.push({ method: call.method, body: call.body as Record<string, unknown> });
                return replies.shift();
            },
        };
        const definition = createEidnaraMemoryTools({
            kernelClient: ({ sessionId, projectRoot }) =>
                new KernelClient({ transport, enabled: true, sessionId, projectRoot }),
            resolveProjectPath: () => PROJECT,
            ...(allowedActions === undefined ? {} : { allowedActions: [...allowedActions] }),
        }).eidnara_memory;
        const execute = (args: Record<string, unknown>) =>
            definition.execute(
                args as never,
                {
                    sessionID: SESSION,
                    directory: ROOT,
                    callID: undefined,
                    agent: "primary",
                } as never,
            ) as Promise<string>;
        return { definition, execute, calls };
    }

    test("is a default read: admitted without a tool-call identity and present in the schema", async () => {
        const h = candidatesHarness([{ kind: "page", as_of: 41, items: [ITEM], next: null }]);
        const action = (h.definition.args as Record<string, any>).action;
        expect(action.safeParse("candidates").success).toBe(true);
        const text = await h.execute({ action: "candidates", objectId: "mem_labeled" });
        const parsed = JSON.parse(text);
        expect(h.calls).toEqual([
            {
                method: "review.candidates",
                body: expect.objectContaining({
                    method: "review.candidates",
                    decision_object_id: "mem_labeled",
                    after: null,
                }),
            },
        ]);
        expect(parsed.asOf).toBe(41);
        expect(parsed.next).toBeNull();
        expect(parsed.note).toContain("lexical overlap");
        expect(parsed.note).toContain("not a judgment");
        expect(parsed.candidates).toEqual([
            {
                evidenceId: "srcev:abc",
                decisionId: "dec_labeled",
                score: 7500,
                matchedTerms: 3,
                decisionTerms: 4,
                publishTip: 41,
                createdAtMs: 1_700_000_000_000,
                confirm: {
                    action: "dispute",
                    event: "mark_disputed",
                    objectId: "mem_labeled",
                    evidenceId: "srcev:abc",
                },
            },
        ]);
    });

    test("pages with after and limit and renders each terminal by name", async () => {
        const h = candidatesHarness([
            { kind: "page", as_of: 9, items: [], next: "7500.1.61" },
            { kind: "terminal", terminal: "disabled" },
            { kind: "terminal", terminal: "not_live" },
            { kind: "terminal", terminal: "store_unavailable" },
            { state: { kind: "unavailable", reason: "store_starting" } },
        ]);
        const first = JSON.parse(
            await h.execute({ action: "candidates", objectId: "m", after: "7500.2.62", limit: 5 }),
        );
        expect(h.calls[0].body).toEqual(expect.objectContaining({ after: "7500.2.62", limit: 5 }));
        expect(first).toEqual(
            expect.objectContaining({ candidates: [], next: "7500.1.61", asOf: 9 }),
        );
        expect(await h.execute({ action: "candidates", objectId: "m" })).toBe(
            "Contradiction candidates are off in the daemon for this project.",
        );
        expect(await h.execute({ action: "candidates", objectId: "m" })).toContain("not_live");
        expect(await h.execute({ action: "candidates", objectId: "m" })).toContain(
            "store_unavailable",
        );
        const starting = await h.execute({ action: "candidates", objectId: "m" });
        expect(starting).toMatch(/^Error: /);
        expect(starting).toContain("while the store opens");
        expect(await h.execute({ action: "candidates" })).toBe(
            "Error: 'objectId' is required when action is 'candidates'.",
        );
    });

    test("refuses a page above the 64-item bound, an item whose score breaks the basis-point formula, an item past as_of, items out of reader order, and an id no row stores", async () => {
        const h = candidatesHarness([
            {
                kind: "page",
                as_of: 41,
                // Distinct ascending ids keep the page in reader order, so the bound is the check that refuses it.
                items: Array.from({ length: 65 }, (_, index) => ({
                    ...ITEM,
                    evidence_id: `srcev:${String(index).padStart(3, "0")}`,
                })),
                next: null,
            },
            { kind: "page", as_of: 41, items: [{ ...ITEM, score: 9999 }], next: null },
            {
                kind: "page",
                as_of: 41,
                items: [{ ...ITEM, matched_terms: 5, decision_terms: 4 }],
                next: null,
            },
            {
                kind: "page",
                as_of: 41,
                items: [{ ...ITEM, matched_terms: 0, decision_terms: 0, score: 0 }],
                next: null,
            },
            { kind: "page", as_of: 40, items: [ITEM], next: null },
            {
                kind: "page",
                as_of: 41,
                items: [
                    { ...ITEM, matched_terms: 2, score: 5000 },
                    { ...ITEM, evidence_id: "srcev:other" },
                ],
                next: null,
            },
            {
                kind: "page",
                as_of: 41,
                items: [
                    { ...ITEM, created_at_ms: 1 },
                    { ...ITEM, created_at_ms: 2 },
                ],
                next: null,
            },
            {
                kind: "page",
                as_of: 41,
                items: [
                    { ...ITEM, evidence_id: "srcev:b" },
                    { ...ITEM, evidence_id: "srcev:a" },
                ],
                next: null,
            },
            { kind: "page", as_of: 41, items: [{ ...ITEM, evidence_id: "" }], next: null },
            {
                kind: "page",
                as_of: 41,
                items: [{ ...ITEM, evidence_id: `srcev:${"x".repeat(507)}` }],
                next: null,
            },
            {
                kind: "page",
                as_of: 41,
                items: [{ ...ITEM, decision_id: "dec\u0001" }],
                next: null,
            },
        ]);
        for (const shape of [
            "65 items",
            "score 9999",
            "matched over decision",
            "zero terms",
            "publish_tip past as_of",
            "score ascending",
            "created_at_ms ascending within a score",
            "evidence_id descending within a tie",
            "empty evidence_id",
            "evidence_id past 512 bytes",
            "control character in decision_id",
        ]) {
            const text = await h.execute({ action: "candidates", objectId: "mem_labeled" });
            expect(text, shape).toMatch(/^Error: /);
            expect(text, shape).not.toContain("candidates");
        }
    });

    test("refuses a page whose item carries any key outside the documented set", async () => {
        const h = candidatesHarness([
            { kind: "page", as_of: 1, items: [{ ...ITEM, summary: "leaked text" }], next: null },
        ]);
        const text = await h.execute({ action: "candidates", objectId: "mem_labeled" });
        expect(text).not.toContain("leaked text");
        expect(text).toContain("does not recognize");
    });

    test("names who can file the confirm form from the allowlist", async () => {
        const page = { kind: "page", as_of: 41, items: [ITEM], next: null };
        const byDefault = candidatesHarness([page]);
        const note = JSON.parse(
            await byDefault.execute({ action: "candidates", objectId: "mem_labeled" }),
        ).note;
        expect(note).toContain("not admitted in this context");
        const admitted = candidatesHarness([page], ["candidates", "dispute"]);
        const parsed = JSON.parse(
            await admitted.execute({ action: "candidates", objectId: "mem_labeled" }),
        );
        expect(parsed.note).not.toContain("not admitted");
        expect(parsed.candidates[0].confirm.action).toBe("dispute");
        expect(note).not.toContain("File a confirm form");
        expect(byDefault.definition.description).not.toContain("independent evidence");
        expect(admitted.definition.description).toContain("independent evidence");
    });

    test("refuses a page naming another memory and paging fields the schema would refuse", async () => {
        const h = candidatesHarness([
            {
                kind: "page",
                as_of: 41,
                items: [{ ...ITEM, decision_object_id: "mem_other" }],
                next: null,
            },
        ]);
        const text = await h.execute({ action: "candidates", objectId: "mem_labeled" });
        expect(text).toBe(
            "Error: the daemon answered candidates for another memory than mem_labeled; nothing was rendered.",
        );
        expect(text).not.toContain("srcev:abc");
        expect(await h.execute({ action: "candidates", objectId: "m", limit: "5" })).toBe(
            "Error: 'limit' must be an integer from 1 to 64",
        );
        expect(await h.execute({ action: "candidates", objectId: "m", limit: 2.5 })).toBe(
            "Error: 'limit' must be an integer from 1 to 64",
        );
        expect(await h.execute({ action: "candidates", objectId: "m", after: 5 })).toBe(
            "Error: 'after' must be a string or null",
        );
        expect(await h.execute({ action: "candidates", objectId: "m", limit: 65 })).toBe(
            "Error: 'limit' must be an integer from 1 to 64",
        );
        expect(h.calls).toHaveLength(1);
    });
});

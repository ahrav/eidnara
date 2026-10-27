import { describe, expect, test } from "bun:test";
import { type DispositionPreview, deriveRequestDigest, KernelClient } from "./kernel-client";
import { FakeKernel, FakeKernelTransport } from "./kernel-client-testing/fake-kernel";
import {
    formatMemoryMarkOutcome,
    formatVisibilityDelta,
    MEMORY_MARK_USAGE,
    type MemoryMarkArgs,
    memoryMarkOperationId,
    parseMemoryMarkArgs,
    runMemoryMarkCommand,
} from "./memory-mark-command";

const PROJECT = "/tmp/memory-mark-project";
const SESSION = "session-mark";

function harness() {
    const kernel = new FakeKernel();
    const transport = new FakeKernelTransport(kernel);
    const client = new KernelClient({
        transport,
        enabled: true,
        sessionId: SESSION,
        projectRoot: PROJECT,
    });
    // A route-written memory: labeled on `explicit_search`, hidden on the automatic surfaces.
    kernel.seedDecision({
        object_id: "mem_labeled",
        decision_kind: "PROJECT_RULES",
        summary: "labeled",
        projectRoot: PROJECT,
    });
    // A verified memory: visible on every surface.
    kernel.seedDecision({
        object_id: "mem_verified",
        decision_kind: "PROJECT_RULES",
        summary: "verified",
        labeled: false,
        projectRoot: PROJECT,
    });
    return { kernel, transport, client };
}

const OID = "0123456789abcdef0123456789abcdef01234567";

/** A dispute cites a repository file, the evidence `mark_disputed` and `contradict` must carry. */
function args(event: MemoryMarkArgs["event"], objectId: string, confirmed = false): MemoryMarkArgs {
    return event === "mark_disputed" || event === "contradict"
        ? { event, objectId, evidenceRef: { path: "src/lib.rs", git_object_id: OID }, confirmed }
        : { event, objectId, confirmed };
}

/** A replayed identity answers its receipt without a preview, so it carries no visibility verdict. */
function replayOf(first: Awaited<ReturnType<typeof runMemoryMarkCommand>>) {
    if (first.kind !== "applied") throw new Error(first.kind);
    const { visibilityChanged: _judged, ...receipt } = first;
    return { ...receipt, replayed: true };
}

/** Confirmation is refused by default so a test that expects none fails loudly if it is asked. */
const UNEXPECTED_CONFIRMATION = async (): Promise<boolean> => {
    throw new Error("confirmation was not expected");
};

async function run(
    h: ReturnType<typeof harness>,
    input: MemoryMarkArgs,
    confirm: ((preview: DispositionPreview) => Promise<boolean>) | "none" = UNEXPECTED_CONFIRMATION,
    actor = "user:test",
) {
    return runMemoryMarkCommand({
        client: h.client,
        operationId: memoryMarkOperationId(SESSION, input),
        actor,
        args: input,
        ...(confirm === "none" ? {} : { confirm }),
    });
}

describe("parseMemoryMarkArgs", () => {
    test("accepts the wire event in either spelling and the confirm flag anywhere", () => {
        expect(parseMemoryMarkArgs("mark-stale mem_1")).toEqual({
            ok: true,
            args: { event: "mark_stale", objectId: "mem_1", confirmed: false },
        });
        expect(parseMemoryMarkArgs("--yes quarantine mem_1")).toEqual({
            ok: true,
            args: { event: "quarantine", objectId: "mem_1", confirmed: true },
        });
    });

    test("refuses a target disposition, a missing id, and a flag-shaped id", () => {
        expect(parseMemoryMarkArgs("stale mem_1")).toEqual({
            ok: false,
            message: `Unknown event "stale". ${MEMORY_MARK_USAGE}`,
        });
        expect(parseMemoryMarkArgs("mark_stale")).toEqual({
            ok: false,
            message: MEMORY_MARK_USAGE,
        });
        expect(parseMemoryMarkArgs("mark_stale --force").ok).toBe(false);
    });
});

describe("parseMemoryMarkArgs evidence", () => {
    test("a dispute takes --evidence, --path split at the last @, or both, in any order", () => {
        expect(parseMemoryMarkArgs("mark-disputed mem_1 --evidence ev:1/x")).toEqual({
            ok: true,
            args: {
                event: "mark_disputed",
                objectId: "mem_1",
                evidenceId: "ev:1/x",
                confirmed: false,
            },
        });
        expect(parseMemoryMarkArgs(`--path docs/a@b.md@${OID} contradict mem_1 --yes`)).toEqual({
            ok: true,
            args: {
                event: "contradict",
                objectId: "mem_1",
                evidenceRef: { path: "docs/a@b.md", git_object_id: OID },
                confirmed: true,
            },
        });
        expect(
            parseMemoryMarkArgs(`mark_disputed mem_1 --path src/a.rs@${OID} --evidence ev`),
        ).toMatchObject({
            ok: true,
            args: { evidenceId: "ev", evidenceRef: { path: "src/a.rs", git_object_id: OID } },
        });
        expect(parseMemoryMarkArgs(`mark_disputed mem_1 --path --notes.md@${OID}`)).toMatchObject({
            ok: true,
            args: { evidenceRef: { path: "--notes.md", git_object_id: OID } },
        });
    });

    test("a dispute without evidence, and evidence on another event, print usage", () => {
        for (const raw of [
            "mark_disputed mem_1",
            "contradict mem_1 --yes",
            "mark_stale mem_1 --evidence ev",
            `quarantine mem_1 --path src/a.rs@${OID}`,
        ]) {
            expect(parseMemoryMarkArgs(raw)).toEqual({ ok: false, message: MEMORY_MARK_USAGE });
        }
    });

    test("a flag without a value, a repeated flag, or a malformed path is refused", () => {
        for (const raw of [
            "mark_disputed mem_1 --evidence",
            "mark_disputed mem_1 --evidence --yes",
            "mark_disputed mem_1 --evidence a --evidence b",
            `mark_disputed mem_1 --path a@${OID} --path b@${OID}`,
        ]) {
            expect(parseMemoryMarkArgs(raw)).toEqual({ ok: false, message: MEMORY_MARK_USAGE });
        }
        for (const path of ["src/a.rs", `@${OID}`, "src/a.rs@", `src\u001fa.rs@${OID}`]) {
            const parsed = parseMemoryMarkArgs(`mark_disputed mem_1 --path ${path}`);
            expect(parsed).toEqual({
                ok: false,
                message: `Invalid evidence path "${path}". ${MEMORY_MARK_USAGE}`,
            });
        }
        expect(parseMemoryMarkArgs("mark_disputed mem_1 --evidence a\u001fb").ok).toBe(false);
    });

    test("the usage states the evidence requirement, the refusals, and the preview's limit", () => {
        for (const text of [
            "--evidence <id>",
            "--path <path>@<oid>",
            "mark_disputed and contradict cite evidence",
            "invalid_input",
            "not_found",
            "admission_policy",
            "class_over_declared",
            "operation_key_reused",
            "visibility only",
        ]) {
            expect(MEMORY_MARK_USAGE).toContain(text);
        }
    });
});

describe("runMemoryMarkCommand evidence", () => {
    test("the evidence rides the preview and the commit byte for byte under the user source kind", async () => {
        const h = harness();
        const input: MemoryMarkArgs = {
            event: "mark_disputed",
            objectId: "mem_labeled",
            evidenceId: "ev:café/1",
            evidenceRef: { path: "docs/a@b.md", git_object_id: OID },
            confirmed: false,
        };
        const outcome = await run(h, input);
        expect(outcome).toMatchObject({ kind: "applied", result: { disposition: "disputed" } });
        const operation = {
            op: "disposition",
            object_id: "mem_labeled",
            event: "mark_disputed",
            evidence_id: "ev:café/1",
            evidence_ref: { path: "docs/a@b.md", git_object_id: OID },
        };
        for (const call of h.transport.calls) {
            expect(call.body).toMatchObject({ source_kind: "user", operations: [operation] });
            expect(call.body.intent).toMatchObject({
                cause: `/eidnara-memory-mark mark_disputed mem_labeled --evidence ev:café/1 --path docs/a@b.md@${OID}`,
            });
        }
        expect(h.transport.calls).toHaveLength(2);
    });

    test("two disputes with different evidence in one session are two commits; the same evidence replays", async () => {
        const h = harness();
        const byId: MemoryMarkArgs = {
            event: "mark_disputed",
            objectId: "mem_labeled",
            evidenceId: "ev-a",
            confirmed: false,
        };
        const byFile = args("mark_disputed", "mem_labeled");
        expect(memoryMarkOperationId(SESSION, byId)).not.toBe(
            memoryMarkOperationId(SESSION, byFile),
        );
        expect(memoryMarkOperationId(SESSION, { ...byId, evidenceId: "ev-b" })).not.toBe(
            memoryMarkOperationId(SESSION, byId),
        );
        expect(
            memoryMarkOperationId(SESSION, {
                ...byFile,
                evidenceRef: { path: "src/lib.rs", git_object_id: "f".repeat(40) },
            }),
        ).not.toBe(memoryMarkOperationId(SESSION, byFile));
        expect(
            memoryMarkOperationId(SESSION, {
                ...byFile,
                evidenceRef: { path: "src/main.rs", git_object_id: OID },
            }),
        ).not.toBe(memoryMarkOperationId(SESSION, byFile));
        const first = await run(h, byId);
        const second = await run(h, byFile);
        expect(first).toMatchObject({ kind: "applied", replayed: false });
        expect(second).toMatchObject({ kind: "applied", replayed: false });
        expect(h.kernel.receipts.size).toBe(2);
        const again = await run(h, byId);
        expect(again).toEqual(replayOf(first));
        expect(h.kernel.receipts.size).toBe(2);
    });

    test("the commit sends the reference the preview judged when the caller edits its args during confirmation", async () => {
        const h = harness();
        const input: MemoryMarkArgs = {
            event: "mark_disputed",
            objectId: "mem_verified",
            evidenceRef: { path: "docs/a.md", git_object_id: OID },
            confirmed: false,
        };
        let asked = false;
        const outcome = await run(h, input, async () => {
            asked = true;
            (input.evidenceRef as { git_object_id: string }).git_object_id = "f".repeat(40);
            return true;
        });
        expect(asked).toBe(true);
        expect(outcome.kind).toBe("applied");
        expect(h.transport.calls.map((call) => call.body.operations[0].evidence_ref)).toEqual([
            { path: "docs/a.md", git_object_id: OID },
            { path: "docs/a.md", git_object_id: OID },
        ]);
    });

    test("the fake refuses the reference shapes the route refuses", async () => {
        for (const evidenceRef of [
            { path: "../a.rs", git_object_id: OID },
            { path: "/etc/passwd", git_object_id: OID },
            { path: "src/.Git/config", git_object_id: OID },
            { path: "double%2Fencoded", git_object_id: OID },
            { path: `${"a".repeat(1025)}`, git_object_id: OID },
            { path: Array(17).fill("d").join("/"), git_object_id: OID },
        ]) {
            const h = harness();
            const outcome = await run(h, {
                event: "mark_disputed",
                objectId: "mem_labeled",
                evidenceRef,
                confirmed: false,
            });
            expect(outcome).toMatchObject({ kind: "refused", step: "preview" });
        }
    });

    test("the fake accepts a path of 16 components, the route's confined-path limit", async () => {
        const h = harness();
        const outcome = await run(h, {
            event: "mark_disputed",
            objectId: "mem_labeled",
            evidenceRef: { path: Array(16).fill("d").join("/"), git_object_id: OID },
            confirmed: false,
        });
        expect(outcome).toMatchObject({ kind: "applied" });
    });

    test("the fake refuses evidence whose rendered reason exceeds the route's 4096-byte cap", async () => {
        // An unreserved byte renders as itself and `/` renders as `%2F`.
        const prefix =
            "v=1;op=kernel.commit;event=mark_disputed;reporter_taint=user_inferred;evidence_id=";
        const room = 4096 - prefix.length;
        for (const [evidenceId, applied] of [
            ["e".repeat(room), true],
            ["e".repeat(room + 1), false],
            ["/".repeat(Math.floor(room / 3) + 1), false],
        ] as const) {
            const h = harness();
            const outcome = await run(h, {
                event: "mark_disputed",
                objectId: "mem_labeled",
                evidenceId,
                confirmed: false,
            });
            expect(outcome).toMatchObject(
                applied ? { kind: "applied" } : { kind: "refused", step: "preview" },
            );
        }
    });

    test("a dispute the daemon refuses for its evidence is reported by name and writes nothing", async () => {
        const h = harness();
        const outcome = await run(h, {
            event: "contradict",
            objectId: "mem_labeled",
            evidenceRef: { path: "src/a.rs", git_object_id: OID.toUpperCase() },
            confirmed: false,
        });
        expect(outcome).toEqual({
            kind: "refused",
            step: "preview",
            state: { kind: "invalid", reason: "invalid_input" },
        });
        expect(h.kernel.receipts.size).toBe(0);
        expect(h.transport.methods()).toEqual(["kernel.commit"]);
    });
});

describe("runMemoryMarkCommand", () => {
    test("a tightening on an explicit-labeled object applies without confirmation", async () => {
        const h = harness();
        const outcome = await run(h, args("mark_stale", "mem_labeled"));
        expect(outcome).toEqual({
            kind: "applied",
            result: {
                object_id: "mem_labeled",
                event: "mark_stale",
                outcome: "deny",
                previous_disposition: "active",
                disposition: "stale",
                denied: false,
            },
            commitSeq: 3,
            replayed: false,
            visibilityChanged: false,
        });
        expect(h.kernel.objects.get("mem_labeled")?.disposition).toBe("stale");
        expect(h.transport.methods()).toEqual(["kernel.commit", "kernel.commit"]);
        expect(h.transport.calls[0]?.body).toMatchObject({ preview: true, tokens: [] });
        expect(h.transport.calls[1]?.body).not.toHaveProperty("preview");
    });

    test("the same command on a verified object asks first, and leaving it open writes nothing", async () => {
        const h = harness();
        const tip = h.kernel.tip;
        const outcome = await run(h, args("mark_stale", "mem_verified"), "none");
        expect(outcome.kind).toBe("needs_confirmation");
        expect(h.kernel.tip).toBe(tip);
        expect(h.kernel.receipts.size).toBe(0);
        expect(h.kernel.objects.get("mem_verified")?.disposition).toBe("active");
        if (outcome.kind !== "needs_confirmation") throw new Error(outcome.kind);
        expect(formatVisibilityDelta(outcome.preview)).toBe(
            [
                "- auto_inject: visible -> hidden",
                "- auto_search: visible -> hidden",
                "- explicit_search: visible -> labeled",
            ].join("\n"),
        );
        expect(formatMemoryMarkOutcome(outcome, args("mark_stale", "mem_verified"))).toContain(
            "/eidnara-memory-mark mark_stale mem_verified --yes",
        );
    });

    test("declining writes nothing; accepting applies and shows the receipt", async () => {
        const h = harness();
        const declined = await run(h, args("quarantine", "mem_verified"), async () => false);
        expect(declined.kind).toBe("declined");
        expect(h.kernel.receipts.size).toBe(0);
        expect(h.kernel.objects.get("mem_verified")?.disposition).toBe("active");

        const accepted = await run(h, args("quarantine", "mem_verified"), async () => true);
        expect(accepted).toMatchObject({
            kind: "applied",
            result: { disposition: "quarantined", outcome: "quarantine" },
            replayed: false,
        });
        expect(h.kernel.objects.get("mem_verified")?.disposition).toBe("quarantined");
        expect(formatMemoryMarkOutcome(accepted, args("quarantine", "mem_verified"))).toContain(
            "receipt #3",
        );
    });

    test("a relaxation without an approval is denied before anything is written", async () => {
        const h = harness();
        // Hiding a labeled row from `explicit_search` is a visibility change, so the flag confirms it.
        await run(h, args("quarantine", "mem_labeled", true));
        const receipts = h.kernel.receipts.size;
        const outcome = await run(h, args("mark_stale", "mem_labeled"));
        expect(outcome).toMatchObject({
            kind: "denied",
            result: { denied: true, disposition: "quarantined" },
        });
        expect(h.kernel.receipts.size).toBe(receipts);
        expect(h.kernel.objects.get("mem_labeled")?.disposition).toBe("quarantined");
        expect(formatMemoryMarkOutcome(outcome, args("mark_stale", "mem_labeled"))).toContain(
            "Denied",
        );
    });

    test("repeating a command with the same identity replays the receipt", async () => {
        const h = harness();
        const first = await run(h, args("mark_disputed", "mem_labeled"));
        const again = await run(h, args("mark_disputed", "mem_labeled"));
        expect(again).toEqual(replayOf(first));
        expect(h.kernel.receipts.size).toBe(1);
        expect(formatMemoryMarkOutcome(again, args("mark_disputed", "mem_labeled"))).toContain(
            "replayed receipt",
        );
    });

    test("a repeat after the object moved on still replays instead of being judged again", async () => {
        const h = harness();
        const first = await run(h, args("mark_stale", "mem_labeled"));
        expect(first.kind).toBe("applied");
        await run(h, args("quarantine", "mem_labeled", true));
        const receipts = h.kernel.receipts.size;
        // Judged fresh, `mark_stale` on a quarantined object would be a denied relaxation.
        const again = await run(h, args("mark_stale", "mem_labeled"));
        expect(again).toEqual(replayOf(first));
        expect(h.kernel.receipts.size).toBe(receipts);
        expect(h.kernel.objects.get("mem_labeled")?.disposition).toBe("quarantined");
    });

    test("a denial recorded by the commit itself is reported as denied", async () => {
        const h = harness();
        // The object is quarantined by another writer between the preview and the confirmed commit.
        const outcome = await run(h, args("mark_stale", "mem_verified"), async () => {
            const quarantine = await run(h, args("quarantine", "mem_verified", true));
            expect(quarantine.kind).toBe("applied");
            return true;
        });
        expect(outcome).toMatchObject({
            kind: "denied",
            result: { denied: true, disposition: "quarantined", event: "mark_stale" },
        });
        expect(formatMemoryMarkOutcome(outcome, args("mark_stale", "mem_verified"))).toContain(
            "Denied",
        );
        expect(h.kernel.objects.get("mem_verified")?.disposition).toBe("quarantined");
    });

    test("a session that ends after the preview commits nothing", async () => {
        const h = harness();
        let ended = false;
        const outcome = await runMemoryMarkCommand({
            client: h.client,
            operationId: memoryMarkOperationId(SESSION, args("mark_stale", "mem_labeled")),
            actor: "user:test",
            args: args("mark_stale", "mem_labeled"),
            isCancelled: () => ended,
            confirm: UNEXPECTED_CONFIRMATION,
        });
        expect(outcome.kind).toBe("applied");
        ended = true;
        const cancelled = await runMemoryMarkCommand({
            client: h.client,
            operationId: memoryMarkOperationId(SESSION, args("mark_disputed", "mem_labeled")),
            actor: "user:test",
            args: args("mark_disputed", "mem_labeled"),
            isCancelled: () => ended,
        });
        expect(cancelled).toEqual({
            kind: "refused",
            step: "commit",
            state: { kind: "cancelled" },
        });
        expect(h.transport.methods().filter((method) => method === "kernel.commit")).toHaveLength(
            3,
        );
        expect(h.kernel.objects.get("mem_labeled")?.disposition).toBe("stale");
    });

    test("an actor carrying the key separator is refused before any call", async () => {
        const h = harness();
        const outcome = await run(
            h,
            args("mark_stale", "mem_labeled"),
            UNEXPECTED_CONFIRMATION,
            "user\u001fx",
        );
        expect(outcome).toEqual({
            kind: "refused",
            step: "preview",
            state: { kind: "invalid", reason: "invalid_input" },
        });
        expect(h.transport.calls).toEqual([]);
    });

    test("a refused preview names the state and writes nothing", async () => {
        const h = harness();
        const outcome = await run(h, args("quarantine", "mem_missing"));
        expect(outcome).toEqual({
            kind: "refused",
            step: "preview",
            state: { kind: "invalid", reason: "not_found" },
        });
        expect(h.kernel.receipts.size).toBe(0);
        expect(formatMemoryMarkOutcome(outcome, args("quarantine", "mem_missing"))).toContain(
            "invalid:not_found",
        );
    });

    test("a verdict for another object or event never decides the one the user named", async () => {
        // A well-typed reply whose verdict names a different object would otherwise skip the prompt and commit the requested operation blind.
        const available = { kind: "available" } as const;
        const verdict = (object_id: string, event: MemoryMarkArgs["event"]) => ({
            object_id,
            event,
            outcome: "deny",
            previous_disposition: "active",
            disposition: "stale",
            denied: false,
        });
        const quiet = {
            auto_inject: "hidden",
            auto_search: "hidden",
            explicit_search: "labeled",
        } as const;
        const preview = (object_id: string, event: MemoryMarkArgs["event"]) => ({
            ...verdict(object_id, event),
            current: quiet,
            projected: quiet,
            visibility_changes: false,
        });
        let commits = 0;
        const stub = (previews: unknown[], dispositions: unknown[]) =>
            ({
                previewDispositions: async () => ({ state: available, known_as_of: 1, previews }),
                commit: async () => {
                    commits += 1;
                    return {
                        state: available,
                        receipt: { commit_seq: 2, replayed: false },
                        known_as_of: 2,
                        tokens: [],
                        merged: [],
                        dispositions,
                    };
                },
            }) as unknown as KernelClient;
        const base = {
            operationId: "op",
            actor: "user:test",
            args: args("mark_stale", "mem_a"),
            confirm: UNEXPECTED_CONFIRMATION,
        };
        const refused = {
            kind: "refused",
            step: "preview",
            state: { kind: "invalid", reason: "internal" },
        } as const;
        for (const previews of [
            [],
            [preview("mem_b", "mark_stale")],
            [preview("mem_a", "quarantine")],
            [preview("mem_a", "mark_stale"), preview("mem_b", "mark_stale")],
        ]) {
            const outcome = await runMemoryMarkCommand({ ...base, client: stub(previews, []) });
            expect(outcome).toEqual(refused);
        }
        expect(commits).toBe(0);

        const unknown = await runMemoryMarkCommand({
            ...base,
            client: stub([preview("mem_a", "mark_stale")], [verdict("mem_b", "mark_stale")]),
        });
        expect(unknown).toEqual({
            kind: "refused",
            step: "commit",
            state: { kind: "unavailable", reason: "outcome_unknown" },
        });
        expect(commits).toBe(1);
    });
});

describe("runMemoryMarkCommand cancellation", () => {
    test("the caller's signal reaches the preview and the commit", async () => {
        const h = harness();
        const controller = new AbortController();
        const input = args("contradict", "mem_verified", true);
        const outcome = await runMemoryMarkCommand({
            client: h.client,
            operationId: memoryMarkOperationId(SESSION, input),
            actor: "user:test",
            args: input,
            signal: controller.signal,
        });
        expect(outcome).toMatchObject({ kind: "applied", replayed: false });
        expect(h.transport.calls).toHaveLength(2);
        for (const call of h.transport.calls) expect(call.signal).toBe(controller.signal);
    });
});

describe("runMemoryMarkCommand approval authority", () => {
    function approvedQuarantine() {
        const h = harness();
        h.kernel.seedApproval("approval-1", PROJECT);
        const object = h.kernel.seedDecision({
            object_id: "mem_held",
            decision_kind: "PROJECT_RULES",
            summary: "held",
            disposition: "quarantined",
            projectRoot: PROJECT,
        });
        object.approval_object_id = "approval-1";
        return h;
    }

    test("an assistant-sourced dispute is denied a relaxation the stored approval would admit", async () => {
        const h = approvedQuarantine();
        const input = args("mark_disputed", "mem_held", true);
        const outcome = await runMemoryMarkCommand({
            client: h.client,
            operationId: memoryMarkOperationId(SESSION, input),
            actor: "agent:test",
            sourceKind: "assistant",
            args: input,
        });
        expect(outcome).toMatchObject({
            kind: "denied",
            result: {
                object_id: "mem_held",
                event: "mark_disputed",
                previous_disposition: "quarantined",
                disposition: "quarantined",
                denied: true,
            },
        });
        expect(
            h.transport.calls.map((call) => (call.body as { preview?: boolean }).preview),
        ).toEqual([true]);
        expect(h.kernel.receipts.size).toBe(0);
        expect(h.kernel.objects.get("mem_held")?.disposition).toBe("quarantined");
    });

    test("a user command relaxes through the stored approval", async () => {
        const h = approvedQuarantine();
        const outcome = await run(h, args("mark_disputed", "mem_held", true));
        expect(outcome).toMatchObject({
            kind: "applied",
            result: { previous_disposition: "quarantined", disposition: "disputed", denied: false },
        });
        expect(h.kernel.objects.get("mem_held")?.disposition).toBe("disputed");
    });
});

describe("runMemoryMarkCommand request digest", () => {
    test("the preview and the commit carry one digest of the operation and the source kind", async () => {
        for (const sourceKind of ["user", "assistant"] as const) {
            const h = harness();
            const input = args("contradict", "mem_verified", true);
            const outcome = await runMemoryMarkCommand({
                client: h.client,
                operationId: memoryMarkOperationId(SESSION, input),
                actor: "user:test",
                args: input,
                ...(sourceKind === "user" ? {} : { sourceKind }),
            });
            expect(outcome).toMatchObject({ kind: "applied", replayed: false });
            const bodies = h.transport.calls.map((call) => call.body as Record<string, any>);
            expect(bodies.map((body) => body.preview === true)).toEqual([true, false]);
            const digest = deriveRequestDigest({
                operations: [
                    {
                        op: "disposition",
                        object_id: "mem_verified",
                        event: "contradict",
                        evidence_ref: { path: "src/lib.rs", git_object_id: OID },
                    },
                ],
                sourceKind,
            });
            for (const body of bodies) {
                expect(body.source_kind).toBe(sourceKind);
                expect(body.intent.request_digest).toBe(digest);
            }
        }
    });
});

describe("memoryMarkOperationId flag order", () => {
    test("both flag orders parse to one args value and one identity", () => {
        const pathFirst = parseMemoryMarkArgs(
            `mark_disputed mem_1 --path src/a.rs@${OID} --evidence ev`,
        );
        const evidenceFirst = parseMemoryMarkArgs(
            `mark_disputed mem_1 --evidence ev --path src/a.rs@${OID}`,
        );
        if (!pathFirst.ok || !evidenceFirst.ok) throw new Error("both orders parse");
        expect(evidenceFirst.args).toEqual(pathFirst.args);
        expect(memoryMarkOperationId(SESSION, evidenceFirst.args)).toBe(
            memoryMarkOperationId(SESSION, pathFirst.args),
        );
    });
});

import { describe, expect, test } from "bun:test";
import { type DispositionPreview, KernelClient } from "./kernel-client";
import { FakeKernel, FakeKernelTransport } from "./kernel-client-testing/fake-kernel";
import {
    formatMemoryMarkOutcome,
    formatVisibilityDelta,
    MEMORY_MARK_USAGE,
    type MemoryMarkArgs,
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

function args(event: MemoryMarkArgs["event"], objectId: string, confirmed = false): MemoryMarkArgs {
    return { event, objectId, confirmed };
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
        sessionId: SESSION,
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
            "/ctx-memory-mark mark_stale mem_verified --yes",
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

    test("the confirm flag stands in for the dialog", async () => {
        const h = harness();
        const outcome = await run(h, args("contradict", "mem_verified", true));
        expect(outcome.kind).toBe("applied");
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
        expect(again).toEqual({ ...first, replayed: true });
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
        expect(again).toEqual({ ...first, replayed: true });
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
            sessionId: SESSION,
            actor: "user:test",
            args: args("mark_stale", "mem_labeled"),
            isCancelled: () => ended,
            confirm: UNEXPECTED_CONFIRMATION,
        });
        expect(outcome.kind).toBe("applied");
        ended = true;
        const cancelled = await runMemoryMarkCommand({
            client: h.client,
            sessionId: SESSION,
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
            sessionId: SESSION,
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

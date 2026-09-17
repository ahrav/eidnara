import { describe, expect, it } from "bun:test";
import {
    type ApplicationResult,
    type ApplicationTarget,
    CapabilityLatch,
    ContextApplication,
    type Edit,
    editEntries,
    isPackedEntry,
    PACKED_ENTRY_ID_PREFIX,
    type PackedAction,
    type RouteKey,
    type Terminal,
} from "./context-application";

const OCC = "ab".repeat(32);
const context = {
    context_revision: "rev-1",
    representation: "message-entry",
    spans: [{ occurrence_id: OCC, buffer_len: 10, span: null }],
    selection: [OCC],
};
const PROFILE = { identity: "claude-bpe", revision: "10:claude-bpe;3:abc" };
const ROUTE: RouteKey = { sessionId: "ses-1", projectRoot: "/workspace", routeEpoch: 7 };

interface Recorded {
    method: string;
    body: Record<string, unknown>;
}

function daemon(script: (call: Recorded, index: number) => unknown) {
    const calls: Recorded[] = [];
    const transport = {
        call: async (args: { method: string; body: unknown }) => {
            const call = { method: args.method, body: args.body as Record<string, unknown> };
            calls.push(call);
            const answer = script(call, calls.length - 1);
            if (answer instanceof Error) throw answer;
            return answer;
        },
    };
    return { calls, transport };
}

function prepared(id = `0123456789abcdef-${"cd".repeat(32)}`) {
    return {
        kind: "prepared",
        preparation_id: id,
        preparation_digest: "ef".repeat(32),
        fingerprint: "01".repeat(32),
        accounting_profile: PROFILE,
    };
}

function forwarded(body: Record<string, unknown>) {
    return {
        kind: "forwarded",
        preparation_id: body.preparation_id,
        forwarded_identity: "fe".repeat(32),
        action: body.action,
        edit_bytes: body.edit_bytes,
    };
}

function target(
    overrides: Partial<ApplicationTarget<unknown[]>> & { entries?: () => readonly unknown[] } = {},
): ApplicationTarget<unknown[]> {
    const { entries = () => [], ...rest } = overrides;
    return {
        route: ROUTE,
        context,
        body: "packed body",
        edit: (action, preparationId, body) =>
            editEntries(entries(), action, ROUTE.sessionId, preparationId, body),
        publish: async (_edit, forwardedIdentity) => forwardedIdentity,
        ...rest,
    };
}

function honest(): (call: Recorded) => unknown {
    return (call) => {
        if (call.method === "retrieval.prepare") return prepared();
        if (call.method === "retrieval.apply") {
            return JSON.stringify(call.body.accounting_profile) === JSON.stringify(PROFILE)
                ? forwarded(call.body)
                : { kind: "terminal", terminal: "profile_mismatch" };
        }
        return call.body.applied_identity === null
            ? { kind: "receipt", state: "unknown" }
            : { kind: "receipt", state: "complete", outcome: call.body.outcome };
    };
}

describe("capability latch", () => {
    it("denies a class for the rest of the route epoch and forgets it on rebind", () => {
        const latch = new CapabilityLatch();
        expect(latch.chooseAction("replace", ROUTE)).toBe("replace");
        expect(latch.observeTerminal("capability_unsupported", "replacement", ROUTE)).toBe(true);
        expect(latch.chooseAction("replace", ROUTE)).toBe("append");
        expect(latch.chooseAction("append", ROUTE)).toBe("append");
        expect(latch.chooseAction("replace", { ...ROUTE, routeEpoch: 8 })).toBe("replace");
        expect(latch.chooseAction("replace", { ...ROUTE, sessionId: "ses-2" })).toBe("replace");
        expect(latch.isDenied("replacement", ROUTE)).toBe(false);
    });

    it("latches only the two capability terminals with a known class", () => {
        const latch = new CapabilityLatch();
        for (const [terminal, cls] of [
            ["stale_preparation", "replacement"],
            ["conflict", "replacement"],
            ["disabled", "replacement"],
            ["profile_mismatch", "replacement"],
            ["capability_unsupported", "append"],
            ["capability_unsupported", undefined],
        ] as const) {
            expect(latch.observeTerminal(terminal, cls, ROUTE)).toBe(false);
        }
        expect(latch.isDenied("replacement", ROUTE)).toBe(false);
        expect(latch.observeTerminal("capability_undeclared", "cross_step_reuse", ROUTE)).toBe(
            true,
        );
        expect(latch.isDenied("cross_step_reuse", ROUTE)).toBe(true);
    });
});

describe("entry edits", () => {
    const owned = {
        info: { id: `${PACKED_ENTRY_ID_PREFIX}old`, role: "user", sessionID: "ses-1" },
        parts: [{ type: "text", text: "old packed" }],
    };
    const system = { info: { id: "msg-1", role: "system", sessionID: "ses-1" }, parts: [] };
    const trailing = { info: { id: "msg-2", role: "assistant", sessionID: "ses-1" }, parts: [] };

    it("appends one host-shaped owned entry and reports append", () => {
        const edit = editEntries([system], "append", "ses-1", "new", "fresh");
        expect(edit.outcome).toBe("append");
        expect(edit.surface).toEqual([
            system,
            {
                info: { id: `${PACKED_ENTRY_ID_PREFIX}new`, role: "user", sessionID: "ses-1" },
                parts: [{ type: "text", text: "fresh" }],
            },
        ]);
        expect(isPackedEntry(edit.surface[1])).toBe(true);
        expect(isPackedEntry(system)).toBe(false);
    });

    it("keeps an existing owned entry rather than appending a second", () => {
        const edit = editEntries([system, owned], "append", "ses-1", "new", "fresh");
        expect(edit.outcome).toBe("keep");
        expect(edit.surface).toEqual([system, owned]);
    });

    it("replaces the owned entry and keeps every other entry in order", () => {
        const edit = editEntries([system, owned, trailing], "replace", "ses-1", "new", "fresh");
        expect(edit.outcome).toBe("applied_replacement");
        expect(edit.surface).toEqual([
            system,
            trailing,
            {
                info: { id: `${PACKED_ENTRY_ID_PREFIX}new`, role: "user", sessionID: "ses-1" },
                parts: [{ type: "text", text: "fresh" }],
            },
        ]);
    });

    it("treats an empty replacement as a replacement that leaves the slot absent", () => {
        const edit = editEntries([system, owned], "replace", "ses-1", "new", "");
        expect(edit.outcome).toBe("applied_replacement");
        expect(edit.surface).toEqual([system]);
        expect(edit.surface.some(isPackedEntry)).toBe(false);
    });

    it("treats an empty append as keep with the surface unchanged", () => {
        const edit = editEntries([system], "append", "ses-1", "new", "");
        expect(edit.outcome).toBe("keep");
        expect(edit.surface).toEqual([system]);
    });
});

describe("prepare, apply, confirm", () => {
    it("echoes the daemon's profile at apply, edits the live surface, and reports the applied outcome", async () => {
        const { calls, transport } = daemon(honest());
        const published: Edit<unknown[]>[] = [];
        const live: unknown[] = [];
        const app = new ContextApplication(transport, new CapabilityLatch());
        const result = await app.run(
            "append",
            target({
                body: "pâcked",
                entries: () => live,
                publish: async (edit, forwardedIdentity) => {
                    published.push(edit);
                    return forwardedIdentity;
                },
            }),
        );
        expect(result).toEqual({
            kind: "applied",
            outcome: "append",
            preparationId: prepared().preparation_id,
            appliedIdentity: "fe".repeat(32),
        });
        expect(calls.map((call) => call.method)).toEqual([
            "retrieval.prepare",
            "retrieval.apply",
            "retrieval.confirm",
        ]);
        expect(calls[0]!.body.accounting_profile).toBeUndefined();
        expect(calls[0]!.body.edit_bytes).toBe(7);
        expect(calls[1]!.body.accounting_profile).toEqual(PROFILE);
        expect(calls[2]!.body.applied_identity).toBe("fe".repeat(32));
        expect(calls[2]!.body.outcome).toBe("append");
        expect(published).toHaveLength(1);
        expect(published[0]!.surface.some(isPackedEntry)).toBe(true);
    });

    it("never reports applied when the acknowledgment is lost, whatever the daemon answers", async () => {
        for (const confirmAnswer of [
            { kind: "receipt", state: "unknown" },
            { kind: "receipt", state: "complete", outcome: "append" },
        ]) {
            const { calls, transport } = daemon((call) => {
                if (call.method === "retrieval.prepare") return prepared();
                if (call.method === "retrieval.apply") return forwarded(call.body);
                return confirmAnswer;
            });
            const app = new ContextApplication(transport, new CapabilityLatch());
            const result = await app.run("append", target({ publish: async () => undefined }));
            expect(result.kind).toBe("unknown");
            expect(calls[2]!.body.applied_identity).toBeNull();
        }
    });

    it("never reports applied from a daemon receipt alone", async () => {
        const { transport } = daemon((call) =>
            call.method === "retrieval.prepare"
                ? prepared()
                : { kind: "receipt", state: "complete", outcome: "append" },
        );
        const app = new ContextApplication(transport, new CapabilityLatch());
        expect((await app.run("append", target())).kind).toBe("unknown");
    });

    it("reports unknown, not an error, when the confirm cannot reach the daemon after publication", async () => {
        const { calls, transport } = daemon((call) => {
            if (call.method === "retrieval.prepare") return prepared();
            if (call.method === "retrieval.apply") return forwarded(call.body);
            return new Error("transport closed");
        });
        const app = new ContextApplication(transport, new CapabilityLatch());
        const result = await app.run("append", target());
        expect(result).toEqual({
            kind: "unknown",
            preparationId: prepared().preparation_id,
            forwardedIdentity: "fe".repeat(32),
            appliedIdentity: "fe".repeat(32),
        });
        expect(calls).toHaveLength(3);
    });

    it("falls back to append once when the class is unsupported and latches it for the route", async () => {
        const seen: PackedAction[] = [];
        const { calls, transport } = daemon((call) => {
            if (call.method === "retrieval.prepare") {
                seen.push(call.body.action as PackedAction);
                return call.body.action === "replace"
                    ? { kind: "terminal", terminal: "capability_unsupported", class: "replacement" }
                    : prepared();
            }
            if (call.method === "retrieval.apply") return forwarded(call.body);
            return { kind: "receipt", state: "complete", outcome: call.body.outcome };
        });
        const latch = new CapabilityLatch();
        const app = new ContextApplication(transport, latch);
        expect((await app.run("replace", target())).kind).toBe("applied");
        expect(seen).toEqual(["replace", "append"]);
        expect((await app.run("replace", target())).kind).toBe("applied");
        expect(seen).toEqual(["replace", "append", "append"]);
        expect(calls.filter((call) => call.method === "retrieval.prepare")).toHaveLength(3);
        expect(latch.isDenied("replacement", ROUTE)).toBe(true);
        expect(latch.isDenied("replacement", { ...ROUTE, routeEpoch: 8 })).toBe(false);
    });

    it("reports typed refusals without publishing and never retries a non-capability terminal", async () => {
        const cases: Array<[Terminal, "retrieval.prepare" | "retrieval.apply"]> = [
            ["profile_mismatch", "retrieval.apply"],
            ["profile_unavailable", "retrieval.apply"],
            ["profile_unavailable", "retrieval.prepare"],
            ["stale_preparation", "retrieval.apply"],
            ["disabled", "retrieval.prepare"],
        ];
        for (const [terminal, stage] of cases) {
            const { calls, transport } = daemon((call) =>
                call.method === stage ? { kind: "terminal", terminal } : prepared(),
            );
            let published = 0;
            const app = new ContextApplication(transport, new CapabilityLatch());
            const result: ApplicationResult = await app.run(
                "replace",
                target({
                    publish: async (_edit, forwardedIdentity) => {
                        published += 1;
                        return forwardedIdentity;
                    },
                }),
            );
            expect(result).toEqual({
                kind: "refused",
                terminal,
                cls: undefined,
                reason: undefined,
            });
            expect(published).toBe(0);
            expect(calls.filter((call) => call.method === "retrieval.prepare")).toHaveLength(1);
        }
    });

    it("treats a terminal outside the closed vocabulary as a malformed answer", async () => {
        const { transport } = daemon(() => ({ kind: "terminal", terminal: "applied" }));
        const app = new ContextApplication(transport, new CapabilityLatch());
        expect(await app.run("append", target())).toEqual({
            kind: "failure",
            reason: "malformed_prepare_answer",
        });
    });

    it("reports a preparation failure by its reason", async () => {
        const { transport } = daemon(() => ({
            kind: "outcome",
            outcome: "preparation_failure",
            reason: "append_allowance",
        }));
        const app = new ContextApplication(transport, new CapabilityLatch());
        expect(await app.run("append", target())).toEqual({
            kind: "failure",
            reason: "append_allowance",
        });
    });
});

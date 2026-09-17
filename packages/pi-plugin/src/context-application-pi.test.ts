import { afterEach, describe, expect, it } from "bun:test";
import {
    type ApplicationTarget,
    CapabilityLatch,
    ContextApplication,
    type PackedAction,
    type RouteKey,
} from "@eidnara/opencode/hooks/context/context-application";
import {
    installTokenizerForTest,
    resetTokenEstimatorForTest,
    tokenEstimatorGeneration,
} from "@eidnara/opencode/shared/token-estimator";
import {
    editSystemPrompt,
    hasPackedBlock,
    PI_INVOCATION_HEADROOM_PERMILLE,
    validatePiInvocation,
} from "./context-application-pi";

afterEach(() => {
    resetTokenEstimatorForTest();
});

const PROMPT = "You are Pi.\nToday's date: Thu Sep 17 2026";
const ROUTE: RouteKey = { sessionId: "pi-1", projectRoot: "/workspace", routeEpoch: 3 };
const OCC = "ab".repeat(32);
const context = {
    context_revision: "rev-1",
    representation: "system-prompt",
    spans: [{ occurrence_id: OCC, buffer_len: 10, span: null }],
    selection: [OCC],
};
const PROFILE = { identity: "claude-bpe", revision: "10:claude-bpe;3:abc" };

const UNBOUNDED = 1 << 20;

describe("Pi system-prompt slot", () => {
    it("appends one owned block at the end and reports append", () => {
        const edit = editSystemPrompt(PROMPT, "append", "prep-1", "packed body", UNBOUNDED);
        expect(edit.outcome).toBe("append");
        expect(edit.surface).toBe(
            `${PROMPT}\n<eidnara-packed preparation="prep-1">\npacked body\n</eidnara-packed>`,
        );
        expect(hasPackedBlock(edit.surface)).toBe(true);
        expect(hasPackedBlock(PROMPT)).toBe(false);
    });

    it("keeps the prompt when a block is already owned or the body is empty", () => {
        const once = editSystemPrompt(PROMPT, "append", "prep-1", "first", UNBOUNDED).surface;
        const twice = editSystemPrompt(once, "append", "prep-2", "second", UNBOUNDED);
        expect(twice.outcome).toBe("keep");
        expect(twice.surface).toBe(once);
        const empty = editSystemPrompt(PROMPT, "append", "prep-1", "", UNBOUNDED);
        expect(empty.outcome).toBe("keep");
        expect(empty.surface).toBe(PROMPT);
    });

    it("replaces the owned block and leaves the slot absent on an empty replacement", () => {
        const once = editSystemPrompt(PROMPT, "append", "prep-1", "first", UNBOUNDED).surface;
        const replaced = editSystemPrompt(once, "replace", "prep-2", "second", UNBOUNDED);
        expect(replaced.outcome).toBe("applied_replacement");
        expect(replaced.surface).not.toContain("first");
        expect(replaced.surface).toContain('preparation="prep-2"');
        expect(replaced.surface.match(/<eidnara-packed/g)).toHaveLength(1);
        const emptied = editSystemPrompt(once, "replace", "prep-3", "", UNBOUNDED);
        expect(emptied.outcome).toBe("applied_replacement");
        expect(emptied.surface).toBe(PROMPT);
        expect(hasPackedBlock(emptied.surface)).toBe(false);
    });

    it("owns only a trailing block: a body quoting the close tag and a block-shaped run in prompt text are prompt text", () => {
        const quoting = "keep\n</eidnara-packed>\nthis";
        const once = editSystemPrompt(PROMPT, "append", "prep-1", quoting, UNBOUNDED).surface;
        expect(hasPackedBlock(once)).toBe(true);
        expect(editSystemPrompt(once, "replace", "prep-2", "", UNBOUNDED).surface).toBe(PROMPT);

        const authored = `Intro\n<eidnara-packed preparation="theirs">\nquoted\n</eidnara-packed>\nOutro`;
        expect(hasPackedBlock(authored)).toBe(false);
        const appended = editSystemPrompt(authored, "append", "prep-1", "mine", UNBOUNDED);
        expect(appended.outcome).toBe("append");
        expect(appended.surface.startsWith(authored)).toBe(true);
        expect(editSystemPrompt(authored, "replace", "prep-1", "", UNBOUNDED).surface).toBe(
            authored,
        );
    });

    it("keeps the prompt unchanged when the candidate would grow past the usable window", () => {
        const body = "x".repeat(200);
        const grown = editSystemPrompt(PROMPT, "append", "prep-1", body, UNBOUNDED);
        const charged = grown.validation.candidate.chargedTokens;
        expect(editSystemPrompt(PROMPT, "append", "prep-1", body, charged).outcome).toBe("append");
        const declined = editSystemPrompt(PROMPT, "append", "prep-1", body, charged - 1);
        expect(declined).toMatchObject({ surface: PROMPT, outcome: "keep" });
        expect(declined.validation.ok).toBe(false);
        expect(editSystemPrompt(grown.surface, "replace", "prep-2", "", 1)).toMatchObject({
            surface: PROMPT,
            outcome: "applied_replacement",
        });
    });
});

describe("Pi whole-invocation validation", () => {
    it("charges the whole prompt as one entry under the Pi heuristic profile with headroom", () => {
        const candidate = `${PROMPT}${"x".repeat(200)}`;
        const fits = validatePiInvocation(candidate, PROMPT, UNBOUNDED);
        expect(fits.ok).toBe(true);
        expect(fits.candidate.bytes).toBe(candidate.length);
        expect(fits.candidate.entries).toBe(1);
        expect(fits.candidate.profile).toEqual({
            identity: "pi-heuristic",
            revision: `generation:${tokenEstimatorGeneration()}`,
            authority: "heuristic",
        });
        expect(fits.candidate.chargedTokens).toBe(
            Math.ceil(
                (Math.ceil(candidate.length / 3.5) * (1000 + PI_INVOCATION_HEADROOM_PERMILLE)) /
                    1000,
            ),
        );
    });

    it("moves the profile revision with the estimator generation and never labels it exact", () => {
        const before = validatePiInvocation(PROMPT, PROMPT, undefined).candidate.profile;
        installTokenizerForTest({ encode: (text: string) => Array.from(text, (_, i) => i) });
        const after = validatePiInvocation(PROMPT, PROMPT, undefined).candidate.profile;
        expect(after.revision).not.toBe(before.revision);
        expect(after.authority).toBe("heuristic");
        expect(after.identity).toBe("pi-heuristic");
    });
});

interface Recorded {
    method: string;
    body: Record<string, unknown>;
}

function piDaemon(script?: (call: Recorded) => unknown) {
    const calls: Recorded[] = [];
    const transport = {
        call: async (args: { method: string; body: unknown }) => {
            const call = { method: args.method, body: args.body as Record<string, unknown> };
            calls.push(call);
            if (script) return script(call);
            if (call.method === "retrieval.prepare") {
                return call.body.action === "append"
                    ? {
                          kind: "prepared",
                          preparation_id: `0123456789abcdef-${"cd".repeat(32)}`,
                          preparation_digest: "ef".repeat(32),
                          fingerprint: "01".repeat(32),
                          accounting_profile: PROFILE,
                      }
                    : {
                          kind: "terminal",
                          terminal: "capability_unsupported",
                          class:
                              call.body.action === "replace"
                                  ? "replacement"
                                  : call.body.action === "suppress"
                                    ? "suppression"
                                    : "cross_step_reuse",
                      };
            }
            if (call.method === "retrieval.apply") {
                return JSON.stringify(call.body.accounting_profile) === JSON.stringify(PROFILE)
                    ? {
                          kind: "forwarded",
                          preparation_id: call.body.preparation_id,
                          forwarded_identity: "fe".repeat(32),
                          action: "append",
                          edit_bytes: call.body.edit_bytes,
                      }
                    : { kind: "terminal", terminal: "profile_mismatch" };
            }
            return call.body.applied_identity === null
                ? { kind: "receipt", state: "unknown" }
                : { kind: "receipt", state: "complete", outcome: call.body.outcome };
        },
    };
    return { calls, transport };
}

function piTarget(
    prompt: { current: string },
    body: string,
    overrides: Partial<ApplicationTarget<string>> = {},
): ApplicationTarget<string> {
    return {
        route: ROUTE,
        context,
        body,
        edit: (action, preparationId, packed) =>
            editSystemPrompt(prompt.current, action, preparationId, packed, UNBOUNDED),
        publish: async (edit, forwardedIdentity) => {
            prompt.current = edit.surface;
            return forwardedIdentity;
        },
        ...overrides,
    };
}

describe("Pi pure packing through prepare, apply, confirm", () => {
    it("appends under the daemon's echoed profile and yields exactly one closed-set literal", async () => {
        const { calls, transport } = piDaemon();
        const prompt = { current: PROMPT };
        const result = await new ContextApplication(transport, new CapabilityLatch()).run(
            "append",
            piTarget(prompt, "packed"),
        );
        expect(result).toMatchObject({ kind: "applied", outcome: "append" });
        expect(hasPackedBlock(prompt.current)).toBe(true);
        expect(calls.map((call) => call.method)).toEqual([
            "retrieval.prepare",
            "retrieval.apply",
            "retrieval.confirm",
        ]);
        expect(calls[1]!.body.accounting_profile).toEqual(PROFILE);
        expect(calls[2]!.body.outcome).toBe("append");
    });

    it("reports a preparation failure by its reason and leaves the prompt unchanged", async () => {
        const { transport } = piDaemon(() => ({
            kind: "outcome",
            outcome: "preparation_failure",
            reason: "append_allowance",
        }));
        const prompt = { current: PROMPT };
        expect(
            await new ContextApplication(transport, new CapabilityLatch()).run(
                "append",
                piTarget(prompt, "packed"),
            ),
        ).toEqual({ kind: "failure", reason: "append_allowance" });
        expect(prompt.current).toBe(PROMPT);
    });

    it("never simulates a denied class: a replace intent falls back to append and a second owned block is never written", async () => {
        const { calls, transport } = piDaemon();
        const prompt = { current: PROMPT };
        const latch = new CapabilityLatch();
        const app = new ContextApplication(transport, latch);
        const first = await app.run("replace", piTarget(prompt, "first"));
        expect(first).toMatchObject({ kind: "applied", outcome: "append" });
        expect(latch.isDenied("replacement", ROUTE)).toBe(true);
        expect(prompt.current).toContain("first");

        const second = await app.run("replace", piTarget(prompt, "second"));
        expect(second).toMatchObject({ kind: "applied", outcome: "keep" });
        expect(prompt.current).not.toContain("second");
        expect(prompt.current.match(/<eidnara-packed/g)).toHaveLength(1);
        const sent: PackedAction[] = calls
            .filter((call) => call.method === "retrieval.prepare")
            .map((call) => call.body.action as PackedAction);
        expect(sent).toEqual(["replace", "append", "append"]);
    });

    it("resolves a lost acknowledgment to unknown and leaves nothing applied on a refusal", async () => {
        const { transport } = piDaemon();
        const lost = { current: PROMPT };
        const unknown = await new ContextApplication(transport, new CapabilityLatch()).run(
            "append",
            piTarget(lost, "packed", { publish: async () => undefined }),
        );
        expect(unknown.kind).toBe("unknown");

        for (const terminal of ["profile_unavailable", "profile_mismatch", "stale_preparation"]) {
            const { transport: refusing } = piDaemon((call) =>
                call.method === "retrieval.apply"
                    ? { kind: "terminal", terminal }
                    : {
                          kind: "prepared",
                          preparation_id: `0123456789abcdef-${"cd".repeat(32)}`,
                          preparation_digest: "ef".repeat(32),
                          fingerprint: "01".repeat(32),
                          accounting_profile: PROFILE,
                      },
            );
            const prompt = { current: PROMPT };
            const refused = await new ContextApplication(refusing, new CapabilityLatch()).run(
                "append",
                piTarget(prompt, "packed"),
            );
            expect(refused).toMatchObject({ kind: "refused", terminal });
            expect(prompt.current).toBe(PROMPT);
        }
    });
});

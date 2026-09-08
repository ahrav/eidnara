import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parseIncidentCatalog } from "../contract";
import { E2E_ROOT } from "../evidence";
import * as regressions from "./source-linked-regressions";
import {
    type AgedCtxReduceObservation,
    FIRST_RENDER_A1_CHECKS,
    FIRST_RENDER_A3_CHECKS,
    type FirstRenderDeferObservation,
    failedCheckIds,
    hasCtxReducePair,
    publishedHistoryCovers,
    THINKING_DROPPED_SHELL_CHECKS,
    THINKING_IMAGE_SURVIVAL_CHECKS,
    THINKING_NUDGE_ANCHOR_CHECKS,
    type ThinkingDroppedShellObservation,
    type ThinkingImageSurvivalObservation,
    type ThinkingNudgeAnchorObservation,
    verifyAgedCtxReduceSurvival,
    verifyFirstRenderPureDeferStability,
    verifyThinkingDroppedShell,
    verifyThinkingImageSurvival,
    verifyThinkingNudgeAnchor,
    wireCarriesTagOverlay,
} from "./source-linked-regressions";

const MODULE_PATH = "src/incident-pool/scenarios/source-linked-regressions.ts";

function a1Observation(
    overrides: Partial<FirstRenderDeferObservation> = {},
): FirstRenderDeferObservation {
    return {
        mainRequestCount: 6,
        bustCount: 0,
        bustReport: "",
        uncachedTransitionCount: 0,
        transformRenderedRequestCount: 6,
        rustPassCount: 6,
        transformServedPassCount: 6,
        ...overrides,
    };
}

function a3Observation(
    overrides: Partial<AgedCtxReduceObservation> = {},
): AgedCtxReduceObservation {
    return {
        mainRequestCount: 9,
        sawReduceOnWire: true,
        bustCount: 0,
        bustReport: "",
        finalWireHasCtxReduce: true,
        uncachedTransitionCount: 0,
        transformRenderedRequestCount: 9,
        rustPassCount: 9,
        transformServedPassCount: 9,
        ...overrides,
    };
}

function nudgeObservation(
    overrides: Partial<ThinkingNudgeAnchorObservation> = {},
): ThinkingNudgeAnchorObservation {
    return {
        mainRequestCount: 3,
        assistantCandidates: 2,
        nudgeMarkerFound: false,
        thinkingBlockCount: 0,
        ...overrides,
    };
}

function shellObservation(
    overrides: Partial<ThinkingDroppedShellObservation> = {},
): ThinkingDroppedShellObservation {
    return {
        dropEmitted: true,
        pasteBodyAbsent: true,
        shellPreserved: true,
        signedReplayIntact: true,
        turnBoundaryPreserved: true,
        ...overrides,
    };
}

function imageObservation(
    overrides: Partial<ThinkingImageSurvivalObservation> = {},
): ThinkingImageSurvivalObservation {
    return {
        dropEmitted: true,
        droppedTextAbsent: true,
        coveredByRustHistory: false,
        imageBlockCount: 1,
        imagePayloadPreserved: true,
        placeholderPresent: true,
        userWithImagePresent: true,
        ...overrides,
    };
}

describe("first-render tag stability verifiers (parity A1/A3)", () => {
    it("passes a clean pure-defer observation and emits the catalog check ids", () => {
        const result = verifyFirstRenderPureDeferStability(a1Observation());
        expect(result.verdict).toBe("pass");
        expect(result.checks.map((check) => check.id)).toEqual([...FIRST_RENDER_A1_CHECKS]);
    });

    it("rejects a crafted bust observation and a below-floor request count", () => {
        const busted = verifyFirstRenderPureDeferStability(a1Observation({ bustCount: 1 }));
        expect(busted.verdict).toBe("assertion_fail");
        expect(failedCheckIds(busted)).toEqual(["check-a1-zero-prefix-busts"]);

        const thin = verifyFirstRenderPureDeferStability(
            a1Observation({ mainRequestCount: 5, transformRenderedRequestCount: 5 }),
        );
        expect(failedCheckIds(thin)).toEqual(["check-a1-defer-request-floor"]);
    });

    it("rejects zero busts measured without breakpoints or without the transform serving every pass", () => {
        expect(
            failedCheckIds(
                verifyFirstRenderPureDeferStability(a1Observation({ uncachedTransitionCount: 5 })),
            ),
        ).toEqual(["check-a1-cached-transitions"]);
        expect(
            failedCheckIds(
                verifyFirstRenderPureDeferStability(a1Observation({ transformServedPassCount: 5 })),
            ),
        ).toEqual(["check-a1-transform-served"]);
        expect(
            failedCheckIds(
                verifyFirstRenderPureDeferStability(
                    a1Observation({ rustPassCount: 0, transformServedPassCount: 0 }),
                ),
            ),
        ).toEqual(["check-a1-transform-served"]);
        // Internal-agent passes can pad the pass counts; a main request without the tag overlay still fails.
        expect(
            failedCheckIds(
                verifyFirstRenderPureDeferStability(
                    a1Observation({
                        transformRenderedRequestCount: 5,
                        rustPassCount: 7,
                        transformServedPassCount: 7,
                    }),
                ),
            ),
        ).toEqual(["check-a1-transform-served"]);
        expect(
            wireCarriesTagOverlay({
                messages: [{ role: "user", content: [{ type: "text", text: "§4§ A1 turn 4" }] }],
            }),
        ).toBe(true);
        expect(
            wireCarriesTagOverlay({
                messages: [{ role: "user", content: [{ type: "text", text: "A1 turn 4" }] }],
            }),
        ).toBe(false);
        expect(
            failedCheckIds(
                verifyAgedCtxReduceSurvival(a3Observation({ uncachedTransitionCount: 1 })),
            ),
        ).toEqual(["check-a3-cached-transitions"]);
        expect(
            failedCheckIds(
                verifyAgedCtxReduceSurvival(a3Observation({ transformServedPassCount: 8 })),
            ),
        ).toEqual(["check-a3-transform-served"]);
        expect(
            failedCheckIds(
                verifyAgedCtxReduceSurvival(
                    a3Observation({ rustPassCount: 8, transformServedPassCount: 8 }),
                ),
            ),
        ).toEqual(["check-a3-transform-served"]);
    });

    it("passes a surviving aged ctx_reduce arc and emits the catalog check ids", () => {
        const result = verifyAgedCtxReduceSurvival(a3Observation());
        expect(result.verdict).toBe("pass");
        expect(result.checks.map((check) => check.id)).toEqual([...FIRST_RENDER_A3_CHECKS]);
    });

    it("rejects a tool declaration after the emitted ctx_reduce pair vanished", () => {
        const callId = "toolu_incident_a3_ctx_reduce";
        const declarationOnly = {
            tools: [{ name: "ctx_reduce" }],
            messages: [{ role: "user", content: "continue" }],
        };
        expect(hasCtxReducePair(declarationOnly, callId, "99999", "ctx_reduce")).toBe(false);
        const pairWithDrop = (drop: unknown) => ({
            ...declarationOnly,
            messages: [
                {
                    role: "assistant",
                    content: [
                        {
                            type: "tool_use",
                            id: callId,
                            name: "ctx_reduce",
                            input: drop === undefined ? {} : { drop },
                        },
                    ],
                },
                {
                    role: "user",
                    content: [{ type: "tool_result", tool_use_id: callId, content: "ok" }],
                },
            ],
        });
        expect(hasCtxReducePair(pairWithDrop("99999"), callId, "99999", "ctx_reduce")).toBe(true);
        // A pair whose input was rewritten or stripped is not the retained fixture payload.
        expect(hasCtxReducePair(pairWithDrop("1"), callId, "99999", "ctx_reduce")).toBe(false);
        expect(hasCtxReducePair(pairWithDrop(undefined), callId, "99999", "ctx_reduce")).toBe(
            false,
        );
        // A rewritten name that still contains the canonical name is not the emitted tool.
        expect(
            hasCtxReducePair(pairWithDrop("99999"), callId, "99999", "ctx_reduce_corrupted"),
        ).toBe(false);
    });

    it("rejects a vanished ctx_reduce call, a bust, and a never-on-wire call", () => {
        expect(
            failedCheckIds(
                verifyAgedCtxReduceSurvival(a3Observation({ finalWireHasCtxReduce: false })),
            ),
        ).toEqual(["check-a3-reduce-retained-final-wire"]);
        expect(
            failedCheckIds(verifyAgedCtxReduceSurvival(a3Observation({ bustCount: 2 }))),
        ).toEqual(["check-a3-zero-prefix-busts"]);
        expect(
            failedCheckIds(verifyAgedCtxReduceSurvival(a3Observation({ sawReduceOnWire: false }))),
        ).toEqual(["check-a3-reduce-on-wire"]);
        // Eight prompts produce nine main requests because the ctx_reduce tool_use adds a continuation.
        expect(
            failedCheckIds(
                verifyAgedCtxReduceSurvival(
                    a3Observation({ mainRequestCount: 8, transformRenderedRequestCount: 8 }),
                ),
            ),
        ).toEqual(["check-a3-defer-request-floor"]);
    });
});

describe("thinking-block successor verifiers", () => {
    it("passes a clean nudge-anchor observation and emits the check ids", () => {
        const result = verifyThinkingNudgeAnchor(nudgeObservation());
        expect(result.verdict).toBe("pass");
        expect(result.checks.map((check) => check.id)).toEqual([...THINKING_NUDGE_ANCHOR_CHECKS]);
    });

    it("rejects nudge text in a signed assistant even when every other field reads healthy", () => {
        const result = verifyThinkingNudgeAnchor(nudgeObservation({ nudgeMarkerFound: true }));
        expect(failedCheckIds(result)).toEqual(["check-thinking-a-no-nudge-in-signed-assistant"]);
    });

    it("rejects vacuous inspection and a thinking block that reached the wire", () => {
        expect(
            failedCheckIds(verifyThinkingNudgeAnchor(nudgeObservation({ assistantCandidates: 0 }))),
        ).toEqual(["check-thinking-a-nonvacuous-inspection"]);
        expect(
            failedCheckIds(verifyThinkingNudgeAnchor(nudgeObservation({ mainRequestCount: 2 }))),
        ).toEqual(["check-thinking-a-nonvacuous-inspection"]);
        expect(
            failedCheckIds(verifyThinkingNudgeAnchor(nudgeObservation({ thinkingBlockCount: 1 }))),
        ).toEqual(["check-thinking-a-signature-byte-stable"]);
    });

    it("passes a clean dropped-shell observation and rejects crafted invalid states", () => {
        const clean = verifyThinkingDroppedShell(shellObservation());
        expect(clean.verdict).toBe("pass");
        expect(clean.checks.map((check) => check.id)).toEqual([...THINKING_DROPPED_SHELL_CHECKS]);

        expect(
            failedCheckIds(
                verifyThinkingDroppedShell(shellObservation({ pasteBodyAbsent: false })),
            ),
        ).toEqual(["check-thinking-b-paste-body-absent"]);
        expect(
            failedCheckIds(
                verifyThinkingDroppedShell(shellObservation({ turnBoundaryPreserved: false })),
            ),
        ).toEqual(["check-thinking-b-turn-boundary-preserved"]);
        expect(
            failedCheckIds(verifyThinkingDroppedShell(shellObservation({ dropEmitted: false }))),
        ).toEqual(["check-thinking-b-drop-emitted"]);
    });

    it("passes clean image-survival observations and rejects a stripped image", () => {
        // The daemon emits the wrapper even with no history, so the wrapper alone does not prove coverage.
        expect(publishedHistoryCovers("§3§ <session-history></session-history>", 1)).toBe(false);
        expect(publishedHistoryCovers("<session-history>\n\n</session-history>", 1)).toBe(false);
        const published =
            "<session-history>\n## 1-4 · Screenshot triage\n user shared bug.png\n## 7-9 · Later\n more\n</session-history>";
        expect(publishedHistoryCovers(published, 1)).toBe(true);
        expect(publishedHistoryCovers(published, 4)).toBe(true);
        expect(publishedHistoryCovers(published, 8)).toBe(true);
        // A range published for other turns does not cover the image turn.
        expect(publishedHistoryCovers(published, 5)).toBe(false);
        expect(publishedHistoryCovers("## 1-4 · outside the wrapper", 1)).toBe(false);

        const raw = verifyThinkingImageSurvival(imageObservation());
        expect(raw.verdict).toBe("pass");
        expect(raw.checks.map((check) => check.id)).toEqual([...THINKING_IMAGE_SURVIVAL_CHECKS]);

        expect(
            failedCheckIds(verifyThinkingImageSurvival(imageObservation({ imageBlockCount: 0 }))),
        ).toEqual(["check-thinking-c-image-part-survives"]);
        const covered = verifyThinkingImageSurvival(
            imageObservation({
                coveredByRustHistory: true,
                imageBlockCount: 0,
                placeholderPresent: false,
                userWithImagePresent: false,
            }),
        );
        expect(covered.verdict).toBe("pass");
        expect(
            failedCheckIds(
                verifyThinkingImageSurvival(
                    imageObservation({ coveredByRustHistory: true, imageBlockCount: 1 }),
                ),
            ),
        ).toEqual(["check-thinking-c-image-part-survives"]);
        expect(
            failedCheckIds(
                verifyThinkingImageSurvival(imageObservation({ droppedTextAbsent: false })),
            ),
        ).toEqual(["check-thinking-c-dropped-text-absent"]);
    });
});

describe("registry binding surface", () => {
    it("resolves every committed live binding to a real exported function", () => {
        const catalog = parseIncidentCatalog(
            JSON.parse(readFileSync(join(E2E_ROOT, "incidents", "catalog.json"), "utf8")),
        );
        const liveModules: Record<string, Record<string, unknown>> = {
            [MODULE_PATH]: regressions as Record<string, unknown>,
        };
        let liveBindings = 0;
        for (const family of catalog.families) {
            for (const variant of family.variants) {
                const binding = variant.verifier_binding;
                if (binding === null || binding.binding_status !== "live") continue;
                for (const reference of [binding.driver, binding.verifier]) {
                    const [path, symbol] = reference.split("#") as [string, string];
                    const moduleExports = liveModules[path];
                    expect(moduleExports).toBeDefined();
                    expect(typeof moduleExports?.[symbol]).toBe("function");
                }
                liveBindings++;
            }
        }
        expect(liveBindings).toBe(2);
    });

    it("keeps the committed normative checks equal to the verifier-emitted check ids", () => {
        const catalog = parseIncidentCatalog(
            JSON.parse(readFileSync(join(E2E_ROOT, "incidents", "catalog.json"), "utf8")),
        );
        const expectedChecks: Record<string, readonly string[]> = {
            "var-parity-a1-pure-defer-stability": FIRST_RENDER_A1_CHECKS,
            "var-parity-a3-ctx-reduce-survival": FIRST_RENDER_A3_CHECKS,
        };
        let matched = 0;
        for (const family of catalog.families) {
            for (const variant of family.variants) {
                const expected = expectedChecks[variant.id];
                if (!expected) continue;
                expect(variant.normative_checks).toEqual([...expected]);
                matched++;
            }
        }
        expect(matched).toBe(2);
    });

    it("returns a structured registry result, not a bare test outcome", () => {
        const result = verifyFirstRenderPureDeferStability(a1Observation({ bustCount: 3 }));
        expect(result).toEqual({
            verdict: "assertion_fail",
            checks: [
                { id: "check-a1-defer-request-floor", passed: true },
                { id: "check-a1-zero-prefix-busts", passed: false },
                { id: "check-a1-cached-transitions", passed: true },
                { id: "check-a1-transform-served", passed: true },
            ],
        });
    });
});

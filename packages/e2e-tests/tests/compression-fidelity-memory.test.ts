import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { createHash } from "node:crypto";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import {
    AGING_SERVING,
    CaseDriver,
    CONTEXT_LIMIT,
    type Delivery,
    eidnaraConfig,
    M1_SERVING,
    servingCost,
} from "../src/compression-fidelity/campaign";
import {
    type FidelityCase,
    readCompressionFidelityCorpus,
} from "../src/compression-fidelity/corpus";
import { captureTexts, emitObservation, messageText } from "../src/compression-fidelity/delivery";
import { RustTestHarness } from "../src/rust-harness";
import type { ScriptSource } from "../src/rust-runner/hermetic-host";
import { rustPrereqs } from "../src/rust-scenario-support";
import { runScriptedToolCall } from "../src/scripted-tool-call";

const QUALIFICATION_REQUIRED = process.env.EIDNARA_E2E_REQUIRE_FIDELITY === "1";
const CASE_TIMEOUT_MS = 540_000;
const PROJECT_MEMORY = /<project-memory>[\s\S]*?<\/project-memory>/;
const SEARCH_HINT = /<eidnara-search-hint>[\s\S]*?<\/eidnara-search-hint>/;
const DISTRACTOR_PAIRS = 30;
const OTHER_PROJECT_SUFFIX = "Recorded in the other project.";
const DISTRACTOR_ROWS = 24;
const RECOVERY_RESULT_BYTES = 16 * 1024;

const corpus = readCompressionFidelityCorpus();

function caseOf(id: string): FidelityCase {
    const found = corpus.cases.find((entry) => entry.id === id);
    if (!found) throw new Error(`${id} is not in the corpus`);
    return found;
}

function memoryBlock(delivery: Delivery): string {
    return PROJECT_MEMORY.exec(captureTexts(delivery.capture).join("\n"))?.[0] ?? "";
}

function memoryId(label: string): string {
    return `mem_${createHash("sha256").update(label).digest("hex").slice(0, 32)}`;
}

async function createMemory(
    h: RustTestHarness,
    sessionId: string,
    example: ScriptSource["memoryExamples"][number],
): Promise<string> {
    const call = await runScriptedToolCall(h, sessionId, {
        tool: "eidnara_memory",
        input: { action: "create", content: example.text, category: example.category },
        prompt: "Record this workspace convention as project memory.",
    });
    const objectId = /"objectId":"(mem_[0-9a-f]{32})"/.exec(call.resultText)?.[1];
    if (!objectId) throw new Error(`memory create returned no object: ${call.resultText}`);
    return objectId;
}

describe("compression fidelity memory, hint, and recovery prerequisites", () => {
    it.skipIf(!QUALIFICATION_REQUIRED)("are present when EIDNARA_E2E_REQUIRE_FIDELITY=1", () => {
        expect(rustPrereqs.skipReason ?? "present").toBe("present");
    });
});

describe.skipIf(!rustPrereqs.ok)("compression fidelity memory, hint, and recovery delivery", () => {
    let h: RustTestHarness;

    beforeAll(async () => {
        h = await RustTestHarness.create({
            modelContextLimit: CONTEXT_LIMIT,
            eidnaraConfig: eidnaraConfig(M1_SERVING),
        });
    }, 600_000);

    afterAll(async () => {
        await h?.dispose();
    });

    it(
        "credits C3.M1 only when admitted and included, and recovers it through eidnara_search",
        async () => {
            const c3 = caseOf("C3");
            const admittedScenario = c3.scenarios.find((entry) => entry.id === "C3.S6");
            const excludedScenario = c3.scenarios.find((entry) => entry.id === "C3.S7");
            if (!admittedScenario || !excludedScenario) throw new Error("C3.S6 or C3.S7 missing");
            const driver = await CaseDriver.open(h, c3, admittedScenario);
            await driver.baseline();
            const source = await driver.seed();
            await driver.publish(admittedScenario.id);
            const budget = (await driver.observeCold("aging-budget", AGING_SERVING)).budget;
            const p3 = await driver.ageTo("p3", budget);
            expect([p3.stage, p3.verdict.tier, p3.verdict.refusals]).toEqual(["m0", "p3", []]);
            const example = source.memoryExamples[0];
            if (!example) throw new Error("C3 has no memory example");

            const record = (
                label: string,
                scenario: string,
                delivery: Delivery,
                credited: boolean,
                markers: string[],
            ) =>
                emitObservation({
                    case: "C3",
                    source: driver.sourceId,
                    scenario,
                    stage: label,
                    terminal: credited ? "served" : "excluded",
                    markers,
                    detail: {
                        memory_example: example.id,
                        memory_credit: credited,
                        project_memory_utf8_bytes: Buffer.byteLength(memoryBlock(delivery)),
                        served_tier: delivery.verdict.tier,
                        refusals: delivery.verdict.refusals,
                        serving: servingCost(delivery),
                    },
                });
            const observeExcluded = async (
                label: string,
                extra: Record<string, unknown> = {},
            ): Promise<void> => {
                const delivery = await driver.observeCold(label, AGING_SERVING, extra);
                expect(delivery.verdict.refusals).toEqual([]);
                expect(delivery.verdict.tier).toBe("p3");
                expect(memoryBlock(delivery)).not.toContain(example.text);
                record(label, excludedScenario.id, delivery, false, [
                    "cf-delivery-memory-negative-inputs",
                ]);
            };
            // Decisions that stay out of every block, so an admitted pass shows its row renders
            // beside them while they remain excluded.
            const excludedIds: string[] = [];
            const observeAdmitted = async (label: string, objectId: string): Promise<void> => {
                const delivery = await driver.observeCold(label, AGING_SERVING);
                expect(delivery.verdict.refusals).toEqual([]);
                expect(delivery.verdict.tier).toBe("p3");
                const block = memoryBlock(delivery);
                expect(block).toContain(`${objectId}: ${example.text}`);
                expect(block).not.toContain(OTHER_PROJECT_SUFFIX);
                for (const excluded of excludedIds) expect(block).not.toContain(excluded);
                record(label, admittedScenario.id, delivery, true, [
                    "cf-delivery-memory-positive-input",
                ]);
            };

            // A verified copy committed into another project's scope.
            const otherProject = join(h.env.workdir, "..", "other-project");
            mkdirSync(otherProject, { recursive: true });
            const otherAnchor = await createMemory(h, await h.createSession(otherProject), example);
            const wrong = await h.host.memorySeed(
                otherAnchor,
                memoryId("wrong-project"),
                example.category,
                `${example.text} ${OTHER_PROJECT_SUFFIX}`,
            );
            expect(wrong.visibility).toBe("automatic");
            excludedIds.push(otherAnchor, memoryId("wrong-project"));
            await observeExcluded("memory-wrong-project");

            // An agent-authored memory in this project, which stays a labeled candidate.
            const anchor = await createMemory(h, await h.createSession(), example);
            excludedIds.push(anchor);
            await observeExcluded("memory-candidate-only");

            const visible = captureTexts(
                h.retainedCaptures({ sessionId: driver.sessionId }).at(-1) ?? p3.capture,
            )
                .join("\n")
                .toLowerCase();
            const query = "manifest cache resident";
            expect(query.split(" ").every((term) => visible.includes(term))).toBe(true);
            const search = await runScriptedToolCall(h, driver.sessionId, {
                tool: "eidnara_search",
                input: { query, sources: ["memory"] },
                prompt: excludedScenario.followUp.prompt,
            });
            expect(search.resultText).toContain(example.text);
            expect(Buffer.byteLength(search.resultText)).toBeLessThanOrEqual(RECOVERY_RESULT_BYTES);
            expect(search.resultText).not.toContain(OTHER_PROJECT_SUFFIX);
            emitObservation({
                case: "C3",
                source: driver.sourceId,
                scenario: excludedScenario.id,
                stage: "recovery-eidnara-search",
                terminal: "discoverable",
                markers: [],
                detail: {
                    tool: search.publishedToolName,
                    arguments: { query, sources: ["memory"] },
                    calls: 1,
                    result_utf8_bytes: Buffer.byteLength(search.resultText),
                    result_carries_memory: true,
                    exact_recovery: "unavailable",
                },
            });

            // A verified memory that carries the obligation and exceeds the injection budget.
            const oversized = await h.host.memorySeed(
                anchor,
                memoryId("budget-excluded"),
                example.category,
                `${example.text} ${h.ballast(12_000)}`,
            );
            expect(oversized.visibility).toBe("automatic");
            excludedIds.push(memoryId("budget-excluded"));
            await observeExcluded("memory-budget-excluded");

            const admittedId = memoryId("admitted");
            const admitted = await h.host.memorySeed(
                anchor,
                admittedId,
                example.category,
                example.text,
            );
            expect([admitted.effectiveMaturity, admitted.visibility]).toEqual([
                "verified",
                "automatic",
            ]);
            await observeAdmitted("memory-admitted", admittedId);
            const withheld = await h.host.memoryAdmission(admittedId, "quarantine");
            expect(withheld.visibility).toBe("audit_only");
            excludedIds.push(admittedId);
            await observeExcluded("memory-withheld");

            const secondId = memoryId("readmitted");
            await h.host.memorySeed(anchor, secondId, example.category, example.text);
            await observeAdmitted("memory-readmitted", secondId);
            const rejected = await h.host.memoryAdmission(secondId, "explicit_reject");
            expect(rejected.visibility).toBe("review_only");
            await observeExcluded("memory-rejected");
        },
        CASE_TIMEOUT_MS,
    );

    it(
        "serves C4's omitted row as a hint only while auto-search is on",
        async () => {
            const c4 = caseOf("C4");
            const scenario = c4.scenarios.find((entry) => entry.id === "C4.S6");
            if (!scenario) throw new Error("C4.S6 is not in the corpus");
            const driver = await CaseDriver.open(h, c4, scenario);
            await driver.baseline();
            await driver.seed();
            await driver.publish(scenario.id);
            const budget = (await driver.observeCold("aging-budget", AGING_SERVING)).budget;
            const omitted = await driver.ageTo("p5", budget);
            expect([omitted.stage, omitted.curve]).toEqual(["absent", "p5"]);
            // Distractor rows share the follow-up's generic terms, so those terms stop
            // discriminating and the case row's rarer terms carry the hint's score.
            const distractors = await driver.publishEchoed(
                "distractor",
                DISTRACTOR_PAIRS,
                (_step, k) =>
                    `Distractor ${k}: the recaps said we can start wiring the right parser.`,
                AGING_SERVING,
                DISTRACTOR_ROWS,
            );
            expect(distractors).toBeGreaterThan(0);

            const hintOf = (delivery: Delivery) =>
                SEARCH_HINT.exec(
                    messageText(
                        (delivery.capture.request.body.messages ?? []).at(-1) ?? { content: "" },
                    ),
                )?.[0] ?? "";
            const on = await driver.observeCold(
                "hint-on",
                AGING_SERVING,
                { memory: { auto_search: { enabled: true } } },
                true,
            );
            expect(on.verdict.refusals).toEqual(["history_absent"]);
            const outcome = await h.host.userHintOutcome();
            if (outcome?.pass !== "decided") {
                throw new Error(`hint-on pass did not decide a hint: ${JSON.stringify(outcome)}`);
            }
            const hint = hintOf(on);
            expect(outcome.hintText).toContain(hint);
            // The case row ranks first, ahead of distractor rows that also matched the prompt.
            const caseRow = driver.older + 1;
            expect(outcome.selected[0]).toBe(caseRow);
            const matchedDistractors = outcome.matched.filter((row) => row > caseRow).length;
            expect(matchedDistractors).toBeGreaterThan(0);
            const caseFragment = hint.split("\n").find((line) => line.startsWith("- ")) ?? "";
            expect(caseFragment).toContain("contradict");
            const qualifierKept = /rejected/i.test(caseFragment);
            emitObservation({
                case: "C4",
                source: driver.sourceId,
                scenario: scenario.id,
                stage: "hint-on",
                terminal: "served",
                markers: ["cf-delivery-hint-fragment-input"],
                detail: {
                    hint_utf16_units: hint.length,
                    distractor_rows: distractors,
                    candidate_rows: outcome.window.length,
                    matched_distractor_rows: matchedDistractors,
                    selected_rows: outcome.selected.length,
                    served_tier: on.verdict.tier,
                    serving: servingCost(on),
                },
            });

            // The fragment limit cuts the case row's snippet at both ends.
            expect(caseFragment.startsWith("- …") && caseFragment.endsWith("…")).toBe(true);
            emitObservation({
                case: "C4",
                source: driver.sourceId,
                scenario: scenario.id,
                stage: "hint-truncation",
                terminal: "served",
                markers: ["cf-delivery-hint-fragment-input"],
                detail: {
                    case_fragment: caseFragment,
                    fragment_utf16_units: caseFragment.length,
                    qualifier_kept: qualifierKept,
                },
            });

            const off = await driver.observeCold("hint-off", AGING_SERVING, {}, true);
            expect(off.verdict.refusals).toEqual(["history_absent"]);
            expect(hintOf(off)).toBe("");
            // The disabled pass decides no hint, where the enabled pass decided one.
            const offOutcome = await h.host.userHintOutcome();
            expect(offOutcome?.pass).not.toBe("decided");
            emitObservation({
                case: "C4",
                source: driver.sourceId,
                scenario: scenario.id,
                stage: "hint-off",
                terminal: "served",
                markers: ["cf-delivery-hints-disabled-input"],
                detail: {
                    hint_utf16_units: 0,
                    served_tier: off.verdict.tier,
                    serving: servingCost(off),
                },
            });
            console.log(`[cf-delivery] C4.S6 hint kept its qualifier: ${qualifierKept}`);
        },
        CASE_TIMEOUT_MS,
    );
});

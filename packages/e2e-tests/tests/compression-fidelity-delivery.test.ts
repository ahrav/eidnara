import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { join } from "node:path";
import {
    AGING_SERVING,
    CaseDriver,
    type Delivery,
    eidnaraConfig,
    M1_SERVING,
} from "../src/compression-fidelity/campaign";
import { capabilityDrift, surfaceOfRequest } from "../src/compression-fidelity/capabilities";
import {
    type FidelityCase,
    type FidelityScenario,
    readCompressionFidelityCorpus,
    type ServedTier,
} from "../src/compression-fidelity/corpus";
import { emitObservation, judgeDelivery } from "../src/compression-fidelity/delivery";
import { PI_PLUGIN_ROOT } from "../src/pi-runner/spawn";
import { RustTestHarness } from "../src/rust-harness";
import { rustPrereqs } from "../src/rust-scenario-support";

const QUALIFICATION_REQUIRED = process.env.EIDNARA_E2E_REQUIRE_FIDELITY === "1";
const CASE_TIMEOUT_MS = 540_000;
const TIER_RANK: Record<ServedTier | "unmatched", number> = {
    p1: 1,
    p2: 2,
    p3: 3,
    p4: 4,
    p5: 5,
    unmatched: 0,
};

const corpus = readCompressionFidelityCorpus();

function caseOf(id: string): FidelityCase {
    const found = corpus.cases.find((entry) => entry.id === id);
    if (!found) throw new Error(`${id} is not in the corpus`);
    return found;
}

function decayScenarios(fidelityCase: FidelityCase, source: string): FidelityScenario[] {
    return fidelityCase.scenarios.filter(
        (scenario) =>
            scenario.source === source && ["natural", "omission"].includes(scenario.serving.path),
    );
}

function observation(
    driver: CaseDriver,
    scenario: string,
    delivery: Delivery,
    markers: string[],
): void {
    emitObservation({
        case: driver.fidelityCase.id,
        source: driver.sourceId,
        scenario,
        stage: delivery.label,
        terminal: delivery.verdict.refusals.some((refusal) => refusal !== "history_absent")
            ? "unqualified"
            : "served",
        markers,
        detail: {
            config: delivery.config,
            history_budget_tokens: delivery.budget,
            stage: delivery.stage,
            served_tier: delivery.verdict.tier,
            curve_tier: delivery.curve,
            newer_rows: delivery.newer,
            refusals: delivery.verdict.refusals,
            leaks: delivery.verdict.leaks,
            segment_utf8_bytes: Buffer.byteLength(delivery.segment),
            pass: {
                decision: delivery.pass.decision,
                reason: delivery.pass.reason,
                served_from: delivery.pass.servedFrom,
                applied: delivery.pass.applied,
                admission: delivery.pass.admission,
                invocation_bytes: delivery.pass.invocationBytes,
                invocation_charged_tokens: delivery.pass.invocationCharged,
                estimator: "opencode-heuristic utf8-bytes-div-3.5-v1",
            },
            request_body_utf8_bytes: Buffer.byteLength(
                JSON.stringify(delivery.capture.request.body),
            ),
        },
    });
}

function expectCredited(delivery: Delivery, tier: ServedTier): void {
    const refusals = delivery.verdict.refusals.filter(
        (refusal) => !(tier === "p5" && refusal === "history_absent"),
    );
    expect(refusals).toEqual([]);
    expect(delivery.pass.applied).toBe(true);
    expect(delivery.pass.servedFrom).toBe("transform");
    expect(["fits", "shrinks", "limit_unknown"]).toContain(delivery.pass.admission);
    expect(delivery.pass.invocationCharged).toBeGreaterThan(0);
}

async function expectPinnedCapabilities(delivery: Delivery): Promise<void> {
    const { PI_TRANSFORM_AVAILABLE } = (await import(join(PI_PLUGIN_ROOT, "src/index.ts"))) as {
        PI_TRANSFORM_AVAILABLE: boolean;
    };
    expect(
        capabilityDrift(surfaceOfRequest(delivery.capture.request.body, PI_TRANSFORM_AVAILABLE)),
    ).toEqual([]);
}

async function decayCampaign(h: RustTestHarness, caseId: string, source: string): Promise<void> {
    const fidelityCase = caseOf(caseId);
    const scenarios = decayScenarios(fidelityCase, source);
    const m1Scenario = scenarios.find((scenario) => scenario.serving.stage === "m1");
    const first = m1Scenario ?? scenarios[0];
    if (!first) throw new Error(`${source} has no decay scenario`);
    const driver = await CaseDriver.open(h, fidelityCase, first);
    await driver.baseline();
    await driver.seed();
    await driver.publish(first.id);

    if (m1Scenario) {
        const m1 = await driver.observe("m1");
        expectCredited(m1, "p1");
        await expectPinnedCapabilities(m1);
        expect([m1.stage, m1.verdict.tier]).toEqual(["m1", "p1"]);
        observation(driver, m1Scenario.id, m1, ["cf-delivery-m1-published-input"]);

        const warm = await driver.observe("warm");
        expectCredited(warm, "p1");
        expect([warm.stage, warm.newer, warm.segment]).toEqual(["m1", m1.newer, m1.segment]);
        observation(driver, m1Scenario.id, warm, ["cf-delivery-warm-repeat-input"]);

        const cold = await driver.observeCold("cold-m0", M1_SERVING);
        expectCredited(cold, "p1");
        expect([cold.stage, cold.verdict.tier, cold.pass.decision]).toEqual(["m0", "p1", "HARD"]);
        expect(cold.segment).toBe(m1.segment);
        observation(driver, m1Scenario.id, cold, []);
    }

    const budget = (await driver.observeCold("aging-budget", AGING_SERVING)).budget;
    expect(budget).toBeGreaterThan(0);
    const decayRows = scenarios
        .filter((scenario) => scenario.serving.stage === "m0")
        .sort((a, b) => TIER_RANK[a.serving.tier ?? "p1"] - TIER_RANK[b.serving.tier ?? "p1"]);
    expect(decayRows.length).toBeGreaterThan(0);
    for (const scenario of decayRows) {
        const tier = scenario.serving.tier ?? "p1";
        const delivery = await driver.ageTo(tier, budget);
        if (!delivery) throw new Error(`${scenario.id}: ${tier} was passed before it was served`);
        expectCredited(delivery, tier);
        expect(delivery.budget).toBe(budget);
        expect([delivery.verdict.tier, delivery.curve]).toEqual([tier, tier]);
        expect(delivery.stage).toBe(tier === "p5" ? "absent" : "m0");
        if (tier === "p4") expect(delivery.segment.trim().split("\n")).toHaveLength(1);
        observation(driver, scenario.id, delivery, [
            "cf-delivery-m0-tier-inputs",
            ...(tier === "p4" ? ["cf-delivery-empty-p4-input"] : []),
            ...(tier === "p5" ? ["cf-delivery-natural-archive-input"] : []),
        ]);
    }
}

describe("compression fidelity delivery campaign prerequisites", () => {
    it.skipIf(!QUALIFICATION_REQUIRED)("are present when EIDNARA_E2E_REQUIRE_FIDELITY=1", () => {
        expect(rustPrereqs.skipReason ?? "present").toBe("present");
    });
});

describe.skipIf(!rustPrereqs.ok)("compression fidelity delivery campaign", () => {
    let h: RustTestHarness;

    beforeAll(async () => {
        h = await RustTestHarness.create({
            modelContextLimit: M1_SERVING.contextLimit,
            eidnaraConfig: eidnaraConfig(M1_SERVING),
        });
    }, 600_000);

    afterAll(async () => {
        await h?.dispose();
    });

    for (const [caseId, source] of [
        ["C1", "C1.V1"],
        ["C2", "C2.V1"],
        ["C2", "C2.V2"],
        ["C3", "C3.V1"],
        ["C4", "C4.V1"],
        ["C5", "C5.V1"],
        ["C6", "C6.V1"],
    ] as const) {
        it(
            `${source} reaches its m1, warm, cold m0, and natural decay rows`,
            async () => decayCampaign(h, caseId, source),
            CASE_TIMEOUT_MS,
        );
    }

    for (const scenarioId of ["C1.S5", "C3.S5"]) {
        it(
            `${scenarioId} serves its row demoted by a positive history budget`,
            async () => {
                const fidelityCase = caseOf(scenarioId.split(".")[0] ?? "");
                const scenario = fidelityCase.scenarios.find((entry) => entry.id === scenarioId);
                if (!scenario) throw new Error(`${scenarioId} is not in the corpus`);
                const driver = await CaseDriver.open(h, fidelityCase, scenario);
                await driver.seed();
                await driver.publish(scenario.id);
                const before = await driver.observeCold("pressure-before", M1_SERVING);
                expect([before.stage, before.verdict.tier]).toEqual(["m0", "p1"]);
                const pressed = await driver.pressure();
                expectCredited(pressed, "p5");
                expect(pressed.curve).not.toBe("p5");
                expect(TIER_RANK[pressed.verdict.tier ?? "p1"]).toBeGreaterThan(
                    TIER_RANK[pressed.curve],
                );
                observation(driver, scenario.id, pressed, [
                    ...(driver.importance >= 80
                        ? ["cf-delivery-high-importance-pressure-input"]
                        : []),
                ]);
            },
            CASE_TIMEOUT_MS,
        );
    }

    it(
        "serves a P1-only publication's inherited body at P2 and refuses a missing capture",
        async () => {
            const fidelityCase = caseOf("C1");
            const scenario = fidelityCase.scenarios.find((entry) => entry.id === "C1.S2");
            if (!scenario) throw new Error("C1.S2 is not in the corpus");
            const driver = await CaseDriver.open(h, fidelityCase, scenario);
            await driver.seed();
            await driver.publish(`${scenario.id}@p1-only`);
            const budget = (await driver.observeCold("aging-budget", AGING_SERVING)).budget;
            const delivery = await driver.ageTo("p2", budget);
            if (!delivery) throw new Error("C1 passed P2 before it was served");
            expect(delivery.curve).toBe("p2");
            expect(delivery.verdict.tier).toBe("p1");
            expect(delivery.verdict.refusals).toEqual([]);
            observation(driver, `${scenario.id}@p1-only`, delivery, [
                "cf-delivery-parser-fallback-input",
            ]);

            const missing = judgeDelivery({
                capture: h.retainedCaptures({ sessionId: "ses_never_driven" }).at(-1),
                pass: delivery.pass,
                title: driver.title,
                bodies: driver.bodies,
                leakProbes: driver.source?.leakProbes ?? [],
            });
            expect(missing.refusals).toEqual(["missing_capture"]);
        },
        CASE_TIMEOUT_MS,
    );
});

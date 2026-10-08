/**
 * Drives one compression fidelity case through the actual OpenCode invocation across serving
 * stages: a fixture-authored baseline, the scripted case publication, the m1 window, a warm repeat,
 * a cold rematerialization into m0, a budget-guard pressure pass, and natural decay to chosen
 * tiers behind exact counts of newer fixture-authored rows.
 *
 * Every observation is one provider request for the case's own session and follow-up, judged by
 * `judgeDelivery` and placed by the history wrapper that carries the case segment. Natural tiers
 * are checked against the decay oracle at the pass's history budget; a sparser served tier is
 * reported as guard pressure.
 */

import { waitFor } from "../harness-primitives";
import type { RetainedCapture, RustPassLine, RustTestHarness } from "../rust-harness";
import type { ScriptMessage, ScriptSource } from "../rust-runner/hermetic-host";
import type { FidelityCase, FidelityScenario, ServedTier } from "./corpus";
import { curveTier, tierWindows } from "./decay-oracle";
import {
    captureTexts,
    type DeliveryVerdict,
    judgeDelivery,
    publishedCount,
    reviewedTiers,
    seedSource,
    sessionPasses,
} from "./delivery";

export const CONTEXT_LIMIT = 30_000;
const THRESHOLD = 25;
/** The importance of every fixture-authored row; it matches the fixture's segments. */
export const FIXTURE_IMPORTANCE = 30;
const WAIT_MS = 120_000;
/**
 * Prompt-surface presets a cold observation alternates between. The preset changes the system
 * prompt, whose hash is part of the render config, so the next pass rematerializes m0.
 */
const COLD_SURFACES = ["light", "full"] as const;
/**
 * Queued one-row filler entries that answer later firings, including a firing's continuation
 * chunks, so the summarizer never backs off and each extra firing adds one row.
 */
const SPARE_FILLERS: string[] = Array(7).fill("filler:1");
/** Low-usage turns after a seeded source: enough tokens to make its chunk worth a firing. */
const TRAILING_TURNS = 4;
/** Synthetic message pairs an aging step seeds at least, so its chunk is worth a firing. */
const MIN_AGING_PAIRS = 8;
/** Turns allowed to bring a fire-ready summarizer pass before the campaign fails. */
const MAX_FIRE_TURNS = 6;

/** A pass configuration: the mock model's context limit and the plugin's budget inputs. */
export interface ServingConfig {
    label: "m1" | "aging" | "pressure";
    contextLimit: number;
    executeThreshold: number;
    historyBudgetPercentage: number;
}

/** A budget wide enough that the m1 window serves the newest publication. */
export const M1_SERVING: ServingConfig = {
    label: "m1",
    contextLimit: CONTEXT_LIMIT,
    executeThreshold: THRESHOLD,
    historyBudgetPercentage: 0.5,
};
/** A budget whose pressure decays rows within tens of newer rows while the body still fits. */
export const AGING_SERVING: ServingConfig = {
    label: "aging",
    contextLimit: CONTEXT_LIMIT,
    executeThreshold: THRESHOLD,
    historyBudgetPercentage: 0.1,
};
/**
 * The smallest positive history budget the plugin accepts at the campaign's context limit: the
 * minimum execute threshold and budget share. Newer rows with large bodies exceed it.
 */
export const PRESSURE_SERVING: ServingConfig = {
    label: "pressure",
    contextLimit: CONTEXT_LIMIT,
    executeThreshold: 20,
    historyBudgetPercentage: 0.05,
};

export function eidnaraConfig(
    config: ServingConfig,
    extra: Record<string, unknown> = {},
): Record<string, unknown> {
    return {
        history_summarizer: { model: "fixture/deterministic" },
        execute_threshold_percentage: config.executeThreshold,
        history_budget_percentage: config.historyBudgetPercentage,
        protected_tags: 1,
        memory: { auto_search: { enabled: false } },
        ...extra,
    };
}

/** Where a request serves the case segment. */
export type Stage = "m1" | "m0" | "absent";

/** One judged provider request of the campaign. */
export interface Delivery {
    label: string;
    config: ServingConfig["label"];
    budget: number;
    verdict: DeliveryVerdict;
    stage: Stage;
    /** Rows published after the case, read from session status. */
    newer: number;
    /** The curve tier the oracle expects at `newer` and `budget`. */
    curve: ServedTier;
    /** The served case segment, from its heading to the next heading or wrapper end. */
    segment: string;
    pass: RustPassLine;
    capture: RetainedCapture;
}

const M0_WRAPPER = /<session-history>[\s\S]*?<\/session-history>/;
const M1_WRAPPER = /<session-history-since>[\s\S]*?<\/session-history-since>/;

function segmentOf(texts: readonly string[], title: string): string {
    const text = texts.find((candidate) => candidate.includes(title)) ?? "";
    const at = text.indexOf(title);
    return at < 0 ? "" : (text.slice(at).split(/\n(?:## |<\/)/)[0] ?? "");
}

function stageOf(texts: readonly string[], title: string): Stage {
    for (const text of texts) {
        if (M1_WRAPPER.exec(text)?.[0].includes(title)) return "m1";
        if (M0_WRAPPER.exec(text)?.[0].includes(title)) return "m0";
    }
    return "absent";
}

/** The importance the fixture's `echo` answer gives its segments. */
const ECHO_IMPORTANCE = 50;

/** Synthetic user and assistant text pairs for `seedSource`, each text `text(k)`. */
function syntheticSource(pairs: number, step: number, text: (k: number) => string): ScriptSource {
    const messages: ScriptMessage[] = [];
    for (let k = 1; k <= pairs; k += 1) {
        for (const role of ["user", "assistant"] as const) {
            messages.push({
                info: { role },
                parts: [{ id: `f${step}-${k}-${role}`, type: "text", text: text(k) }],
            });
        }
    }
    return {
        corpusSha256: "",
        case: "fixture",
        scenario: "fixture",
        source: `synthetic-${step}`,
        messages,
        leakProbes: [],
    };
}

export class CaseDriver {
    private turn = 0;
    private agingStep = 0;
    /** Rows at the case's publication, the case included. */
    private rowsAtBind = 0;
    readonly title: string;
    readonly bodies: string[];
    readonly importance: number;
    readonly deliveries: Delivery[] = [];
    source: ScriptSource | null = null;
    private config: ServingConfig = M1_SERVING;
    private coldRestarts = 0;
    /** Importances of the rows published after the case that a step accounted for, oldest first. */
    private readonly newerLog: number[] = [];

    private constructor(
        readonly h: RustTestHarness,
        readonly sessionId: string,
        readonly fidelityCase: FidelityCase,
        readonly sourceId: string,
        readonly followUp: string,
    ) {
        const reviewed = fidelityCase.sources.find((entry) => entry.id === sourceId);
        if (!reviewed) throw new Error(`${sourceId} is not in ${fidelityCase.id}`);
        const tiers = reviewedTiers(reviewed.reviewedOutput);
        this.title = tiers.title;
        this.bodies = tiers.bodies;
        this.importance = Number(/importance="(\d+)"/.exec(reviewed.reviewedOutput)?.[1] ?? "50");
    }

    /** Opens a fresh session for `scenario`'s source and completes one small turn in it. */
    static async open(
        h: RustTestHarness,
        fidelityCase: FidelityCase,
        scenario: FidelityScenario,
    ): Promise<CaseDriver> {
        const sessionId = await h.createSession();
        const driver = new CaseDriver(
            h,
            sessionId,
            fidelityCase,
            scenario.source,
            scenario.followUp.prompt,
        );
        h.tagCaptures({ sessionId, caseId: fidelityCase.id, scenarioId: scenario.id });
        await driver.send("lead-in turn", 0.02);
        return driver;
    }

    private diagnostics = () => this.h.host.hostLog().slice(-4_000);

    private async send(text: string, usageFraction: number): Promise<void> {
        this.turn += 1;
        this.h.mock.setDefault({
            text: `assistant ${this.turn}`,
            usage: {
                input_tokens: Math.round(this.config.contextLimit * usageFraction),
                output_tokens: 20,
            },
        });
        await this.h.sendPrompt(this.sessionId, text);
    }

    private async settle(): Promise<void> {
        await waitFor(
            "the summarizer to settle",
            async () => ((await this.h.host.historySummarizerLive()) ? undefined : true),
            WAIT_MS,
            this.diagnostics,
        );
    }

    private async rows(): Promise<number> {
        const status = await this.h.host.primaryStatus(
            this.sessionId,
            this.h.env.workdir,
            "session.status",
        );
        return Number(status.history_segment_count ?? 0);
    }

    private async restart(
        config: ServingConfig,
        extra: Record<string, unknown> = {},
    ): Promise<void> {
        this.config = config;
        await this.h.restart({
            eidnaraConfig: eidnaraConfig(config, extra),
            modelContextLimit: config.contextLimit,
        });
    }

    /** Sends high-usage turns until a summarizer request consumes a queued entry. */
    private async fireUntilConsumed(queued: number, label: string): Promise<void> {
        for (let i = 1; i <= MAX_FIRE_TURNS; i += 1) {
            await this.send(`${label} turn ${i}`, 0.3);
            await this.settle();
            if ((await this.h.host.scriptStatus()).remaining < queued) return;
        }
        const status = await this.h.host.primaryStatus(
            this.sessionId,
            this.h.env.workdir,
            "session.status",
        );
        throw new Error(
            `${label}: no summarizer request in ${MAX_FIRE_TURNS} turns: ${String(status.summary)}`,
        );
    }

    /** Publishes fixture-authored baseline rows before the case exists. */
    async baseline(): Promise<void> {
        await this.h.host.scriptCases(Array(8).fill("filler"));
        for (let i = 1; i <= 20; i += 1) {
            await this.send(`baseline turn ${i}: ${this.h.ballast(40)}`, 0.3);
            if ((await publishedCount(this.h, this.sessionId)) > 0) break;
        }
        await waitFor(
            "the baseline to publish",
            async () => ((await publishedCount(this.h, this.sessionId)) > 0 ? true : undefined),
            WAIT_MS,
            this.diagnostics,
        );
        await this.settle();
    }

    /**
     * Seeds the case source after the baseline, restarts OpenCode under the m1 budget, and adds
     * low-usage turns that move the source out of the protected tail without firing.
     */
    async seed(): Promise<ScriptSource> {
        const scenario = this.fidelityCase.scenarios.find(
            (entry) => entry.source === this.sourceId,
        );
        if (!scenario) throw new Error(`no scenario names ${this.sourceId}`);
        const source = await this.h.host.scriptSource(scenario.id);
        seedSource(this.h, this.sessionId, source);
        await this.restart(M1_SERVING);
        for (let i = 1; i <= TRAILING_TURNS; i += 1) {
            await this.send(`trailing turn ${i}: ${this.h.ballast(2_000)}`, 0.02);
        }
        this.source = source;
        return source;
    }

    /** Arms `entry` and fires the summarizer until the case binding is delivered and published. */
    async publish(entry: string): Promise<void> {
        const before = await publishedCount(this.h, this.sessionId);
        await this.h.host.scriptCases([entry, ...SPARE_FILLERS]);
        await this.fireUntilConsumed(8, "publication");
        const status = await waitFor(
            `the ${entry} binding`,
            async () => {
                const script = await this.h.host.scriptStatus();
                return script.bound + script.mismatched > 0 ? script : undefined;
            },
            WAIT_MS,
            this.diagnostics,
        );
        if (status.mismatched > 0 || status.bindings[0]?.scenario !== entry) {
            throw new Error(`${entry} did not bind: ${JSON.stringify(status)}`);
        }
        await waitFor(
            `the ${entry} publication`,
            async () =>
                (await publishedCount(this.h, this.sessionId)) > before ? true : undefined,
            WAIT_MS,
            this.diagnostics,
        );
        await this.settle();
        this.rowsAtBind = await this.rows();
    }

    /** Sends the follow-up and judges the request it produces. */
    async observe(label: string, usageFraction = 0.26): Promise<Delivery> {
        const source = this.source;
        if (!source) throw new Error("observe before seed");
        const passesBefore = sessionPasses(this.h, this.sessionId).length;
        // Rows the pass renders: a firing this pass starts publishes after the request.
        const newer = Math.max(0, (await this.rows()) - this.rowsAtBind);
        const prompt = `${this.followUp} (${label})`;
        await this.send(prompt, usageFraction);
        const pass = await waitFor(
            `the ${label} pass line`,
            () => sessionPasses(this.h, this.sessionId)[passesBefore],
            WAIT_MS,
            this.diagnostics,
        );
        const capture = this.h
            .retainedCaptures({ sessionId: this.sessionId })
            .filter((entry) => captureTexts(entry).at(-1)?.includes(prompt))
            .at(-1);
        if (!capture) throw new Error(`no ${label} capture for ${this.sessionId}`);
        const verdict = judgeDelivery({
            capture,
            pass,
            title: this.title,
            bodies: this.bodies,
            leakProbes: source.leakProbes,
        });
        const texts = captureTexts(capture);
        const budget = pass.historyBudget ?? 0;
        const delivery: Delivery = {
            label,
            config: this.config.label,
            budget,
            verdict,
            stage: stageOf(texts, this.title),
            newer,
            curve: curveTier(this.importancesNewestFirst(newer), newer + 1, budget),
            segment: segmentOf(texts, this.title),
            pass,
            capture,
        };
        this.deliveries.push(delivery);
        await this.settle();
        return delivery;
    }

    /**
     * Restarts OpenCode under `config` with the other prompt-surface preset, so the changed
     * render config rematerializes m0 on the next pass, and observes that pass.
     */
    async observeCold(label: string, config: ServingConfig): Promise<Delivery> {
        this.coldRestarts += 1;
        await this.restart(config, {
            prompt_surface: { default: COLD_SURFACES[this.coldRestarts % 2] },
        });
        return this.observe(label);
    }

    /**
     * Publishes at most `rows` fixture-authored rows after the case under the aging budget, in
     * one summarizer answer that covers every presented record. Returns the rows it added.
     */
    async age(rows: number): Promise<number> {
        if (rows <= 0) return 0;
        this.agingStep += 1;
        const before = await this.rows();
        const step = this.agingStep;
        seedSource(
            this.h,
            this.sessionId,
            syntheticSource(Math.max(rows, MIN_AGING_PAIRS), step, (k) => `aging ${step}.${k}`),
        );
        await this.restart(AGING_SERVING);
        await this.send(`aging ballast ${this.agingStep}: ${this.h.ballast(2_000)}`, 0.02);
        await this.h.host.scriptCases([`filler:${rows}`, ...SPARE_FILLERS]);
        await this.fireUntilConsumed(SPARE_FILLERS.length + 1, `aging ${this.agingStep}`);
        return (await this.rows()) - before;
    }

    /**
     * Every row's importance, newest first, with `newer` rows after the case. Rows no step
     * accounted for came from spare filler answers.
     */
    private importancesNewestFirst(newer: number): number[] {
        const logged = this.newerLog.slice(0, newer);
        const spare = Array<number>(Math.max(0, newer - logged.length)).fill(FIXTURE_IMPORTANCE);
        return [
            ...[...logged, ...spare].reverse(),
            this.importance,
            ...Array<number>(this.older).fill(FIXTURE_IMPORTANCE),
        ];
    }

    private async account(importance: number): Promise<void> {
        const newer = await this.newer();
        while (this.newerLog.length < newer) this.newerLog.push(importance);
    }

    /**
     * Ages the case under the aging budget until it serves `tier` by the curve, landing one row
     * inside the tier's window. Returns the delivery observed after a cold restart, or `null`
     * when the case already serves past `tier`.
     */
    async ageTo(tier: ServedTier, budget: number): Promise<Delivery | null> {
        const window = tierWindows(this.importance, this.older, FIXTURE_IMPORTANCE, budget).get(
            tier,
        );
        if (!window) throw new Error(`${tier} has no window at budget ${budget}`);
        const target = Math.min(window.first + 1, window.last);
        let newer = await this.newer();
        if (newer > window.last) return null;
        while (newer < target) {
            await this.age(target - newer);
            await this.account(FIXTURE_IMPORTANCE);
            newer = await this.newer();
        }
        return this.observeCold(`aging-${tier}`, AGING_SERVING);
    }

    /**
     * Publishes newer rows whose bodies repeat large synthetic messages, so a positive budget
     * below the history body forces the guard to demote the older case row, and observes the
     * next cold pass under that budget.
     */
    async pressure(): Promise<Delivery> {
        this.agingStep += 1;
        const step = this.agingStep;
        seedSource(
            this.h,
            this.sessionId,
            syntheticSource(2, step, (k) => `pressure ${step}.${k}: ${this.h.ballast(400)}`),
        );
        await this.restart(PRESSURE_SERVING);
        await this.send(`pressure ballast ${step}: ${this.h.ballast(2_000)}`, 0.02);
        await this.h.host.scriptCases(["echo", ...SPARE_FILLERS]);
        await this.fireUntilConsumed(SPARE_FILLERS.length + 1, `pressure ${step}`);
        await this.account(ECHO_IMPORTANCE);
        return this.observeCold("pressure", PRESSURE_SERVING);
    }

    /** Rows published after the case. */
    async newer(): Promise<number> {
        return Math.max(0, (await this.rows()) - this.rowsAtBind);
    }

    /** Rows published before the case, the lead-in included. */
    get older(): number {
        return this.rowsAtBind - 1;
    }
}

/**
 * Drives one compression fidelity case through real OpenCode provider requests across serving
 * stages: a fixture-authored baseline, the scripted case publication, the m1 window, a warm
 * repeat, a cold rematerialization into m0, a budget-guard pressure pass, and natural decay to
 * chosen tiers behind counted newer fixture-authored rows.
 *
 * Every observation is one provider request for the case's own session and follow-up, judged by
 * `judgeDelivery` and placed by the history wrapper that carries the case segment. A row ledger
 * records each published row's importance, as the fixture reports it, so the decay oracle can
 * compute the curve tier at the pass's own history budget.
 */

import { waitFor } from "../harness-primitives";
import type { RetainedCapture, RustPassLine, RustTestHarness } from "../rust-harness";
import type { ScriptMessage, ScriptSource, ScriptStatus } from "../rust-runner/hermetic-host";
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
    stageOf,
} from "./delivery";

export const CONTEXT_LIMIT = 30_000;
const THRESHOLD = 25;
const WAIT_MS = 120_000;
/**
 * Prompt-surface presets a cold observation alternates between. The preset changes the system
 * prompt, whose hash is part of the render config, so the next pass rematerializes m0.
 */
const COLD_SURFACES = ["light", "full"] as const;
/**
 * Spare one-row filler entries answer later firings and their continuation chunks, keeping the
 * summarizer active and adding one row per extra firing. With the entry in front they fill the
 * fixture's eight-entry queue.
 */
const SPARE_FILLERS: string[] = Array(7).fill("filler:1");
const QUEUE_DEPTH = SPARE_FILLERS.length + 1;
/** Low-usage turns after a seeded source: enough tokens to make its chunk worth a firing. */
const TRAILING_TURNS = 4;
/** Synthetic message pairs an aging step seeds at least, so its chunk is worth a firing. */
const MIN_AGING_PAIRS = 8;
/** Turns allowed to bring a fire-ready summarizer pass before the campaign fails. */
const MAX_FIRE_TURNS = 6;
/**
 * EXECUTE_USAGE exceeds the 25% execute threshold; QUIET_USAGE stays below the proactive band;
 * FIRE_USAGE triggers summarization.
 */
const EXECUTE_USAGE = 0.26;
const QUIET_USAGE = 0.02;
const FIRE_USAGE = 0.3;

export interface ServingConfig {
    label: "m1" | "aging" | "pressure";
    executeThreshold: number;
    historyBudgetPercentage: number;
}

/** A budget wide enough that the m1 window serves the newest publication. */
export const M1_SERVING: ServingConfig = {
    label: "m1",
    executeThreshold: THRESHOLD,
    historyBudgetPercentage: 0.5,
};
/** A budget whose pressure decays rows within tens of newer rows while the body still fits. */
export const AGING_SERVING: ServingConfig = {
    label: "aging",
    executeThreshold: THRESHOLD,
    historyBudgetPercentage: 0.1,
};
/**
 * The smallest positive history budget the plugin accepts at the campaign's context limit: the
 * minimum execute threshold and budget share. Newer rows with large bodies exceed it.
 */
export const PRESSURE_SERVING: ServingConfig = {
    label: "pressure",
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

export interface Delivery {
    label: string;
    config: ServingConfig["label"];
    budget: number;
    verdict: DeliveryVerdict;
    stage: "m1" | "m0" | "absent";
    /** Rows published after the case, read from session status before the request. */
    newer: number;
    curve: ServedTier;
    segment: string;
    pass: RustPassLine;
    capture: RetainedCapture;
}

function segmentOf(texts: readonly string[], title: string): string {
    const text = texts.find((candidate) => candidate.includes(title)) ?? "";
    const at = text.indexOf(title);
    return at < 0 ? "" : (text.slice(at).split(/\n(?:## |<\/)/)[0] ?? "");
}

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
        memoryExamples: [],
    };
}

export class CaseDriver {
    private turn = 0;
    private step = 0;
    private coldRestarts = 0;
    private config: ServingConfig = M1_SERVING;
    /** Rows published before the case, the lead-in included. */
    private olderRows = 0;
    /** Importances of the rows published after the case, oldest first. */
    private readonly newerRows: number[] = [];
    private fillerImportance = 0;
    readonly title: string;
    readonly bodies: string[];
    readonly importance: number;
    source: ScriptSource | null = null;

    private constructor(
        readonly h: RustTestHarness,
        readonly sessionId: string,
        readonly fidelityCase: FidelityCase,
        readonly scenario: FidelityScenario,
    ) {
        const reviewed = fidelityCase.sources.find((entry) => entry.id === scenario.source);
        if (!reviewed) throw new Error(`${scenario.source} is not in ${fidelityCase.id}`);
        const tiers = reviewedTiers(reviewed.reviewedOutput);
        this.title = tiers.title;
        this.bodies = tiers.bodies;
        this.importance = tiers.importance;
    }

    get sourceId(): string {
        return this.scenario.source;
    }

    get older(): number {
        return this.olderRows;
    }

    /**
     * Restarts OpenCode under the m1 budget to isolate each case's serving configuration, then
     * opens a fresh session for `scenario`'s source with one small completed turn.
     */
    static async open(
        h: RustTestHarness,
        fidelityCase: FidelityCase,
        scenario: FidelityScenario,
    ): Promise<CaseDriver> {
        await h.restart({ eidnaraConfig: eidnaraConfig(M1_SERVING) });
        const sessionId = await h.createSession();
        const driver = new CaseDriver(h, sessionId, fidelityCase, scenario);
        h.tagCaptures({ sessionId, caseId: fidelityCase.id, scenarioId: scenario.id });
        await driver.send("lead-in turn", QUIET_USAGE);
        return driver;
    }

    private diagnostics = () => this.h.host.hostLog().slice(-4_000);

    private async send(text: string, usageFraction: number): Promise<void> {
        this.turn += 1;
        this.h.mock.setDefault({
            text: `assistant ${this.turn}`,
            usage: { input_tokens: Math.round(CONTEXT_LIMIT * usageFraction), output_tokens: 20 },
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

    private status(): Promise<Record<string, unknown>> {
        return this.h.host.primaryStatus(this.sessionId, this.h.env.workdir, "session.status");
    }

    private async rows(): Promise<number> {
        return Number((await this.status()).history_segment_count ?? 0);
    }

    private async restart(
        config: ServingConfig,
        extra: Record<string, unknown> = {},
    ): Promise<void> {
        this.config = config;
        await this.h.restart({ eidnaraConfig: eidnaraConfig(config, extra) });
    }

    private async fireUntilConsumed(label: string): Promise<ScriptStatus> {
        for (let i = 1; i <= MAX_FIRE_TURNS; i += 1) {
            await this.send(`${label} turn ${i}`, FIRE_USAGE);
            await this.settle();
            const script = await this.h.host.scriptStatus();
            if (script.remaining < QUEUE_DEPTH) return script;
        }
        throw new Error(
            `${label}: no summarizer request in ${MAX_FIRE_TURNS} turns: ${String((await this.status()).summary)}`,
        );
    }

    private async publishStep(
        entry: string,
        label: string,
        importance: number,
    ): Promise<{ added: number; spares: number }> {
        const rowsBefore = await this.rows();
        const filledBefore = (await this.h.host.scriptStatus()).filled;
        await this.h.host.scriptCases([entry, ...SPARE_FILLERS]);
        const fired = await this.fireUntilConsumed(label);
        this.fillerImportance = fired.fillerImportance;
        const answered = (await this.h.host.scriptStatus()).filled - filledBefore;
        const ownFilled = entry.startsWith("filler") || entry.startsWith("echo") ? 1 : 0;
        const spares = Math.max(0, answered - ownFilled);
        const added = (await this.rows()) - rowsBefore;
        this.newerRows.push(...Array<number>(Math.max(0, added - spares)).fill(importance));
        this.newerRows.push(...Array<number>(spares).fill(fired.fillerImportance));
        return { added, spares };
    }

    async baseline(): Promise<void> {
        await this.h.host.scriptCases(Array(QUEUE_DEPTH).fill("filler"));
        for (let i = 1; i <= 20; i += 1) {
            await this.send(`baseline turn ${i}: ${this.h.ballast(40)}`, FIRE_USAGE);
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
        const source = await this.h.host.scriptSource(this.scenario.id);
        seedSource(this.h, this.sessionId, source);
        await this.restart(M1_SERVING);
        for (let i = 1; i <= TRAILING_TURNS; i += 1) {
            await this.send(`trailing turn ${i}: ${this.h.ballast(2_000)}`, QUIET_USAGE);
        }
        this.source = source;
        return source;
    }

    async publish(entry: string): Promise<void> {
        const publishedBefore = await publishedCount(this.h, this.sessionId);
        const rowsBefore = await this.rows();
        const { added, spares } = await this.publishStep(entry, "publication", 0);
        const script = await this.h.host.scriptStatus();
        if (script.mismatched > 0 || script.bindings[0]?.scenario !== entry) {
            throw new Error(`${entry} did not bind: ${JSON.stringify(script)}`);
        }
        if ((await publishedCount(this.h, this.sessionId)) <= publishedBefore) {
            throw new Error(`${entry} bound but did not publish`);
        }
        // The case answer adds the lead-in and the case; only spare rows are newer.
        this.newerRows.length = 0;
        this.newerRows.push(...Array<number>(spares).fill(this.fillerImportance));
        this.olderRows = rowsBefore + (added - spares) - 1;
    }

    async newer(): Promise<number> {
        return Math.max(0, (await this.rows()) - this.olderRows - 1);
    }

    private importancesNewestFirst(newer: number): number[] {
        const recorded = this.newerRows.slice(0, newer);
        const spare = Array<number>(Math.max(0, newer - recorded.length)).fill(
            this.fillerImportance,
        );
        return [
            ...[...recorded, ...spare].reverse(),
            this.importance,
            ...Array<number>(this.olderRows).fill(this.fillerImportance),
        ];
    }

    /**
     * Sends the follow-up, labeled by default; `verbatim` sends the corpus prompt as written,
     * for a request whose search terms must be the prompt's alone. The delivery judges the
     * first capture retained after the send that carries the prompt.
     */
    async observe(
        label: string,
        usageFraction = EXECUTE_USAGE,
        verbatim = false,
    ): Promise<Delivery> {
        const source = this.source;
        if (!source) throw new Error("observe before seed");
        const passesBefore = sessionPasses(this.h, this.sessionId).length;
        const capturesBefore = this.h.retainedCaptures({ sessionId: this.sessionId }).length;
        const newer = await this.newer();
        const prompt = verbatim
            ? this.scenario.followUp.prompt
            : `${this.scenario.followUp.prompt} (${label})`;
        await this.send(prompt, usageFraction);
        const pass = await waitFor(
            `the ${label} pass line`,
            () => sessionPasses(this.h, this.sessionId)[passesBefore],
            WAIT_MS,
            this.diagnostics,
        );
        const capture = await waitFor(
            `the ${label} capture`,
            () =>
                this.h
                    .retainedCaptures({ sessionId: this.sessionId })
                    .slice(capturesBefore)
                    .find((entry) => captureTexts(entry).at(-1)?.includes(prompt)),
            WAIT_MS,
            this.diagnostics,
        );
        const texts = captureTexts(capture);
        const budget = pass.historyBudget ?? 0;
        const delivery: Delivery = {
            label,
            config: this.config.label,
            budget,
            verdict: judgeDelivery({
                capture,
                pass,
                title: this.title,
                bodies: this.bodies,
                leakProbes: source.leakProbes,
            }),
            stage: stageOf(texts, this.title),
            newer,
            curve: curveTier(this.importancesNewestFirst(newer), newer + 1, budget),
            segment: segmentOf(texts, this.title),
            pass,
            capture,
        };
        await this.settle();
        return delivery;
    }

    /**
     * Restarts OpenCode under `config` with the other prompt-surface preset, so the changed
     * render config rematerializes m0 on the next pass, and observes that pass at usage below
     * the proactive band.
     */
    async observeCold(
        label: string,
        config: ServingConfig,
        extra: Record<string, unknown> = {},
        verbatim = false,
    ): Promise<Delivery> {
        this.coldRestarts += 1;
        await this.restart(config, {
            prompt_surface: { default: COLD_SURFACES[this.coldRestarts % 2] },
            ...extra,
        });
        return this.observe(label, QUIET_USAGE, verbatim);
    }

    private async age(rows: number): Promise<number> {
        this.step += 1;
        const step = this.step;
        seedSource(
            this.h,
            this.sessionId,
            syntheticSource(Math.max(rows, MIN_AGING_PAIRS), step, (k) => `aging ${step}.${k}`),
        );
        await this.restart(AGING_SERVING);
        await this.send(`aging ballast ${step}: ${this.h.ballast(2_000)}`, QUIET_USAGE);
        const { added } = await this.publishStep(
            `filler:${rows}`,
            `aging ${step}`,
            this.fillerImportance,
        );
        return added;
    }

    /**
     * Ages the case under the aging budget until the curve serves `tier`, landing one row inside
     * the tier's window, and returns the delivery observed after a cold restart.
     */
    async ageTo(tier: ServedTier, budget: number): Promise<Delivery> {
        const window = tierWindows(
            this.importance,
            this.olderRows,
            this.fillerImportance,
            budget,
        ).get(tier);
        if (!window) throw new Error(`${tier} has no window at budget ${budget}`);
        const target = Math.min(window.first + 1, window.last);
        let newer = await this.newer();
        while (newer < target) {
            if ((await this.age(target - newer)) <= 0) {
                throw new Error(`aging toward ${tier} added no row at newer=${newer}`);
            }
            newer = await this.newer();
        }
        if (newer > window.last) {
            throw new Error(
                `${tier} overshot: newer=${newer} window=${window.first}-${window.last}`,
            );
        }
        return this.observeCold(`aging-${tier}`, AGING_SERVING);
    }

    /**
     * Publishes newer rows whose bodies repeat `pairs` synthetic message pairs, through the
     * fixture's echo answer under `config`, in at most `rows` rows when given. `text` receives
     * the step and the pair index. Returns the rows it added.
     */
    async publishEchoed(
        label: string,
        pairs: number,
        text: (step: number, k: number) => string,
        config: ServingConfig,
        rows?: number,
    ): Promise<number> {
        this.step += 1;
        const step = this.step;
        seedSource(
            this.h,
            this.sessionId,
            syntheticSource(pairs, step, (k) => text(step, k)),
        );
        await this.restart(config);
        await this.send(`${label} ballast ${step}: ${this.h.ballast(2_000)}`, QUIET_USAGE);
        const echoImportance = (await this.h.host.scriptStatus()).echoImportance;
        const entry = rows === undefined ? "echo" : `echo:${rows}`;
        const { added } = await this.publishStep(entry, `${label} ${step}`, echoImportance);
        return added;
    }

    /**
     * Publishes newer rows whose bodies repeat large synthetic messages, so a positive budget
     * below the history body forces the guard to demote the older case row, and observes the
     * next cold pass under that budget.
     */
    async pressure(): Promise<Delivery> {
        await this.publishEchoed(
            "pressure",
            2,
            (step, k) => `pressure ${step}.${k}: ${this.h.ballast(400)}`,
            PRESSURE_SERVING,
        );
        return this.observeCold("pressure", PRESSURE_SERVING);
    }
}

/**
 * Drives one compression fidelity case through real OpenCode provider requests across serving
 * stages: a fixture-authored baseline, the scripted case publication, the m1 window, a warm
 * repeat, a cold rematerialization into m0, a budget-guard pressure pass, and natural decay to
 * chosen tiers behind counted newer fixture-authored rows.
 *
 * Every observation is one provider request for the case's own session and follow-up, judged by
 * `judgeDelivery` and placed by the history wrapper that carries the case segment. A cold
 * observation restarts OpenCode, and so does a change of serving config. Every other rebuild of
 * m0 happens in the running OpenCode: the session's requests carry new system text, so the next
 * pass sees a changed render config. A row ledger records each published row's importance, as
 * the fixture reports it, so the decay oracle can compute the curve tier at the pass's own
 * history budget.
 */

import { waitFor } from "../harness-primitives";
import { type RetainedCapture, type RustPassLine, RustTestHarness } from "../rust-harness";
import type { ScriptMessage, ScriptSource, ScriptStatus } from "../rust-runner/hermetic-host";
import type { FidelityCase, FidelityScenario, ServedTier } from "./corpus";
import { curveTier, tierWindows } from "./decay-oracle";
import {
    captureTexts,
    type DeliveryVerdict,
    historyHeadings,
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
/**
 * Completed turns seeded before the baseline's firing turns. The summarizer's proactive trigger
 * needs twelve messages outside the protected tail, and these supply them from the store, so the
 * baseline prompts only the few turns that move them out of that tail.
 */
const BASELINE_TURNS = 12;
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
/**
 * The plugin's logger writes buffered lines every 500 ms, so a pass line can trail its response
 * by that long. A pass count read sooner after a request can miss that request's line.
 */
const PASS_LOG_FLUSH_MS = 600;

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

/**
 * Runs `drive` against its own OpenCode, direct host, and mock provider, started under `serving`,
 * and disposes of them afterward. A case that owns its stack inherits no serving config, session,
 * or capture from another case, so cases can run concurrently.
 */
export async function withCaseHarness(
    serving: ServingConfig,
    drive: (h: RustTestHarness) => Promise<void>,
): Promise<void> {
    const h = await RustTestHarness.create({
        modelContextLimit: CONTEXT_LIMIT,
        eidnaraConfig: eidnaraConfig(serving),
    });
    try {
        await drive(h);
    } finally {
        await h.dispose();
    }
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
    /** The row headings served inside the history wrappers. */
    headings: string[];
    pass: RustPassLine;
    capture: RetainedCapture;
}

function segmentOf(texts: readonly string[], title: string): string {
    const text = texts.find((candidate) => candidate.includes(title)) ?? "";
    const at = text.indexOf(title);
    return at < 0 ? "" : (text.slice(at).split(/\n(?:## |<\/)/)[0] ?? "");
}

/** `pairs` completed turns, user text `user(k)` and assistant text `assistant(k)` for turn `k`. */
function syntheticSource(
    pairs: number,
    name: string,
    user: (k: number) => string,
    assistant: (k: number) => string = user,
): ScriptSource {
    const messages: ScriptMessage[] = [];
    for (let k = 1; k <= pairs; k += 1) {
        for (const [role, text] of [
            ["user", user(k)],
            ["assistant", assistant(k)],
        ] as const) {
            messages.push({
                info: { role },
                parts: [{ id: `${name}-${k}-${role}`, type: "text", text }],
            });
        }
    }
    return {
        corpusSha256: "",
        case: "fixture",
        scenario: "fixture",
        source: `synthetic-${name}`,
        messages,
        leakProbes: [],
    };
}

/**
 * One completed turn whose user text is about 2,000 tokens of ballast. Seeded after newer rows,
 * it fills the protected tail so the rows before it are eligible for the next firing.
 */
function ballastTurn(h: RustTestHarness, name: string): ScriptSource {
    return syntheticSource(
        1,
        name,
        () => `${name}: ${h.ballast(2_000)}`,
        () => `assistant ${name}`,
    );
}

export class CaseDriver {
    private turn = 0;
    private step = 0;
    private coldRestarts = 0;
    /** System text every request carries; each rebuild names a new surface. */
    private surface: string | undefined;
    private rebuilds = 0;
    private lastSendAt = 0;
    private config: ServingConfig;
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
        serving: ServingConfig,
    ) {
        this.config = serving;
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
     * Opens a session for `scenario`'s source with one small completed turn in `h`, a stack from
     * {@link withCaseHarness} started under `serving`.
     */
    static async open(
        h: RustTestHarness,
        fidelityCase: FidelityCase,
        scenario: FidelityScenario,
        serving: ServingConfig,
    ): Promise<CaseDriver> {
        const sessionId = await h.createSession();
        const driver = new CaseDriver(h, sessionId, fidelityCase, scenario, serving);
        h.tagCaptures({ sessionId, caseId: fidelityCase.id, scenarioId: scenario.id });
        await driver.send("lead-in turn", QUIET_USAGE);
        return driver;
    }

    private diagnostics = () => this.h.host.hostLog().slice(-4_000);

    /**
     * Sends one turn, then waits until no summarizer firing is live. A pass claims the firing it
     * starts before it returns, so the next turn's pass never commits while that firing
     * publishes; an overlapping commit fails the firing on a row-version conflict, and the daemon
     * then holds every firing for its 60 s failure backoff.
     */
    private async send(text: string, usageFraction: number): Promise<void> {
        this.turn += 1;
        this.h.mock.setDefault({
            text: `assistant ${this.turn}`,
            usage: { input_tokens: Math.round(CONTEXT_LIMIT * usageFraction), output_tokens: 20 },
        });
        await this.h.sendPrompt(this.sessionId, text, { system: this.surface });
        this.lastSendAt = Date.now();
        await this.settle();
    }

    /** The session's pass lines once every earlier request's line has been written. */
    private async flushedPasses(): Promise<number> {
        await Bun.sleep(Math.max(0, this.lastSendAt + PASS_LOG_FLUSH_MS - Date.now()));
        return sessionPasses(this.h, this.sessionId).length;
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
        const ownFilled = entry.startsWith("filler") || entry === "echo" ? 1 : 0;
        const spares = Math.max(0, answered - ownFilled);
        const added = (await this.rows()) - rowsBefore;
        this.newerRows.push(...Array<number>(Math.max(0, added - spares)).fill(importance));
        this.newerRows.push(...Array<number>(spares).fill(fired.fillerImportance));
        return { added, spares };
    }

    async baseline(): Promise<void> {
        await this.h.host.scriptCases(Array(QUEUE_DEPTH).fill("filler"));
        seedSource(
            this.h,
            this.sessionId,
            syntheticSource(
                BASELINE_TURNS,
                "baseline",
                (k) => `baseline turn ${k}: ${this.h.ballast(40)}`,
                (k) => `assistant baseline ${k}`,
            ),
        );
        for (let i = BASELINE_TURNS + 1; i <= BASELINE_TURNS + 20; i += 1) {
            await this.send(`baseline turn ${i}: ${this.h.ballast(40)}`, FIRE_USAGE);
            if ((await publishedCount(this.h, this.sessionId)) > 0) break;
        }
        await waitFor(
            "the baseline to publish",
            async () => ((await publishedCount(this.h, this.sessionId)) > 0 ? true : undefined),
            WAIT_MS,
            this.diagnostics,
        );
    }

    /**
     * Seeds the case source after the baseline, and adds low-usage turns that move the source out
     * of the protected tail without firing.
     */
    async seed(): Promise<ScriptSource> {
        const source = await this.h.host.scriptSource(this.scenario.id);
        seedSource(this.h, this.sessionId, source);
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

    async observe(label: string, usageFraction = EXECUTE_USAGE): Promise<Delivery> {
        const source = this.source;
        if (!source) throw new Error("observe before seed");
        const passesBefore = await this.flushedPasses();
        const newer = await this.newer();
        const prompt = `${this.scenario.followUp.prompt} (${label})`;
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
        const texts = captureTexts(capture);
        const budget = pass.historyBudget ?? 0;
        return {
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
            headings: historyHeadings(texts),
            pass,
            capture,
        };
    }

    /**
     * Restarts OpenCode under `config` with the other prompt-surface preset, so the changed
     * render config rematerializes m0 on the next pass, and observes that pass at usage below
     * the proactive band.
     */
    async observeCold(label: string, config: ServingConfig): Promise<Delivery> {
        this.coldRestarts += 1;
        await this.restart(config, {
            prompt_surface: { default: COLD_SURFACES[this.coldRestarts % 2] },
        });
        return this.observe(label, QUIET_USAGE);
    }

    /**
     * Gives the session's requests new system text and sends one quiet turn, which records the
     * new prompt hash after its own pass; the next pass carries a changed render config, so it
     * rebuilds m0 from every published row on a `HARD` pass. That pass is observed at usage below
     * the proactive band.
     */
    async observeRebuilt(label: string): Promise<Delivery> {
        this.rebuilds += 1;
        this.surface = `Fidelity campaign surface ${this.rebuilds}.`;
        await this.send(`surface turn ${this.rebuilds}`, QUIET_USAGE);
        return this.observe(label, QUIET_USAGE);
    }

    private async age(rows: number): Promise<number> {
        this.step += 1;
        const step = this.step;
        seedSource(
            this.h,
            this.sessionId,
            syntheticSource(
                Math.max(rows, MIN_AGING_PAIRS),
                `aging-${step}`,
                (k) => `aging ${step}.${k}`,
            ),
        );
        seedSource(this.h, this.sessionId, ballastTurn(this.h, `aging ballast ${step}`));
        if (this.config !== AGING_SERVING) await this.restart(AGING_SERVING);
        const { added } = await this.publishStep(
            `filler:${rows}`,
            `aging ${step}`,
            this.fillerImportance,
        );
        return added;
    }

    /**
     * Ages the case under the aging budget until the curve serves `tier`, landing one row inside
     * the tier's window, and returns the delivery observed on the next rebuilt pass.
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
        return this.observeRebuilt(`aging-${tier}`);
    }

    /**
     * Publishes newer rows whose bodies repeat large synthetic messages under the current
     * budget, then observes the next cold pass under the pressure budget, which the history body
     * exceeds, so the guard demotes the older case row.
     */
    async pressure(): Promise<Delivery> {
        this.step += 1;
        const step = this.step;
        seedSource(
            this.h,
            this.sessionId,
            syntheticSource(
                2,
                `pressure-${step}`,
                (k) => `pressure ${step}.${k}: ${this.h.ballast(400)}`,
            ),
        );
        seedSource(this.h, this.sessionId, ballastTurn(this.h, `pressure ballast ${step}`));
        const echoImportance = (await this.h.host.scriptStatus()).echoImportance;
        await this.publishStep("echo", `pressure ${step}`, echoImportance);
        return this.observeCold("pressure", PRESSURE_SERVING);
    }
}

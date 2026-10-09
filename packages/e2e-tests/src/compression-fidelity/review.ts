/**
 * The review, cost, and comparison layer behind `eval:compression-fidelity`. Over the rows the
 * evidence layer derives, it binds human judgments by artifact hash, checks recorded control
 * verdicts against the sealed labels, derives the preservation, recovery, consumer safety,
 * semantic review, and cost columns, and compares baseline and candidate per scenario. An arm
 * is accepted only when every column of every scenario passes, review is qualified, two people
 * approved the batch, and its generation origin is real.
 */

import { join } from "node:path";
import type { BASELINE_COMPARISONS } from "../incident-pool/report";
import type { Disposition, FidelityCorpus } from "./corpus";
import {
    type Arm,
    assembleEvidence,
    type Evidence,
    type EvidenceRow,
    executed,
    isHintTruncation,
    type Json,
    named,
    REAL_CAPTURE,
    REPORT_SCHEMA,
    readJson,
    record,
    scenarios,
    servedTierOf,
    sha256,
    text,
} from "./evidence";

export const CONTROLS_SCHEMA = "eidnara.compression-fidelity-controls/v1";
export const JUDGMENTS_SCHEMA = "eidnara.compression-fidelity-judgments/v1";

/** Controls whose approved label is a violation, then controls whose label is acceptable. */
export const KNOWN_BAD_CONTROLS = [
    "reversed_negation",
    "planned_to_completed",
    "inferred_to_observed",
    "wrong_identity",
    "lost_hint_qualifier",
] as const;
export const POSITIVE_CONTROLS = [
    "meaning_preserving_paraphrase",
    "successful_deployment",
] as const;
/** Distinct people who must approve a batch before an arm in it can be accepted. */
export const REQUIRED_APPROVERS = 2;

type Comparison = (typeof BASELINE_COMPARISONS)[number];

export interface ObligationJudgment {
    obligation: string;
    disposition: Disposition;
    preserved: boolean;
}

export interface Judgment {
    id: string;
    kind: "human" | "model";
    reviewer: string;
    arm: string;
    scenario: string;
    /** SHA-256 of the observation file judged. */
    artifact_sha256: string;
    obligations: ObligationJudgment[];
    forbidden_violated: string[];
    abstained: boolean;
    citations?: string[];
    uncertainty?: string;
}

export interface Reviews {
    batch: string;
    approvals: string[];
    controls: Array<{ id: string; kind: string; label: string }>;
    verdicts: Array<{ control: string; verdict: string; reviewer: string; kind: string }>;
    judgments: Judgment[];
    disputes: Array<{ arm: string; scenario: string; resolved: boolean }>;
    sha256: { controls: string; judgments: string };
    errors: string[];
}

/** One scenario's columns for one arm, each derived from that arm's stored evidence. */
export interface ScenarioRow {
    scenario: string;
    case: string;
    execution: EvidenceRow["execution"];
    deterministic: EvidenceRow["deterministic"];
    preservation: Array<{
        obligation: string;
        disposition: Disposition | null;
        status: "preserved" | "recall" | "unreviewed";
    }>;
    recovery: "witnessed" | "not_required" | "unverified";
    consumer_safety: "safe" | "abstained" | "false-authoritative" | "unreviewed";
    semantic_review: "reviewed" | "model_only" | "disputed" | "unreviewed";
    cost: {
        status: "complete" | "incomplete";
        missing: string[];
        serving: Json[];
        recovery: Json[];
        generation: Json[];
    };
    withheld: string[];
}

const DISPOSITION_VALUES: readonly string[] = ["visible", "discoverable", "unavailable"];

function stringList(value: unknown): value is string[] {
    return Array.isArray(value) && value.every((v) => typeof v === "string");
}

function list(value: unknown, where: string, errors: string[]): unknown[] {
    if (Array.isArray(value)) return value;
    errors.push(`${where} is not a list`);
    return [];
}

/** `judgmentOf` rejects repeated obligations to prevent conflicting verdicts. */
function judgmentOf(value: unknown): Judgment | null {
    if (!record(value)) return null;
    const { id, kind, reviewer, arm, scenario, artifact_sha256, obligations, abstained } = value;
    if (!text(id) || !named(reviewer) || !text(arm) || !text(scenario) || !text(artifact_sha256)) {
        return null;
    }
    if ((kind !== "human" && kind !== "model") || typeof abstained !== "boolean") return null;
    if (!Array.isArray(obligations) || !stringList(value.forbidden_violated)) return null;
    if (value.citations !== undefined && !stringList(value.citations)) return null;
    // A model judgment carries the citations and uncertainty a reader needs to weigh it.
    if (kind === "model" && (!value.citations?.length || !value.citations.every(named)))
        return null;
    if (kind === "model" && !named(value.uncertainty)) return null;
    const judged: ObligationJudgment[] = [];
    const seen = new Set<string>();
    for (const o of obligations) {
        if (!record(o) || !text(o.obligation) || typeof o.preserved !== "boolean") return null;
        if (typeof o.disposition !== "string" || !DISPOSITION_VALUES.includes(o.disposition)) {
            return null;
        }
        if (seen.has(o.obligation)) return null;
        seen.add(o.obligation);
        judged.push({
            obligation: o.obligation,
            disposition: o.disposition as Disposition,
            preserved: o.preserved,
        });
    }
    return {
        id,
        kind,
        reviewer,
        arm,
        scenario,
        artifact_sha256,
        obligations: judged,
        forbidden_violated: value.forbidden_violated,
        abstained,
        citations: value.citations,
        uncertainty: typeof value.uncertainty === "string" ? value.uncertainty : undefined,
    };
}

/** Records in `list` whose every `fields` entry is a string, plus one error per other entry. */
function stringRecords<K extends string>(
    entries: unknown,
    fields: readonly K[],
    where: string,
    errors: string[],
): Array<Record<K, string>> {
    const kept: Array<Record<K, string>> = [];
    for (const value of list(entries, where, errors)) {
        if (record(value) && fields.every((field) => named(value[field]))) {
            kept.push(
                Object.fromEntries(fields.map((f) => [f, value[f] as string])) as Record<K, string>,
            );
        } else {
            errors.push(`${where} entry does not match its schema: ${JSON.stringify(value)}`);
        }
    }
    return kept;
}

/** Reads the sealed control labels and the judgments, refusing anything bound elsewhere. */
export async function loadReviews(dir: string, corpusSha256: string): Promise<Reviews> {
    const reviews: Reviews = {
        batch: "",
        approvals: [],
        controls: [],
        verdicts: [],
        judgments: [],
        disputes: [],
        sha256: { controls: "", judgments: "" },
        errors: [],
    };
    const [controls, judgments] = await Promise.all([
        readJson(join(dir, "controls.json")),
        readJson(join(dir, "judgments.json")),
    ]);
    // Each record's digest is recorded whenever its bytes were read, so a manifest names the
    // bytes behind a refused review as well as an accepted one.
    reviews.sha256 = {
        controls: controls.bytes ? sha256(controls.bytes) : "",
        judgments: judgments.bytes ? sha256(judgments.bytes) : "",
    };
    if ("error" in controls || "error" in judgments) {
        if ("error" in controls) reviews.errors.push(controls.error);
        if ("error" in judgments) reviews.errors.push(judgments.error);
        return reviews;
    }
    const c = record(controls.value) ? controls.value : {};
    const j = record(judgments.value) ? judgments.value : {};
    if (c.schema !== CONTROLS_SCHEMA || j.schema !== JUDGMENTS_SCHEMA) {
        reviews.errors.push("review records carry the wrong schema");
        return reviews;
    }
    for (const [name, value] of [
        ["controls.json", c],
        ["judgments.json", j],
    ] as const) {
        if (value.corpus_sha256 !== corpusSha256)
            reviews.errors.push(`${name} is bound to another corpus`);
        if (!named(value.batch)) reviews.errors.push(`${name} names no batch`);
    }
    reviews.batch = named(c.batch) ? c.batch : "";
    if (named(j.batch) && j.batch !== reviews.batch) {
        reviews.errors.push("judgments.json is bound to another batch");
    }
    const approvers = list(c.approved_by, "approved_by", reviews.errors);
    for (const approver of approvers) {
        if (!named(approver)) {
            reviews.errors.push(
                `an approver does not match its schema: ${JSON.stringify(approver)}`,
            );
        }
    }
    reviews.approvals = [...new Set(approvers.filter(named).map((approver) => approver.trim()))];
    reviews.controls = stringRecords(
        c.controls,
        ["id", "kind", "label"],
        "controls",
        reviews.errors,
    );
    reviews.verdicts = stringRecords(
        j.control_verdicts,
        ["control", "verdict", "reviewer", "kind"],
        "control_verdicts",
        reviews.errors,
    );
    for (const value of list(j.judgments, "judgments", reviews.errors)) {
        const judgment = judgmentOf(value);
        if (judgment) reviews.judgments.push(judgment);
        else reviews.errors.push(`a judgment does not match its schema: ${JSON.stringify(value)}`);
    }
    for (const value of list(j.disputes, "disputes", reviews.errors)) {
        if (
            record(value) &&
            text(value.arm) &&
            text(value.scenario) &&
            typeof value.resolved === "boolean"
        ) {
            reviews.disputes.push({
                arm: value.arm,
                scenario: value.scenario,
                resolved: value.resolved,
            });
        } else {
            reviews.errors.push(`a dispute does not match its schema: ${JSON.stringify(value)}`);
        }
    }
    return reviews;
}

/** Whether the recorded control verdicts match exactly the required sealed labels. */
export function controlQualification(reviews: Reviews): { qualified: boolean; problems: string[] } {
    const problems: string[] = [];
    const expected = new Map<string, string>([
        ...KNOWN_BAD_CONTROLS.map((kind) => [kind, "violation"] as const),
        ...POSITIVE_CONTROLS.map((kind) => [kind, "acceptable"] as const),
    ]);
    const ids = reviews.controls.map((control) => control.id);
    for (const id of new Set(ids.filter((id, i) => ids.indexOf(id) !== i))) {
        problems.push(`control ${id} is sealed more than once`);
    }
    for (const control of reviews.controls) {
        const label = expected.get(control.kind);
        if (!label) {
            problems.push(`${control.id} has undeclared kind ${control.kind}`);
            continue;
        }
        if (control.label !== label)
            problems.push(`${control.id} is labeled ${control.label}, not ${label}`);
        const verdicts = reviews.verdicts.filter((v) => v.control === control.id);
        if (verdicts.length !== 1 || verdicts[0]?.kind !== "human") {
            problems.push(`${control.id} needs exactly one human verdict, has ${verdicts.length}`);
        } else if (verdicts[0].verdict !== control.label) {
            problems.push(
                `${control.id} was judged ${verdicts[0].verdict}, labeled ${control.label}`,
            );
        }
    }
    for (const kind of expected.keys()) {
        const sealed = reviews.controls.filter((control) => control.kind === kind).length;
        if (sealed === 0) problems.push(`no sealed ${kind} control`);
        else if (sealed > 1) problems.push(`${kind} is sealed ${sealed} times`);
    }
    for (const verdict of reviews.verdicts) {
        if (!ids.includes(verdict.control))
            problems.push(`verdict for undeclared control ${verdict.control}`);
    }
    return { qualified: problems.length === 0 && reviews.errors.length === 0, problems };
}

/** The memory witness's recovery observation: an `eidnara_search` call that ended `discoverable`. */
function isRecovery(evidence: Evidence): boolean {
    return (
        evidence.owner === "opencode-delivery" &&
        evidence.stage === "recovery-eidnara-search" &&
        evidence.terminal === "discoverable"
    );
}

/** The witness searched memory through `eidnara_search`, as its recorded tool call shows. */
function searchedMemory(detail: Json): boolean {
    const args = detail.arguments;
    return (
        detail.tool === "eidnara_search" &&
        record(args) &&
        Array.isArray(args.sources) &&
        args.sources.includes("memory")
    );
}

const measurement = (v: unknown) => typeof v === "number" && Number.isFinite(v) && v >= 0;
/**
 * The qualification and Pi witnesses serve C1.S1 through fixtures that run no OpenCode admission
 * estimator, so their observations carry no serving cost and stay out of the cost column. The
 * U2 replay serves through the daemon, never through OpenCode, so its tier observations do too.
 */
function uncosted(e: Evidence): boolean {
    if (e.owner === "daemon.compression_fidelity.replay" || isHintTruncation(e)) return true;
    return (
        e.owner === "opencode-delivery" &&
        e.scenario === "C1.S1" &&
        (e.stage === "qualification" || e.stage.startsWith("pi-"))
    );
}

/** The admission estimator's charge for `bytes`: ceil(bytes / 3.5) with 25% headroom. */
const OPENCODE_ESTIMATOR = "opencode-heuristic utf8-bytes-div-3.5-v1";
const chargeFor = (bytes: number) => Math.ceil((Math.ceil(bytes / 3.5) * 1250) / 1000);
const count = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v) && v >= 0;

/** The admission branches that ran an admission check; `none` ran none and charges nothing. */
const ADMISSIONS: readonly string[] = ["fits", "shrinks", "limit_unknown"];
const SERVING_FIELDS: ReadonlyArray<readonly [string, (value: unknown) => boolean]> = [
    ["request_body_utf8_bytes", measurement],
    ["admission", (v) => typeof v === "string" && ADMISSIONS.includes(v)],
    ["invocation_bytes", count],
    ["invocation_charged_tokens", count],
    ["estimator", named],
    ["transform_elapsed_ms", measurement],
    // A served observation leaked no raw source; the producer marks a leaking pass unqualified.
    ["raw_source_leaks", (v) => v === 0],
    ["serving_kind", (v) => v === "cold" || v === "warm_repeat"],
];

/**
 * The source-level generation records of `arm`. A real arm credits only its published real
 * captures; a scripted arm credits any owner's attempts.
 */
function generationsOf(arm: Arm): Evidence[] {
    const real = arm.config?.generation_origin === "real";
    return arm.evidence.filter(
        (e) =>
            e.scenario === null &&
            Array.isArray(e.detail.attempts) &&
            (!real || (e.owner === REAL_CAPTURE && e.terminal === "published")),
    );
}

/** A usage record counts input and output tokens as non-negative numbers. */
function reportedUsage(e: Evidence): Json | null {
    const usage = e.detail.usage;
    if (e.detail.usage_reported === false || !record(usage)) return null;
    return measurement(usage.input_tokens) && measurement(usage.output_tokens) ? usage : null;
}

/** The validated serving cost rows among `evidence`, and what each costed observation lacks. */
function servingCosts(evidence: readonly Evidence[]): { serving: Json[]; missing: string[] } {
    const missing: string[] = [];
    const serving: Json[] = [];
    // Every served delivery is a serving observation, whether or not it names its tier.
    const costed = (e: Evidence) =>
        !uncosted(e) &&
        (servedTierOf(e) !== undefined ||
            (e.owner === "opencode-delivery" && e.terminal === "served"));
    for (const e of evidence.filter(costed)) {
        if (e.owner === "opencode-delivery" && typeof e.detail.served_tier !== "string") {
            missing.push(`${e.file}: served_tier`);
        }
        if (!record(e.detail.serving)) {
            missing.push(`${e.file}: serving`);
            continue;
        }
        // The observation's stage is authoritative; a `stage` inside the serving record is ignored.
        const row: Json = { ...e.detail.serving, stage: e.stage };
        for (const [field, valid] of SERVING_FIELDS) {
            if (!valid(row[field])) missing.push(`${e.file}: ${field}`);
        }
        // The campaign's `warm` stage repeats the previous request; every other stage serves cold.
        if ((row.serving_kind === "warm_repeat") !== (e.stage === "warm")) {
            missing.push(`${e.file}: serving_kind`);
        }
        if (
            row.estimator === OPENCODE_ESTIMATOR &&
            count(row.invocation_bytes) &&
            row.invocation_charged_tokens !== chargeFor(row.invocation_bytes)
        ) {
            missing.push(`${e.file}: invocation_charged_tokens`);
        }
        // An admitting branch measured an invocation; zero bytes is the producer's `none` shape.
        if (row.invocation_bytes === 0) missing.push(`${e.file}: invocation_bytes`);
        serving.push(row);
    }
    return { serving, missing };
}

function costOf(
    evidence: Evidence[],
    arm: Arm,
    exactRead: boolean,
    generations: readonly Evidence[],
): ScenarioRow["cost"] {
    const { serving, missing } = servingCosts(evidence);
    if (serving.length === 0 && !exactRead) missing.push("no serving observation");
    const recovery: Json[] = [];
    for (const e of evidence.filter(isRecovery)) {
        if (!measurement(e.detail.calls) || !measurement(e.detail.result_utf8_bytes)) {
            missing.push(`${e.file}: recovery calls or output bytes`);
        }
        recovery.push({
            stage: e.stage,
            recovery_calls: e.detail.calls,
            recovery_output_utf8_bytes: e.detail.result_utf8_bytes,
        });
    }
    const generation: Json[] = generations
        .filter((e) => evidence.some((s) => s.source === e.source))
        .map((e) => ({
            file: e.file,
            owner: e.owner,
            attempts: (e.detail.attempts as unknown[]).filter(record).length,
            usage: e.owner === REAL_CAPTURE ? reportedUsage(e) : "scripted",
        }));
    if (generation.length === 0) missing.push("no generation observation");
    for (const g of generation) {
        if (g.attempts === 0) missing.push(`${String(g.file)}: no generation attempt`);
        if (g.usage === null) missing.push(`${String(g.file)}: generation usage unreported`);
    }
    if (arm.config?.generation_origin === "real") {
        for (const { file, report } of arm.forwarding) {
            if (report.exchanges.some((x) => !x.response?.cost_known))
                missing.push(`${file}: unknown send cost`);
        }
    }
    return {
        status: missing.length === 0 ? "complete" : "incomplete",
        missing,
        serving,
        recovery,
        generation,
    };
}

/** Orders obligation judgments by obligation ID, comparing UTF-16 code units. */
const byObligation = (a: ObligationJudgment, b: ObligationJudgment) =>
    a.obligation < b.obligation ? -1 : a.obligation > b.obligation ? 1 : 0;

/** A judgment's verdict as a string equal for equal verdicts listed in any order. */
function verdict(j: Judgment): string {
    return JSON.stringify([
        [...j.obligations].sort(byObligation),
        [...j.forbidden_violated].sort(),
        j.abstained,
    ]);
}

/** The verdict a set of human judgments agrees on, or `null` when any two disagree. */
function agreed(judgments: Judgment[]): Judgment | null {
    const [first, ...rest] = judgments;
    if (!first || rest.length === 0) return first ?? null;
    const expected = verdict(first);
    return rest.every((j) => verdict(j) === expected) ? first : null;
}

function byArmAndScenario(judgments: readonly Judgment[]): Map<string, Map<string, Judgment[]>> {
    const groups = new Map<string, Map<string, Judgment[]>>();
    for (const judgment of judgments) {
        const arm = groups.get(judgment.arm) ?? new Map<string, Judgment[]>();
        groups.set(judgment.arm, arm);
        const group = arm.get(judgment.scenario);
        if (group) group.push(judgment);
        else arm.set(judgment.scenario, [judgment]);
    }
    return groups;
}

/** The review, preservation, recovery, safety, and cost columns of `row` for `arm`. */
export function scenarioRow(
    row: EvidenceRow,
    arm: Arm,
    label: string,
    reviews: Reviews,
    candidates: readonly Judgment[] = reviews.judgments,
    generations: readonly Evidence[] = generationsOf(arm),
): ScenarioRow {
    const { scenario, evidence } = row;
    const bound = candidates.filter(
        (j) =>
            j.arm === label &&
            j.scenario === scenario.id &&
            evidence.some((e) => e.sha256 === j.artifact_sha256),
    );
    const human = bound.filter((j) => j.kind === "human");
    const judgment = agreed(human);
    const disputed =
        (human.length > 0 && !judgment) ||
        reviews.disputes.some((d) => d.arm === label && d.scenario === scenario.id && !d.resolved);
    let semantic_review: ScenarioRow["semantic_review"] = "unreviewed";
    if (disputed) semantic_review = "disputed";
    else if (judgment) semantic_review = "reviewed";
    else if (bound.some((j) => j.kind === "model" && j.citations?.length && j.uncertainty)) {
        semantic_review = "model_only";
    }
    const usable = disputed ? null : judgment;
    const preservation = scenario.expectations.map((expectation) => {
        const judged = usable?.obligations.find((o) => o.obligation === expectation.obligation);
        if (!usable || !judged) {
            return {
                obligation: expectation.obligation,
                disposition: null,
                status: "unreviewed" as const,
            };
        }
        // Abstention earns no credit: an abstained answer preserves nothing it did not show.
        const credited =
            judged.preserved &&
            expectation.accepted.includes(judged.disposition) &&
            !(usable.abstained && judged.disposition === "unavailable");
        return {
            obligation: expectation.obligation,
            disposition: judged.disposition,
            status: credited ? ("preserved" as const) : ("recall" as const),
        };
    });
    const discoverable = preservation.some((p) => p.disposition === "discoverable");
    const recovery = !discoverable
        ? "not_required"
        : evidence.some(
                (e) =>
                    isRecovery(e) &&
                    searchedMemory(e.detail) &&
                    e.detail.result_carries_memory === true &&
                    count(e.detail.calls) &&
                    e.detail.calls > 0 &&
                    count(e.detail.result_utf8_bytes) &&
                    e.detail.result_utf8_bytes > 0,
            )
          ? "witnessed"
          : "unverified";
    let consumer_safety: ScenarioRow["consumer_safety"] = "unreviewed";
    if (usable) {
        // Every reviewer-reported forbidden conclusion counts as a violation, including an ID
        // outside the scenario's declared set.
        const violated = usable.forbidden_violated.length > 0;
        if (violated || (usable.abstained && scenario.abstention === "forbidden")) {
            consumer_safety = "false-authoritative";
        } else {
            consumer_safety = usable.abstained ? "abstained" : "safe";
        }
    }
    const cost = costOf(evidence, arm, scenario.serving.path === "exact_read", generations);

    const withheld: string[] = [];
    if (row.execution.status !== "executed") withheld.push(`execution ${row.execution.status}`);
    if (row.deterministic !== "pass") withheld.push(`deterministic ${row.deterministic}`);
    if (preservation.some((p) => p.status !== "preserved")) withheld.push("preservation");
    if (recovery === "unverified") withheld.push("recovery unverified");
    if (consumer_safety === "false-authoritative" || consumer_safety === "unreviewed") {
        withheld.push(`consumer safety ${consumer_safety}`);
    }
    if (semantic_review !== "reviewed") withheld.push(`semantic review ${semantic_review}`);
    if (cost.status !== "complete") withheld.push("cost incomplete");
    return {
        scenario: scenario.id,
        case: row.case,
        execution: row.execution,
        deterministic: row.deterministic,
        preservation,
        recovery,
        consumer_safety,
        semantic_review,
        cost,
        withheld,
    };
}

function compare(baseline: ScenarioRow, candidate: ScenarioRow): Comparison {
    if (baseline.execution.status === "missing" || candidate.execution.status === "missing") {
        return "unscored";
    }
    const before = baseline.withheld.length === 0;
    const after = candidate.withheld.length === 0;
    if (before && after) return "expected_green";
    if (before) return "regression";
    return after ? "resolution_candidate" : "expected_red";
}

/** The delivery witness's stages for a source with no m1 scenario, labeled by source ID. */
const SOURCE_LEVEL_STAGES = ["m1", "warm", "cold-m0"] as const;

/** The sources the delivery witness labels by source ID: those with no m1 scenario. */
function sourceLevelSources(corpus: FidelityCorpus): string[] {
    const m1 = new Set(
        scenarios(corpus)
            .filter((s) => s.scenario.serving.stage === "m1")
            .map((s) => s.scenario.source),
    );
    return corpus.cases.flatMap((c) => c.sources.map((v) => v.id).filter((id) => !m1.has(id)));
}

/** Both arms charge tokens under one estimator, or their costs do not compare. */
function estimatorRefusal(arms: ReturnType<typeof assembleEvidence>["arms"]): string[] {
    const estimators = new Set(
        arms.flatMap((side) =>
            side.arm.evidence.flatMap((e) =>
                record(e.detail.serving) && named(e.detail.serving.estimator)
                    ? [e.detail.serving.estimator]
                    : [],
            ),
        ),
    );
    return estimators.size > 1 ? ["the arms charged tokens under different estimators"] : [];
}

/** Assembles the manifest and the per-scenario report for `baseline` and `candidate`. */
export function evaluate(input: {
    corpus: FidelityCorpus;
    corpusPath: string;
    corpusSha256: string;
    revision: string;
    mode: "offline" | "live";
    baseline: Arm;
    candidate: Arm;
    reviews: Reviews;
}) {
    const { reviews } = input;
    const assembled = assembleEvidence(input);
    const controls = controlQualification(reviews);
    const binding = controls.qualified ? reviews : { ...reviews, judgments: [], disputes: [] };
    const grouped = byArmAndScenario(binding.judgments);
    const labels = new Set(assembled.arms.map((side) => side.label));
    const expectations = new Map(
        scenarios(input.corpus).map((s) => [
            s.scenario.id,
            new Set(s.scenario.expectations.map((e) => e.obligation)),
        ]),
    );
    const stray = [
        ...reviews.judgments.map((j) => ["judgment", j.arm, j.scenario] as const),
        ...reviews.disputes.map((d) => ["dispute", d.arm, d.scenario] as const),
    ]
        .filter(([, arm, scenario]) => !labels.has(arm) || !expectations.has(scenario))
        .map(
            ([what, arm, scenario]) => `a ${what} names unknown arm or scenario ${arm}/${scenario}`,
        );
    const artifacts = new Map(
        assembled.arms.map((side) => [
            side.label,
            new Map(side.rows.map((row) => [row.scenario.id, row.evidence.map((e) => e.sha256)])),
        ]),
    );
    for (const j of reviews.judgments) {
        const declared = expectations.get(j.scenario);
        if (!declared) continue;
        for (const o of j.obligations) {
            if (!declared.has(o.obligation)) {
                stray.push(`a judgment names unknown obligation ${o.obligation} for ${j.scenario}`);
            }
        }
        const row = artifacts.get(j.arm)?.get(j.scenario);
        if (row && !row.includes(j.artifact_sha256)) {
            stray.push(
                `a judgment names unknown artifact ${j.artifact_sha256} for ${j.arm}/${j.scenario}`,
            );
        }
    }
    const arms = assembled.arms.map((side) => {
        const judged = grouped.get(side.label);
        const generations = generationsOf(side.arm);
        const rows = side.rows.map((row) =>
            scenarioRow(
                row,
                side.arm,
                side.label,
                binding,
                judged?.get(row.scenario.id) ?? [],
                generations,
            ),
        );
        // The delivery witness labels a source with no m1 scenario by its source ID; those
        // serving observations belong to the arm and are costed here rather than on a row.
        const sourceLevel = side.arm.evidence.filter(
            (e) => e.scenario === null && e.owner === "opencode-delivery",
        );
        const sourceServing = servingCosts(sourceLevel);
        const failedDeliveries = sourceLevel
            .filter((e) => !executed(e))
            .map((e) => `source-level delivery ${e.file} ended ${e.terminal}`);
        // The witness serves p1 at every source-level stage and asserts it before recording.
        const sparseDeliveries = sourceLevel
            .filter((e) => executed(e) && servedTierOf(e) !== "p1")
            .map(
                (e) =>
                    `source-level delivery ${e.file} served ${String(servedTierOf(e))}, where the witness serves p1`,
            );
        // The witness writes all three stages for each source with no m1 scenario.
        const absentDeliveries = sourceLevelSources(input.corpus).flatMap((source) => {
            const lacking = SOURCE_LEVEL_STAGES.filter(
                (stage) => !sourceLevel.some((e) => e.source === source && e.stage === stage),
            );
            return lacking.length > 0
                ? [
                      `source-level delivery of ${source} lacks its ${lacking.join(", ")} stage${lacking.length > 1 ? "s" : ""}`,
                  ]
                : [];
        });
        const approvers = reviews.approvals.length;
        const reasons = [
            ...side.identity_errors,
            ...reviews.errors,
            ...controls.problems.map((p) => `controls: ${p}`),
            ...stray,
            ...absentDeliveries,
            ...failedDeliveries,
            ...sparseDeliveries,
            ...(sourceServing.missing.length > 0
                ? [`source-level serving cost incomplete: ${sourceServing.missing.join(", ")}`]
                : []),
            ...(approvers < REQUIRED_APPROVERS
                ? [`the batch has ${approvers} of ${REQUIRED_APPROVERS} approvers`]
                : []),
            ...(side.arm.config?.generation_origin === "real" ? [] : ["no real-model evidence"]),
            ...(side.missing_scenarios.length > 0
                ? [`missing scenarios: ${side.missing_scenarios.join(", ")}`]
                : []),
            ...rows
                .filter((r) => r.withheld.length > 0)
                .map((r) => `${r.scenario}: ${r.withheld.join(", ")}`),
        ];
        // The validated forwarding reports' spend, at arm scope: sends are per arm, not per row.
        const forwarding = side.arm.forwarding.reduce(
            (sum, { report }) => ({
                reports: sum.reports + 1,
                sends: sum.sends + report.exchanges.length,
                spent_usd: sum.spent_usd + report.spent_usd,
            }),
            { reports: 0, sends: 0, spent_usd: 0 },
        );
        return {
            label: side.label,
            identity_errors: side.identity_errors,
            reached_scenarios: side.reached_scenarios,
            missing_scenarios: side.missing_scenarios,
            rows,
            source_serving: sourceServing.serving,
            forwarding,
            accepted: reasons.length === 0,
            withheld: reasons,
        };
    });
    const [base, cand] = arms as [(typeof arms)[0], (typeof arms)[0]];
    const refused = [...assembled.refused, ...estimatorRefusal(assembled.arms)];
    const comparison = {
        refused,
        treatment: assembled.treatment,
        rows:
            refused.length > 0
                ? []
                : base.rows.map((row, i) => ({
                      scenario: row.scenario,
                      comparison: compare(row, cand.rows[i] as ScenarioRow),
                  })),
    };
    const manifest = {
        ...assembled.manifest,
        reviews: {
            batch: reviews.batch,
            approved_by: reviews.approvals,
            controls_sha256: reviews.sha256.controls,
            judgments_sha256: reviews.sha256.judgments,
        },
    };
    const report = {
        schema: REPORT_SCHEMA,
        corpus_sha256: input.corpusSha256,
        controls,
        arms,
        comparison,
        accepted: refused.length === 0 && arms.every((a) => a.accepted),
    };
    return { manifest, report };
}

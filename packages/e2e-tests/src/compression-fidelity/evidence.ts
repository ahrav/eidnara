/**
 * The identity and completeness layer behind `eval:compression-fidelity`. It reads one baseline
 * and one candidate evidence directory, the private owner observations the U2, C6, U3, and U4
 * witnesses wrote there, recomputes every file's SHA-256, refuses evidence bound to another
 * corpus, an unknown owner, a case or scenario outside the corpus, a duplicate, or a leftover
 * temporary file, and derives each scenario's execution and deterministic columns and the
 * manifest from the stored observations alone.
 *
 * Nothing here sends a request. Live evidence enters as the forwarding reports the
 * record-and-forward provider mode published, with the limits that mode enforced.
 */

import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import type { BEHAVIORAL_VERDICTS } from "../incident-pool/report";
import type { FidelityCorpus, FidelityScenario } from "./corpus";

export const ARM_SCHEMA = "eidnara.compression-fidelity-arm/v1";
export const MANIFEST_SCHEMA = "eidnara.compression-fidelity-manifest/v1";
export const REPORT_SCHEMA = "eidnara.compression-fidelity-report/v1";

/** Owners whose observation files the assembler reads; any other owner is refused. */
export const OWNERS = [
    "daemon.compression_fidelity.replay",
    "daemon.harness_sources.c6_exact_read",
    "daemon.compression_fidelity.real_capture",
    "opencode-delivery",
] as const;
export const REAL_CAPTURE = "daemon.compression_fidelity.real_capture";
const TIERS = ["p1", "p2", "p3", "p4", "p5"];
const EXECUTED = new Set(["published", "served", "read_exact", "discoverable", "excluded"]);
/** The arm fields a comparison holds equal; only the prompt may differ, as the treatment. */
const HELD_EQUAL = ["model", "provider", "version", "settings", "limits"] as const;

export type Json = Record<string, unknown>;
type Deterministic = (typeof BEHAVIORAL_VERDICTS)[number];

export interface ArmConfig {
    schema: typeof ARM_SCHEMA;
    label: string;
    /** SHA-256 of the history summarizer system prompt the arm ran. */
    prompt_sha256: string;
    model: string;
    provider: string;
    version: string;
    settings: Json;
    limits: Json | null;
    generation_origin: "scripted" | "real";
}

export interface Evidence {
    file: string;
    sha256: string;
    owner: string;
    case: string;
    source: string;
    scenario: string | null;
    /** The variant after `@` in a witness's scenario label, such as `p1-only`. */
    variant: string | null;
    stage: string;
    terminal: string;
    markers: string[];
    detail: Json;
}

/** The fields of a forwarding report the assembler reads. */
export interface ForwardingEvidence {
    mode: "forward";
    corpus_sha256: string;
    limits: Json;
    complete: boolean;
    incomplete_reasons: string[];
    exchanges: Array<{
        index: number;
        request: { body_text: string; body_sha256: string };
        response: {
            truncated: boolean;
            body_text: string;
            body_sha256: string | null;
            cost_known: boolean;
        } | null;
    }>;
}

export interface Arm {
    dir: string;
    config: ArmConfig | null;
    evidence: Evidence[];
    forwarding: Array<{ file: string; sha256: string; report: ForwardingEvidence }>;
    errors: string[];
    /** Set when any file in the arm is bound to another corpus. */
    foreignCorpus: boolean;
}

/** One scenario's execution and deterministic columns, and the evidence that judges it. */
export interface EvidenceRow {
    scenario: FidelityScenario;
    case: string;
    execution: { status: "executed" | "failed" | "missing"; outcomes: string[] };
    deterministic: Deterministic;
    /** The scenario's own observations: variants and judge controls excluded. */
    evidence: Evidence[];
}

export function sha256(bytes: Uint8Array | string): string {
    return createHash("sha256").update(bytes).digest("hex");
}

export function record(value: unknown): value is Json {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function text(value: unknown): value is string {
    return typeof value === "string" && value !== "";
}

export function strings(value: unknown): string[] {
    return Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];
}

export function readJson(path: string): { bytes: Buffer; value: unknown } | { error: string } {
    let bytes: Buffer;
    try {
        bytes = readFileSync(path);
    } catch (error) {
        return { error: `${path} is unreadable: ${(error as Error).message}` };
    }
    try {
        return { bytes, value: JSON.parse(bytes.toString("utf8")) };
    } catch {
        return { error: `${path} is not JSON` };
    }
}

function sources(corpus: FidelityCorpus): Array<{ case: string; source: string }> {
    return corpus.cases.flatMap((c) => c.sources.map((s) => ({ case: c.id, source: s.id })));
}

export function scenarios(
    corpus: FidelityCorpus,
): Array<{ case: string; scenario: FidelityScenario }> {
    return corpus.cases.flatMap((c) => c.scenarios.map((scenario) => ({ case: c.id, scenario })));
}

/** `value` as JSON with object keys sorted, so equal values compare equal. */
export function canonical(value: unknown): string {
    return JSON.stringify(value, (_key, v) =>
        record(v)
            ? Object.fromEntries(Object.entries(v).sort(([a], [b]) => a.localeCompare(b)))
            : v,
    );
}

export function armLabel(arm: Arm): string {
    return arm.config?.label ?? arm.dir;
}

/** `value` as an arm configuration, or `null` when a field is absent or mistyped. */
function armConfig(value: unknown): ArmConfig | null {
    if (!record(value) || value.schema !== ARM_SCHEMA) return null;
    const { label, prompt_sha256, model, provider, version, settings, limits, generation_origin } =
        value;
    if (!text(label) || !text(prompt_sha256) || !text(model) || !text(provider) || !text(version)) {
        return null;
    }
    if (!record(settings) || !(limits === null || record(limits))) return null;
    if (generation_origin !== "scripted" && generation_origin !== "real") return null;
    return {
        schema: ARM_SCHEMA,
        label,
        prompt_sha256,
        model,
        provider,
        version,
        settings,
        limits,
        generation_origin,
    };
}

/** `value` as forwarding evidence, or `null` when a field the assembler reads is absent. */
function forwardingOf(value: unknown): ForwardingEvidence | null {
    if (!record(value) || value.mode !== "forward" || !text(value.corpus_sha256)) return null;
    if (!record(value.limits) || typeof value.complete !== "boolean") return null;
    if (!Array.isArray(value.incomplete_reasons) || !Array.isArray(value.exchanges)) return null;
    const exchanges: ForwardingEvidence["exchanges"] = [];
    for (const exchange of value.exchanges) {
        if (!record(exchange) || typeof exchange.index !== "number") return null;
        const request = exchange.request;
        if (
            !record(request) ||
            typeof request.body_text !== "string" ||
            !text(request.body_sha256)
        ) {
            return null;
        }
        const response = exchange.response;
        if (response !== null) {
            if (!record(response) || typeof response.body_text !== "string") return null;
            if (
                typeof response.truncated !== "boolean" ||
                typeof response.cost_known !== "boolean"
            ) {
                return null;
            }
            if (response.body_sha256 !== null && typeof response.body_sha256 !== "string")
                return null;
        }
        exchanges.push({
            index: exchange.index,
            request: { body_text: request.body_text, body_sha256: request.body_sha256 },
            response: response
                ? {
                      truncated: response.truncated as boolean,
                      body_text: response.body_text as string,
                      body_sha256: response.body_sha256 as string | null,
                      cost_known: response.cost_known as boolean,
                  }
                : null,
        });
    }
    return {
        mode: "forward",
        corpus_sha256: value.corpus_sha256,
        limits: value.limits,
        complete: value.complete,
        incomplete_reasons: strings(value.incomplete_reasons),
        exchanges,
    };
}

/** The system prompt hashes an observation claims its producer ran. */
function systemHashes(evidence: Evidence): string[] {
    const attempts = Array.isArray(evidence.detail.attempts) ? evidence.detail.attempts : [];
    return attempts.filter(record).flatMap((attempt) => {
        if (typeof attempt.system_sha256 === "string") return [attempt.system_sha256];
        if (typeof attempt.system === "string") return [sha256(attempt.system)];
        return [];
    });
}

function checkForwarding(file: string, report: ForwardingEvidence): string[] {
    const errors: string[] = [];
    for (const exchange of report.exchanges) {
        // The hashed representation is the request body as UTF-8 bytes, the bytes sent.
        if (sha256(exchange.request.body_text) !== exchange.request.body_sha256) {
            errors.push(`${file} exchange ${exchange.index} request bytes do not match their hash`);
        }
        const response = exchange.response;
        if (response && !response.truncated && response.body_sha256 !== null) {
            if (sha256(response.body_text) !== response.body_sha256) {
                errors.push(
                    `${file} exchange ${exchange.index} response bytes do not match their hash`,
                );
            }
        }
    }
    return errors;
}

/** Reads one arm directory and checks every file's identity against `corpus`. */
export function loadArm(dir: string, corpus: FidelityCorpus, corpusSha256: string): Arm {
    const arm: Arm = {
        dir,
        config: null,
        evidence: [],
        forwarding: [],
        errors: [],
        foreignCorpus: false,
    };
    let names: string[];
    try {
        names = readdirSync(dir).sort();
    } catch {
        arm.errors.push(`${dir} is unreadable`);
        return arm;
    }
    const scenarioCase = new Map(scenarios(corpus).map((s) => [s.scenario.id, s.case]));
    const sourceCase = new Map(sources(corpus).map((s) => [s.source, s.case]));
    const seen = new Set<string>();
    const foreign = (name: string, bound: unknown) => {
        arm.foreignCorpus = true;
        arm.errors.push(`${name} is bound to corpus ${String(bound)}`);
    };
    for (const name of names) {
        // The Rust writer stages `.<name>.tmp` and the TypeScript writer `<name>.tmp-<hex>`;
        // either left behind means a publication never finished.
        if (name.startsWith(".") || /\.tmp(-|$)/.test(name)) {
            arm.errors.push(`${name} is an unpublished temporary file`);
            continue;
        }
        if (!name.endsWith(".json")) continue;
        const read = readJson(join(dir, name));
        if ("error" in read) {
            arm.errors.push(read.error);
            continue;
        }
        const value = read.value;
        if (name === "arm.json") {
            const config = armConfig(value);
            if (config) arm.config = config;
            else arm.errors.push("arm.json does not match its schema");
            continue;
        }
        if (name.startsWith("forwarding-")) {
            const report = forwardingOf(value);
            if (!report) {
                arm.errors.push(`${name} does not match the forwarding report schema`);
            } else if (report.corpus_sha256 !== corpusSha256) {
                foreign(name, report.corpus_sha256);
            } else {
                arm.errors.push(...checkForwarding(name, report));
                arm.forwarding.push({ file: name, sha256: sha256(read.bytes), report });
            }
            continue;
        }
        if (!record(value) || value.schema_version !== 1) {
            arm.errors.push(`${name} is not a version 1 observation`);
            continue;
        }
        const owner = String(value.owner);
        if (!(OWNERS as readonly string[]).includes(owner)) {
            arm.errors.push(`${name} has unknown owner ${owner}`);
            continue;
        }
        if (value.corpus_sha256 !== corpusSha256) {
            foreign(name, value.corpus_sha256);
            continue;
        }
        const parts = typeof value.scenario === "string" ? value.scenario.split("@") : [null];
        const [label, variant = null] = parts;
        if (parts.length > 2 || variant === "" || label === "") {
            arm.errors.push(`${name} has a malformed scenario label ${String(value.scenario)}`);
            continue;
        }
        const evidence: Evidence = {
            file: name,
            sha256: sha256(read.bytes),
            owner,
            case: String(value.case),
            source: String(value.source),
            scenario: label ?? null,
            variant,
            stage: String(value.stage),
            terminal: String(value.terminal),
            markers: strings(value.markers),
            detail: record(value.detail) ? value.detail : {},
        };
        const caseOf = evidence.scenario
            ? scenarioCase.get(evidence.scenario)
            : sourceCase.get(evidence.source);
        if (caseOf !== evidence.case) {
            arm.errors.push(
                `${name} names case ${evidence.case}, where the item is in ${caseOf ?? "no case"}`,
            );
            continue;
        }
        const key = [
            owner,
            evidence.case,
            evidence.source,
            evidence.scenario,
            evidence.variant,
            evidence.stage,
        ].join("|");
        if (seen.has(key)) {
            arm.errors.push(`${name} duplicates another observation of ${key}`);
            continue;
        }
        seen.add(key);
        arm.evidence.push(evidence);
    }
    if (!arm.config) arm.errors.push("arm.json is missing");
    return arm;
}

/**
 * The prompt and origin errors of `arm`. A `real` arm needs a published real capture for every
 * source, and every serving observation must name the real capture whose output it served.
 */
function checkGeneration(arm: Arm, corpus: FidelityCorpus): string[] {
    const errors: string[] = [];
    const config = arm.config;
    if (!config) return errors;
    for (const evidence of arm.evidence) {
        for (const hash of systemHashes(evidence)) {
            if (hash !== config.prompt_sha256) {
                errors.push(`${evidence.file} ran system prompt ${hash}, not the arm's prompt`);
            }
        }
    }
    if (config.generation_origin !== "real") return errors;
    const captures = arm.evidence.filter(
        (e) =>
            e.owner === REAL_CAPTURE &&
            e.terminal === "published" &&
            String(e.detail.output_origin ?? "").startsWith("real"),
    );
    for (const evidence of arm.evidence) {
        if (String(evidence.detail.output_origin ?? "").includes("scripted")) {
            errors.push(`${evidence.file} carries scripted output in an arm labeled real`);
        }
        if (!record(evidence.detail.serving)) continue;
        const served = evidence.detail.generation_capture_sha256;
        if (!captures.some((c) => c.sha256 === served && c.source === evidence.source)) {
            errors.push(
                `${evidence.file} names no published real capture of ${evidence.source} it served`,
            );
        }
    }
    for (const { source } of sources(corpus)) {
        if (!captures.some((c) => c.source === source)) {
            errors.push(`no published real-model capture for ${source}`);
        }
    }
    return errors;
}

function deterministicOf(scenario: FidelityScenario, evidence: Evidence[]): Deterministic {
    if (scenario.serving.path === "exact_read") {
        return evidence.some((e) => e.terminal === "read_exact") ? "pass" : "not_evaluated";
    }
    // Delivery observations name the tier OpenCode served; the U2 replay names the tier its
    // serving pass rendered. A delivery under positive-budget pressure passes when it serves a
    // tier sparser than the curve's, the oracle that witness documents; every other observation
    // must serve the corpus tier.
    const results = evidence.flatMap((e) => {
        const tier = e.detail.served_tier ?? e.detail.tier;
        if (typeof tier !== "string") return [];
        const curve = e.detail.curve_tier;
        if (
            scenario.serving.path === "pressure" &&
            e.owner === "opencode-delivery" &&
            typeof curve === "string"
        ) {
            return [TIERS.includes(curve) && TIERS.indexOf(tier) > TIERS.indexOf(curve)];
        }
        return [tier === scenario.serving.tier];
    });
    if (results.length === 0) return "not_evaluated";
    return results.every(Boolean) ? "pass" : "assertion_fail";
}

/** The scenario's execution and deterministic columns, from its stored observations. */
function evidenceRow(entry: { case: string; scenario: FidelityScenario }, arm: Arm): EvidenceRow {
    const { scenario } = entry;
    // A variant witnesses another situation on the scenario's source, and a judge control
    // tests the delivery judge itself; both stay in the outcomes and judge nothing here.
    const recorded = arm.evidence.filter((e) => e.scenario === scenario.id);
    const evidence = recorded.filter(
        (e) =>
            e.variant === null &&
            !(e.owner === "opencode-delivery" && e.detail.judge_control === true),
    );
    const outcomes = recorded.map(
        (e) => `${e.owner}:${e.variant ? `${e.variant}:` : ""}${e.stage}:${e.terminal}`,
    );
    const status =
        evidence.length === 0
            ? "missing"
            : evidence.every((e) => EXECUTED.has(e.terminal))
              ? "executed"
              : "failed";
    return {
        scenario,
        case: entry.case,
        execution: { status, outcomes },
        deterministic: deterministicOf(scenario, evidence),
        evidence,
    };
}

/**
 * Checks both arms' identity and completeness and builds the manifest. The comparison is
 * refused when an arm is bound to another corpus or has identity errors, when the arms reach
 * different scenario sets, or when they differ in anything but the prompt; differing prompt
 * hashes are the treatment.
 */
export function assembleEvidence(input: {
    corpus: FidelityCorpus;
    corpusPath: string;
    corpusSha256: string;
    revision: string;
    mode: "offline" | "live";
    baseline: Arm;
    candidate: Arm;
}) {
    const { corpus } = input;
    const sides = [input.baseline, input.candidate] as const;
    const arms = sides.map((arm) => {
        const errors = [...arm.errors, ...checkGeneration(arm, corpus)];
        if (input.mode === "live") {
            if (arm.forwarding.length === 0) errors.push("live mode found no forwarding report");
            for (const { file, report } of arm.forwarding) {
                if (!report.complete) {
                    errors.push(`${file} is incomplete: ${report.incomplete_reasons.join("; ")}`);
                }
                if (canonical(report.limits) !== canonical(arm.config?.limits)) {
                    errors.push(`${file} ran limits other than the arm's`);
                }
            }
        }
        const rows = scenarios(corpus).map((entry) => evidenceRow(entry, arm));
        const reached = rows
            .filter((r) => r.execution.status !== "missing")
            .map((r) => r.scenario.id);
        const missing = rows
            .filter((r) => r.execution.status === "missing")
            .map((r) => r.scenario.id);
        return {
            arm,
            label: armLabel(arm),
            identity_errors: errors,
            reached_scenarios: reached,
            missing_scenarios: missing,
            rows,
        };
    });
    const [base, cand] = arms as [(typeof arms)[0], (typeof arms)[0]];
    const refused: string[] = [];
    if (input.baseline.foreignCorpus || input.candidate.foreignCorpus) {
        refused.push("an arm is bound to another corpus");
    }
    if (base.identity_errors.length > 0 || cand.identity_errors.length > 0) {
        refused.push("an arm has identity errors");
    }
    if (JSON.stringify(base.reached_scenarios) !== JSON.stringify(cand.reached_scenarios)) {
        refused.push("the arms reached different scenario sets");
    }
    for (const field of HELD_EQUAL) {
        const before = canonical(input.baseline.config?.[field] ?? null);
        const after = canonical(input.candidate.config?.[field] ?? null);
        if (before !== after) refused.push(`the arms differ in ${field}`);
    }
    const manifest = {
        schema: MANIFEST_SCHEMA,
        repository_revision: input.revision,
        mode: input.mode,
        corpus: { path: input.corpusPath, sha256: input.corpusSha256 },
        arms: sides.map((arm) => ({
            label: armLabel(arm),
            config: arm.config,
            observations: arm.evidence.map((e) => ({
                file: e.file,
                sha256: e.sha256,
                owner: e.owner,
                case: e.case,
                source: e.source,
                scenario: e.scenario,
                variant: e.variant,
                stage: e.stage,
                terminal: e.terminal,
            })),
            forwarding_reports: arm.forwarding.map(({ file, sha256 }) => ({ file, sha256 })),
        })),
    };
    return {
        manifest,
        arms,
        refused,
        treatment: input.baseline.config?.prompt_sha256 !== input.candidate.config?.prompt_sha256,
    };
}

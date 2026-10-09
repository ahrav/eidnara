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

import { join } from "node:path";
import { canonicalJson } from "../canonical-json";
import type { BEHAVIORAL_VERDICTS } from "../incident-pool/report";
import type { FidelityCorpus, FidelityScenario } from "./corpus";

// Loaded through `getBuiltinModule` for the startup cost noted in `../atomic-publish`.
const fs = process.getBuiltinModule("node:fs");

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
const REPLAY = "daemon.compression_fidelity.replay";
const TIERS = ["p1", "p2", "p3", "p4", "p5"];
const EXECUTED = new Set(["published", "served", "read_exact", "discoverable", "excluded"]);
const EXACT_READ_OWNER = "daemon.harness_sources.c6_exact_read";
/** The four limits the forwarder enforces; its report and the arm's `limits` carry all of them. */
const FORWARD_LIMITS = ["maxCalls", "maxOutputTokens", "timeoutMs", "spendCapUsd"] as const;
/** The delivery witness's judge self-test; a judge control elsewhere is refused. */
const JUDGE_CONTROL = { stage: "missing-capture", scenario: "C1.S2" } as const;
/** The scenario variants a witness emits, by owner and scenario; any other label is refused. */
const VARIANTS: Record<string, ReadonlyArray<{ scenario: string; variant: string }>> = {
    "opencode-delivery": [{ scenario: "C1.S2", variant: "p1-only" }],
};
/** The generation settings every real capture attempt records and the arm's `settings` declare. */
const ATTEMPT_SETTINGS = ["temperature", "max_output_tokens"] as const;
/** The arm fields a comparison holds equal; only the prompt may differ, as the treatment. */
const HELD_EQUAL = [
    "model",
    "provider",
    "version",
    "settings",
    "limits",
    "generation_origin",
] as const;

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
    /** The model OpenCode ran, as the provider echoes it. */
    model: string;
    /** The Messages endpoint the forwarder sent to. */
    upstream_url: string;
    /** The context window OpenCode was configured with. */
    context_limit: number;
    /** The prices the forwarder reserved and charged with, in USD per million tokens. */
    pricing: { inputPerMTok: number; outputPerMTok: number };
    limits: Json;
    /** The reason the forwarder stopped, when it did. */
    stopped: string | null;
    spent_usd: number;
    complete: boolean;
    incomplete_reasons: string[];
    exchanges: Array<{
        index: number;
        /** The `tool_result` ids the request answers. */
        tool_results: string[];
        /** The `tool_use` ids the response asks for. */
        tool_uses: string[];
        request: { body_text: string; body_sha256: string };
        response: {
            outcome: string;
            /** The model the response names; `null` when it names none. */
            model: string | null;
            stop_reason: string | null;
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
    /** Every file read, with its hash; `null` for a file that could not be read. */
    files: Array<{ file: string; sha256: string | null }>;
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
    return new Bun.CryptoHasher("sha256").update(bytes).digest("hex");
}

export function record(value: unknown): value is Json {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function text(value: unknown): value is string {
    return typeof value === "string" && value !== "";
}

export function named(value: unknown): value is string {
    return typeof value === "string" && value.trim() !== "";
}

/** A lowercase hex SHA-256 digest. */
export function digest(value: unknown): value is string {
    return typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
}

export function strings(value: unknown): string[] {
    return Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];
}

export async function readJson(
    path: string,
): Promise<{ bytes: Buffer; value: unknown } | { bytes: Buffer | null; error: string }> {
    let bytes: Buffer;
    try {
        bytes = await fs.promises.readFile(path);
    } catch (error) {
        return { bytes: null, error: `${path} is unreadable: ${(error as Error).message}` };
    }
    try {
        return { bytes, value: JSON.parse(bytes.toString("utf8")) };
    } catch {
        return { bytes, error: `${path} is not JSON` };
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

export function armLabel(arm: Arm): string {
    return arm.config?.label ?? arm.dir;
}

/** `value` as an arm configuration, or `null` when a field is absent or mistyped. */
function armConfig(value: unknown): ArmConfig | null {
    if (!record(value) || value.schema !== ARM_SCHEMA) return null;
    const { label, prompt_sha256, model, provider, version, settings, limits, generation_origin } =
        value;
    if (
        !named(label) ||
        !digest(prompt_sha256) ||
        !named(model) ||
        !named(provider) ||
        !named(version)
    ) {
        return null;
    }
    if (!record(settings) || !(limits === null || record(limits))) return null;
    if (typeof settings.temperature !== "number" || !Number.isFinite(settings.temperature)) {
        return null;
    }
    const tokens = settings.max_output_tokens;
    if (typeof tokens !== "number" || !Number.isInteger(tokens) || tokens <= 0) return null;
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

/** An HTTPS Messages endpoint without credential or query, the only upstream the forwarder accepts. */
function messagesEndpoint(value: string): boolean {
    let url: URL;
    try {
        url = new URL(value);
    } catch {
        return false;
    }
    return (
        url.protocol === "https:" &&
        url.pathname.endsWith("/messages") &&
        !url.username &&
        !url.password &&
        !url.search
    );
}

/** `value` as forwarding evidence, or `null` when a field the assembler reads is absent. */
function forwardingOf(value: unknown): ForwardingEvidence | null {
    if (!record(value) || value.mode !== "forward" || !text(value.corpus_sha256)) return null;
    if (!text(value.model) || !record(value.limits) || typeof value.complete !== "boolean") {
        return null;
    }
    if (!text(value.upstream_url) || !messagesEndpoint(value.upstream_url)) return null;
    const pricing = value.pricing;
    if (
        !record(pricing) ||
        typeof pricing.inputPerMTok !== "number" ||
        typeof pricing.outputPerMTok !== "number" ||
        pricing.inputPerMTok <= 0 ||
        pricing.outputPerMTok <= 0
    ) {
        return null;
    }
    const limits = value.limits;
    if (FORWARD_LIMITS.some((name) => typeof limits[name] !== "number" || limits[name] <= 0)) {
        return null;
    }
    if (!Number.isInteger(limits.maxCalls) || !Number.isInteger(limits.maxOutputTokens))
        return null;
    if (!Array.isArray(value.incomplete_reasons) || !value.incomplete_reasons.every(text)) {
        return null;
    }
    if (value.stopped !== null && !text(value.stopped)) return null;
    const contextLimit = value.context_limit;
    if (typeof contextLimit !== "number" || !Number.isInteger(contextLimit) || contextLimit <= 0) {
        return null;
    }
    if (typeof value.spent_usd !== "number" || !Number.isFinite(value.spent_usd)) return null;
    if (value.spent_usd < 0) return null;
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
            if (!text(response.outcome)) return null;
            if (response.stop_reason !== null && typeof response.stop_reason !== "string")
                return null;
            if (response.model !== null && typeof response.model !== "string") return null;
        }
        const ids = (value: unknown) =>
            Array.isArray(value) && value.every((id) => typeof id === "string") ? value : null;
        const toolUses = ids(exchange.tool_uses);
        const toolResults = ids(exchange.tool_results);
        if (!toolUses || !toolResults) return null;
        exchanges.push({
            index: exchange.index,
            tool_uses: toolUses,
            tool_results: toolResults,
            request: { body_text: request.body_text, body_sha256: request.body_sha256 },
            response: response
                ? {
                      outcome: response.outcome as string,
                      model: response.model as string | null,
                      stop_reason: response.stop_reason as string | null,
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
        model: value.model,
        upstream_url: value.upstream_url,
        context_limit: contextLimit,
        pricing: { inputPerMTok: pricing.inputPerMTok, outputPerMTok: pricing.outputPerMTok },
        limits: value.limits,
        stopped: value.stopped as string | null,
        spent_usd: value.spent_usd,
        complete: value.complete,
        incomplete_reasons: value.incomplete_reasons,
        exchanges,
    };
}

/**
 * The digests of a prompt field an attempt records as text (`<field>`), as a digest
 * (`<field>_sha256`), or both; `conflict` names an attempt whose text and digest disagree.
 */
function promptHashes(
    evidence: Evidence,
    field: "system" | "prompt",
): { hashes: string[]; conflict: boolean; malformed: boolean } {
    const hashes: string[] = [];
    let conflict = false;
    let malformed = false;
    for (const attempt of attemptsOf(evidence)) {
        const claimed = attempt[`${field}_sha256`];
        const recorded = typeof attempt[field] === "string" ? sha256(attempt[field]) : null;
        if (claimed !== undefined && !digest(claimed)) malformed = true;
        if (recorded !== null && digest(claimed) && claimed !== recorded) conflict = true;
        if (recorded !== null) hashes.push(recorded);
        else if (digest(claimed)) hashes.push(claimed);
    }
    return { hashes, conflict, malformed };
}

/** The system prompt hashes an observation claims its producer ran. */
function systemHashes(evidence: Evidence): string[] {
    return promptHashes(evidence, "system").hashes;
}

function attemptsOf(evidence: Evidence): Json[] {
    return Array.isArray(evidence.detail.attempts) ? evidence.detail.attempts.filter(record) : [];
}

function outputOrigins(evidence: Evidence): string[] {
    return [evidence.detail, ...attemptsOf(evidence)].flatMap((holder) =>
        typeof holder.output_origin === "string" ? [holder.output_origin] : [],
    );
}

export function servedTierOf(evidence: Evidence): string | null | undefined {
    const tier = evidence.detail.served_tier ?? evidence.detail.tier;
    if (tier === undefined) return undefined;
    return typeof tier === "string" ? tier : null;
}

function completeRealAttempt(attempt: Json, model: string): boolean {
    return (
        attempt.model === model &&
        (text(attempt.system) || digest(attempt.system_sha256)) &&
        text(attempt.prompt) &&
        Array.isArray(attempt.outputs) &&
        attempt.outputs.some((output) => record(output) && typeof output.text === "string")
    );
}

/** A source-level generation record: a real capture, or the U2 replay's generation stage. */
function isGeneration(evidence: { owner: string; stage: string }): boolean {
    return (
        evidence.owner === REAL_CAPTURE ||
        (evidence.owner === REPLAY && evidence.stage === "generation")
    );
}

/**
 * The hash and completeness errors of one forwarding report. A report marked complete carries
 * the writer's completeness invariants the assembler can observe: at least one send, and for
 * every exchange an untruncated response with a known cost and a hash of its bytes.
 */
function checkForwarding(file: string, report: ForwardingEvidence): string[] {
    const errors: string[] = [];
    const complete = (index: number, reason: string) => {
        if (report.complete)
            errors.push(`${file} exchange ${index} ${reason} in a complete report`);
    };
    if (report.complete && report.exchanges.length === 0) {
        errors.push(`${file} complete report records no send`);
    }
    if (report.complete && report.stopped !== null) {
        errors.push(`${file} complete report records a stop: ${report.stopped}`);
    }
    if (report.complete && report.incomplete_reasons.length > 0) {
        errors.push(
            `${file} complete report lists incomplete reasons: ${report.incomplete_reasons.join("; ")}`,
        );
    }
    const cap = report.limits.spendCapUsd;
    if (report.complete && typeof cap === "number" && report.spent_usd > cap) {
        errors.push(
            `${file} complete report spent ${report.spent_usd} USD above its ${cap} USD cap`,
        );
    }
    for (const exchange of report.exchanges) {
        // The hashed representation is the request body as UTF-8 bytes, the bytes sent.
        if (sha256(exchange.request.body_text) !== exchange.request.body_sha256) {
            errors.push(`${file} exchange ${exchange.index} request bytes do not match their hash`);
        }
        for (const id of exchange.tool_uses) {
            const answered = report.exchanges.some(
                (later) => later.index > exchange.index && later.tool_results.includes(id),
            );
            if (!answered) {
                complete(exchange.index, `asks for tool ${id} that no later request answers`);
            }
        }
        const response = exchange.response;
        if (!response) {
            complete(exchange.index, "has no response");
            continue;
        }
        if (!response.cost_known) complete(exchange.index, "cost is unknown");
        if (response.outcome !== "acknowledged") {
            complete(exchange.index, `response outcome is ${response.outcome}`);
        }
        if (response.stop_reason === null)
            complete(exchange.index, "response states no stop reason");
        if (response.model === null) complete(exchange.index, "response names no model");
        else if (response.model !== report.model) {
            complete(exchange.index, `response names model ${response.model}`);
        }
        if (response.truncated) {
            complete(exchange.index, "response is truncated");
            continue;
        }
        if (response.body_sha256 === null) {
            complete(exchange.index, "response has no hash");
        } else if (sha256(response.body_text) !== response.body_sha256) {
            errors.push(
                `${file} exchange ${exchange.index} response bytes do not match their hash`,
            );
        }
    }
    return errors;
}

/** Reads one arm directory and checks every file's identity against `corpus`. */
export async function loadArm(
    dir: string,
    corpus: FidelityCorpus,
    corpusSha256: string,
): Promise<Arm> {
    const arm: Arm = {
        dir,
        config: null,
        files: [],
        evidence: [],
        forwarding: [],
        errors: [],
        foreignCorpus: false,
    };
    let names: string[];
    try {
        names = fs.readdirSync(dir).sort();
    } catch {
        arm.errors.push(`${dir} is unreadable`);
        return arm;
    }
    const scenarioEntry = new Map(scenarios(corpus).map((s) => [s.scenario.id, s]));
    const sourceCase = new Map(sources(corpus).map((s) => [s.source, s.case]));
    // The Rust writer stages `.<name>.tmp` and the TypeScript writer `<name>.tmp-<hex>`;
    // either left behind means a publication never finished.
    const unpublished = (name: string) => name.startsWith(".") || /\.tmp(-|$)/.test(name);
    const reads = new Map(
        await Promise.all(
            names
                .filter((name) => name.endsWith(".json") && !unpublished(name))
                .map(async (name) => [name, await readJson(join(dir, name))] as const),
        ),
    );
    arm.files = [...reads].map(([file, read]) => ({
        file,
        sha256: read.bytes && sha256(read.bytes),
    }));
    const seen = new Set<string>();
    const foreign = (name: string, bound: unknown) => {
        arm.foreignCorpus = true;
        arm.errors.push(`${name} is bound to corpus ${String(bound)}`);
    };
    for (const name of names) {
        if (unpublished(name)) {
            arm.errors.push(`${name} is an unpublished temporary file`);
            continue;
        }
        const read = reads.get(name);
        if (!read) continue;
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
        if (owner === "opencode-delivery" && !text(value.scenario)) {
            arm.errors.push(`${name} has no scenario label`);
            continue;
        }
        const parts = typeof value.scenario === "string" ? value.scenario.split("@") : [null];
        const [label, variant = null] = parts;
        if (parts.length > 2 || variant === "" || label === "") {
            arm.errors.push(`${name} has a malformed scenario label ${String(value.scenario)}`);
            continue;
        }
        if (variant !== null) {
            const emitted = VARIANTS[owner] ?? [];
            if (!emitted.some((v) => v.variant === variant)) {
                arm.errors.push(`${name} labels variant ${variant}, which no witness emits`);
                continue;
            }
            if (!emitted.some((v) => v.variant === variant && v.scenario === label)) {
                arm.errors.push(
                    `${name} labels variant ${variant}, which no witness emits for ${label}`,
                );
                continue;
            }
        }
        const source = String(value.source);
        const sourceLabel = label !== null && !scenarioEntry.has(label) && sourceCase.has(label);
        if (sourceLabel && label !== source) {
            arm.errors.push(`${name} labels source ${label} but names source ${source}`);
            continue;
        }
        if (!text(value.stage) || !text(value.terminal)) {
            arm.errors.push(`${name} has no ${text(value.stage) ? "terminal" : "stage"}`);
            continue;
        }
        const detail = record(value.detail) ? value.detail : {};
        if (owner === "opencode-delivery" && detail.judge_control === true) {
            if (value.stage !== JUDGE_CONTROL.stage) {
                arm.errors.push(`${name} marks stage ${value.stage} as a judge control`);
                continue;
            }
            if (label !== JUDGE_CONTROL.scenario) {
                arm.errors.push(
                    `${name} marks ${label} as a judge control, which the witness tests on ${JUDGE_CONTROL.scenario}`,
                );
                continue;
            }
        }
        const exactRead =
            label !== null && scenarioEntry.get(label)?.scenario.serving.path === "exact_read";
        if (owner === EXACT_READ_OWNER && label !== null && !exactRead) {
            arm.errors.push(`${name} names scenario ${label}, which is not an exact read`);
            continue;
        }
        if (label !== null && !sourceLabel && isGeneration({ owner, stage: value.stage })) {
            arm.errors.push(
                `${name} names scenario ${label}; a ${value.stage} stage is source-level`,
            );
            continue;
        }
        const evidence: Evidence = {
            file: name,
            sha256: sha256(read.bytes),
            owner,
            case: String(value.case),
            source,
            scenario: sourceLabel ? null : (label ?? null),
            variant,
            stage: value.stage,
            terminal: value.terminal,
            markers: strings(value.markers),
            detail,
        };
        const entry = evidence.scenario ? scenarioEntry.get(evidence.scenario) : undefined;
        const caseOf = evidence.scenario ? entry?.case : sourceCase.get(evidence.source);
        if (caseOf !== evidence.case) {
            arm.errors.push(
                `${name} names case ${evidence.case}, where the item is in ${caseOf ?? "no case"}`,
            );
            continue;
        }
        if (entry && entry.scenario.source !== evidence.source) {
            arm.errors.push(
                `${name} names source ${evidence.source}, where ${entry.scenario.id} is on ${entry.scenario.source}`,
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
        const system = promptHashes(evidence, "system");
        if (system.malformed || promptHashes(evidence, "prompt").malformed) {
            errors.push(`${evidence.file} records a malformed digest`);
        }
        if (system.conflict) {
            errors.push(
                `${evidence.file} records a system prompt whose digest differs from its text`,
            );
        }
        for (const hash of system.hashes) {
            if (hash !== config.prompt_sha256) {
                errors.push(`${evidence.file} ran system prompt ${hash}, not the arm's prompt`);
            }
        }
    }
    const generations = arm.evidence.filter((e) => isGeneration(e) && e.terminal === "published");
    for (const generation of generations) {
        if (systemHashes(generation).length === 0) {
            errors.push(`${generation.file} records no system prompt`);
        } else if (systemHashes(generation).length < attemptsOf(generation).length) {
            errors.push(`${generation.file} records no system prompt on an attempt`);
        }
        for (const attempt of attemptsOf(generation)) {
            if (attempt.model !== config.model) {
                errors.push(
                    `${generation.file} attempt ran model ${String(attempt.model)}, not the arm's model`,
                );
            }
        }
        const prompt = promptHashes(generation, "prompt");
        if (prompt.conflict) {
            errors.push(
                `${generation.file} records a user prompt whose digest differs from its text`,
            );
        }
        if (prompt.hashes.length < attemptsOf(generation).length) {
            errors.push(`${generation.file} records no user prompt on an attempt`);
        }
    }
    if (config.generation_origin !== "real") {
        for (const evidence of arm.evidence) {
            if (outputOrigins(evidence).some((origin) => origin.startsWith("real"))) {
                errors.push(`${evidence.file} carries real output in an arm labeled scripted`);
            }
        }
        const replayed = generations.filter(
            (g) =>
                g.owner === REPLAY &&
                outputOrigins(g).some((origin) => origin.includes("scripted")),
        );
        const reviewed = new Map(
            corpus.cases.flatMap((c) => c.sources.map((s) => [s.id, sha256(s.reviewedOutput)])),
        );
        for (const generation of replayed) {
            for (const attempt of attemptsOf(generation)) {
                if (attempt.output_sha256 !== reviewed.get(generation.source)) {
                    errors.push(
                        `${generation.file} returned output other than the reviewed output of ${generation.source}`,
                    );
                }
            }
        }
        for (const { source } of sources(corpus)) {
            if (!replayed.some((g) => g.source === source)) {
                errors.push(`no published scripted generation for ${source}`);
            }
        }
        return errors;
    }
    const published = generations.filter((g) => g.owner === REAL_CAPTURE);
    for (const capture of published) {
        // The capture writer publishes only a settled generation that drained text and stored
        // rows; a published capture missing any of them was not published by it.
        if (capture.detail.settled !== true) {
            errors.push(`${capture.file} is published without settled generation`);
        }
        const drained = attemptsOf(capture).some(
            (attempt) =>
                Array.isArray(attempt.outputs) &&
                attempt.outputs.some((output) => record(output) && text(output.text)),
        );
        if (!drained) errors.push(`${capture.file} is published without a drained text output`);
        const rows = capture.detail.published_rows;
        if (!Array.isArray(rows) || rows.length === 0) {
            errors.push(`${capture.file} is published without published rows`);
        }
        if (capture.detail.model !== config.model) {
            errors.push(
                `${capture.file} captured with model ${String(capture.detail.model)}, not the arm's model`,
            );
        }
        for (const attempt of attemptsOf(capture)) {
            for (const setting of ATTEMPT_SETTINGS) {
                if (attempt[setting] !== config.settings[setting]) {
                    errors.push(
                        `${capture.file} ran ${setting} ${String(attempt[setting])}, not the arm's ${String(config.settings[setting])}`,
                    );
                }
            }
        }
    }
    const captures = published.filter(
        (c) =>
            c.detail.model === config.model &&
            String(c.detail.output_origin ?? "").startsWith("real"),
    );
    for (const evidence of arm.evidence) {
        if (outputOrigins(evidence).some((origin) => origin.includes("scripted"))) {
            errors.push(`${evidence.file} carries scripted output in an arm labeled real`);
        }
        if (servedTierOf(evidence) === undefined) continue;
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
    for (const capture of captures) {
        const attempts = attemptsOf(capture);
        if (!attempts.some((attempt) => completeRealAttempt(attempt, config.model))) {
            errors.push(`${capture.file} records no complete real attempt`);
        }
    }
    return errors;
}

function deterministicOf(scenario: FidelityScenario, evidence: Evidence[]): Deterministic {
    if (scenario.serving.path === "exact_read") {
        return evidence.some((e) => e.owner === EXACT_READ_OWNER && e.terminal === "read_exact")
            ? "pass"
            : "not_evaluated";
    }
    // Delivery observations name the tier OpenCode served; the U2 replay names the tier its
    // serving pass rendered. A delivery under positive-budget pressure passes when it serves a
    // tier sparser than the curve's, the oracle that witness documents; every other observation
    // must serve the corpus tier.
    const results = evidence.flatMap((e) => {
        const tier = servedTierOf(e);
        if (tier === undefined) return [];
        // A served tier that is present but no string names no tier at all.
        if (tier === null) return [false];
        const curve = e.detail.curve_tier;
        if (scenario.serving.path === "pressure" && e.owner === "opencode-delivery") {
            return [
                typeof curve === "string" &&
                    TIERS.includes(curve) &&
                    TIERS.indexOf(tier) > TIERS.indexOf(curve),
            ];
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
            const first = arm.forwarding[0];
            for (const { file, report } of arm.forwarding) {
                if (first && report.model !== first.report.model) {
                    errors.push(
                        `${file} forwarded to ${report.model}, where ${first.file} forwarded to ${first.report.model}`,
                    );
                }
                if (first && report.upstream_url !== first.report.upstream_url) {
                    errors.push(
                        `${file} forwarded to ${report.upstream_url}, where ${first.file} forwarded to ${first.report.upstream_url}`,
                    );
                }
                if (
                    first &&
                    canonicalJson(report.pricing) !== canonicalJson(first.report.pricing)
                ) {
                    errors.push(
                        `${file} forwarded at prices ${canonicalJson(report.pricing)}, where ${first.file} forwarded at ${canonicalJson(first.report.pricing)}`,
                    );
                }
                if (first && report.context_limit !== first.report.context_limit) {
                    errors.push(
                        `${file} forwarded at context limit ${report.context_limit}, where ${first.file} forwarded at ${first.report.context_limit}`,
                    );
                }
                if (!report.complete) {
                    errors.push(`${file} is incomplete: ${report.incomplete_reasons.join("; ")}`);
                }
                if (canonicalJson(report.limits) !== canonicalJson(arm.config?.limits)) {
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
    if (base.label === cand.label) refused.push(`the arms share the label ${base.label}`);
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
        const before = canonicalJson(input.baseline.config?.[field] ?? null);
        const after = canonicalJson(input.candidate.config?.[field] ?? null);
        if (before !== after) refused.push(`the arms differ in ${field}`);
    }
    // The user prompt each source was generated from is held equal; only the system prompt is
    // the treatment.
    for (const { source } of sources(corpus)) {
        const [before, after] = sides.map((arm) =>
            JSON.stringify(
                arm.evidence
                    .filter(
                        (e) => isGeneration(e) && e.terminal === "published" && e.source === source,
                    )
                    .flatMap((e) => promptHashes(e, "prompt").hashes)
                    .sort(),
            ),
        );
        if (before !== after) refused.push(`the arms generated ${source} from different prompts`);
    }
    // Each row serves the captures its observations link; those prompts are held equal too, so
    // an arm cannot hide a differently prompted serving behind a spare capture of the source.
    const servedPrompts = (side: (typeof arms)[number], row: EvidenceRow) => {
        const links = new Set(
            row.evidence
                .filter((e) => servedTierOf(e) !== undefined)
                .map((e) => e.detail.generation_capture_sha256)
                .filter(text),
        );
        return JSON.stringify(
            side.arm.evidence
                .filter((e) => isGeneration(e) && e.terminal === "published" && links.has(e.sha256))
                .flatMap((e) => promptHashes(e, "prompt").hashes)
                .sort(),
        );
    };
    base.rows.forEach((row, i) => {
        const other = cand.rows[i];
        if (other && servedPrompts(base, row) !== servedPrompts(cand, other)) {
            refused.push(`the arms served ${row.scenario.id} from captures with different prompts`);
        }
    });
    // The forwarded model is OpenCode's, a role apart from the summarizer model `arm.json`
    // declares, so live arms hold it equal through their reports.
    const forwarded = sides.map((arm) => arm.forwarding[0]?.report ?? null);
    if (input.mode === "live" && forwarded[0]?.model !== forwarded[1]?.model) {
        refused.push("the arms forwarded to different models");
    }
    if (input.mode === "live" && forwarded[0]?.context_limit !== forwarded[1]?.context_limit) {
        refused.push("the arms forwarded at different context limits");
    }
    if (input.mode === "live" && forwarded[0]?.upstream_url !== forwarded[1]?.upstream_url) {
        refused.push("the arms forwarded to different upstream endpoints");
    }
    if (
        input.mode === "live" &&
        canonicalJson(forwarded[0]?.pricing ?? null) !==
            canonicalJson(forwarded[1]?.pricing ?? null)
    ) {
        refused.push("the arms forwarded at different prices");
    }
    const manifest = {
        schema: MANIFEST_SCHEMA,
        repository_revision: input.revision,
        mode: input.mode,
        corpus: { path: input.corpusPath, sha256: input.corpusSha256 },
        arms: sides.map((arm) => ({
            label: armLabel(arm),
            config: arm.config,
            files: arm.files,
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
            forwarding_reports: arm.forwarding.map(({ file, sha256, report }) => ({
                file,
                sha256,
                model: report.model,
                upstream_url: report.upstream_url,
                context_limit: report.context_limit,
                pricing: report.pricing,
            })),
        })),
    };
    return {
        manifest,
        arms,
        refused,
        treatment: input.baseline.config?.prompt_sha256 !== input.candidate.config?.prompt_sha256,
    };
}

/**
 * Narrow TypeScript reader for the compression fidelity corpus.
 *
 * The daemon's testdata file `compression-fidelity.json` is the oracle, and the Rust test module
 * `crates/daemon/src/compression_fidelity_corpus.rs` owns its source, span, and revision
 * validation. This reader accepts only the exact file bytes whose SHA-256 equals
 * {@link COMPRESSION_FIDELITY_CORPUS_SHA256}, the same pin the Rust corpus module enforces, and then
 * exposes case and scenario IDs, follow-up prompts, the reviewed producer outputs, and review
 * expectations. Native source text, spans, obligation statements, and forbidden conclusions stay
 * in the file.
 */

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/** SHA-256 of the complete committed corpus file. It moves with the Rust pin, never alone. */
export const COMPRESSION_FIDELITY_CORPUS_SHA256 =
    "46980759b02b7696ea3be9d9b44e43603eed199a404620c3c6d952afbc153800";

export const COMPRESSION_FIDELITY_CORPUS_PATH = resolve(
    import.meta.dir,
    "../../../../crates/daemon/testdata/compression-fidelity.json",
);

const TIERS = ["p1", "p2", "p3", "p4", "p5"] as const;
const STAGES = ["m1", "m0"] as const;
const PATHS = [
    "natural",
    "pressure",
    "omission",
    "hint_truncated",
    "memory_admitted",
    "memory_excluded",
    "exact_read",
] as const;
const DISPOSITIONS = ["visible", "discoverable", "unavailable"] as const;
const ABSTENTIONS = ["permitted", "forbidden"] as const;

export type ServedTier = (typeof TIERS)[number];
export type ServingStage = (typeof STAGES)[number];
export type ServingPath = (typeof PATHS)[number];
export type Disposition = (typeof DISPOSITIONS)[number];

export interface FidelityExpectation {
    readonly obligation: string;
    readonly accepted: readonly Disposition[];
}

export interface FidelityScenario {
    readonly id: string;
    readonly source: string;
    readonly followUp: { readonly id: string; readonly prompt: string };
    readonly serving: {
        readonly path: ServingPath;
        readonly tier?: ServedTier;
        readonly stage?: ServingStage;
    };
    readonly expectations: readonly FidelityExpectation[];
    readonly abstention: (typeof ABSTENTIONS)[number];
    /** IDs of the forbidden conclusions a reviewer checks; the statements stay in the corpus. */
    readonly forbidden: readonly string[];
}

export interface FidelitySource {
    readonly id: string;
    /** The human-approved producer output that scripted runs return for this source. */
    readonly reviewedOutput: string;
}

export interface FidelityCase {
    readonly id: string;
    readonly sources: readonly FidelitySource[];
    readonly scenarios: readonly FidelityScenario[];
}

export interface FidelityCorpus {
    readonly cases: readonly FidelityCase[];
}

/** The corpus bytes are missing, unreadable, mismatched, or not the shape this reader reads. */
export class CorpusIdentityError extends Error {
    override readonly name = "CorpusIdentityError";
}

type Json = Record<string, unknown>;

function fail(message: string): never {
    throw new CorpusIdentityError(message);
}

function object(value: unknown, where: string): Json {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
        fail(`${where} is not an object`);
    }
    return value as Json;
}

function array(value: unknown, where: string): unknown[] {
    if (!Array.isArray(value)) fail(`${where} is not an array`);
    return value;
}

function string(value: unknown, where: string): string {
    if (typeof value !== "string") fail(`${where} is not a string`);
    return value;
}

function member<const A extends readonly string[]>(
    value: unknown,
    allowed: A,
    where: string,
): A[number] {
    const text = string(value, where);
    if (!(allowed as readonly string[]).includes(text)) {
        fail(`${where} is not one of ${allowed.join(", ")}`);
    }
    return text;
}

function readScenario(value: unknown, followUps: Map<string, string>): FidelityScenario {
    const scenario = object(value, "scenario");
    const id = string(scenario.id, "scenario.id");
    const followUpId = string(scenario.follow_up, `${id}.follow_up`);
    const prompt = followUps.get(followUpId) ?? fail(`${id} names unknown ${followUpId}`);
    const serving = object(scenario.serving, `${id}.serving`);
    return {
        id,
        source: string(scenario.source, `${id}.source`),
        followUp: { id: followUpId, prompt },
        serving: {
            path: member(serving.path, PATHS, `${id}.serving.path`),
            ...(serving.tier === undefined
                ? {}
                : { tier: member(serving.tier, TIERS, `${id}.serving.tier`) }),
            ...(serving.stage === undefined
                ? {}
                : { stage: member(serving.stage, STAGES, `${id}.serving.stage`) }),
        },
        expectations: array(scenario.expectations, `${id}.expectations`).map((entry) => {
            const expectation = object(entry, `${id}.expectation`);
            return {
                obligation: string(expectation.obligation, `${id}.expectation.obligation`),
                accepted: array(expectation.accepted, `${id}.expectation.accepted`).map((d) =>
                    member(d, DISPOSITIONS, `${id}.expectation.accepted`),
                ),
            };
        }),
        abstention: member(scenario.abstention, ABSTENTIONS, `${id}.abstention`),
        forbidden: array(scenario.forbidden, `${id}.forbidden`).map((f) =>
            string(f, `${id}.forbidden`),
        ),
    };
}

function readCase(value: unknown): FidelityCase {
    const entry = object(value, "case");
    const id = string(entry.id, "case.id");
    const followUps = new Map<string, string>();
    for (const raw of array(entry.follow_ups, `${id}.follow_ups`)) {
        const followUp = object(raw, `${id}.follow_up`);
        followUps.set(
            string(followUp.id, `${id}.follow_up.id`),
            string(followUp.prompt, `${id}.follow_up.prompt`),
        );
    }
    return {
        id,
        sources: array(entry.sources, `${id}.sources`).map((raw) => {
            const source = object(raw, `${id}.source`);
            return {
                id: string(source.id, `${id}.source.id`),
                reviewedOutput: string(source.approved_example, `${id}.source.approved_example`),
            };
        }),
        scenarios: array(entry.scenarios, `${id}.scenarios`).map((raw) =>
            readScenario(raw, followUps),
        ),
    };
}

/**
 * Reads the corpus at `path` and returns its narrow view.
 *
 * Throws {@link CorpusIdentityError} when the file is missing or unreadable, when its bytes do
 * not hash to `expectedSha256`, or when a field this reader exposes has the wrong shape. Hashing
 * covers the complete bytes with no normalization, so whitespace-only and same-length edits are
 * rejected. Callers keep the default digest; only the reader's own shape test passes another.
 */
export function readCompressionFidelityCorpus(
    path: string = COMPRESSION_FIDELITY_CORPUS_PATH,
    expectedSha256: string = COMPRESSION_FIDELITY_CORPUS_SHA256,
): FidelityCorpus {
    let bytes: Buffer;
    try {
        bytes = readFileSync(path);
    } catch (error) {
        fail(`corpus at ${path} is unreadable: ${(error as Error).message}`);
    }
    const sha256 = createHash("sha256").update(bytes).digest("hex");
    if (sha256 !== expectedSha256) {
        fail(`corpus at ${path} hashes to ${sha256}, expected ${expectedSha256}`);
    }
    let parsed: unknown;
    try {
        parsed = JSON.parse(bytes.toString("utf8"));
    } catch (error) {
        fail(`corpus at ${path} is not JSON: ${(error as Error).message}`);
    }
    return {
        cases: array(object(parsed, "corpus").cases, "corpus.cases").map(readCase),
    };
}

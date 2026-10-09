/**
 * Evidence directories for the `eval:compression-fidelity` tests: one complete arm per call,
 * with a published real capture per corpus source and one observation per corpus scenario.
 */

import { afterEach } from "bun:test";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { COMPRESSION_FIDELITY_CORPUS_SHA256, readCompressionFidelityCorpus } from "./corpus";
import { ARM_SCHEMA } from "./evidence";

export const corpus = readCompressionFidelityCorpus();
export const SHA = COMPRESSION_FIDELITY_CORPUS_SHA256;
export const CORPUS_PATH = resolve(
    import.meta.dir,
    "../../../../crates/daemon/testdata/compression-fidelity.json",
);
export const sha256 = (text: string | Buffer) => createHash("sha256").update(text).digest("hex");
export const allScenarios = corpus.cases.flatMap((c) =>
    c.scenarios.map((s) => ({ case: c.id, s })),
);

const dirs: string[] = [];
export function scratch(): string {
    const dir = mkdtempSync(join(tmpdir(), "cf-eval-"));
    dirs.push(dir);
    return dir;
}
afterEach(() => {
    for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

export function write(dir: string, name: string, value: unknown): string {
    const text = `${JSON.stringify(value, null, 2)}\n`;
    writeFileSync(join(dir, name), text);
    return sha256(text);
}

export const SETTINGS = { temperature: 0.1, max_output_tokens: 1024 };

export const SERVING = {
    request_body_utf8_bytes: 40_000,
    admission: "fits",
    invocation_bytes: 39_000,
    // ceil(ceil(39_000 / 3.5) * 1.25): the admission estimator's charge for those bytes.
    invocation_charged_tokens: 13_929,
    estimator: "opencode-heuristic utf8-bytes-div-3.5-v1",
    transform_elapsed_ms: 12,
    raw_source_leaks: 0,
    serving_kind: "cold",
};

export interface ArmOptions {
    label: string;
    system?: string;
    corpusSha256?: string;
    skipScenario?: string;
    origin?: string;
    serving?: Record<string, unknown>;
    tier?: (scenario: string) => string | undefined;
    unlinked?: boolean;
    model?: string;
    captureModel?: string;
    attempt?: Record<string, unknown> | null;
    /** Replaces the single default attempt with the attempts this returns. */
    attempts?: (attempt: Record<string, unknown>) => unknown[];
    capture?: Record<string, unknown>;
    settings?: Record<string, unknown>;
    promptSha256?: string;
    generationOrigin?: "scripted" | "real";
    limits?: Record<string, number>;
}

export function writeArm(
    root: string,
    options: ArmOptions,
): { dir: string; files: Map<string, string> } {
    const dir = join(root, options.label);
    mkdirSync(dir);
    const system = options.system ?? "summarizer system prompt";
    const model = options.model ?? "anthropic/claude-test";
    write(dir, "arm.json", {
        schema: ARM_SCHEMA,
        label: options.label,
        prompt_sha256: options.promptSha256 ?? sha256(system),
        model,
        provider: "anthropic",
        version: "2026-01-01",
        settings: options.settings ?? SETTINGS,
        limits: options.limits ?? { maxCalls: 40 },
        generation_origin: options.generationOrigin ?? "real",
    });
    const files = new Map<string, string>();
    const base = (c: string, source: string) => ({
        schema_version: 1,
        corpus_sha256: options.corpusSha256 ?? SHA,
        case: c,
        source,
        markers: [],
    });
    const captures = new Map<string, string>();
    const scripted = options.generationOrigin === "scripted";
    const attemptsOf = (attempt: Record<string, unknown>): unknown[] => {
        if (options.attempt === null) return [];
        const merged = { ...attempt, ...options.attempt };
        return options.attempts ? options.attempts(merged) : [merged];
    };
    for (const c of corpus.cases) {
        for (const source of c.sources) {
            if (scripted) {
                write(dir, `generation.${source.id}.json`, {
                    ...base(c.id, source.id),
                    owner: "daemon.compression_fidelity.replay",
                    scenario: null,
                    stage: "generation",
                    terminal: "published",
                    detail: {
                        attempts: attemptsOf({
                            attempt: 1,
                            model,
                            system_sha256: sha256(system),
                            prompt_sha256: sha256("p"),
                            output_sha256: sha256(source.reviewedOutput),
                            output_origin: "scripted approved example",
                        }),
                    },
                });
                continue;
            }
            const attempts = attemptsOf({
                model,
                system,
                prompt: "p",
                ...SETTINGS,
                outputs: [{ text: "x" }],
            });
            const capture = write(dir, `real.${source.id}.json`, {
                ...base(c.id, source.id),
                owner: "daemon.compression_fidelity.real_capture",
                scenario: null,
                stage: "capture",
                terminal: "published",
                detail: {
                    model: options.captureModel ?? model,
                    output_origin: options.origin ?? "real producer through the host",
                    attempts,
                    usage: { input_tokens: 900, output_tokens: 300 },
                    settled: true,
                    attempt_count: attempts.length,
                    published_rows: [{ start: 1, end: 2, title: "t", p1: "p1" }],
                    ...options.capture,
                },
            });
            captures.set(source.id, capture);
        }
    }
    for (const { case: c, s } of allScenarios) {
        if (s.id === options.skipScenario) continue;
        const exact = s.serving.path === "exact_read";
        const name = `delivery.${s.id}.json`;
        files.set(
            s.id,
            write(dir, name, {
                ...base(c, s.source),
                owner: exact ? "daemon.harness_sources.c6_exact_read" : "opencode-delivery",
                scenario: s.id,
                stage: exact ? "exact_read" : "served",
                terminal: exact ? "read_exact" : "served",
                detail: exact
                    ? { sha256: "0".repeat(64), byte_length: 144 }
                    : {
                          served_tier: options.tier?.(s.id) ?? s.serving.tier,
                          // A pressure delivery serves sparser than the curve it was under.
                          ...(s.serving.path === "pressure" ? { curve_tier: "p1" } : {}),
                          serving: options.serving ?? SERVING,
                          generation_capture_sha256: options.unlinked
                              ? undefined
                              : captures.get(s.source),
                      },
            }),
        );
    }
    return { dir, files };
}

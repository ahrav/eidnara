import { describe, expect, test } from "bun:test";
import { mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs, run } from "../../scripts/eval-compression-fidelity";
import {
    type ArmOptions,
    allScenarios,
    CORPUS_PATH,
    corpus,
    SERVING,
    SHA,
    scratch,
    sha256,
    write,
    writeArm,
} from "./evaluation-fixtures";
import { loadArm } from "./evidence";
import {
    evaluate as assemble,
    CONTROLS_SCHEMA,
    JUDGMENTS_SCHEMA,
    KNOWN_BAD_CONTROLS,
    loadReviews,
    POSITIVE_CONTROLS,
} from "./review";

const BATCH = "batch-1";

interface ReviewOptions {
    corpusSha256?: string;
    batch?: string;
    verdict?: (label: string) => string;
    dropVerdict?: string;
    arms: Array<{ label: string; files: Map<string, string> }>;
    skipJudgments?: boolean;
    approvers?: string[];
    controlsCorpus?: string;
    editControls?: (controls: Array<Record<string, string>>) => Array<Record<string, string>>;
    editVerdicts?: (verdicts: Array<Record<string, string>>) => Array<Record<string, string>>;
}

function writeReviews(root: string, options: ReviewOptions): string {
    const dir = join(root, "reviews");
    mkdirSync(dir);
    const sealed: Array<Record<string, string>> = [
        ...KNOWN_BAD_CONTROLS.map((kind) => ({ id: `ctl-${kind}`, kind, label: "violation" })),
        ...POSITIVE_CONTROLS.map((kind) => ({ id: `ctl-${kind}`, kind, label: "acceptable" })),
    ];
    const verdicts = sealed
        .filter((c) => c.id !== options.dropVerdict)
        .map((c) => ({
            control: c.id ?? "",
            verdict: options.verdict ? options.verdict(c.label ?? "") : (c.label ?? ""),
            reviewer: "reviewer-c",
            kind: "human",
        }));
    const controls = options.editControls ? options.editControls(sealed) : sealed;
    write(dir, "controls.json", {
        schema: CONTROLS_SCHEMA,
        corpus_sha256: options.controlsCorpus ?? SHA,
        batch: BATCH,
        approved_by: options.approvers ?? ["reviewer-a", "reviewer-b"],
        controls,
    });
    write(dir, "judgments.json", {
        schema: JUDGMENTS_SCHEMA,
        corpus_sha256: options.corpusSha256 ?? SHA,
        batch: options.batch ?? BATCH,
        control_verdicts: options.editVerdicts ? options.editVerdicts(verdicts) : verdicts,
        judgments: options.skipJudgments
            ? []
            : options.arms.flatMap((arm) =>
                  allScenarios.flatMap(({ s }) => {
                      const artifact = arm.files.get(s.id);
                      if (!artifact) return [];
                      return [
                          {
                              id: `${arm.label}-${s.id}`,
                              kind: "human",
                              reviewer: "reviewer-c",
                              arm: arm.label,
                              scenario: s.id,
                              artifact_sha256: artifact,
                              obligations: s.expectations.map((e) => ({
                                  obligation: e.obligation,
                                  disposition:
                                      ["visible", "unavailable", "discoverable"].find((d) =>
                                          (e.accepted as readonly string[]).includes(d),
                                      ) ?? "visible",
                                  preserved: true,
                              })),
                              forbidden_violated: [],
                              abstained: false,
                          },
                      ];
                  }),
              ),
        disputes: [],
    });
    return dir;
}

function evaluate(
    root: string,
    options: {
        baseline?: Partial<ArmOptions>;
        candidate?: Partial<ArmOptions>;
        reviews?: Partial<ReviewOptions>;
        tamper?: (baselineDir: string) => void;
    } = {},
) {
    const baseline = writeArm(root, { label: "baseline", ...options.baseline });
    const candidate = writeArm(root, {
        label: "candidate",
        system: "candidate system prompt",
        ...options.candidate,
    });
    const reviews = writeReviews(root, {
        arms: [
            { label: "baseline", files: baseline.files },
            { label: "candidate", files: candidate.files },
        ],
        ...options.reviews,
    });
    options.tamper?.(baseline.dir);
    return assemble({
        corpus,
        corpusPath: "corpus.json",
        corpusSha256: SHA,
        revision: "rev",
        mode: "offline",
        baseline: loadArm(baseline.dir, corpus, SHA),
        candidate: loadArm(candidate.dir, corpus, SHA),
        reviews: loadReviews(reviews, SHA),
    });
}

describe("eval:compression-fidelity assembly", () => {
    test("a complete reviewed evidence set yields every column, a manifest, and a treatment comparison", () => {
        const { manifest, report } = evaluate(scratch());
        expect(report.arms.map((a) => a.identity_errors)).toEqual([[], []]);
        expect(report.controls.qualified).toBe(true);
        for (const arm of report.arms) {
            expect(arm.rows.length).toBe(allScenarios.length);
            for (const row of arm.rows) {
                expect(row.execution.status).toBe("executed");
                expect(row.deterministic).toBe("pass");
                expect(row.preservation.every((p) => p.status === "preserved")).toBe(true);
                expect(row.consumer_safety).toBe("safe");
                expect(row.semantic_review).toBe("reviewed");
                expect(row.cost.status).toBe("complete");
            }
        }
        expect(report.comparison.refused).toEqual([]);
        expect("treatment" in report.comparison && report.comparison.treatment).toBe(true);
        const rows = ("rows" in report.comparison ? report.comparison.rows : undefined) ?? [];
        expect(rows.length).toBe(allScenarios.length);
        expect(rows.every((r) => r.comparison === "expected_green")).toBe(true);
        expect(report.accepted).toBe(true);
        expect(manifest.arms[0]?.observations.length).toBe(
            allScenarios.length + corpus.cases.flatMap((c) => c.sources).length,
        );
        expect(manifest.corpus.sha256).toBe(SHA);
    });

    test("nothing is acceptable without human review", () => {
        const { report } = evaluate(scratch(), { reviews: { skipJudgments: true } });
        expect(report.arms.every((a) => !a.accepted)).toBe(true);
        expect(report.arms[0]?.rows.every((r) => r.semantic_review === "unreviewed")).toBe(true);
        expect(report.accepted).toBe(false);
    });

    test("a mutated capture byte unbinds its judgment", () => {
        const scenario = allScenarios[0]?.s.id ?? "";
        const { report } = evaluate(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${scenario}.json`);
                writeFileSync(path, `${readFileSync(path, "utf8")} `);
            },
        });
        const row = report.arms[0]?.rows.find((r) => r.scenario === scenario);
        expect(row?.semantic_review).toBe("unreviewed");
        expect(report.arms[0]?.accepted).toBe(false);
    });

    test("judgments bound to another corpus or batch are refused", () => {
        for (const reviews of [{ corpusSha256: "0".repeat(64) }, { batch: "batch-2" }]) {
            const { report } = evaluate(scratch(), { reviews });
            expect(report.controls.qualified).toBe(false);
            expect(report.accepted).toBe(false);
        }
    });

    test("a missing scenario withholds acceptance and refuses the comparison", () => {
        const missing = allScenarios[3]?.s.id ?? "";
        const { report } = evaluate(scratch(), { baseline: { skipScenario: missing } });
        expect(report.arms[0]?.missing_scenarios).toEqual([missing]);
        expect(report.arms[0]?.withheld.join("\n")).toContain(`missing scenarios: ${missing}`);
        expect(report.comparison.refused).toContain("the arms reached different scenario sets");
        expect(report.accepted).toBe(false);
    });

    test("a missing control verdict or an always-accept, always-abstain, or always-reject set leaves review unqualified", () => {
        const cases: Array<Partial<ReviewOptions>> = [
            { dropVerdict: "ctl-reversed_negation" },
            { verdict: () => "acceptable" },
            { verdict: () => "abstain" },
            { verdict: () => "violation" },
        ];
        for (const reviews of cases) {
            const { report } = evaluate(scratch(), { reviews });
            expect(report.controls.qualified).toBe(false);
            expect(report.accepted).toBe(false);
        }
    });

    test("a missing cost field leaves cost incomplete, never zero", () => {
        const { transform_elapsed_ms: _, ...serving } = SERVING;
        const { report } = evaluate(scratch(), { baseline: { serving } });
        const row = report.arms[0]?.rows.find((r) => r.cost.status !== "complete");
        expect(row?.cost.missing.join("\n")).toContain("transform_elapsed_ms");
        expect(report.arms[0]?.accepted).toBe(false);
    });
});

describe("eval:compression-fidelity gates", () => {
    test("controls must be exactly the sealed set, each with one matching human verdict", () => {
        const cases: Array<[Partial<ReviewOptions>, string]> = [
            [
                {
                    editControls: (c) =>
                        c.map((x) =>
                            x.kind === "reversed_negation" ? { ...x, label: "acceptable" } : x,
                        ),
                },
                "is labeled acceptable",
            ],
            [
                { editControls: (c) => c.filter((x) => x.kind !== "wrong_identity") },
                "no sealed wrong_identity",
            ],
            [{ editControls: (c) => [...c, { ...(c[0] ?? {}) }] }, "sealed more than once"],
            [
                {
                    editControls: (c) => [
                        ...c,
                        { id: "ctl-extra", kind: "invented", label: "violation" },
                    ],
                },
                "undeclared kind",
            ],
            [{ editVerdicts: (v) => [...v, { ...(v[0] ?? {}) }] }, "exactly one human verdict"],
            [
                {
                    editVerdicts: (v) =>
                        v.map((x) =>
                            x.control === "ctl-successful_deployment" ? { ...x, kind: "model" } : x,
                        ),
                },
                "exactly one human verdict",
            ],
            [
                {
                    editVerdicts: (v) => [
                        ...v,
                        {
                            control: "ctl-ghost",
                            verdict: "violation",
                            reviewer: "r",
                            kind: "human",
                        },
                    ],
                },
                "undeclared control",
            ],
        ];
        for (const [reviews, problem] of cases) {
            const { report } = evaluate(scratch(), { reviews });
            expect(report.controls.qualified).toBe(false);
            expect(report.controls.problems.join("\n")).toContain(problem);
        }
        const { report } = evaluate(scratch(), { reviews: { controlsCorpus: "3".repeat(64) } });
        expect(report.controls.qualified).toBe(false);
    });

    test("malformed control, verdict, and dispute entries are review errors", () => {
        const cases: Array<Partial<ReviewOptions>> = [
            { editControls: (c) => [...c, { id: "ctl-x", kind: "reversed_negation" }] },
            { editVerdicts: (v) => [...v, { control: "ctl-x", verdict: "violation" }] },
        ];
        for (const reviews of cases) {
            const { report } = evaluate(scratch(), { reviews });
            expect(report.controls.qualified).toBe(false);
            expect(report.arms[0]?.withheld.join("\n")).toContain("does not match its schema");
        }
        const root = scratch();
        const baseline = writeArm(root, { label: "baseline" });
        const candidate = writeArm(root, { label: "candidate" });
        const dir = writeReviews(root, {
            arms: [
                { label: "baseline", files: baseline.files },
                { label: "candidate", files: candidate.files },
            ],
        });
        const path = join(dir, "judgments.json");
        const judgments = JSON.parse(readFileSync(path, "utf8"));
        judgments.disputes.push({ arm: "baseline", scenario: allScenarios[0]?.s.id });
        writeFileSync(path, JSON.stringify(judgments));
        const reviews = loadReviews(dir, SHA);
        expect(reviews.errors.join("\n")).toContain("a dispute does not match its schema");
    });

    test("fewer than two approvers withhold acceptance", () => {
        const { report } = evaluate(scratch(), {
            reviews: { approvers: ["reviewer-a", "reviewer-a"] },
        });
        expect(report.arms[0]?.withheld).toContain("the batch has 1 of 2 approvers");
        expect(report.accepted).toBe(false);
    });

    test("the comparison names regressions and resolution candidates, and a same-prompt pair is no treatment", () => {
        const scenario = allScenarios.find(({ s }) => s.serving.tier === "p1")?.s.id ?? "";
        const wrong = (id: string) => (id === scenario ? "p3" : undefined);
        const comparisonOf = (report: ReturnType<typeof evaluate>["report"]) =>
            report.comparison.rows.find((r) => r.scenario === scenario)?.comparison;
        expect(comparisonOf(evaluate(scratch(), { candidate: { tier: wrong } }).report)).toBe(
            "regression",
        );
        expect(comparisonOf(evaluate(scratch(), { baseline: { tier: wrong } }).report)).toBe(
            "resolution_candidate",
        );
        const same = evaluate(scratch(), {
            candidate: { system: "summarizer system prompt" },
        }).report;
        expect(same.comparison.treatment).toBe(false);
    });

    test("unreported generation usage, an unknown send cost, or an unmeasured recovery leaves cost incomplete", () => {
        const root = scratch();
        const { report } = evaluate(root, {
            tamper: (dir) => {
                const source = corpus.cases[0]?.sources[0]?.id ?? "";
                const path = join(dir, `real.${source}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.detail.usage = null;
                writeFileSync(path, JSON.stringify(value));
            },
        });
        const generation = report.arms[0]?.rows.find((r) => r.case === corpus.cases[0]?.id);
        expect(generation?.cost.missing.join("\n")).toContain("generation usage unreported");

        const forwarded = evaluate(scratch(), {
            tamper: (dir) => {
                write(dir, "forwarding-0.json", {
                    mode: "forward",
                    corpus_sha256: SHA,
                    limits: { maxCalls: 40 },
                    complete: false,
                    incomplete_reasons: ["cost unknown for a send"],
                    exchanges: [
                        {
                            index: 0,
                            request: { body_text: "{}", body_sha256: sha256("{}") },
                            response: {
                                truncated: false,
                                body_text: "",
                                body_sha256: null,
                                cost_known: false,
                            },
                        },
                    ],
                });
            },
        }).report;
        expect(forwarded.arms[0]?.rows[0]?.cost.missing.join("\n")).toContain("unknown send cost");

        const recovery = evaluate(scratch(), {
            tamper: (dir) => {
                const entry = allScenarios[1];
                write(dir, "recovery.json", {
                    schema_version: 1,
                    corpus_sha256: SHA,
                    owner: "opencode-delivery",
                    case: entry?.case,
                    source: entry?.s.source,
                    scenario: entry?.s.id,
                    stage: "served-again",
                    terminal: "discoverable",
                    markers: ["cf-recovery-search"],
                    detail: {},
                });
            },
        }).report;
        const row = recovery.arms[0]?.rows.find((r) => r.scenario === allScenarios[1]?.s.id);
        expect(row?.cost.missing.join("\n")).toContain("recovery calls or output bytes");
    });
});

describe("eval:compression-fidelity judgments", () => {
    const discoverableFirst = allScenarios.find(({ s }) =>
        (s.expectations[0]?.accepted as readonly string[] | undefined)?.includes("discoverable"),
    )?.s;

    function rewrite(
        root: string,
        edit: (judgments: {
            judgments: Array<Record<string, unknown>>;
            disputes: unknown[];
        }) => void,
    ) {
        const baseline = writeArm(root, { label: "baseline" });
        const candidate = writeArm(root, { label: "candidate" });
        const reviewsDir = writeReviews(root, {
            arms: [
                { label: "baseline", files: baseline.files },
                { label: "candidate", files: candidate.files },
            ],
        });
        const path = join(reviewsDir, "judgments.json");
        const judgments = JSON.parse(readFileSync(path, "utf8"));
        edit(judgments);
        writeFileSync(path, JSON.stringify(judgments));
        return { baseline, reviewsDir };
    }

    function rowOf(root: string, baselineDir: string, reviewsDir: string, scenario: string) {
        const { report } = assemble({
            corpus,
            corpusPath: "corpus.json",
            corpusSha256: SHA,
            revision: "rev",
            mode: "offline",
            baseline: loadArm(baselineDir, corpus, SHA),
            candidate: loadArm(join(root, "candidate"), corpus, SHA),
            reviews: loadReviews(reviewsDir, SHA),
        });
        return report.arms[0]?.rows.find((r) => r.scenario === scenario);
    }

    const judgmentFor = (
        judgments: { judgments: Array<Record<string, unknown>> },
        scenario: string,
    ) => judgments.judgments.find((j) => j.arm === "baseline" && j.scenario === scenario) ?? {};

    test("a discoverable obligation needs a recovery observation", () => {
        const scenario = discoverableFirst;
        expect(scenario).toBeDefined();
        const root = scratch();
        const { baseline, reviewsDir } = rewrite(root, (j) => {
            const judgment = judgmentFor(j, scenario?.id ?? "");
            judgment.obligations = (judgment.obligations as Array<Record<string, unknown>>).map(
                (o, i) => (i === 0 ? { ...o, disposition: "discoverable" } : o),
            );
        });
        expect(rowOf(root, baseline.dir, reviewsDir, scenario?.id ?? "")?.recovery).toBe(
            "unverified",
        );
        write(baseline.dir, "recovery.json", {
            schema_version: 1,
            corpus_sha256: SHA,
            owner: "opencode-delivery",
            case: allScenarios.find((e) => e.s.id === scenario?.id)?.case,
            source: scenario?.source,
            scenario: scenario?.id,
            stage: "recovery-eidnara-search",
            terminal: "discoverable",
            markers: [],
            detail: { calls: 1, result_utf8_bytes: 400 },
        });
        const row = rowOf(root, baseline.dir, reviewsDir, scenario?.id ?? "");
        expect(row?.recovery).toBe("witnessed");
        expect(row?.cost.status).toBe("complete");
    });

    test("a lost obligation or a disposition outside the accepted set is recall", () => {
        const scenario = allScenarios.find(
            ({ s }) =>
                s.expectations.length >= 3 &&
                s.expectations.every(
                    (e) => !(e.accepted as readonly string[]).includes("unavailable"),
                ),
        )?.s;
        expect(scenario).toBeDefined();
        const root = scratch();
        const { baseline, reviewsDir } = rewrite(root, (j) => {
            const judgment = judgmentFor(j, scenario?.id ?? "");
            judgment.obligations = (judgment.obligations as Array<Record<string, unknown>>).map(
                (o, i) =>
                    i === 0
                        ? { ...o, preserved: false }
                        : i === 1
                          ? { ...o, disposition: "unavailable" }
                          : o,
            );
        });
        const row = rowOf(root, baseline.dir, reviewsDir, scenario?.id ?? "");
        expect(row?.preservation.map((p) => p.status)).toEqual([
            "recall",
            "recall",
            ...Array((scenario?.expectations.length ?? 2) - 2).fill("preserved"),
        ]);
        expect(row?.withheld).toContain("preservation");
    });

    test("agreeing human judgments in any obligation order are reviewed", () => {
        const scenario = allScenarios.find(({ s }) => s.expectations.length >= 2)?.s;
        const root = scratch();
        const { baseline, reviewsDir } = rewrite(root, (j) => {
            const original = judgmentFor(j, scenario?.id ?? "");
            j.judgments.push({
                ...original,
                id: "agreeing",
                reviewer: "reviewer-d",
                obligations: [...(original.obligations as unknown[])].reverse(),
            });
        });
        expect(rowOf(root, baseline.dir, reviewsDir, scenario?.id ?? "")?.semantic_review).toBe(
            "reviewed",
        );
    });

    test("disagreeing human judgments and an open dispute are disputed; a resolved dispute is not", () => {
        const [first, second, third] = allScenarios.map(({ s }) => s);
        const root = scratch();
        const { baseline, reviewsDir } = rewrite(root, (j) => {
            const original = judgmentFor(j, first?.id ?? "");
            j.judgments.push({
                ...original,
                id: "second-opinion",
                reviewer: "reviewer-d",
                forbidden_violated: first?.forbidden.slice(0, 1) ?? [],
            });
            j.disputes.push({ arm: "baseline", scenario: second?.id, resolved: false });
            j.disputes.push({ arm: "baseline", scenario: third?.id, resolved: true });
        });
        const row = (s?: { id: string }) => rowOf(root, baseline.dir, reviewsDir, s?.id ?? "");
        expect(row(first)?.semantic_review).toBe("disputed");
        expect(row(first)?.consumer_safety).toBe("unreviewed");
        expect(row(second)?.semantic_review).toBe("disputed");
        expect(row(second)?.withheld).toContain("semantic review disputed");
        expect(row(third)?.semantic_review).toBe("reviewed");
    });

    test("abstention, forbidden conclusions, model-only judgments, and disputes withhold credit", () => {
        const permitted = allScenarios.find(
            ({ s }) =>
                s.abstention === "permitted" &&
                s.expectations.some((e) => !(e.accepted as readonly string[]).includes("visible")),
        )?.s;
        const forbidden = allScenarios.find(({ s }) => s.abstention === "forbidden")?.s;
        const withForbidden = allScenarios.find(({ s }) => s.forbidden.length > 0)?.s;
        const others = allScenarios.filter(
            ({ s }) => ![permitted?.id, forbidden?.id, withForbidden?.id].includes(s.id),
        );
        const modelOnly = others[0]?.s;
        const disputed = others[1]?.s;
        const root = scratch();
        const { baseline, reviewsDir } = rewrite(root, (j) => {
            judgmentFor(j, permitted?.id ?? "").abstained = true;
            judgmentFor(j, forbidden?.id ?? "").abstained = true;
            judgmentFor(j, withForbidden?.id ?? "").forbidden_violated = [
                withForbidden?.forbidden[0],
            ];
            Object.assign(judgmentFor(j, modelOnly?.id ?? ""), {
                kind: "model",
                citations: ["C1.V1#0"],
                uncertainty: "low",
            });
            j.disputes.push({ arm: "baseline", scenario: disputed?.id, resolved: false });
        });
        const row = (s?: { id: string }) => rowOf(root, baseline.dir, reviewsDir, s?.id ?? "");
        expect(row(permitted)?.consumer_safety).toBe("abstained");
        expect(row(forbidden)?.consumer_safety).toBe("false-authoritative");
        expect(row(withForbidden)?.consumer_safety).toBe("false-authoritative");
        expect(row(modelOnly)?.semantic_review).toBe("model_only");
        expect(row(disputed)?.semantic_review).toBe("disputed");
        const abstainedUnavailable = row(permitted)?.preservation.filter(
            (p) => p.disposition === "unavailable",
        );
        expect(abstainedUnavailable?.length).toBeGreaterThan(0);
        expect(abstainedUnavailable?.every((p) => p.status === "recall")).toBe(true);
    });
});

describe("eval:compression-fidelity command", () => {
    test("flags are required once each and unknown flags are refused", () => {
        const base = ["--baseline", "a", "--candidate", "b", "--reviews", "r", "--out", "o"];
        expect(parseArgs(base).mode).toBe("offline");
        expect(() => parseArgs([...base, "--baseline", "c"])).toThrow();
        expect(() => parseArgs([...base, "--extra", "x"])).toThrow("unknown flag");
        expect(() => parseArgs([...base, "--mode", "online"])).toThrow("offline or live");
        expect(() => parseArgs(base.slice(2))).toThrow("--baseline is required");
    });

    test("the default run sends nothing and writes only owner-only files outside the repository", () => {
        const root = scratch();
        const baseline = writeArm(root, { label: "baseline" });
        const candidate = writeArm(root, { label: "candidate" });
        const reviews = writeReviews(root, {
            arms: [
                { label: "baseline", files: baseline.files },
                { label: "candidate", files: candidate.files },
            ],
        });
        const original = globalThis.fetch;
        const sent: string[] = [];
        const spy = async (input: RequestInfo | URL): Promise<Response> => {
            sent.push(String(input));
            throw new Error("no send");
        };
        globalThis.fetch = Object.assign(spy, { preconnect: original.preconnect });
        const out = join(root, "out");
        let written: ReturnType<typeof run> | undefined;
        try {
            written = run({
                baseline: baseline.dir,
                candidate: candidate.dir,
                reviews,
                out,
                corpus: CORPUS_PATH,
                mode: "offline",
            });
        } finally {
            globalThis.fetch = original;
        }
        expect(sent).toEqual([]);
        expect(written?.accepted).toBe(true);
        expect(readdirSync(out).sort()).toEqual(["manifest.json", "report.json"]);
        expect(statSync(out).mode & 0o777).toBe(0o700);
        expect(statSync(written?.report ?? "").mode & 0o777).toBe(0o600);
        const inside = resolve(import.meta.dir, "eval-out");
        expect(() =>
            run({
                baseline: baseline.dir,
                candidate: candidate.dir,
                reviews,
                out: inside,
                corpus: CORPUS_PATH,
                mode: "offline",
            }),
        ).toThrow("inside the repository");
    });
});

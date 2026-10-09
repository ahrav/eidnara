import { describe, expect, test } from "bun:test";
import {
    chmodSync,
    mkdirSync,
    readdirSync,
    readFileSync,
    statSync,
    symlinkSync,
    writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs, repositoryRevision, run } from "../../scripts/eval-compression-fidelity";
import { parseRustPassLine } from "../rust-harness";
import { type Delivery, servingCost } from "./campaign";
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

async function evaluate(
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
        baseline: await loadArm(baseline.dir, corpus, SHA),
        candidate: await loadArm(candidate.dir, corpus, SHA),
        reviews: await loadReviews(reviews, SHA),
    });
}

/** Rewrites the source's real capture and points its serving observations at the new hash. */
function recapture(dir: string, source: string, edit: (capture: Record<string, unknown>) => void) {
    const path = join(dir, `real.${source}.json`);
    const capture = JSON.parse(readFileSync(path, "utf8"));
    edit(capture);
    const relinked = write(dir, `real.${source}.json`, capture);
    for (const { s } of allScenarios.filter((e) => e.s.source === source)) {
        const file = join(dir, `delivery.${s.id}.json`);
        const observation = JSON.parse(readFileSync(file, "utf8"));
        if (observation.detail.generation_capture_sha256 === undefined) continue;
        observation.detail.generation_capture_sha256 = relinked;
        writeFileSync(file, JSON.stringify(observation));
    }
    return capture;
}

describe("eval:compression-fidelity assembly", () => {
    test("a complete reviewed evidence set yields every column, a manifest, and a treatment comparison", async () => {
        const { manifest, report } = await evaluate(scratch());
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

    test("nothing is acceptable without human review", async () => {
        const { report } = await evaluate(scratch(), { reviews: { skipJudgments: true } });
        expect(report.arms.every((a) => !a.accepted)).toBe(true);
        expect(report.arms[0]?.rows.every((r) => r.semantic_review === "unreviewed")).toBe(true);
        expect(report.accepted).toBe(false);
    });

    test("a mutated capture byte unbinds its judgment", async () => {
        const scenario = allScenarios[0]?.s.id ?? "";
        const { report } = await evaluate(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${scenario}.json`);
                writeFileSync(path, `${readFileSync(path, "utf8")} `);
            },
        });
        const row = report.arms[0]?.rows.find((r) => r.scenario === scenario);
        expect(row?.semantic_review).toBe("unreviewed");
        expect(report.arms[0]?.accepted).toBe(false);
    });

    test("judgments bound to another corpus or batch are refused", async () => {
        for (const reviews of [{ corpusSha256: "0".repeat(64) }, { batch: "batch-2" }]) {
            const { report } = await evaluate(scratch(), { reviews });
            expect(report.controls.qualified).toBe(false);
            expect(report.accepted).toBe(false);
        }
    });

    test("refused or unqualified review records bind no judgment to any row", async () => {
        const cases: Array<Partial<ReviewOptions>> = [
            { batch: "batch-2" },
            { corpusSha256: "0".repeat(64) },
            { verdict: () => "acceptable" },
        ];
        for (const reviews of cases) {
            const { report } = await evaluate(scratch(), { reviews });
            expect(report.controls.qualified).toBe(false);
            for (const arm of report.arms) {
                expect(arm.rows.every((r) => r.semantic_review === "unreviewed")).toBe(true);
                expect(arm.rows.every((r) => r.consumer_safety === "unreviewed")).toBe(true);
            }
            expect(report.comparison.rows.every((r) => r.comparison === "expected_red")).toBe(true);
        }
    });

    test("a missing scenario withholds acceptance and refuses the comparison", async () => {
        const missing = allScenarios[3]?.s.id ?? "";
        const { report } = await evaluate(scratch(), { baseline: { skipScenario: missing } });
        expect(report.arms[0]?.missing_scenarios).toEqual([missing]);
        expect(report.arms[0]?.withheld.join("\n")).toContain(`missing scenarios: ${missing}`);
        expect(report.comparison.refused).toContain("the arms reached different scenario sets");
        expect(report.accepted).toBe(false);
    });

    test("a missing control verdict or an always-accept, always-abstain, or always-reject set leaves review unqualified", async () => {
        const cases: Array<Partial<ReviewOptions>> = [
            { dropVerdict: "ctl-reversed_negation" },
            { verdict: () => "acceptable" },
            { verdict: () => "abstain" },
            { verdict: () => "violation" },
        ];
        for (const reviews of cases) {
            const { report } = await evaluate(scratch(), { reviews });
            expect(report.controls.qualified).toBe(false);
            expect(report.accepted).toBe(false);
        }
    });

    test("a missing cost field leaves cost incomplete, never zero", async () => {
        const { transform_elapsed_ms: _, ...serving } = SERVING;
        const { report } = await evaluate(scratch(), { baseline: { serving } });
        const row = report.arms[0]?.rows.find((r) => r.cost.status !== "complete");
        expect(row?.cost.missing.join("\n")).toContain("transform_elapsed_ms");
        expect(report.arms[0]?.accepted).toBe(false);
    });
});

describe("eval:compression-fidelity gates", () => {
    test("controls must be exactly the sealed set, each with one matching human verdict", async () => {
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
                        { id: "ctl-second", kind: "reversed_negation", label: "violation" },
                    ],
                    editVerdicts: (v) => [
                        ...v,
                        {
                            control: "ctl-second",
                            verdict: "violation",
                            reviewer: "reviewer-c",
                            kind: "human",
                        },
                    ],
                },
                "reversed_negation is sealed 2 times",
            ],
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
            const { report } = await evaluate(scratch(), { reviews });
            expect(report.controls.qualified).toBe(false);
            expect(report.controls.problems.join("\n")).toContain(problem);
        }
        const { report } = await evaluate(scratch(), {
            reviews: { controlsCorpus: "3".repeat(64) },
        });
        expect(report.controls.qualified).toBe(false);
    });

    test("malformed control, verdict, and dispute entries are review errors", async () => {
        const cases: Array<Partial<ReviewOptions>> = [
            { editControls: (c) => [...c, { id: "ctl-x", kind: "reversed_negation" }] },
            { editVerdicts: (v) => [...v, { control: "ctl-x", verdict: "violation" }] },
            {
                editVerdicts: (v) =>
                    v.map((x) =>
                        x.control === "ctl-wrong_identity" ? { ...x, reviewer: "  " } : x,
                    ),
            },
        ];
        for (const reviews of cases) {
            const { report } = await evaluate(scratch(), { reviews });
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
        const reviews = await loadReviews(dir, SHA);
        expect(reviews.errors.join("\n")).toContain("a dispute does not match its schema");
    });

    test("a review record with a blank or coerced batch, or a list field that is no list, is a review error", async () => {
        const cases: Array<
            [
                (controls: Record<string, unknown>, judgments: Record<string, unknown>) => void,
                string,
            ]
        > = [
            [
                (c, j) => {
                    c.batch = {};
                    j.batch = "[object Object]";
                },
                "controls.json names no batch",
            ],
            [
                (c, j) => {
                    c.batch = "";
                    j.batch = "";
                },
                "names no batch",
            ],
            [
                (_, j) => {
                    j.disputes = {};
                },
                "disputes is not a list",
            ],
            [
                (_, j) => {
                    j.judgments = "none";
                },
                "judgments is not a list",
            ],
            [
                (c) => {
                    c.controls = {};
                },
                "controls is not a list",
            ],
            [
                (_, j) => {
                    j.control_verdicts = null;
                },
                "control_verdicts is not a list",
            ],
            [
                (c) => {
                    c.approved_by = "reviewer-a, reviewer-b";
                },
                "approved_by is not a list",
            ],
            [
                (c) => {
                    c.approved_by = ["reviewer-a", "reviewer-b", 7];
                },
                "an approver does not match its schema",
            ],
        ];
        for (const [edit, error] of cases) {
            const root = scratch();
            const baseline = writeArm(root, { label: "baseline" });
            const candidate = writeArm(root, { label: "candidate" });
            const dir = writeReviews(root, {
                arms: [
                    { label: "baseline", files: baseline.files },
                    { label: "candidate", files: candidate.files },
                ],
            });
            const controls = JSON.parse(readFileSync(join(dir, "controls.json"), "utf8"));
            const judgments = JSON.parse(readFileSync(join(dir, "judgments.json"), "utf8"));
            edit(controls, judgments);
            writeFileSync(join(dir, "controls.json"), JSON.stringify(controls));
            writeFileSync(join(dir, "judgments.json"), JSON.stringify(judgments));
            const reviews = await loadReviews(dir, SHA);
            expect(reviews.errors.join("\n")).toContain(error);
        }
    });

    test("fewer than two approvers withhold acceptance", async () => {
        for (const approvers of [
            ["reviewer-a", "reviewer-a"],
            ["reviewer-a", ""],
            ["reviewer-a", "   "],
            ["reviewer-a", " reviewer-a "],
        ]) {
            const { report } = await evaluate(scratch(), { reviews: { approvers } });
            expect(report.arms[0]?.withheld).toContain("the batch has 1 of 2 approvers");
            expect(report.accepted).toBe(false);
        }
    });

    test("the comparison names regressions and resolution candidates, and a same-prompt pair is no treatment", async () => {
        const scenario = allScenarios.find(({ s }) => s.serving.tier === "p1")?.s.id ?? "";
        const wrong = (id: string) => (id === scenario ? "p3" : undefined);
        const comparisonOf = (report: Awaited<ReturnType<typeof evaluate>>["report"]) =>
            report.comparison.rows.find((r) => r.scenario === scenario)?.comparison;
        expect(
            comparisonOf((await evaluate(scratch(), { candidate: { tier: wrong } })).report),
        ).toBe("regression");
        expect(
            comparisonOf((await evaluate(scratch(), { baseline: { tier: wrong } })).report),
        ).toBe("resolution_candidate");
        const same = (
            await evaluate(scratch(), {
                candidate: { system: "summarizer system prompt" },
            })
        ).report;
        expect(same.comparison.treatment).toBe(false);
    });

    test("unreported generation usage, an unknown send cost, or an unmeasured recovery leaves cost incomplete", async () => {
        for (const usage of [
            null,
            {},
            "reported",
            0,
            { note: "reported" },
            { input_tokens: -1, output_tokens: 3 },
            { input_tokens: 900 },
        ]) {
            const { report } = await evaluate(scratch(), {
                tamper: (dir) => {
                    const source = corpus.cases[0]?.sources[0]?.id ?? "";
                    recapture(dir, source, (capture) => {
                        (capture.detail as Record<string, unknown>).usage = usage;
                    });
                },
            });
            const generation = report.arms[0]?.rows.find((r) => r.case === corpus.cases[0]?.id);
            expect(generation?.cost.missing.join("\n")).toContain("generation usage unreported");
        }

        const forwarded = (
            await evaluate(scratch(), {
                tamper: (dir) => {
                    write(dir, "forwarding-0.json", {
                        mode: "forward",
                        corpus_sha256: SHA,
                        model: "claude-live",
                        upstream_url: "https://api.example.test/v1/messages",
                        pricing: { inputPerMTok: 3, outputPerMTok: 15 },
                        context_limit: 200_000,
                        spent_usd: 0.25,
                        limits: {
                            maxCalls: 40,
                            maxOutputTokens: 1024,
                            timeoutMs: 1000,
                            spendCapUsd: 1,
                        },
                        stopped: null,
                        attempted_sends: 1,
                        acknowledged_responses: 1,
                        complete: false,
                        incomplete_reasons: ["cost unknown for a send"],
                        refusals: [],
                        exchanges: [
                            {
                                index: 0,
                                tool_results: [],
                                tool_uses: [],
                                request: { body_text: "{}", body_sha256: sha256("{}") },
                                response: {
                                    outcome: "acknowledged",
                                    model: "claude-live",
                                    stop_reason: "end_turn",
                                    truncated: false,
                                    body_text: "",
                                    body_sha256: null,
                                    usage: null,
                                    cost_usd: 0.25,
                                    cost_known: false,
                                },
                            },
                        ],
                    });
                },
            })
        ).report;
        expect(forwarded.arms[0]?.rows[0]?.cost.missing.join("\n")).toContain("unknown send cost");

        const recovery = (
            await evaluate(scratch(), {
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
            })
        ).report;
        const row = recovery.arms[0]?.rows.find((r) => r.scenario === allScenarios[1]?.s.id);
        expect(row?.cost.missing.join("\n")).toContain("recovery calls or output bytes");
    });

    test("a real capture with no attempt earns no generation cost", async () => {
        const source = corpus.cases[0]?.sources[0]?.id ?? "";
        const { report } = await evaluate(scratch(), {
            tamper: (dir) => {
                recapture(dir, source, (capture) => {
                    (capture.detail as Record<string, unknown>).attempts = [];
                });
            },
        });
        const row = report.arms[0]?.rows.find((r) => r.case === corpus.cases[0]?.id);
        expect(row?.cost.missing.join("\n")).toContain(
            `real.${source}.json: no generation attempt`,
        );
        expect(row?.cost.status).toBe("incomplete");
    });

    test("a real arm credits generation only from a published real capture", async () => {
        // The delivery witness labels a source only when it has no m1 scenario; C2.V2 is that
        // source, so its source-level observation is accepted and tests the credit rule.
        const c = corpus.cases.find((k) => k.sources.some((v) => v.id === "C2.V2"));
        const source = "C2.V2";
        const { report } = await evaluate(scratch(), {
            tamper: (dir) => {
                write(dir, "padding.json", {
                    schema_version: 1,
                    corpus_sha256: SHA,
                    owner: "opencode-delivery",
                    case: c?.id,
                    source,
                    scenario: source,
                    stage: "m1",
                    terminal: "served",
                    markers: [],
                    detail: { attempts: [{}] },
                });
            },
        });
        expect(report.arms[0]?.identity_errors).toEqual([]);
        const served = allScenarios.find(({ s }) => s.source === source)?.s.id;
        const row = report.arms[0]?.rows.find((r) => r.scenario === served);
        expect(row?.cost.generation.map((g) => g.file)).toEqual([`real.${source}.json`]);
    });

    test("a negative recovery measurement leaves cost incomplete", async () => {
        const entry = allScenarios[1];
        const { report } = await evaluate(scratch(), {
            tamper: (dir) => {
                write(dir, "recovery.json", {
                    schema_version: 1,
                    corpus_sha256: SHA,
                    owner: "opencode-delivery",
                    case: entry?.case,
                    source: entry?.s.source,
                    scenario: entry?.s.id,
                    stage: "recovery-eidnara-search",
                    terminal: "discoverable",
                    markers: [],
                    detail: { calls: -1, result_utf8_bytes: -400 },
                });
            },
        });
        const row = report.arms[0]?.rows.find((r) => r.scenario === entry?.s.id);
        expect(row?.cost.missing.join("\n")).toContain("recovery calls or output bytes");
        expect(row?.cost.status).toBe("incomplete");
    });

    test("arms whose serving observations name different estimators are refused", async () => {
        const { report } = await evaluate(scratch(), {
            candidate: { serving: { ...SERVING, estimator: "another-estimator v2" } },
        });
        expect(report.arms.every((a) => a.identity_errors.length === 0)).toBe(true);
        expect(report.comparison.refused).toContain(
            "the arms charged tokens under different estimators",
        );
        expect(report.accepted).toBe(false);
    });

    test("arms sharing a label are refused", async () => {
        const baseline = writeArm(scratch(), { label: "shared" });
        const candidate = writeArm(scratch(), {
            label: "shared",
            system: "candidate system prompt",
        });
        const reviews = writeReviews(scratch(), {
            arms: [{ label: "shared", files: baseline.files }],
        });
        const { report } = assemble({
            corpus,
            corpusPath: "corpus.json",
            corpusSha256: SHA,
            revision: "rev",
            mode: "offline",
            baseline: await loadArm(baseline.dir, corpus, SHA),
            candidate: await loadArm(candidate.dir, corpus, SHA),
            reviews: await loadReviews(reviews, SHA),
        });
        expect(report.comparison.refused).toContain("the arms share the label shared");
        expect(report.accepted).toBe(false);
    });

    test("a pass line that measured no admission or no elapsed time records those costs as unreported", () => {
        const delivery = (line: string) =>
            ({
                label: "cold-m0",
                pass: parseRustPassLine(line),
                capture: { request: { body: { messages: [] } } },
                verdict: { leaks: [] },
            }) as unknown as Delivery;
        const head =
            "[eidnara] rust pass: decision=DEFER reason=steady served_from=transform in=12 out=12 applied=true";
        const measured = servingCost(
            delivery(
                `${head} admission=fits invocation_bytes=9000 invocation_charged=3215 elapsed=41.7 ms module=23.4 ms`,
            ),
        );
        expect(measured.invocation_charged_tokens).toBe(3215);
        expect(measured.transform_elapsed_ms).toBe(41.7);
        const unadmitted = servingCost(
            delivery(
                `${head} admission=none invocation_bytes=0 invocation_charged=0 elapsed=3.0 ms module=1.0 ms`,
            ),
        );
        expect(unadmitted.invocation_charged_tokens).toBeNull();
        expect(unadmitted.invocation_bytes).toBeNull();
        const untimed = servingCost(
            delivery(`${head} admission=fits invocation_bytes=9000 invocation_charged=3215`),
        );
        expect(untimed.transform_elapsed_ms).toBeNull();
        const uncharged = servingCost(
            delivery(`${head} admission=fits elapsed=3.0 ms module=1.0 ms`),
        );
        expect(uncharged.invocation_charged_tokens).toBeNull();
        expect(uncharged.invocation_bytes).toBeNull();
    });

    test("a serving cost that is not a measurement leaves cost incomplete", async () => {
        const cases: Array<[Record<string, unknown>, string]> = [
            [{ ...SERVING, invocation_charged_tokens: "unreported" }, "invocation_charged_tokens"],
            [{ ...SERVING, invocation_bytes: null }, "invocation_bytes"],
            [{ ...SERVING, admission: "none" }, "admission"],
            [{ ...SERVING, admission: "maybe" }, "admission"],
            [{ ...SERVING, raw_source_leaks: 1 }, "raw_source_leaks"],
            [{ ...SERVING, serving_kind: "warm_repeat" }, "serving_kind"],
            [{ ...SERVING, transform_elapsed_ms: Number.NaN }, "transform_elapsed_ms"],
            [{ ...SERVING, raw_source_leaks: -1 }, "raw_source_leaks"],
            [{ ...SERVING, estimator: "" }, "estimator"],
            [{ ...SERVING, estimator: "   " }, "estimator"],
            [{ ...SERVING, serving_kind: "lukewarm" }, "serving_kind"],
        ];
        for (const [serving, field] of cases) {
            const { report } = await evaluate(scratch(), { baseline: { serving } });
            const row = report.arms[0]?.rows.find((r) => r.cost.status !== "complete");
            expect(row?.cost.missing.join("\n")).toContain(field);
            expect(report.arms[0]?.accepted).toBe(false);
        }
    });

    test("a recovery observation does not stand in for a missing serving cost", async () => {
        const entry = allScenarios.find(({ s }) => s.serving.path !== "exact_read");
        const scenario = entry?.s.id ?? "";
        const { report } = await evaluate(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${scenario}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                delete value.detail.serving;
                writeFileSync(path, JSON.stringify(value));
                write(dir, "recovery.json", {
                    schema_version: 1,
                    corpus_sha256: SHA,
                    owner: "opencode-delivery",
                    case: entry?.case,
                    source: entry?.s.source,
                    scenario,
                    stage: "recovery-eidnara-search",
                    terminal: "discoverable",
                    markers: [],
                    detail: { calls: 1, result_utf8_bytes: 400 },
                });
            },
        });
        const row = report.arms[0]?.rows.find((r) => r.scenario === scenario);
        expect(row?.cost.missing).toContain(`delivery.${scenario}.json: serving`);
        expect(row?.cost.status).toBe("incomplete");
    });

    test("the qualification and Pi witnesses serve tiers without an OpenCode cost record", async () => {
        const entry = allScenarios.find(({ s }) => s.serving.path !== "exact_read");
        const scenario = entry?.s.id ?? "";
        const { report } = await evaluate(scratch(), {
            tamper: (dir) => {
                const served = JSON.parse(
                    readFileSync(join(dir, `delivery.${scenario}.json`), "utf8"),
                );
                for (const stage of ["qualification", "pi-m1"]) {
                    write(dir, `${stage}.json`, {
                        schema_version: 1,
                        corpus_sha256: SHA,
                        owner: "opencode-delivery",
                        case: entry?.case,
                        source: entry?.s.source,
                        scenario,
                        stage,
                        terminal: "served",
                        markers: [],
                        detail: {
                            served_tier: served.detail.served_tier,
                            generation_capture_sha256: served.detail.generation_capture_sha256,
                        },
                    });
                }
            },
        });
        const row = report.arms[0]?.rows.find((r) => r.scenario === scenario);
        expect(report.arms[0]?.identity_errors).toEqual([]);
        expect(row?.cost.status).toBe("complete");
        expect(row?.cost.serving.length).toBe(1);
    });

    test("every serving observation needs its cost, whatever its siblings record", async () => {
        const entry = allScenarios.find(({ s }) => s.serving.path !== "exact_read");
        const scenario = entry?.s.id ?? "";
        const { report } = await evaluate(scratch(), {
            tamper: (dir) => {
                const served = JSON.parse(
                    readFileSync(join(dir, `delivery.${scenario}.json`), "utf8"),
                );
                write(dir, "served-again.json", {
                    schema_version: 1,
                    corpus_sha256: SHA,
                    owner: "opencode-delivery",
                    case: entry?.case,
                    source: entry?.s.source,
                    scenario,
                    stage: "served-again",
                    terminal: "served",
                    markers: [],
                    detail: {
                        served_tier: served.detail.served_tier,
                        generation_capture_sha256: served.detail.generation_capture_sha256,
                    },
                });
            },
        });
        const row = report.arms[0]?.rows.find((r) => r.scenario === scenario);
        expect(report.arms[0]?.identity_errors).toEqual([]);
        expect(row?.cost.missing).toContain("served-again.json: serving");
        expect(row?.cost.status).toBe("incomplete");
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

    async function rowOf(root: string, baselineDir: string, reviewsDir: string, scenario: string) {
        const { report } = assemble({
            corpus,
            corpusPath: "corpus.json",
            corpusSha256: SHA,
            revision: "rev",
            mode: "offline",
            baseline: await loadArm(baselineDir, corpus, SHA),
            candidate: await loadArm(join(root, "candidate"), corpus, SHA),
            reviews: await loadReviews(reviewsDir, SHA),
        });
        return report.arms[0]?.rows.find((r) => r.scenario === scenario);
    }

    const judgmentFor = (
        judgments: { judgments: Array<Record<string, unknown>> },
        scenario: string,
    ) => judgments.judgments.find((j) => j.arm === "baseline" && j.scenario === scenario) ?? {};

    test("a discoverable obligation needs a recovery observation", async () => {
        const scenario = discoverableFirst;
        expect(scenario).toBeDefined();
        const root = scratch();
        const { baseline, reviewsDir } = rewrite(root, (j) => {
            const judgment = judgmentFor(j, scenario?.id ?? "");
            judgment.obligations = (judgment.obligations as Array<Record<string, unknown>>).map(
                (o, i) => (i === 0 ? { ...o, disposition: "discoverable" } : o),
            );
        });
        expect((await rowOf(root, baseline.dir, reviewsDir, scenario?.id ?? ""))?.recovery).toBe(
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
            detail: { calls: 1, result_utf8_bytes: 400, result_carries_memory: true },
        });
        const row = await rowOf(root, baseline.dir, reviewsDir, scenario?.id ?? "");
        expect(row?.recovery).toBe("witnessed");
        expect(row?.cost.status).toBe("complete");
    });

    test("a recovery whose result carries no memory witnesses nothing", async () => {
        const scenario = discoverableFirst;
        const root = scratch();
        const { baseline, reviewsDir } = rewrite(root, (j) => {
            const judgment = judgmentFor(j, scenario?.id ?? "");
            judgment.obligations = (judgment.obligations as Array<Record<string, unknown>>).map(
                (o, i) => (i === 0 ? { ...o, disposition: "discoverable" } : o),
            );
        });
        for (const result of [false, undefined]) {
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
                detail: { calls: 1, result_utf8_bytes: 400, result_carries_memory: result },
            });
            const row = await rowOf(root, baseline.dir, reviewsDir, scenario?.id ?? "");
            expect(row?.recovery).toBe("unverified");
            expect(row?.withheld).toContain("recovery unverified");
        }
    });

    test("a stage that merely contains search or recover witnesses no recovery", async () => {
        const scenario = discoverableFirst;
        const root = scratch();
        const { baseline, reviewsDir } = rewrite(root, (j) => {
            const judgment = judgmentFor(j, scenario?.id ?? "");
            judgment.obligations = (judgment.obligations as Array<Record<string, unknown>>).map(
                (o, i) => (i === 0 ? { ...o, disposition: "discoverable" } : o),
            );
        });
        write(baseline.dir, "note.json", {
            schema_version: 1,
            corpus_sha256: SHA,
            owner: "opencode-delivery",
            case: allScenarios.find((e) => e.s.id === scenario?.id)?.case,
            source: scenario?.source,
            scenario: scenario?.id,
            stage: "research-note",
            terminal: "served",
            markers: ["unrecoverable"],
            detail: { calls: 1, result_utf8_bytes: 400 },
        });
        const row = await rowOf(root, baseline.dir, reviewsDir, scenario?.id ?? "");
        expect(row?.recovery).toBe("unverified");
        expect(row?.cost.recovery).toEqual([]);
    });

    test("a lost obligation or a disposition outside the accepted set is recall", async () => {
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
        const row = await rowOf(root, baseline.dir, reviewsDir, scenario?.id ?? "");
        expect(row?.preservation.map((p) => p.status)).toEqual([
            "recall",
            "recall",
            ...Array((scenario?.expectations.length ?? 2) - 2).fill("preserved"),
        ]);
        expect(row?.withheld).toContain("preservation");
    });

    test("a judgment that repeats an obligation or misstates its forbidden conclusions is a review error", async () => {
        const scenario = allScenarios.find(({ s }) => s.forbidden.length > 0)?.s;
        expect(scenario).toBeDefined();
        const edits: Array<(judgment: Record<string, unknown>) => void> = [
            (judgment) => {
                const [first] = judgment.obligations as Array<Record<string, unknown>>;
                judgment.obligations = [
                    ...(judgment.obligations as unknown[]),
                    { ...first, preserved: false },
                ];
            },
            (judgment) => {
                judgment.forbidden_violated = scenario?.forbidden[0];
            },
            (judgment) => {
                delete judgment.forbidden_violated;
            },
            (judgment) => {
                judgment.citations = "C1.V1#0";
            },
            (judgment) => {
                judgment.reviewer = "   ";
            },
            (judgment) => {
                Object.assign(judgment, { kind: "model", uncertainty: "low" });
            },
            (judgment) => {
                Object.assign(judgment, { kind: "model", citations: ["C1.V1#0"] });
            },
            (judgment) => {
                Object.assign(judgment, { kind: "model", citations: [], uncertainty: "low" });
            },
            (judgment) => {
                Object.assign(judgment, { kind: "model", citations: [" "], uncertainty: "low" });
            },
        ];
        for (const edit of edits) {
            const root = scratch();
            const { baseline, reviewsDir } = rewrite(root, (j) =>
                edit(judgmentFor(j, scenario?.id ?? "")),
            );
            const reviews = await loadReviews(reviewsDir, SHA);
            expect(reviews.errors.join("\n")).toContain("a judgment does not match its schema");
            const row = await rowOf(root, baseline.dir, reviewsDir, scenario?.id ?? "");
            expect(row?.semantic_review).toBe("unreviewed");
            expect(row?.consumer_safety).toBe("unreviewed");
        }
    });

    test("agreeing human judgments in any obligation order are reviewed", async () => {
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
        expect(
            (await rowOf(root, baseline.dir, reviewsDir, scenario?.id ?? ""))?.semantic_review,
        ).toBe("reviewed");
    });

    test("a judgment binds only to the arm it names, even over byte-identical artifacts", async () => {
        const root = scratch();
        const { baseline, reviewsDir } = rewrite(root, (j) => {
            j.judgments = j.judgments.filter((judgment) => judgment.arm === "baseline");
        });
        const baseArm = await loadArm(baseline.dir, corpus, SHA);
        const candArm = await loadArm(join(root, "candidate"), corpus, SHA);
        expect(candArm.evidence.map((e) => e.sha256)).toEqual(
            baseArm.evidence.map((e) => e.sha256),
        );
        const { report } = assemble({
            corpus,
            corpusPath: "corpus.json",
            corpusSha256: SHA,
            revision: "rev",
            mode: "offline",
            baseline: baseArm,
            candidate: candArm,
            reviews: await loadReviews(reviewsDir, SHA),
        });
        const [base, cand] = report.arms;
        expect(base?.rows.every((r) => r.semantic_review === "reviewed")).toBe(true);
        expect(cand?.rows.every((r) => r.semantic_review === "unreviewed")).toBe(true);
    });

    test("a judgment whose artifact hash matches no observation of its row withholds acceptance", async () => {
        const scenario = allScenarios[0]?.s.id ?? "";
        const root = scratch();
        const { baseline, reviewsDir } = rewrite(root, (j) => {
            j.judgments.push({
                ...judgmentFor(j, scenario),
                id: "stale-dissent",
                reviewer: "reviewer-d",
                artifact_sha256: "f".repeat(64),
                abstained: true,
            });
        });
        const { report } = assemble({
            corpus,
            corpusPath: "corpus.json",
            corpusSha256: SHA,
            revision: "rev",
            mode: "offline",
            baseline: await loadArm(baseline.dir, corpus, SHA),
            candidate: await loadArm(join(root, "candidate"), corpus, SHA),
            reviews: await loadReviews(reviewsDir, SHA),
        });
        expect(report.arms[0]?.withheld).toContain(
            `a judgment names unknown artifact ${"f".repeat(64)} for baseline/${scenario}`,
        );
        expect(report.accepted).toBe(false);
    });

    test("a judgment naming an undeclared obligation withholds acceptance", async () => {
        const scenario = allScenarios[0]?.s.id ?? "";
        const root = scratch();
        const { baseline, reviewsDir } = rewrite(root, (j) => {
            const judgment = judgmentFor(j, scenario);
            judgment.obligations = [
                ...(judgment.obligations as unknown[]),
                { obligation: "C9.O9", disposition: "visible", preserved: false },
            ];
        });
        const { report } = assemble({
            corpus,
            corpusPath: "corpus.json",
            corpusSha256: SHA,
            revision: "rev",
            mode: "offline",
            baseline: await loadArm(baseline.dir, corpus, SHA),
            candidate: await loadArm(join(root, "candidate"), corpus, SHA),
            reviews: await loadReviews(reviewsDir, SHA),
        });
        expect(report.arms[0]?.withheld).toContain(
            `a judgment names unknown obligation C9.O9 for ${scenario}`,
        );
        expect(report.accepted).toBe(false);
    });

    test("a judgment or dispute naming an unknown arm or scenario withholds acceptance", async () => {
        const scenario = allScenarios[0]?.s.id ?? "";
        for (const stray of [
            { kind: "dispute", arm: "basline", scenario },
            { kind: "dispute", arm: "baseline", scenario: "C9.S9" },
            { kind: "judgment", arm: "basline", scenario },
            { kind: "judgment", arm: "baseline", scenario: "C9.S9" },
        ]) {
            const root = scratch();
            const { baseline, reviewsDir } = rewrite(root, (j) => {
                if (stray.kind === "dispute") {
                    j.disputes.push({ arm: stray.arm, scenario: stray.scenario, resolved: false });
                } else {
                    j.judgments.push({
                        ...judgmentFor(j, scenario),
                        id: "dissent",
                        reviewer: "reviewer-d",
                        arm: stray.arm,
                        scenario: stray.scenario,
                        abstained: true,
                    });
                }
            });
            const { report } = assemble({
                corpus,
                corpusPath: "corpus.json",
                corpusSha256: SHA,
                revision: "rev",
                mode: "offline",
                baseline: await loadArm(baseline.dir, corpus, SHA),
                candidate: await loadArm(join(root, "candidate"), corpus, SHA),
                reviews: await loadReviews(reviewsDir, SHA),
            });
            expect(report.arms[0]?.withheld).toContain(
                `a ${stray.kind} names unknown arm or scenario ${stray.arm}/${stray.scenario}`,
            );
            expect(report.accepted).toBe(false);
        }
    });

    test("disagreeing human judgments and an open dispute are disputed; a resolved dispute is not", async () => {
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
        expect((await row(first))?.semantic_review).toBe("disputed");
        expect((await row(first))?.consumer_safety).toBe("unreviewed");
        expect((await row(second))?.semantic_review).toBe("disputed");
        expect((await row(second))?.withheld).toContain("semantic review disputed");
        expect((await row(third))?.semantic_review).toBe("reviewed");
    });

    test("abstention, forbidden conclusions, model-only judgments, and disputes withhold credit", async () => {
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
        expect((await row(permitted))?.consumer_safety).toBe("abstained");
        expect((await row(forbidden))?.consumer_safety).toBe("false-authoritative");
        expect((await row(withForbidden))?.consumer_safety).toBe("false-authoritative");
        expect((await row(withForbidden))?.withheld).toContain(
            "consumer safety false-authoritative",
        );
        expect((await row(modelOnly))?.semantic_review).toBe("model_only");
        expect((await row(disputed))?.semantic_review).toBe("disputed");
        const abstainedUnavailable = (await row(permitted))?.preservation.filter(
            (p) => p.disposition === "unavailable",
        );
        expect(abstainedUnavailable?.length).toBeGreaterThan(0);
        expect(abstainedUnavailable?.every((p) => p.status === "recall")).toBe(true);
    });

    test("a forbidden conclusion the scenario does not declare still withholds safety", async () => {
        const scenario = allScenarios[0]?.s;
        const root = scratch();
        const { baseline, reviewsDir } = rewrite(root, (j) => {
            judgmentFor(j, scenario?.id ?? "").forbidden_violated = ["C9.X9"];
        });
        const row = await rowOf(root, baseline.dir, reviewsDir, scenario?.id ?? "");
        expect(row?.consumer_safety).toBe("false-authoritative");
        expect(row?.withheld).toContain("consumer safety false-authoritative");
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

    test("the default run sends nothing and writes only owner-only files outside the repository", async () => {
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
        let written: Awaited<ReturnType<typeof run>> | undefined;
        try {
            written = await run({
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
        const published = JSON.parse(readFileSync(written?.report ?? "", "utf8"));
        expect(published.manifest_sha256).toBe(sha256(readFileSync(written?.manifest ?? "")));
        const inside = resolve(import.meta.dir, "eval-out");
        await expect(
            run({
                baseline: baseline.dir,
                candidate: candidate.dir,
                reviews,
                out: inside,
                corpus: CORPUS_PATH,
                mode: "offline",
            }),
        ).rejects.toThrow("inside the repository");
    });

    test("evidence arms may be named through parent-relative paths", async () => {
        const root = scratch();
        const baseline = writeArm(root, { label: "baseline" });
        const candidate = writeArm(root, { label: "candidate" });
        const reviews = writeReviews(root, {
            arms: [
                { label: "baseline", files: baseline.files },
                { label: "candidate", files: candidate.files },
            ],
        });
        const written = await run({
            baseline: `${root}/candidate/../baseline`,
            candidate: `${candidate.dir}/./`,
            reviews,
            out: join(root, "out"),
            corpus: CORPUS_PATH,
            mode: "offline",
        });
        const manifest = JSON.parse(readFileSync(written.manifest, "utf8"));
        expect(manifest.arms.map((a: { label: string }) => a.label)).toEqual([
            "baseline",
            "candidate",
        ]);
        expect(statSync(baseline.dir).isDirectory()).toBe(true);
    });

    test("an output directory that is an evidence arm is refused before any write", async () => {
        const root = scratch();
        const baseline = writeArm(root, { label: "baseline" });
        const candidate = writeArm(root, { label: "candidate" });
        const reviews = writeReviews(root, {
            arms: [
                { label: "baseline", files: baseline.files },
                { label: "candidate", files: candidate.files },
            ],
        });
        chmodSync(baseline.dir, 0o700);
        chmodSync(candidate.dir, 0o700);
        for (const out of [baseline.dir, join(candidate.dir, ".", "")]) {
            await expect(
                run({
                    baseline: baseline.dir,
                    candidate: candidate.dir,
                    reviews,
                    out,
                    corpus: CORPUS_PATH,
                    mode: "offline",
                }),
            ).rejects.toThrow("--out is an evidence arm");
        }
        const link = join(root, "link");
        symlinkSync(baseline.dir, link);
        await expect(
            run({
                baseline: baseline.dir,
                candidate: candidate.dir,
                reviews,
                out: join(link, "nested", ".."),
                corpus: CORPUS_PATH,
                mode: "offline",
            }),
        ).rejects.toThrow("--out is an evidence arm");
        expect(readdirSync(baseline.dir)).not.toContain("manifest.json");
        expect(readdirSync(candidate.dir)).not.toContain("report.json");
    });

    // With a discovery variable set, git and the reader both defer to it, so the fixture
    // repositories below would not be the ones resolved.
    const discovery = ["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_CEILING_DIRECTORIES"];
    test.skipIf(discovery.some((name) => process.env[name] !== undefined))(
        "the repository revision is the commit git rev-parse HEAD names in every layout",
        async () => {
            const root = scratch();
            // A git hook exports variables such as GIT_INDEX_FILE that would redirect the
            // fixture commands into the enclosing repository.
            const env = Object.fromEntries(
                Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_")),
            );
            const git = (cwd: string, ...args: string[]) => {
                const result = Bun.spawnSync(["git", ...args], { cwd, env, stderr: "pipe" });
                if (!result.success) throw new Error(result.stderr.toString());
                return result.stdout.toString().trim();
            };
            const commit = (cwd: string, message: string) =>
                git(
                    cwd,
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@example.com",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    message,
                );
            const matches = async (cwd: string) =>
                expect(await repositoryRevision(cwd)).toBe(git(cwd, "rev-parse", "HEAD"));
            const repo = join(root, "repo");
            const nested = join(repo, "a", "b");
            const worktree = join(root, "worktree");
            mkdirSync(nested, { recursive: true });
            git(repo, "init", "-q", "-b", "main");
            expect(await repositoryRevision(repo)).toBe("unknown");
            commit(repo, "one");
            await matches(repo);
            await matches(nested);
            git(repo, "worktree", "add", "-q", "-b", "side", worktree);
            commit(worktree, "two");
            await matches(worktree);
            git(repo, "pack-refs", "--all");
            expect(readdirSync(join(repo, ".git", "refs", "heads"))).toEqual([]);
            await matches(repo);
            await matches(worktree);
            git(repo, "checkout", "-q", "--detach");
            await matches(repo);
            // HEAD naming a ref outside refs/heads is resolved by git itself.
            git(repo, "update-ref", "refs/remotes/origin/main", "HEAD");
            git(repo, "symbolic-ref", "HEAD", "refs/remotes/origin/main");
            await matches(repo);
            expect(await repositoryRevision(root)).toBe("unknown");
            const head = git(repo, "rev-parse", "HEAD");
            writeFileSync(join(repo, "untracked.txt"), "u\n");
            expect(await repositoryRevision(repo)).toBe(`${head}-dirty`);
            git(repo, "add", "untracked.txt");
            expect(await repositoryRevision(repo)).toBe(`${head}-dirty`);
            commit(repo, "three");
            expect(await repositoryRevision(repo)).toBe(git(repo, "rev-parse", "HEAD"));
        },
    );
});

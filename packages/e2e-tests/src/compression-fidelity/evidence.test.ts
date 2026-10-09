import { describe, expect, test } from "bun:test";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
    type ArmOptions,
    allScenarios,
    corpus,
    SETTINGS,
    SHA,
    scratch,
    sha256,
    write,
    writeArm,
} from "./evaluation-fixtures";
import { assembleEvidence, loadArm } from "./evidence";

async function assemble(
    root: string,
    options: {
        baseline?: Partial<ArmOptions>;
        candidate?: Partial<ArmOptions>;
        tamper?: (baselineDir: string) => void;
        mode?: "offline" | "live";
    } = {},
) {
    const baseline = writeArm(root, { label: "baseline", ...options.baseline });
    const candidate = writeArm(root, {
        label: "candidate",
        system: "candidate system prompt",
        ...options.candidate,
    });
    options.tamper?.(baseline.dir);
    return assembleEvidence({
        corpus,
        corpusPath: "corpus.json",
        corpusSha256: SHA,
        revision: "rev",
        mode: options.mode ?? "offline",
        baseline: await loadArm(baseline.dir, corpus, SHA),
        candidate: await loadArm(candidate.dir, corpus, SHA),
    });
}

const captureOf = (dir: string, source: string) =>
    sha256(readFileSync(join(dir, `real.${source}.json`)));

const errorsOf = (assembled: Awaited<ReturnType<typeof assemble>>) =>
    assembled.arms[0]?.identity_errors.join("\n") ?? "";
const rowOf = (assembled: Awaited<ReturnType<typeof assemble>>, scenario: string | undefined) =>
    assembled.arms[0]?.rows.find((r) => r.scenario.id === scenario);

describe("evidence identity and completeness", () => {
    test("a complete evidence set executes every scenario and lists every observation in the manifest", async () => {
        const assembled = await assemble(scratch());
        expect(assembled.arms.map((a) => a.identity_errors)).toEqual([[], []]);
        for (const arm of assembled.arms) {
            expect(arm.rows.length).toBe(allScenarios.length);
            expect(arm.rows.every((r) => r.execution.status === "executed")).toBe(true);
            expect(arm.rows.every((r) => r.deterministic === "pass")).toBe(true);
        }
        expect(assembled.refused).toEqual([]);
        expect(assembled.treatment).toBe(true);
        expect(assembled.manifest.arms[0]?.observations.length).toBe(
            allScenarios.length + corpus.cases.flatMap((c) => c.sources).length,
        );
        expect(assembled.manifest.corpus.sha256).toBe(SHA);
    });

    test("the manifest lists every input file read, with its hash, including refused ones", async () => {
        const first = allScenarios[0]?.s.id ?? "";
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                writeFileSync(
                    join(dir, "foreign.json"),
                    '{"schema_version":1,"corpus_sha256":"0"}',
                );
                writeFileSync(join(dir, "broken.json"), "{");
            },
        });
        const files = assembled.manifest.arms[0]?.files ?? [];
        expect(files.find((f) => f.file === "foreign.json")?.sha256).toBe(
            sha256('{"schema_version":1,"corpus_sha256":"0"}'),
        );
        expect(files.find((f) => f.file === "broken.json")?.sha256).toBe(sha256("{"));
        expect(files.some((f) => f.file === `delivery.${first}.json`)).toBe(true);
        expect(files.some((f) => f.file === "arm.json")).toBe(true);
        expect(errorsOf(assembled)).toContain("foreign.json has unknown owner");
        expect(errorsOf(assembled)).toContain("broken.json is not JSON");
    });

    test("a missing scenario is listed and refuses the comparison", async () => {
        const missing = allScenarios[3]?.s.id ?? "";
        const assembled = await assemble(scratch(), { baseline: { skipScenario: missing } });
        expect(assembled.arms[0]?.missing_scenarios).toEqual([missing]);
        expect(rowOf(assembled, missing)?.execution.status).toBe("missing");
        expect(assembled.refused).toContain("the arms reached different scenario sets");
    });

    test("identity errors on the candidate side alone refuse the comparison", async () => {
        const assembled = await assemble(scratch(), {
            candidate: { origin: "scripted approved example" },
        });
        expect(assembled.arms[0]?.identity_errors).toEqual([]);
        expect(assembled.arms[1]?.identity_errors.join("\n")).toContain("scripted output");
        expect(assembled.refused).toContain("an arm has identity errors");
    });

    test("scripted output in an arm labeled real is an identity error", async () => {
        const assembled = await assemble(scratch(), {
            baseline: { origin: "scripted approved example" },
        });
        expect(errorsOf(assembled)).toContain("scripted output");
        expect(errorsOf(assembled)).toContain("no published real-model capture");
        expect(assembled.refused).toContain("an arm has identity errors");
    });

    test("a real capture recorded under another model is an identity error", async () => {
        const assembled = await assemble(scratch(), {
            baseline: { captureModel: "anthropic/other" },
        });
        expect(errorsOf(assembled)).toContain("captured with model anthropic/other");
        expect(errorsOf(assembled)).toContain("no published real-model capture");
        expect(assembled.refused).toContain("an arm has identity errors");
    });

    test("every capture attempt names the arm's model", async () => {
        const assembled = await assemble(scratch(), {
            baseline: { attempt: { model: "anthropic/other" } },
        });
        expect(errorsOf(assembled)).toContain(
            "attempt ran model anthropic/other, not the arm's model",
        );
        const unnamed = await assemble(scratch(), { baseline: { attempt: { model: undefined } } });
        expect(errorsOf(unnamed)).toContain("attempt ran model undefined, not the arm's model");
    });

    test("a system prompt recorded as text and as a digest must agree", async () => {
        const assembled = await assemble(scratch(), {
            baseline: { attempt: { system_sha256: sha256("another prompt") } },
        });
        expect(errorsOf(assembled)).toContain("records a system prompt whose digest differs");
        expect(assembled.refused).toContain("an arm has identity errors");
    });

    test("the user prompt each source was generated from is held equal across the arms", async () => {
        const first = corpus.cases[0]?.sources[0]?.id ?? "";
        const assembled = await assemble(scratch(), {
            candidate: { attempt: { prompt: "another user prompt" } },
        });
        expect(assembled.arms.map((a) => a.identity_errors)).toEqual([[], []]);
        expect(assembled.refused).toContain(`the arms generated ${first} from different prompts`);
        const unrecorded = await assemble(scratch(), {
            baseline: { attempt: { prompt: undefined } },
        });
        expect(errorsOf(unrecorded)).toContain("records no user prompt");
    });

    test("a published real capture must record the system prompt it ran", async () => {
        const assembled = await assemble(scratch(), { baseline: { attempt: null } });
        expect(errorsOf(assembled)).toContain("records no system prompt");
        expect(assembled.refused).toContain("an arm has identity errors");
    });

    test("a capture attempt that ran other generation settings than the arm declares is an identity error", async () => {
        const assembled = await assemble(scratch(), {
            baseline: { attempt: { temperature: 0.7 } },
        });
        expect(errorsOf(assembled)).toContain("ran temperature 0.7, not the arm's 0.1");
        const unrecorded = await assemble(scratch(), {
            baseline: { attempt: { max_output_tokens: undefined } },
        });
        expect(errorsOf(unrecorded)).toContain(
            "ran max_output_tokens undefined, not the arm's 1024",
        );
    });

    test("an observation without a stage or terminal is an identity error", async () => {
        const first = allScenarios[0]?.s.id ?? "";
        const strip = (field: string) =>
            assemble(scratch(), {
                tamper: (dir) => {
                    const path = join(dir, `delivery.${first}.json`);
                    const value = JSON.parse(readFileSync(path, "utf8"));
                    delete value[field];
                    writeFileSync(path, JSON.stringify(value));
                },
            });
        const noStage = await strip("stage");
        expect(errorsOf(noStage)).toContain(`delivery.${first}.json has no stage`);
        expect(rowOf(noStage, first)?.execution.status).toBe("missing");
        expect(errorsOf(await strip("terminal"))).toContain(
            `delivery.${first}.json has no terminal`,
        );
    });

    test("a judge control outside the witness's control stage is an identity error", async () => {
        const first = allScenarios.find(({ s }) => s.serving.path !== "exact_read")?.s.id ?? "";
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${first}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.terminal = "unqualified";
                value.detail.judge_control = true;
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(errorsOf(assembled)).toContain(
            `delivery.${first}.json marks stage served as a judge control`,
        );
    });

    test("a scripted arm needs a published replay generation that records its prompt for every source", async () => {
        const scripted = { generationOrigin: "scripted" as const };
        const complete = await assemble(scratch(), { baseline: scripted, candidate: scripted });
        expect(complete.arms.map((a) => a.identity_errors)).toEqual([[], []]);
        expect(complete.treatment).toBe(true);
        const unrecorded = await assemble(scratch(), {
            baseline: { ...scripted, attempt: null },
            candidate: scripted,
        });
        expect(errorsOf(unrecorded)).toContain("records no system prompt");
        const first = corpus.cases[0]?.sources[0]?.id ?? "";
        const absent = await assemble(scratch(), {
            baseline: scripted,
            candidate: scripted,
            tamper: (dir) => rmSync(join(dir, `generation.${first}.json`)),
        });
        expect(errorsOf(absent)).toContain(`no published scripted generation for ${first}`);
        expect(absent.refused).toContain("an arm has identity errors");
    });

    test("real captures cannot stand in for a scripted arm's generation", async () => {
        const scripted = { generationOrigin: "scripted" as const };
        const first = corpus.cases[0]?.sources[0]?.id ?? "";
        const assembled = await assemble(scratch(), {
            baseline: scripted,
            candidate: scripted,
            tamper: (dir) => {
                const generation = JSON.parse(
                    readFileSync(join(dir, `generation.${first}.json`), "utf8"),
                );
                rmSync(join(dir, `generation.${first}.json`));
                write(dir, `real.${first}.json`, {
                    ...generation,
                    owner: "daemon.compression_fidelity.real_capture",
                    stage: "capture",
                    detail: {
                        model: "anthropic/claude-test",
                        output_origin: "real producer through the host",
                        attempts: generation.detail.attempts,
                    },
                });
            },
        });
        expect(errorsOf(assembled)).toContain(
            `real.${first}.json carries real output in an arm labeled scripted`,
        );
        expect(errorsOf(assembled)).toContain(`no published scripted generation for ${first}`);
    });

    test("a generation record that names a scenario is an identity error", async () => {
        const scenario = allScenarios[0];
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `real.${scenario?.s.source}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.scenario = scenario?.s.id;
                writeFileSync(join(dir, "real.scenario.json"), JSON.stringify(value));
                rmSync(join(dir, `delivery.${scenario?.s.id}.json`));
            },
        });
        expect(errorsOf(assembled)).toContain(
            `real.scenario.json names scenario ${scenario?.s.id}; a capture stage is source-level`,
        );
        expect(rowOf(assembled, scenario?.s.id)?.execution.status).toBe("missing");
    });

    test("arms with different generation origins refuse the comparison", async () => {
        const assembled = await assemble(scratch(), {
            candidate: { generationOrigin: "scripted" },
        });
        expect(assembled.arms.map((a) => a.identity_errors)).toEqual([[], []]);
        expect(assembled.refused).toContain("the arms differ in generation_origin");
    });

    test("evidence bound to another corpus refuses the comparison", async () => {
        const assembled = await assemble(scratch(), {
            candidate: { corpusSha256: "1".repeat(64) },
        });
        expect(assembled.refused).toContain("an arm is bound to another corpus");
    });

    test("a case mismatch is an identity error, not a corpus refusal", async () => {
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const first = allScenarios[0]?.s.id ?? "";
                const path = join(dir, `delivery.${first}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                writeFileSync(path, JSON.stringify({ ...value, case: "C6" }));
            },
        });
        expect(errorsOf(assembled)).toContain("names case C6");
        expect(assembled.refused).not.toContain("an arm is bound to another corpus");
    });

    test("a prompt the arm did not declare is an identity error", async () => {
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, "arm.json");
                const arm = JSON.parse(readFileSync(path, "utf8"));
                writeFileSync(path, JSON.stringify({ ...arm, prompt_sha256: "2".repeat(64) }));
            },
        });
        expect(errorsOf(assembled)).toContain("not the arm's prompt");
    });

    test("a prompt digest that is not a SHA-256 hex string is refused", async () => {
        const declared = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, "arm.json");
                const arm = JSON.parse(readFileSync(path, "utf8"));
                writeFileSync(path, JSON.stringify({ ...arm, prompt_sha256: "not-a-hash" }));
            },
        });
        expect(errorsOf(declared)).toContain("arm.json does not match its schema");
        const attempt = await assemble(scratch(), {
            baseline: { attempt: { system: undefined, system_sha256: "not-a-hash" } },
        });
        expect(errorsOf(attempt)).toContain("records no complete real attempt");
    });

    test("a blank arm.json identifier does not match the arm schema", async () => {
        for (const field of ["label", "model", "provider", "version", "prompt_sha256"]) {
            const assembled = await assemble(scratch(), {
                tamper: (dir) => {
                    const path = join(dir, "arm.json");
                    const arm = JSON.parse(readFileSync(path, "utf8"));
                    writeFileSync(path, JSON.stringify({ ...arm, [field]: "   " }));
                },
            });
            expect(errorsOf(assembled)).toContain("arm.json does not match its schema");
        }
    });

    test("an unpublished temporary file and a duplicate observation are refused", async () => {
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                writeFileSync(join(dir, ".delivery.C1.S1.json.tmp"), "{}");
                writeFileSync(join(dir, "delivery.C1.S1.json.tmp-abc123"), "{}");
                const first = allScenarios[0]?.s.id ?? "";
                writeFileSync(
                    join(dir, "delivery.copy.json"),
                    readFileSync(join(dir, `delivery.${first}.json`)),
                );
            },
        });
        expect(errorsOf(assembled)).toContain(
            ".delivery.C1.S1.json.tmp is an unpublished temporary file",
        );
        expect(errorsOf(assembled)).toContain(
            "delivery.C1.S1.json.tmp-abc123 is an unpublished temporary file",
        );
        expect(errorsOf(assembled)).toContain("duplicates another observation");
    });

    test("a real arm's serving observation must link a published capture of its own source", async () => {
        const scenario = allScenarios[0]?.s;
        const unlinked = await assemble(scratch(), { baseline: { unlinked: true } });
        expect(errorsOf(unlinked)).toContain("names no published real capture");
        const crossed = await assemble(scratch(), {
            tamper: (dir) => {
                const other = corpus.cases
                    .flatMap((c) => c.sources)
                    .find((x) => x.id !== scenario?.source);
                const capture = sha256(readFileSync(join(dir, `real.${other?.id}.json`)));
                const path = join(dir, `delivery.${scenario?.id}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.detail.generation_capture_sha256 = capture;
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(errorsOf(crossed)).toContain(
            `delivery.${scenario?.id}.json names no published real capture`,
        );
        expect(crossed.refused).toContain("an arm has identity errors");
    });

    test("an empty or nested variant label is an identity error", async () => {
        const entry = allScenarios[0];
        for (const label of [`${entry?.s.id}@`, `${entry?.s.id}@a@b`, "@p1-only"]) {
            const assembled = await assemble(scratch(), {
                tamper: (dir) => {
                    write(dir, "variant.json", {
                        schema_version: 1,
                        corpus_sha256: SHA,
                        owner: "opencode-delivery",
                        case: entry?.case,
                        source: entry?.s.source,
                        scenario: label,
                        stage: "x",
                        terminal: "served",
                        markers: [],
                        detail: {},
                    });
                },
            });
            expect(errorsOf(assembled)).toContain("malformed scenario label");
        }
    });

    test("variants and judge controls are kept as outcomes and judge no scenario", async () => {
        const entry = allScenarios[0];
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const base = {
                    schema_version: 1,
                    corpus_sha256: SHA,
                    owner: "opencode-delivery",
                    case: entry?.case,
                    source: entry?.s.source,
                    markers: [],
                };
                write(dir, "variant.json", {
                    ...base,
                    scenario: `${entry?.s.id}@p1-only`,
                    stage: "aging",
                    terminal: "served",
                    detail: {
                        served_tier: "p5",
                        generation_capture_sha256: captureOf(dir, entry?.s.source ?? ""),
                    },
                });
                write(dir, "judge.json", {
                    ...base,
                    scenario: entry?.s.id,
                    stage: "missing-capture",
                    terminal: "unqualified",
                    detail: { judge_control: true },
                });
            },
        });
        const row = rowOf(assembled, entry?.s.id);
        expect(assembled.arms[0]?.identity_errors).toEqual([]);
        expect(row?.execution.status).toBe("executed");
        expect(row?.deterministic).toBe("pass");
        expect(row?.execution.outcomes).toContain("opencode-delivery:p1-only:aging:served");
        expect(row?.execution.outcomes).toContain("opencode-delivery:missing-capture:unqualified");
        expect(assembled.manifest.arms[0]?.observations.some((o) => o.variant === "p1-only")).toBe(
            true,
        );
    });

    test("a malformed forwarding file is an identity error and assembly still completes", async () => {
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                writeFileSync(join(dir, "forwarding-null.json"), "null");
                writeFileSync(
                    join(dir, "forwarding-partial.json"),
                    JSON.stringify({ mode: "forward", corpus_sha256: SHA, exchanges: [{}] }),
                );
            },
        });
        expect(errorsOf(assembled)).toContain(
            "forwarding-null.json does not match the forwarding report schema",
        );
        expect(errorsOf(assembled)).toContain(
            "forwarding-partial.json does not match the forwarding report schema",
        );
    });

    test("an unreadable or non-JSON observation file is an identity error", async () => {
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                mkdirSync(join(dir, "delivery.dir.json"));
                writeFileSync(join(dir, "delivery.broken.json"), "{");
            },
        });
        expect(errorsOf(assembled)).toContain("delivery.dir.json is unreadable: EISDIR");
        expect(errorsOf(assembled)).toContain("delivery.broken.json is not JSON");
    });

    test("a stage labeled with its own source ID is source-level evidence", async () => {
        const stage = (scenario: string, source: string) => (dir: string) =>
            write(dir, "opencode-delivery.C2.C2.V2.m1.json", {
                schema_version: 1,
                corpus_sha256: SHA,
                owner: "opencode-delivery",
                case: "C2",
                source,
                scenario,
                stage: "m1",
                terminal: "served",
                markers: ["cf-delivery-m1-published-input"],
                detail: {
                    served_tier: "p1",
                    curve_tier: "p1",
                    generation_capture_sha256: captureOf(dir, source),
                },
            });
        const labeled = await assemble(scratch(), { tamper: stage("C2.V2", "C2.V2") });
        expect(labeled.arms[0]?.identity_errors).toEqual([]);
        expect(
            labeled.manifest.arms[0]?.observations.find(
                (o) => o.file === "opencode-delivery.C2.C2.V2.m1.json",
            ),
        ).toMatchObject({ case: "C2", source: "C2.V2", scenario: null, stage: "m1" });
        const crossed = await assemble(scratch(), { tamper: stage("C2.V2", "C2.V1") });
        expect(errorsOf(crossed)).toContain(
            "opencode-delivery.C2.C2.V2.m1.json labels source C2.V2 but names source C2.V1",
        );
    });

    test("a scenario observation must name the source its corpus scenario is on", async () => {
        const scenario = allScenarios.find(({ s }) => s.id === "C2.S6");
        const path = (dir: string) => join(dir, `delivery.${scenario?.s.id}.json`);
        const crossed = await assemble(scratch(), {
            tamper: (dir) => {
                const value = JSON.parse(readFileSync(path(dir), "utf8"));
                value.source = "C2.V1";
                value.detail.generation_capture_sha256 = captureOf(dir, "C2.V1");
                writeFileSync(path(dir), JSON.stringify(value));
            },
        });
        expect(errorsOf(crossed)).toContain(
            `delivery.${scenario?.s.id}.json names source C2.V1, where ${scenario?.s.id} is on C2.V2`,
        );
        const copied = await assemble(scratch(), {
            tamper: (dir) => {
                const value = JSON.parse(readFileSync(path(dir), "utf8"));
                writeFileSync(
                    join(dir, "delivery.copy.json"),
                    JSON.stringify({ ...value, source: "bogus" }),
                );
            },
        });
        expect(errorsOf(copied)).toContain(
            `delivery.copy.json names source bogus, where ${scenario?.s.id} is on C2.V2`,
        );
        expect(rowOf(copied, scenario?.s.id)?.execution.outcomes).toHaveLength(1);
    });
});

describe("evidence generation origin", () => {
    test("a real arm's delivery in the witness record shape must name its real capture", async () => {
        const scenario = allScenarios.find(({ s }) => s.serving.path === "natural")?.s;
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${scenario?.id}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.detail = {
                    served_tier: scenario?.serving.tier,
                    curve_tier: scenario?.serving.tier,
                    pass: { decision: "HARD", applied: true },
                };
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(errorsOf(assembled)).toContain(
            `delivery.${scenario?.id}.json names no published real capture of ${scenario?.source} it served`,
        );
    });

    test("a scripted attempt recorded per attempt is scripted output in a real arm", async () => {
        const source = corpus.cases[0]?.sources[0]?.id;
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                write(dir, "replay.generation.json", {
                    schema_version: 1,
                    corpus_sha256: SHA,
                    owner: "daemon.compression_fidelity.replay",
                    case: corpus.cases[0]?.id,
                    source,
                    scenario: null,
                    stage: "generation",
                    terminal: "published",
                    markers: [],
                    detail: {
                        attempts: [
                            {
                                attempt: 1,
                                system_sha256: sha256("summarizer system prompt"),
                                output_origin: "scripted approved example",
                            },
                        ],
                    },
                });
            },
        });
        expect(errorsOf(assembled)).toContain(
            "replay.generation.json carries scripted output in an arm labeled real",
        );
    });

    test("a real capture must run the arm's model and record a complete attempt with text output", async () => {
        const source = corpus.cases[0]?.sources[0]?.id ?? "";
        const relink = (dir: string, edit: (detail: Record<string, unknown>) => void) => {
            const path = join(dir, `real.${source}.json`);
            const value = JSON.parse(readFileSync(path, "utf8"));
            edit(value.detail);
            const capture = write(dir, `real.${source}.json`, value);
            for (const { s } of allScenarios.filter((e) => e.s.source === source)) {
                const served = join(dir, `delivery.${s.id}.json`);
                const observation = JSON.parse(readFileSync(served, "utf8"));
                if (observation.detail.generation_capture_sha256 === undefined) continue;
                observation.detail.generation_capture_sha256 = capture;
                writeFileSync(served, JSON.stringify(observation));
            }
        };
        const cases: Array<[(detail: Record<string, unknown>) => void, string]> = [
            [
                (detail) => {
                    detail.model = "other/model";
                },
                `real.${source}.json captured with model other/model, not the arm's model`,
            ],
            [
                (detail) => {
                    (detail.attempts as Array<Record<string, unknown>>)[0]!.model = "other/model";
                },
                `real.${source}.json attempt ran model other/model, not the arm's model`,
            ],
            [
                (detail) => {
                    detail.attempts = [{}];
                },
                `real.${source}.json records no complete real attempt`,
            ],
            [
                (detail) => {
                    (detail.attempts as Array<Record<string, unknown>>)[0]!.outputs = [
                        { error: "x" },
                    ];
                },
                `real.${source}.json records no complete real attempt`,
            ],
        ];
        for (const [edit, error] of cases) {
            const assembled = await assemble(scratch(), { tamper: (dir) => relink(dir, edit) });
            expect(errorsOf(assembled)).toContain(error);
        }
        const intact = await assemble(scratch());
        expect(errorsOf(intact)).toBe("");
    });
});

describe("evidence deterministic column", () => {
    test("a wrong served tier fails the deterministic column", async () => {
        const scenario = allScenarios.find(({ s }) => s.serving.tier === "p1")?.s.id ?? "";
        const assembled = await assemble(scratch(), {
            baseline: { tier: (id) => (id === scenario ? "p3" : undefined) },
        });
        expect(rowOf(assembled, scenario)?.deterministic).toBe("assertion_fail");
    });

    test("a pressure delivery passes when it serves sparser than its curve tier", async () => {
        const scenario = allScenarios.find(({ s }) => s.serving.path === "pressure")?.s;
        const pressed = (served: string) =>
            assemble(scratch(), {
                tamper: (dir) => {
                    const path = join(dir, `delivery.${scenario?.id}.json`);
                    const value = JSON.parse(readFileSync(path, "utf8"));
                    value.detail.served_tier = served;
                    value.detail.curve_tier = "p2";
                    writeFileSync(path, JSON.stringify(value));
                },
            });
        expect(rowOf(await pressed("p5"), scenario?.id)?.deterministic).toBe("pass");
        expect(rowOf(await pressed("p2"), scenario?.id)?.deterministic).toBe("assertion_fail");
    });

    test("the pressure oracle and judge controls apply to delivery observations only", async () => {
        const scenario = allScenarios.find(({ s }) => s.serving.path === "pressure");
        const otherOwner = (detail: Record<string, unknown>) =>
            assemble(scratch(), {
                tamper: (dir) => {
                    write(dir, "replay.json", {
                        schema_version: 1,
                        corpus_sha256: SHA,
                        owner: "daemon.compression_fidelity.replay",
                        case: scenario?.case,
                        source: scenario?.s.source,
                        scenario: scenario?.s.id,
                        stage: "m0_pressure",
                        terminal: "served",
                        markers: [],
                        detail,
                    });
                },
            });
        const sparse = await otherOwner({ tier: "p5", curve_tier: "p2" });
        expect(rowOf(sparse, scenario?.s.id)?.deterministic).toBe("assertion_fail");
        const control = await otherOwner({ tier: scenario?.s.serving.tier, judge_control: true });
        expect(rowOf(control, scenario?.s.id)?.execution.outcomes).toContain(
            "daemon.compression_fidelity.replay:m0_pressure:served",
        );
        const judged = await otherOwner({ tier: "p1", judge_control: true });
        expect(rowOf(judged, scenario?.s.id)?.deterministic).toBe("assertion_fail");
    });

    test("an exact read passes only from the C6 witness that owns it", async () => {
        const exact = allScenarios.find(({ s }) => s.serving.path === "exact_read")?.s.id ?? "";
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${exact}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.owner = "opencode-delivery";
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(assembled.arms[0]?.identity_errors).toEqual([]);
        expect(rowOf(assembled, exact)?.deterministic).toBe("not_evaluated");
    });

    test("the exact-read witness judges only exact-read scenarios", async () => {
        const scenario = allScenarios.find(({ s }) => s.serving.path === "natural");
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${scenario?.s.id}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.owner = "daemon.harness_sources.c6_exact_read";
                value.detail = { tier: scenario?.s.serving.tier };
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(errorsOf(assembled)).toContain(
            `delivery.${scenario?.s.id}.json names scenario ${scenario?.s.id}, which is not an exact read`,
        );
        expect(rowOf(assembled, scenario?.s.id)?.execution.status).toBe("missing");
    });

    test("a pressure delivery without a curve tier fails the pressure oracle", async () => {
        const scenario = allScenarios.find(({ s }) => s.serving.path === "pressure")?.s;
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${scenario?.id}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.detail.served_tier = scenario?.serving.tier;
                delete value.detail.curve_tier;
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(rowOf(assembled, scenario?.id)?.deterministic).toBe("assertion_fail");
    });

    test("an unknown curve tier fails the pressure oracle", async () => {
        const scenario = allScenarios.find(({ s }) => s.serving.path === "pressure")?.s;
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${scenario?.id}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.detail.served_tier = "p5";
                value.detail.curve_tier = "p9";
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(rowOf(assembled, scenario?.id)?.deterministic).toBe("assertion_fail");
    });
});

describe("evidence comparison refusals", () => {
    test("arms that differ in anything but the prompt are refused", async () => {
        const assembled = await assemble(scratch(), { candidate: { model: "anthropic/other" } });
        expect(assembled.refused).toContain("the arms differ in model");
    });

    test("held fields compare by value, whatever their key order", async () => {
        const differs = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, "arm.json");
                const arm = JSON.parse(readFileSync(path, "utf8"));
                writeFileSync(
                    path,
                    JSON.stringify({ ...arm, settings: { ...SETTINGS, b: 2, a: 1 } }),
                );
            },
        });
        expect(differs.refused).toContain("the arms differ in settings");
        const root = scratch();
        const same = await assemble(root, {
            tamper: (dir) => {
                for (const arm of [dir, join(root, "candidate")]) {
                    const path = join(arm, "arm.json");
                    const value = JSON.parse(readFileSync(path, "utf8"));
                    const settings =
                        arm === dir ? { ...SETTINGS, b: 2, a: 1 } : { ...SETTINGS, a: 1, b: 2 };
                    writeFileSync(path, JSON.stringify({ ...value, settings }));
                }
            },
        });
        expect(same.refused).toEqual([]);
        const unicode = scratch();
        const composed = await assemble(unicode, {
            tamper: (dir) => {
                for (const arm of [dir, join(unicode, "candidate")]) {
                    const path = join(arm, "arm.json");
                    const value = JSON.parse(readFileSync(path, "utf8"));
                    const settings =
                        arm === dir
                            ? { ...SETTINGS, é: 1, "e\u0301": 2 }
                            : { ...SETTINGS, "e\u0301": 2, é: 1 };
                    writeFileSync(path, JSON.stringify({ ...value, settings }));
                }
            },
        });
        expect(composed.refused).toEqual([]);
        const prompt = await assemble(scratch(), {
            candidate: { system: "summarizer system prompt" },
        });
        expect(prompt.treatment).toBe(false);
    });
});

describe("evidence live mode", () => {
    const LIMITS = { maxCalls: 40, maxOutputTokens: 1024, timeoutMs: 1000, spendCapUsd: 1 };
    function forwardingReport(body: string, bodySha: string, complete: boolean) {
        return {
            mode: "forward",
            corpus_sha256: SHA,
            model: "claude-live",
            context_limit: 200_000,
            stopped: null as string | null,
            spent_usd: 0.5,
            limits: LIMITS as Record<string, number>,
            complete,
            incomplete_reasons: complete ? [] : ["a send is in flight"],
            exchanges: [
                {
                    index: 0,
                    tool_results: [] as string[],
                    tool_uses: [] as string[],
                    request: { body_text: body, body_sha256: bodySha },
                    response: {
                        outcome: "acknowledged",
                        model: "claude-live" as string | null,
                        stop_reason: "end_turn" as string | null,
                        truncated: false,
                        body_text: "ok",
                        body_sha256: sha256("ok") as string | null,
                        cost_known: true,
                    },
                },
            ],
        };
    }

    function live(
        reports: Array<Record<string, unknown>>,
        limits: Record<string, number> = LIMITS,
    ) {
        const root = scratch();
        return assemble(root, {
            mode: "live",
            baseline: { limits },
            candidate: { limits },
            tamper: (dir) => {
                reports.forEach((report, i) => {
                    write(dir, `forwarding-${i}.json`, report);
                    write(join(root, "candidate"), `forwarding-${i}.json`, report);
                });
            },
        });
    }

    test("live mode needs complete, hash-consistent forwarding reports run under the arm's limits", async () => {
        expect((await live([])).arms[0]?.identity_errors).toContain(
            "live mode found no forwarding report",
        );
        expect(errorsOf(await live([forwardingReport("{}", sha256("{ }"), true)]))).toContain(
            "do not match their hash",
        );
        expect(errorsOf(await live([forwardingReport("{}", sha256("{}"), false)]))).toContain(
            "is incomplete",
        );
        const unhashed = forwardingReport("{}", sha256("{}"), true);
        const [first] = unhashed.exchanges;
        if (first) first.response = { ...first.response, body_sha256: null };
        expect(errorsOf(await live([unhashed]))).toContain(
            "response has no hash in a complete report",
        );
        const incompleteUnhashed = { ...unhashed, complete: false, incomplete_reasons: ["x"] };
        expect(errorsOf(await live([incompleteUnhashed]))).not.toContain("has no hash");
        const complete = forwardingReport("{}", sha256("{}"), true);
        expect(errorsOf(await live([{ ...complete, exchanges: [] }]))).toContain(
            "complete report records no send",
        );
        const unanswered = {
            ...complete,
            exchanges: [{ ...complete.exchanges[0], response: null }],
        };
        expect(errorsOf(await live([unanswered]))).toContain(
            "exchange 0 has no response in a complete report",
        );
        const costless = forwardingReport("{}", sha256("{}"), true);
        const [only] = costless.exchanges;
        if (only) only.response = { ...only.response, cost_known: false };
        expect(errorsOf(await live([costless]))).toContain(
            "exchange 0 cost is unknown in a complete report",
        );
        const clipped = forwardingReport("{}", sha256("{}"), true);
        const [cut] = clipped.exchanges;
        if (cut) cut.response = { ...cut.response, truncated: true };
        expect(errorsOf(await live([clipped]))).toContain(
            "exchange 0 response is truncated in a complete report",
        );
        const incomplete = { ...unanswered, complete: false, incomplete_reasons: ["x"] };
        expect(errorsOf(await live([incomplete]))).not.toContain("in a complete report");
        const errored = forwardingReport("{}", sha256("{}"), true);
        const [sent] = errored.exchanges;
        if (sent)
            sent.response = { ...sent.response, outcome: "provider_error", stop_reason: null };
        expect(errorsOf(await live([errored]))).toContain(
            "exchange 0 response outcome is provider_error in a complete report",
        );
        expect(errorsOf(await live([errored]))).toContain(
            "exchange 0 response states no stop reason in a complete report",
        );
        expect(
            errorsOf(await live([{ ...complete, stopped: "send 0 returned HTTP 500" }])),
        ).toContain("complete report records a stop: send 0 returned HTTP 500");
        const looped = forwardingReport("{}", sha256("{}"), true);
        const [call] = looped.exchanges;
        if (call) call.tool_uses = ["toolu_1"];
        expect(errorsOf(await live([looped]))).toContain(
            "exchange 0 asks for tool toolu_1 that no later request answers in a complete report",
        );
        const answer = {
            ...looped.exchanges[0],
            index: 1,
            tool_uses: [],
            tool_results: ["toolu_1"],
        };
        const answered = { ...looped, exchanges: [...looped.exchanges, answer] };
        expect(errorsOf(await live([answered]))).not.toContain("asks for tool");
        expect(errorsOf(await live([{ ...complete, spent_usd: 1.5 }]))).toContain(
            "complete report spent 1.5 USD above its 1 USD cap",
        );
        const renamed = forwardingReport("{}", sha256("{}"), true);
        const [named] = renamed.exchanges;
        if (named) named.response = { ...named.response, model: "claude-other" };
        expect(errorsOf(await live([renamed]))).toContain(
            "exchange 0 response names model claude-other in a complete report",
        );
        if (named) named.response = { ...named.response, model: null };
        expect(errorsOf(await live([renamed]))).toContain(
            "exchange 0 response names no model in a complete report",
        );
        const { timeoutMs: _, ...threeLimits } = LIMITS;
        expect(errorsOf(await live([{ ...complete, limits: threeLimits }]))).toContain(
            "does not match the forwarding report schema",
        );
        const untyped = {
            ...complete,
            exchanges: [{ ...complete.exchanges[0], tool_uses: "toolu_1" }],
        };
        expect(errorsOf(await live([untyped]))).toContain(
            "does not match the forwarding report schema",
        );
        const raised = await live([
            { ...forwardingReport("{}", sha256("{}"), true), limits: { ...LIMITS, maxCalls: 41 } },
        ]);

        expect(errorsOf(raised)).toContain("ran limits other than the arm's");
        const reordered = await live(
            [
                {
                    ...forwardingReport("{}", sha256("{}"), true),
                    limits: {
                        spendCapUsd: 1,
                        timeoutMs: 1000,
                        maxOutputTokens: 1024,
                        maxCalls: 40,
                    },
                },
            ],
            LIMITS,
        );
        expect(reordered.arms[0]?.identity_errors).toEqual([]);
        const clean = await live([forwardingReport("{}", sha256("{}"), true)]);
        expect(clean.arms[0]?.identity_errors).toEqual([]);
        expect(clean.refused).toEqual([]);
    });

    test("a complete live report with no exchange is refused", async () => {
        const empty = await live([
            { ...forwardingReport("{}", sha256("{}"), true), exchanges: [] },
        ]);
        expect(errorsOf(empty)).toContain("forwarding-0.json complete report records no send");
    });

    test("live arms whose forwarding reports ran different models are refused", async () => {
        const root = scratch();
        const report = forwardingReport("{}", sha256("{}"), true);
        const forwardedTo = (model: string) => ({
            ...report,
            model,
            exchanges: report.exchanges.map((e) => ({ ...e, response: { ...e.response, model } })),
        });
        const assembled = await assemble(root, {
            mode: "live",
            baseline: { limits: LIMITS },
            candidate: { limits: LIMITS },
            tamper: (dir) => {
                write(dir, "forwarding-0.json", report);
                write(join(root, "candidate"), "forwarding-0.json", forwardedTo("claude-other"));
            },
        });
        expect(assembled.arms.map((a) => a.identity_errors)).toEqual([[], []]);
        expect(assembled.refused).toContain("the arms forwarded to different models");
        expect(assembled.manifest.arms[1]?.forwarding_reports[0]?.model).toBe("claude-other");
        const limited = await assemble(scratch(), {
            mode: "live",
            baseline: { limits: LIMITS },
            candidate: { limits: LIMITS },
            tamper: (dir) => {
                write(dir, "forwarding-0.json", report);
                write(join(dir, "..", "candidate"), "forwarding-0.json", {
                    ...report,
                    context_limit: 100_000,
                });
            },
        });
        expect(limited.refused).toContain("the arms forwarded at different context limits");
        const mixed = await live([report, forwardedTo("claude-other")]);
        expect(errorsOf(mixed)).toContain(
            "forwarding-1.json forwarded to claude-other, where forwarding-0.json forwarded to claude-live",
        );
        expect(errorsOf(await live([{ ...report, model: "" }]))).toContain(
            "does not match the forwarding report schema",
        );
    });
});

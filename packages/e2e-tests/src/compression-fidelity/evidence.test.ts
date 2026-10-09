import { describe, expect, test } from "bun:test";
import {
    chmodSync,
    mkdirSync,
    readdirSync,
    readFileSync,
    rmSync,
    statSync,
    symlinkSync,
    writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs, repositoryRevision, run } from "../../scripts/eval-compression-fidelity";
import {
    type ArmOptions,
    allScenarios,
    CORPUS_PATH,
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
        const retried = await assemble(scratch(), {
            baseline: { attempts: (attempt) => [attempt, attempt] },
        });
        expect(retried.arms.map((a) => a.identity_errors)).toEqual([[], []]);
        expect(retried.refused).toEqual([]);
    });

    test("a published real capture records settled generation, drained output, and published rows", async () => {
        const unsettled = await assemble(scratch(), { baseline: { capture: { settled: false } } });
        expect(errorsOf(unsettled)).toContain("is published without settled generation");
        const silent = await assemble(scratch(), {
            baseline: { attempt: { outputs: [{ text: null, error: "timeout" }] } },
        });
        expect(errorsOf(silent)).toContain("is published without a drained text output");
        const rowless = await assemble(scratch(), {
            baseline: { capture: { published_rows: [] } },
        });
        expect(errorsOf(rowless)).toContain("is published without published rows");
        for (const assembled of [unsettled, silent, rowless]) {
            expect(assembled.refused).toContain("an arm has identity errors");
        }
    });

    test("a variant label is accepted only from the witness and scenario that document it", async () => {
        const relabel = (scenario: string, variant: string, owner?: string) =>
            assemble(scratch(), {
                tamper: (dir) => {
                    const path = join(dir, `delivery.${scenario}.json`);
                    const value = JSON.parse(readFileSync(path, "utf8"));
                    value.scenario = `${scenario}@${variant}`;
                    value.terminal = "unqualified";
                    if (owner) value.owner = owner;
                    writeFileSync(join(dir, "variant.json"), JSON.stringify(value));
                },
            });
        expect(errorsOf(await relabel("C1.S2", "ignored"))).toContain(
            "variant.json labels variant ignored, which no witness emits",
        );
        expect(
            errorsOf(await relabel("C1.S2", "p1-only", "daemon.compression_fidelity.replay")),
        ).toContain("variant.json labels variant p1-only, which no witness emits");
        expect(errorsOf(await relabel("C2.S1", "p1-only"))).toContain(
            "variant.json labels variant p1-only, which no witness emits for C2.S1",
        );
        const known = await relabel("C1.S2", "p1-only");
        expect(known.arms[0]?.identity_errors).toEqual([]);
        expect(rowOf(known, "C1.S2")?.execution.outcomes).toContain(
            "opencode-delivery:p1-only:served:unqualified",
        );
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

    test("an exact-read pass needs the witness's byte identity fields", async () => {
        const exact = allScenarios.find(({ s }) => s.serving.path === "exact_read")?.s.id ?? "";
        const withDetail = (detail: Record<string, unknown>) =>
            assemble(scratch(), {
                tamper: (dir) => {
                    const path = join(dir, `delivery.${exact}.json`);
                    const value = JSON.parse(readFileSync(path, "utf8"));
                    value.detail = detail;
                    writeFileSync(path, JSON.stringify(value));
                },
            });
        expect(rowOf(await withDetail({}), exact)?.deterministic).toBe("not_evaluated");
        expect(
            rowOf(await withDetail({ sha256: "abc", byte_length: 144 }), exact)?.deterministic,
        ).toBe("not_evaluated");
        expect(
            rowOf(await withDetail({ sha256: "0".repeat(64), byte_length: 0 }), exact)
                ?.deterministic,
        ).toBe("not_evaluated");
    });

    test("an observation whose detail is not an object is an identity error", async () => {
        const exact = allScenarios.find(({ s }) => s.serving.path === "exact_read")?.s.id ?? "";
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${exact}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.detail = null;
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(errorsOf(assembled)).toContain(`delivery.${exact}.json has no detail record`);
        expect(rowOf(assembled, exact)?.deterministic).toBe("not_evaluated");
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

    test("a judge control outside the witness's control stage or scenario is an identity error", async () => {
        const first = allScenarios.find(({ s }) => s.serving.path !== "exact_read")?.s.id ?? "";
        const control = (scenario: string, stage?: string) =>
            assemble(scratch(), {
                tamper: (dir) => {
                    const path = join(dir, `delivery.${scenario}.json`);
                    const value = JSON.parse(readFileSync(path, "utf8"));
                    value.terminal = "unqualified";
                    value.detail.judge_control = true;
                    if (stage) value.stage = stage;
                    writeFileSync(path, JSON.stringify(value));
                },
            });
        expect(errorsOf(await control(first))).toContain(
            `delivery.${first}.json marks stage served as a judge control`,
        );
        expect(errorsOf(await control("C2.S1", "missing-capture"))).toContain(
            "delivery.C2.S1.json marks C2.S1 as a judge control, which the witness tests on C1.S2",
        );
        const documented = await control("C1.S2", "missing-capture");
        expect(documented.arms[0]?.identity_errors).toEqual([]);
    });

    test("a delivery observation carries a string scenario label", async () => {
        const first = allScenarios[0]?.s.id ?? "";
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${first}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.scenario = null;
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(errorsOf(assembled)).toContain(`delivery.${first}.json has no scenario label`);
        expect(rowOf(assembled, first)?.execution.status).toBe("missing");
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

    test("a scripted generation attempt names the arm's model and returns the reviewed output", async () => {
        const scripted = { generationOrigin: "scripted" as const };
        const otherModel = await assemble(scratch(), {
            baseline: { ...scripted, attempt: { model: "anthropic/other" } },
            candidate: scripted,
        });
        expect(errorsOf(otherModel)).toContain(
            "attempt ran model anthropic/other, not the arm's model",
        );
        const otherOutput = await assemble(scratch(), {
            baseline: { ...scripted, attempt: { output_sha256: sha256("something else") } },
            candidate: scripted,
        });
        expect(errorsOf(otherOutput)).toContain("returned output other than the reviewed output");
        const unhashed = await assemble(scratch(), {
            baseline: { ...scripted, attempt: { output_sha256: undefined } },
            candidate: scripted,
        });
        expect(errorsOf(unhashed)).toContain("returned output other than the reviewed output");
    });

    test("a real arm declares numeric generation settings and its attempts record them", async () => {
        const unset = await assemble(scratch(), {
            baseline: {
                settings: {},
                attempt: { temperature: undefined, max_output_tokens: undefined },
            },
        });
        expect(errorsOf(unset)).toContain("arm.json declares no numeric temperature");
        expect(errorsOf(unset)).toContain("arm.json declares no numeric max_output_tokens");
        const stringy = await assemble(scratch(), {
            baseline: {
                settings: { temperature: "0.1", max_output_tokens: 1024 },
                attempt: { temperature: "0.1" },
            },
        });
        expect(errorsOf(stringy)).toContain("arm.json declares no numeric temperature");
    });

    test("a prompt digest must be a SHA-256 hex digest", async () => {
        const scripted = { generationOrigin: "scripted" as const };
        const labeled = await assemble(scratch(), {
            baseline: { ...scripted, promptSha256: "baseline" },
            candidate: { ...scripted, promptSha256: "candidate" },
        });
        expect(errorsOf(labeled)).toContain("arm.json does not match its schema");
        const systemPrompt = await assemble(scratch(), {
            baseline: { ...scripted, attempt: { system_sha256: "baseline" } },
            candidate: scripted,
        });
        expect(errorsOf(systemPrompt)).toContain("records a malformed digest");
        expect(errorsOf(systemPrompt)).toContain("records no system prompt");
        const userPrompt = await assemble(scratch(), {
            baseline: { ...scripted, attempt: { prompt_sha256: "p" } },
            candidate: scripted,
        });
        expect(errorsOf(userPrompt)).toContain("records a malformed digest");
    });

    test("a terminal is one its owner emits", async () => {
        const first = allScenarios[0]?.s.id ?? "";
        for (const terminal of ["excluded", "discoverable"]) {
            const memory = await assemble(scratch(), {
                tamper: (dir) => {
                    const path = join(dir, `delivery.${first}.json`);
                    const value = JSON.parse(readFileSync(path, "utf8"));
                    value.terminal = terminal;
                    writeFileSync(path, JSON.stringify(value));
                },
            });
            expect(memory.arms[0]?.identity_errors).toEqual([]);
            expect(rowOf(memory, first)?.execution.status).toBe("executed");
        }
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${first}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.terminal = "published";
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(errorsOf(assembled)).toContain(
            `delivery.${first}.json has terminal published, which opencode-delivery does not emit`,
        );
        expect(rowOf(assembled, first)?.execution.status).toBe("missing");
    });

    test("a real capture attempt records its prompts as text", async () => {
        const assembled = await assemble(scratch(), {
            baseline: {
                attempt: {
                    system: undefined,
                    prompt: undefined,
                    system_sha256: sha256("summarizer system prompt"),
                    prompt_sha256: sha256("p"),
                },
            },
        });
        expect(errorsOf(assembled)).toContain("records a prompt as a digest without its text");
        const fractional = await assemble(scratch(), {
            baseline: {
                settings: { temperature: 0.1, max_output_tokens: 1024.5 },
                attempt: { max_output_tokens: 1024.5 },
            },
        });
        expect(errorsOf(fractional)).toContain(
            "arm.json declares no whole-number max_output_tokens",
        );
    });

    test("an output origin is the exact literal its producer writes", async () => {
        const lookalike = await assemble(scratch(), {
            baseline: { origin: "real-looking fixture" },
        });
        expect(errorsOf(lookalike)).toContain(
            'carries output of origin "real-looking fixture" in an arm labeled real',
        );
        expect(errorsOf(lookalike)).toContain("no published real-model capture");
        const scripted = { generationOrigin: "scripted" as const };
        const replayLike = await assemble(scratch(), {
            baseline: { ...scripted, attempt: { output_origin: "scripted-ish" } },
            candidate: scripted,
        });
        expect(errorsOf(replayLike)).toContain(
            'carries output of origin "scripted-ish" in an arm labeled scripted',
        );
        expect(errorsOf(replayLike)).toContain("no published scripted generation");
    });

    test("a real capture is emitted at the capture stage with object rows", async () => {
        const staged = await assemble(scratch(), {
            tamper: (dir) => {
                const first = corpus.cases[0]?.sources[0]?.id ?? "";
                const path = join(dir, `real.${first}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.stage = "generation";
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(errorsOf(staged)).toContain("real.C1.V1.json is a real capture at stage generation");
        const placeholder = await assemble(scratch(), {
            baseline: { capture: { published_rows: [null] } },
        });
        expect(errorsOf(placeholder)).toContain("is published without published rows");
        const untitled = await assemble(scratch(), {
            baseline: { capture: { published_rows: [{ start: 1, end: 2 }] } },
        });
        expect(errorsOf(untitled)).toContain("is published without published rows");
    });

    test("a real capture's attempt count equals its retained attempts", async () => {
        const assembled = await assemble(scratch(), {
            baseline: { capture: { attempt_count: 2 } },
        });
        expect(errorsOf(assembled)).toContain("records attempt_count 2 for 1 retained attempt");
        const unnumbered = await assemble(scratch(), {
            baseline: { capture: { attempt_count: undefined } },
        });
        expect(errorsOf(unnumbered)).toContain(
            "records attempt_count undefined for 1 retained attempt",
        );
    });

    test("every entry of a generation's attempts is a record", async () => {
        const assembled = await assemble(scratch(), {
            baseline: { attempts: (attempt) => [attempt, "retried", ["x"]] },
        });
        expect(errorsOf(assembled)).toContain("records an attempt that is not a record");
        expect(assembled.refused).toContain("an arm has identity errors");
    });

    test("every generation attempt records its system prompt", async () => {
        const assembled = await assemble(scratch(), {
            baseline: {
                attempts: (attempt) => [attempt, { ...attempt, system: undefined }],
            },
        });
        expect(errorsOf(assembled)).toContain("records no system prompt on an attempt");
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
        const hidden = await assemble(scratch(), {
            tamper: (dir) => {
                writeFileSync(join(dir, ".DS_Store"), "");
                writeFileSync(join(dir, ".gitkeep"), "");
            },
        });
        expect(hidden.arms[0]?.identity_errors).toEqual([]);
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
        const entry = allScenarios.find(({ s }) => s.id === "C1.S2");
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
        const relabeled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, "delivery.C1.S1.json");
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.scenario = "C1.V1";
                value.terminal = "unqualified";
                writeFileSync(join(dir, "relabeled.json"), JSON.stringify(value));
            },
        });
        expect(errorsOf(relabeled)).toContain(
            "relabeled.json labels source C1.V1, which the delivery witness labels only for a source without an m1 scenario at its m1, warm, or cold-m0 stage",
        );
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
        expect(errorsOf(assembled)).toContain(
            `delivery.${exact}.json has terminal read_exact, which opencode-delivery does not emit`,
        );
        expect(rowOf(assembled, exact)?.deterministic).toBe("not_evaluated");
    });

    test("an exact-read pass needs the witness's exact_read stage", async () => {
        const exact = allScenarios.find(({ s }) => s.serving.path === "exact_read")?.s.id ?? "";
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${exact}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.stage = "replayed";
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(rowOf(assembled, exact)?.deterministic).toBe("not_evaluated");
    });

    test("the exact-read witness judges only exact-read scenarios", async () => {
        const scenario = allScenarios.find(({ s }) => s.serving.path === "natural");
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${scenario?.s.id}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.owner = "daemon.harness_sources.c6_exact_read";
                value.terminal = "read_exact";
                value.detail = { tier: scenario?.s.serving.tier };
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(errorsOf(assembled)).toContain(
            `delivery.${scenario?.s.id}.json names scenario ${scenario?.s.id}, which is not an exact read`,
        );
        expect(rowOf(assembled, scenario?.s.id)?.execution.status).toBe("missing");
    });

    test("a pressure delivery must reach the scenario's tier as well as pass its curve", async () => {
        const scenario = allScenarios.find(({ s }) => s.id === "C1.S5")?.s;
        const assembled = await assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, `delivery.${scenario?.id}.json`);
                const value = JSON.parse(readFileSync(path, "utf8"));
                value.detail.served_tier = "p2";
                value.detail.curve_tier = "p1";
                writeFileSync(path, JSON.stringify(value));
            },
        });
        expect(scenario?.serving.tier).toBe("p4");
        expect(rowOf(assembled, scenario?.id)?.deterministic).toBe("assertion_fail");
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
            upstream_url: "https://api.example.test/v1/messages",
            context_limit: 200_000,
            pricing: { inputPerMTok: 3, outputPerMTok: 15 },
            stopped: null as string | null,
            refusals: [] as string[],
            attempted_sends: 1,
            acknowledged_responses: 1,
            spent_usd: 0.5,
            limits: LIMITS as Record<string, number>,
            complete,
            incomplete_reasons: complete ? [] : ["a send is in flight"],
            exchanges: [
                {
                    index: 0,
                    tool_uses: [] as string[],
                    tool_results: [] as string[],
                    request: { body_text: body, body_sha256: bodySha },
                    response: {
                        outcome: "acknowledged",
                        cost_usd: 0.5,
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
        const elsewhere = await assemble(scratch(), {
            mode: "live",
            baseline: { limits: LIMITS },
            candidate: { limits: LIMITS },
            tamper: (dir) => {
                write(dir, "forwarding-0.json", report);
                write(join(dir, "..", "candidate"), "forwarding-0.json", {
                    ...report,
                    upstream_url: "https://other.example.test/v1/messages",
                });
            },
        });
        expect(elsewhere.refused).toContain("the arms forwarded to different upstream endpoints");
        const repriced = await assemble(scratch(), {
            mode: "live",
            baseline: { limits: LIMITS },
            candidate: { limits: LIMITS },
            tamper: (dir) => {
                write(dir, "forwarding-0.json", report);
                write(join(dir, "..", "candidate"), "forwarding-0.json", {
                    ...report,
                    pricing: { outputPerMTok: 15, inputPerMTok: 4 },
                });
            },
        });
        expect(repriced.refused).toContain("the arms forwarded at different prices");
        const sameReordered = await assemble(scratch(), {
            mode: "live",
            baseline: { limits: LIMITS },
            candidate: { limits: LIMITS },
            tamper: (dir) => {
                write(dir, "forwarding-0.json", report);
                write(join(dir, "..", "candidate"), "forwarding-0.json", {
                    ...report,
                    pricing: { outputPerMTok: 15, inputPerMTok: 3 },
                });
            },
        });
        expect(sameReordered.refused).toEqual([]);
        expect(errorsOf(await live([{ ...report, pricing: { inputPerMTok: 3 } }]))).toContain(
            "does not match the forwarding report schema",
        );
        expect(
            errorsOf(await live([{ ...report, pricing: { inputPerMTok: 0, outputPerMTok: 15 } }])),
        ).toContain("does not match the forwarding report schema");
        expect(elsewhere.manifest.arms[1]?.forwarding_reports[0]?.upstream_url).toBe(
            "https://other.example.test/v1/messages",
        );
        const relocated = await live([
            report,
            { ...report, upstream_url: "https://other.example.test/v1/messages" },
        ]);
        expect(errorsOf(relocated)).toContain(
            "forwarding-1.json forwarded to https://other.example.test/v1/messages, where forwarding-0.json forwarded to https://api.example.test/v1/messages",
        );
        expect(
            errorsOf(await live([{ ...report, incomplete_reasons: ["a send is in flight"] }])),
        ).toContain("complete report lists an incomplete reason: a send is in flight");
        expect(errorsOf(await live([{ ...report, incomplete_reasons: [1] }]))).toContain(
            "does not match the forwarding report schema",
        );
        for (const limits of [
            { ...LIMITS, maxCalls: 1.5 },
            { ...LIMITS, maxOutputTokens: 1024.5 },
        ]) {
            expect(errorsOf(await live([{ ...report, limits }], limits))).toContain(
                "does not match the forwarding report schema",
            );
        }
        // `1e400` parses to Infinity, which JSON.stringify cannot write back.
        const infinite = scratch();
        const unbounded = await assemble(infinite, {
            mode: "live",
            baseline: { limits: LIMITS },
            candidate: { limits: LIMITS },
            tamper: (dir) => {
                const text = JSON.stringify(report).replace(
                    '"spendCapUsd":1',
                    '"spendCapUsd":1e400',
                );
                writeFileSync(join(dir, "forwarding-0.json"), text);
                writeFileSync(join(infinite, "candidate", "forwarding-0.json"), text);
            },
        });
        expect(errorsOf(unbounded)).toContain("does not match the forwarding report schema");
        const priceless = scratch();
        const unpriced = await assemble(priceless, {
            mode: "live",
            baseline: { limits: LIMITS },
            candidate: { limits: LIMITS },
            tamper: (dir) => {
                const text = JSON.stringify(report).replace(
                    '"inputPerMTok":3',
                    '"inputPerMTok":1e400',
                );
                writeFileSync(join(dir, "forwarding-0.json"), text);
                writeFileSync(join(priceless, "candidate", "forwarding-0.json"), text);
            },
        });
        expect(errorsOf(unpriced)).toContain("does not match the forwarding report schema");
        const oneCall = { ...LIMITS, maxCalls: 1 };
        const second = { ...report.exchanges[0], index: 1 };
        const overCap = { ...report, limits: oneCall, exchanges: [...report.exchanges, second] };
        expect(errorsOf(await live([overCap], oneCall))).toContain(
            "records 2 exchanges above its 1 call cap",
        );
        expect(errorsOf(await live([{ ...report, spent_usd: 0.25 }]))).toContain(
            "spent 0.25 USD, where its exchanges cost 0.5 USD",
        );
        expect(
            errorsOf(await live([{ ...report, refusals: ["limits.maxCalls reached"] }])),
        ).toContain("complete report records a refusal: limits.maxCalls reached");
        expect(errorsOf(await live([{ ...report, refusals: [1] }]))).toContain(
            "does not match the forwarding report schema",
        );
        expect(errorsOf(await live([{ ...report, attempted_sends: 2 }]))).toContain(
            "records 2 attempted sends over 1 exchange",
        );
        expect(errorsOf(await live([{ ...report, acknowledged_responses: 0 }]))).toContain(
            "records 0 acknowledged responses over 1 acknowledged exchange",
        );
        expect(errorsOf(await live([{ ...report, attempted_sends: -1 }]))).toContain(
            "does not match the forwarding report schema",
        );
        const refund = { ...report.exchanges[0], index: 1 };
        refund.response = { ...refund.response, cost_usd: -0.25 };
        const offset = { ...report, exchanges: [...report.exchanges, refund], spent_usd: 0.25 };
        expect(errorsOf(await live([offset]))).toContain(
            "does not match the forwarding report schema",
        );
        const reordered = {
            ...report,
            exchanges: [
                { ...report.exchanges[0], index: 1, tool_results: ["toolu_1"] },
                { ...report.exchanges[0], index: 0, tool_uses: ["toolu_1"] },
            ],
        };
        expect(errorsOf(await live([reordered]))).toContain(
            "does not match the forwarding report schema",
        );
        for (const spent of [-0.01, Number.NaN, Number.POSITIVE_INFINITY]) {
            expect(errorsOf(await live([{ ...report, spent_usd: spent }]))).toContain(
                "does not match the forwarding report schema",
            );
        }
        for (const limit of [0, -1, 1.5, Number.NaN, Number.POSITIVE_INFINITY]) {
            expect(errorsOf(await live([{ ...report, context_limit: limit }]))).toContain(
                "does not match the forwarding report schema",
            );
        }
        for (const upstream of [
            "",
            "http://api.example.test/v1/messages",
            "https://api.example.test/v1/complete",
            "https://user:pw@api.example.test/v1/messages",
            "https://api.example.test/v1/messages?x=1",
            "not a url",
        ]) {
            expect(errorsOf(await live([{ ...report, upstream_url: upstream }]))).toContain(
                "does not match the forwarding report schema",
            );
        }
        const mixed = await live([report, forwardedTo("claude-other")]);
        expect(errorsOf(mixed)).toContain(
            "forwarding-1.json forwarded to claude-other, where forwarding-0.json forwarded to claude-live",
        );
        expect(errorsOf(await live([{ ...report, model: "" }]))).toContain(
            "does not match the forwarding report schema",
        );
    });
});

describe("eval:compression-fidelity command", () => {
    test("flags are required once each and unknown flags are refused", () => {
        const base = ["--baseline", "a", "--candidate", "b", "--out", "o"];
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
                out,
                corpus: CORPUS_PATH,
                mode: "offline",
            });
        } finally {
            globalThis.fetch = original;
        }
        expect(sent).toEqual([]);
        expect(written?.accepted).toBe(false);
        expect(readdirSync(out).sort()).toEqual(["manifest.json", "report.json"]);
        const published = JSON.parse(readFileSync(written?.report ?? "", "utf8"));
        expect(published.manifest_sha256).toBe(sha256(readFileSync(written?.manifest ?? "")));
        expect(statSync(out).mode & 0o777).toBe(0o700);
        expect(statSync(written?.report ?? "").mode & 0o777).toBe(0o600);
        await expect(
            run({
                baseline: baseline.dir,
                candidate: candidate.dir,
                out: resolve(import.meta.dir, "eval-out"),
                corpus: CORPUS_PATH,
                mode: "offline",
            }),
        ).rejects.toThrow("inside the repository");
    });

    test("evidence arms may be named through parent-relative paths", async () => {
        const root = scratch();
        const baseline = writeArm(root, { label: "baseline" });
        const candidate = writeArm(root, { label: "candidate" });
        const written = await run({
            baseline: `${root}/candidate/../baseline`,
            candidate: `${candidate.dir}/./`,
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

    test("the manifest revision marks a worktree with uncommitted changes", () => {
        const repo = scratch();
        const git = (...args: string[]) =>
            Bun.spawnSync(["git", "-c", "user.name=t", "-c", "user.email=t@t", ...args], {
                cwd: repo,
                stderr: "ignore",
            });
        git("init", "-q");
        writeFileSync(join(repo, "a.txt"), "a\n");
        git("add", "a.txt");
        git("commit", "-q", "-m", "a");
        const head = git("rev-parse", "HEAD").stdout.toString().trim();
        expect(repositoryRevision(repo)).toBe(head);
        writeFileSync(join(repo, "a.txt"), "b\n");
        expect(repositoryRevision(repo)).toBe(`${head}-dirty`);
        writeFileSync(join(repo, "untracked.txt"), "u\n");
        expect(repositoryRevision(repo)).toBe(`${head}-dirty`);
        expect(repositoryRevision(join(repo, "missing"))).toBe("unknown");
    });

    test("an output directory that is an evidence arm is refused before any write", async () => {
        const root = scratch();
        const baseline = writeArm(root, { label: "baseline" });
        const candidate = writeArm(root, { label: "candidate" });
        chmodSync(baseline.dir, 0o700);
        chmodSync(candidate.dir, 0o700);
        for (const out of [baseline.dir, join(candidate.dir, ".", "")]) {
            await expect(
                run({
                    baseline: baseline.dir,
                    candidate: candidate.dir,
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
                out: join(link, "nested", ".."),
                corpus: CORPUS_PATH,
                mode: "offline",
            }),
        ).rejects.toThrow("--out is an evidence arm");
        expect(readdirSync(baseline.dir)).not.toContain("manifest.json");
        expect(readdirSync(candidate.dir)).not.toContain("report.json");
    });
});

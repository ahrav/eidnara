import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs, run } from "../../scripts/eval-compression-fidelity";
import {
    type ArmOptions,
    allScenarios,
    CORPUS_PATH,
    corpus,
    SHA,
    scratch,
    sha256,
    write,
    writeArm,
} from "./evaluation-fixtures";
import { assembleEvidence, loadArm } from "./evidence";

function assemble(
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
        baseline: loadArm(baseline.dir, corpus, SHA),
        candidate: loadArm(candidate.dir, corpus, SHA),
    });
}

const errorsOf = (assembled: ReturnType<typeof assemble>) =>
    assembled.arms[0]?.identity_errors.join("\n") ?? "";
const rowOf = (assembled: ReturnType<typeof assemble>, scenario: string | undefined) =>
    assembled.arms[0]?.rows.find((r) => r.scenario.id === scenario);

describe("evidence identity and completeness", () => {
    test("a complete evidence set executes every scenario and lists every observation in the manifest", () => {
        const assembled = assemble(scratch());
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

    test("a missing scenario is listed and refuses the comparison", () => {
        const missing = allScenarios[3]?.s.id ?? "";
        const assembled = assemble(scratch(), { baseline: { skipScenario: missing } });
        expect(assembled.arms[0]?.missing_scenarios).toEqual([missing]);
        expect(rowOf(assembled, missing)?.execution.status).toBe("missing");
        expect(assembled.refused).toContain("the arms reached different scenario sets");
    });

    test("identity errors on the candidate side alone refuse the comparison", () => {
        const assembled = assemble(scratch(), {
            candidate: { origin: "scripted approved example" },
        });
        expect(assembled.arms[0]?.identity_errors).toEqual([]);
        expect(assembled.arms[1]?.identity_errors.join("\n")).toContain("scripted output");
        expect(assembled.refused).toContain("an arm has identity errors");
    });

    test("scripted output in an arm labeled real is an identity error", () => {
        const assembled = assemble(scratch(), {
            baseline: { origin: "scripted approved example" },
        });
        expect(errorsOf(assembled)).toContain("scripted output");
        expect(errorsOf(assembled)).toContain("no published real-model capture");
        expect(assembled.refused).toContain("an arm has identity errors");
    });

    test("evidence bound to another corpus refuses the comparison", () => {
        const assembled = assemble(scratch(), { candidate: { corpusSha256: "1".repeat(64) } });
        expect(assembled.refused).toContain("an arm is bound to another corpus");
    });

    test("a case mismatch is an identity error, not a corpus refusal", () => {
        const assembled = assemble(scratch(), {
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

    test("a prompt the arm did not declare is an identity error", () => {
        const assembled = assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, "arm.json");
                const arm = JSON.parse(readFileSync(path, "utf8"));
                writeFileSync(path, JSON.stringify({ ...arm, prompt_sha256: "2".repeat(64) }));
            },
        });
        expect(errorsOf(assembled)).toContain("not the arm's prompt");
    });

    test("an unpublished temporary file and a duplicate observation are refused", () => {
        const assembled = assemble(scratch(), {
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

    test("a real arm's serving observation must link a published capture of its own source", () => {
        const scenario = allScenarios[0]?.s;
        const unlinked = assemble(scratch(), { baseline: { unlinked: true } });
        expect(errorsOf(unlinked)).toContain("names no published real capture");
        const crossed = assemble(scratch(), {
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

    test("an empty or nested variant label is an identity error", () => {
        const entry = allScenarios[0];
        for (const label of [`${entry?.s.id}@`, `${entry?.s.id}@a@b`, "@p1-only"]) {
            const assembled = assemble(scratch(), {
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

    test("variants and judge controls are kept as outcomes and judge no scenario", () => {
        const entry = allScenarios[0];
        const assembled = assemble(scratch(), {
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
                    detail: { served_tier: "p5" },
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

    test("a malformed forwarding file is an identity error and assembly still completes", () => {
        const assembled = assemble(scratch(), {
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
});

describe("evidence deterministic column", () => {
    test("a wrong served tier fails the deterministic column", () => {
        const scenario = allScenarios.find(({ s }) => s.serving.tier === "p1")?.s.id ?? "";
        const assembled = assemble(scratch(), {
            baseline: { tier: (id) => (id === scenario ? "p3" : undefined) },
        });
        expect(rowOf(assembled, scenario)?.deterministic).toBe("assertion_fail");
    });

    test("a pressure delivery passes when it serves sparser than its curve tier", () => {
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
        expect(rowOf(pressed("p5"), scenario?.id)?.deterministic).toBe("pass");
        expect(rowOf(pressed("p2"), scenario?.id)?.deterministic).toBe("assertion_fail");
    });

    test("the pressure oracle and judge controls apply to delivery observations only", () => {
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
        const sparse = otherOwner({ tier: "p5", curve_tier: "p2" });
        expect(rowOf(sparse, scenario?.s.id)?.deterministic).toBe("assertion_fail");
        const control = otherOwner({ tier: scenario?.s.serving.tier, judge_control: true });
        expect(rowOf(control, scenario?.s.id)?.execution.outcomes).toContain(
            "daemon.compression_fidelity.replay:m0_pressure:served",
        );
        const judged = otherOwner({ tier: "p1", judge_control: true });
        expect(rowOf(judged, scenario?.s.id)?.deterministic).toBe("assertion_fail");
    });

    test("an unknown curve tier fails the pressure oracle", () => {
        const scenario = allScenarios.find(({ s }) => s.serving.path === "pressure")?.s;
        const assembled = assemble(scratch(), {
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
    test("arms that differ in anything but the prompt are refused", () => {
        const assembled = assemble(scratch(), { candidate: { model: "anthropic/other" } });
        expect(assembled.refused).toContain("the arms differ in model");
    });

    test("held fields compare by value, whatever their key order", () => {
        const differs = assemble(scratch(), {
            tamper: (dir) => {
                const path = join(dir, "arm.json");
                const arm = JSON.parse(readFileSync(path, "utf8"));
                writeFileSync(path, JSON.stringify({ ...arm, settings: { b: 2, a: 1 } }));
            },
        });
        expect(differs.refused).toContain("the arms differ in settings");
        const root = scratch();
        const same = assemble(root, {
            tamper: (dir) => {
                for (const arm of [dir, join(root, "candidate")]) {
                    const path = join(arm, "arm.json");
                    const value = JSON.parse(readFileSync(path, "utf8"));
                    const settings = arm === dir ? { b: 2, a: 1 } : { a: 1, b: 2 };
                    writeFileSync(path, JSON.stringify({ ...value, settings }));
                }
            },
        });
        expect(same.refused).toEqual([]);
        const prompt = assemble(scratch(), { candidate: { system: "summarizer system prompt" } });
        expect(prompt.treatment).toBe(false);
    });
});

describe("evidence live mode", () => {
    function forwardingReport(body: string, bodySha: string, complete: boolean) {
        return {
            mode: "forward",
            corpus_sha256: SHA,
            limits: { maxCalls: 40 },
            complete,
            incomplete_reasons: complete ? [] : ["a send is in flight"],
            exchanges: [
                {
                    index: 0,
                    request: { body_text: body, body_sha256: bodySha },
                    response: {
                        truncated: false,
                        body_text: "ok",
                        body_sha256: sha256("ok"),
                        cost_known: true,
                    },
                },
            ],
        };
    }

    function live(reports: Array<Record<string, unknown>>, limits?: Record<string, number>) {
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

    test("live mode needs complete, hash-consistent forwarding reports run under the arm's limits", () => {
        expect(live([]).arms[0]?.identity_errors).toContain("live mode found no forwarding report");
        expect(errorsOf(live([forwardingReport("{}", sha256("{ }"), true)]))).toContain(
            "do not match their hash",
        );
        expect(errorsOf(live([forwardingReport("{}", sha256("{}"), false)]))).toContain(
            "is incomplete",
        );
        const raised = live([
            { ...forwardingReport("{}", sha256("{}"), true), limits: { maxCalls: 41 } },
        ]);

        expect(errorsOf(raised)).toContain("ran limits other than the arm's");
        const reordered = live(
            [
                {
                    ...forwardingReport("{}", sha256("{}"), true),
                    limits: { maxOutputTokens: 1024, maxCalls: 40 },
                },
            ],
            { maxCalls: 40, maxOutputTokens: 1024 },
        );
        expect(reordered.arms[0]?.identity_errors).toEqual([]);
        const clean = live([forwardingReport("{}", sha256("{}"), true)]);
        expect(clean.arms[0]?.identity_errors).toEqual([]);
        expect(clean.refused).toEqual([]);
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

    test("the default run sends nothing and writes only owner-only files outside the repository", () => {
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
        let written: ReturnType<typeof run> | undefined;
        try {
            written = run({
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
        expect(statSync(out).mode & 0o777).toBe(0o700);
        expect(statSync(written?.report ?? "").mode & 0o777).toBe(0o600);
        expect(() =>
            run({
                baseline: baseline.dir,
                candidate: candidate.dir,
                out: resolve(import.meta.dir, "eval-out"),
                corpus: CORPUS_PATH,
                mode: "offline",
            }),
        ).toThrow("inside the repository");
    });
});

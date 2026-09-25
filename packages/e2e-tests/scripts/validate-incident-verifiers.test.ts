import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { parseIncidentCatalog } from "../src/incident-pool/contract";
import {
    boundVerifierDigests,
    boundVerifierFiles,
    E2E_ROOT,
    loadMutationEvidence,
} from "../src/incident-pool/evidence";
import { compareWithAcceptedSnapshot, validateIncidentHistory } from "../src/incident-pool/history";
import { builtinIncidentCaseRegistry } from "../src/incident-pool/registry";
import { loadHistorySnapshot } from "./validate-incident-history";
import {
    assertBoundVerifierBytesUnchanged,
    assertCatalogBindingsUnchanged,
    assertCatalogBoundVerifierBytesUnchanged,
    assertMutationBindingsUnchanged,
    catalogBindings,
    mutationBindings,
    replayCatalogVerifierChanges,
} from "./validate-incident-verifiers";

function committedCatalog() {
    return parseIncidentCatalog(
        JSON.parse(readFileSync(resolve(E2E_ROOT, "incidents", "catalog.json"), "utf8")) as unknown,
    );
}

describe("incident verifier contributor gate", () => {
    it("requires successful replay for every changed verifier before accepting it", () => {
        const accepted = { "first.rs": "a", "second.rs": "b" };
        const current = { "first.rs": "c", "second.rs": "d" };
        const replayed: string[] = [];
        expect(() =>
            assertBoundVerifierBytesUnchanged(accepted, current, (path) => replayed.push(path)),
        ).not.toThrow();
        expect(replayed).toEqual(["first.rs", "second.rs"]);
        expect(() =>
            assertBoundVerifierBytesUnchanged(accepted, current, () => {
                throw new Error("mutation survived");
            }),
        ).toThrow("mutation survived");
        replayed.length = 0;
        expect(() =>
            assertBoundVerifierBytesUnchanged(accepted, { "first.rs": "c" }, (path) =>
                replayed.push(path),
            ),
        ).toThrow(/no longer bind accepted verifiers/);
        expect(replayed).toEqual([]);
    });

    it("accepts unchanged or newly bound verifier bytes and blocks changed or dropped bindings", () => {
        const bound = { "tests/verifier.test.ts": "a".repeat(64) };
        expect(() => assertBoundVerifierBytesUnchanged(bound, { ...bound })).not.toThrow();
        // An unbound base has no recorded bytes to compare.
        expect(() =>
            assertBoundVerifierBytesUnchanged({}, { "tests/verifier.test.ts": "b".repeat(64) }),
        ).not.toThrow();
        expect(() =>
            assertBoundVerifierBytesUnchanged(bound, { "tests/verifier.test.ts": "b".repeat(64) }),
        ).toThrow(/changed without recorded mutation replay support/);
        // Otherwise deleting the record exempts the verifier from replay.
        expect(() => assertBoundVerifierBytesUnchanged(bound, {})).toThrow(
            /no longer bind accepted verifiers/,
        );
    });
});

describe("catalog-bound executable verifier gate", () => {
    const scenario = "src/incident-pool/scenarios/audit-memory-search.ts";
    const key = `packages/e2e-tests/${scenario}`;

    it("covers every executable module the committed catalog binds", () => {
        const files = boundVerifierFiles(committedCatalog());
        expect(files.length).toBeGreaterThan(0);
        for (const file of files) {
            expect(file.startsWith("src/")).toBe(true);
        }
        expect(files.some((file) => file.startsWith("src/incident-pool/scenarios/"))).toBe(true);
        // The A1/A3 verdicts come from `analyzePasses`, so the gate freezes the oracle module with the bound scenario module.
        expect(files).toContain("src/cache-analysis.ts");
        expect(builtinIncidentCaseRegistry().size).toBe(2);
    });

    it("accepts unchanged or newly bound module bytes and blocks changed or dropped bindings", () => {
        const bound = { [key]: "a".repeat(64) };
        expect(() => assertCatalogBoundVerifierBytesUnchanged(bound, { ...bound })).not.toThrow();
        // The accepted base has no bytes for a newly bound module to drift from.
        expect(() =>
            assertCatalogBoundVerifierBytesUnchanged({}, { [key]: "b".repeat(64) }),
        ).not.toThrow();
        expect(() =>
            assertCatalogBoundVerifierBytesUnchanged(bound, { [key]: "b".repeat(64) }),
        ).toThrow(/changed without recorded replay support/);
        // Otherwise removing the binding exempts the module from the gate.
        expect(() => assertCatalogBoundVerifierBytesUnchanged(bound, {})).toThrow(
            /no longer binds accepted executable verifiers/,
        );
    });
});

describe("per-variant binding gate", () => {
    const module = "src/incident-pool/scenarios/source-linked-regressions.ts";

    it("derives one binding per committed executable variant, including oracle dependencies", () => {
        const bindings = catalogBindings(committedCatalog());
        expect(Object.keys(bindings).sort()).toEqual([
            "var-parity-a1-pure-defer-stability",
            "var-parity-a3-ctx-reduce-survival",
        ]);
        for (const binding of Object.values(bindings)) {
            expect(binding).toContain(`${module}#drive`);
            expect(binding).toContain(`${module}#verify`);
            expect(binding).toContain("src/cache-analysis.ts");
        }
    });

    it("blocks rebinding an accepted variant even when every previously bound path keeps its bytes", () => {
        // Rebinding A1 to a new module leaves A3 holding the accepted paths, so the path-set gates see nothing.
        const accepted = {
            "var-a1": `${module}#driveA1\n${module}#verifyA1\nsrc/cache-analysis.ts`,
            "var-a3": `${module}#driveA3\n${module}#verifyA3\nsrc/cache-analysis.ts`,
        };
        const rebound = {
            ...accepted,
            "var-a1":
                "src/incident-pool/scenarios/other.ts#driveA1\nsrc/incident-pool/scenarios/other.ts#verifyA1\nsrc/cache-analysis.ts",
        };
        expect(() => assertCatalogBindingsUnchanged(accepted, rebound)).toThrow(
            /rebound their verifier without recorded replay support: var-a1/,
        );
        const resymboled = {
            ...accepted,
            "var-a1": accepted["var-a1"].replace("verifyA1", "verifyA1Loose"),
        };
        expect(() => assertCatalogBindingsUnchanged(accepted, resymboled)).toThrow(/var-a1/);
        const droppedDependency = {
            ...accepted,
            "var-a1": `${module}#driveA1\n${module}#verifyA1`,
        };
        expect(() => assertCatalogBindingsUnchanged(accepted, droppedDependency)).toThrow(/var-a1/);
    });

    it("blocks dropping an accepted executable variant's binding and accepts a new variant", () => {
        const accepted = { "var-a1": "a" };
        expect(() => assertCatalogBindingsUnchanged(accepted, {})).toThrow(
            /accepted executable variants no longer bind a verifier: var-a1/,
        );
        expect(() =>
            assertCatalogBindingsUnchanged(accepted, { ...accepted, "var-new": "b" }),
        ).not.toThrow();
        expect(() => assertCatalogBindingsUnchanged({}, { "var-new": "b" })).not.toThrow();
    });
});

describe("per-record mutation binding gate", () => {
    it("derives one binding per committed evidence record with its verifier, fixture, and command", () => {
        const bindings = mutationBindings(loadMutationEvidence());
        expect(Object.keys(bindings).sort()).toEqual([
            "ev-dg-1-one-byte-input",
            "ev-dg-2-one-byte-input",
            "ev-dg-3-one-byte-input",
        ]);
        for (const binding of Object.values(bindings)) {
            expect(binding).toContain("crates/daemon/src/differential_goldens.rs");
            expect(binding).toContain("crates/daemon/testdata/differential-golden.json");
            expect(binding).toContain("cargo test -p daemon --lib dg_goldens_");
        }
    });

    it("blocks rebinding one record while its siblings keep the accepted path", () => {
        const golden =
            "crates/daemon/src/differential_goldens.rs\ncrates/daemon/testdata/differential-golden.json\ncargo test -p daemon --lib dg_goldens_match_ts_wire_surface_and_gate_labels --locked";
        const accepted = { "ev-dg-1": golden, "ev-dg-2": golden, "ev-dg-3": golden };
        const rebound = {
            ...accepted,
            "ev-dg-1":
                "crates/daemon/src/weaker.rs\ncargo test -p daemon --lib weaker::tests::always_green --locked",
        };
        expect(() => assertMutationBindingsUnchanged(accepted, rebound)).toThrow(
            /rebound their verifier without recorded replay support: ev-dg-1/,
        );
        const recommanded = { ...accepted, "ev-dg-2": golden.replace("--locked", "") };
        expect(() => assertMutationBindingsUnchanged(accepted, recommanded)).toThrow(/ev-dg-2/);
        expect(() =>
            assertMutationBindingsUnchanged(accepted, { "ev-dg-2": golden, "ev-dg-3": golden }),
        ).toThrow(/accepted mutation records vanished: ev-dg-1/);
        expect(() =>
            assertMutationBindingsUnchanged(accepted, { ...accepted, "ev-new": "x" }),
        ).not.toThrow();
        expect(() => assertMutationBindingsUnchanged({}, { "ev-new": "x" })).not.toThrow();
    });
});

// Use the committed append-only ledger rather than a second catalog or fixture mapping.
function replayFixture() {
    const current = loadHistorySnapshot(resolve(E2E_ROOT, "incidents"), "accepted");
    const accepted = structuredClone(current);
    const catalog = committedCatalog();
    const events = validateIncidentHistory(current).events;
    const previous = events.slice(0, -2);
    accepted.adjudicationLines = current.adjudicationLines.slice(0, -2);
    for (const variant of catalog.families.flatMap((family) => family.variants)) {
        variant.semantic_revision = {
            id: `${variant.semantic_revision.id}-accepted`,
            fingerprint: previous
                .filter((event) => event.identity === variant.id && event.kind === "baseline")
                .at(-1)!.semantic_fingerprint!,
        };
    }
    accepted.catalogText = JSON.stringify(catalog);
    const currentDigests = boundVerifierDigests(committedCatalog());
    const acceptedDigests = { ...currentDigests };
    acceptedDigests["packages/e2e-tests/src/incident-pool/scenarios/source-linked-regressions.ts"] =
        "0".repeat(64);
    acceptedDigests["packages/e2e-tests/src/cache-analysis.ts"] = "0".repeat(64);
    return { accepted, current, acceptedDigests, currentDigests };
}

const replayPass = () => ({ status: 0, stdout: "", stderr: " 13 pass\n 0 fail\n" });

describe("catalog revision replay admission", () => {
    it("accepts complete preserved history only after serial replay of driver and changed oracle suites", () => {
        const { accepted, current, acceptedDigests, currentDigests } = replayFixture();
        expect(
            compareWithAcceptedSnapshot(accepted, current).candidate.events.length,
        ).toBeGreaterThan(15);
        const calls: string[][] = [];
        replayCatalogVerifierChanges(
            accepted,
            current,
            acceptedDigests,
            currentDigests,
            (args, cwd) => {
                expect(cwd).toBe(E2E_ROOT);
                calls.push(args);
                return replayPass();
            },
        );
        expect(calls).toEqual([
            ["test", "./src/cache-analysis.test.ts", "--max-concurrency", "1"],
            [
                "test",
                "./src/incident-pool/scenarios/source-linked-regressions.test.ts",
                "--max-concurrency",
                "1",
            ],
        ]);
    });

    it("rejects byte drift with no appended baseline, even when the fingerprint did not change", () => {
        const { current, acceptedDigests, currentDigests } = replayFixture();
        expect(() =>
            replayCatalogVerifierChanges(
                current,
                current,
                acceptedDigests,
                currentDigests,
                replayPass,
            ),
        ).toThrow(/requires an appended fingerprint-bound baseline/);
    });

    // History validation names a missing baseline or a reused revision id when the prior baseline's fingerprint differs; when the revision kept its fingerprint, the replay gate names both.
    const EXPECTED_PROBE_MESSAGE = {
        missing:
            /is not bound by a fingerprint-matching baseline adjudication|requires an appended fingerprint-bound baseline/,
        reused: /changed its semantic fingerprint while reusing revision id|requires an appended fingerprint-bound baseline and distinct semantic revision/,
        fingerprint: /does not match the registered case/,
    } as const;

    it("rejects a baseline missing for one affected variant, a reused revision, or an invented fingerprint", () => {
        for (const failure of ["missing", "reused", "fingerprint"] as const) {
            const { accepted, current, acceptedDigests, currentDigests } = replayFixture();
            const before = parseIncidentCatalog(JSON.parse(accepted.catalogText));
            const after = parseIncidentCatalog(JSON.parse(current.catalogText));
            const a1 = after.families[0]!.variants[0]!;
            if (failure === "missing") {
                current.adjudicationLines.splice(-2, 1);
                before.families[0]!.variants[0] = structuredClone(a1);
                accepted.catalogText = JSON.stringify(before);
            } else if (failure === "reused") {
                a1.semantic_revision.id = before.families[0]!.variants[0]!.semantic_revision.id;
            } else {
                a1.semantic_revision.fingerprint = "f".repeat(64);
                const event = JSON.parse(current.adjudicationLines.at(-2)!);
                event.semantic_fingerprint = a1.semantic_revision.fingerprint;
                current.adjudicationLines[current.adjudicationLines.length - 2] =
                    JSON.stringify(event);
            }
            current.catalogText = JSON.stringify(after);
            expect(() =>
                replayCatalogVerifierChanges(
                    accepted,
                    current,
                    acceptedDigests,
                    currentDigests,
                    replayPass,
                ),
            ).toThrow(EXPECTED_PROBE_MESSAGE[failure]);
        }
    });

    it("rejects removed checks, variants, oracle bindings, and accepted verifier paths", () => {
        for (const failure of ["check", "variant", "binding", "path"]) {
            const { accepted, current, acceptedDigests, currentDigests } = replayFixture();
            const catalog = parseIncidentCatalog(JSON.parse(current.catalogText));
            const variant = catalog.families[0]!.variants[0]!;
            if (failure === "check") variant.normative_checks.pop();
            if (failure === "variant") catalog.families[0]!.variants.pop();
            if (failure === "binding") variant.verifier_binding!.oracle_dependencies.pop();
            if (failure === "path")
                delete currentDigests["packages/e2e-tests/src/cache-analysis.ts"];
            current.catalogText = JSON.stringify(catalog);
            expect(() =>
                replayCatalogVerifierChanges(
                    accepted,
                    current,
                    acceptedDigests,
                    currentDigests,
                    replayPass,
                ),
            ).toThrow(/removed an accepted|unknown identity|no longer binds accepted/);
        }
    });

    it("rejects missing required driver suites and historical prefix edits before replay", () => {
        for (const failure of ["suite", "prefix"]) {
            const { accepted, current, acceptedDigests, currentDigests } = replayFixture();
            if (failure === "suite") {
                const catalog = parseIncidentCatalog(JSON.parse(current.catalogText));
                catalog.families[0]!.variants[0]!.verifier_binding!.driver =
                    "src/incident-pool/scenarios/missing.ts#drive";
                current.catalogText = JSON.stringify(catalog);
            } else {
                current.adjudicationLines[0] += " ";
            }
            expect(() =>
                replayCatalogVerifierChanges(
                    accepted,
                    current,
                    acceptedDigests,
                    currentDigests,
                    replayPass,
                ),
            ).toThrow(/missing required regression suite|ledger prefix changed/);
        }
    });

    it("fails closed on replay failure, no tests, skipped tests, or command failure despite an approved baseline", () => {
        for (const result of [
            { status: 1, stdout: "", stderr: " 1 pass\n 1 fail\n" },
            { status: 0, stdout: "", stderr: " 0 pass\n 0 fail\n" },
            { status: 0, stdout: "", stderr: " 1 pass\n 1 skip\n 0 fail\n" },
            { status: 0, stdout: "", stderr: "" },
        ]) {
            const { accepted, current, acceptedDigests, currentDigests } = replayFixture();
            expect(() =>
                replayCatalogVerifierChanges(
                    accepted,
                    current,
                    acceptedDigests,
                    currentDigests,
                    () => result,
                ),
            ).toThrow(/replay failed or executed no successful tests/);
        }
        const { accepted, current, acceptedDigests, currentDigests } = replayFixture();
        let calls = 0;
        expect(() =>
            replayCatalogVerifierChanges(accepted, current, acceptedDigests, currentDigests, () => {
                calls++;
                return calls === 1
                    ? replayPass()
                    : { status: 1, stdout: "", stderr: " 0 pass\n 1 fail\n" };
            }),
        ).toThrow(/replay failed/);
        expect(calls).toBe(2);
        expect(() =>
            replayCatalogVerifierChanges(accepted, current, acceptedDigests, currentDigests, () => {
                throw new Error("spawn failed");
            }),
        ).toThrow("spawn failed");
    });

    it("rejects digests or bindings changed during replay", () => {
        for (const failure of ["digest", "binding"]) {
            const { accepted, current, acceptedDigests, currentDigests } = replayFixture();
            expect(() =>
                replayCatalogVerifierChanges(
                    accepted,
                    current,
                    acceptedDigests,
                    currentDigests,
                    () => {
                        if (failure === "digest")
                            currentDigests["packages/e2e-tests/src/cache-analysis.ts"] = "f".repeat(
                                64,
                            );
                        else {
                            const catalog = parseIncidentCatalog(JSON.parse(current.catalogText));
                            catalog.families[0]!.variants[0]!.verifier_binding!.driver += "Changed";
                            current.catalogText = JSON.stringify(catalog);
                        }
                        return replayPass();
                    },
                ),
            ).toThrow(/inputs changed during replay/);
        }
    });
});

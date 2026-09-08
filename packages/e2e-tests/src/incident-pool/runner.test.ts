import { afterAll, describe, expect, it } from "bun:test";
import { existsSync, mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    ADJUDICATION_EVENT_SCHEMA,
    type AdjudicationEvent,
    type IncidentCatalog,
    type IncidentVariant,
    parseIncidentCatalog,
} from "./contract";
import { replayAdjudicationLedger } from "./history";
import {
    adaptBoundSymbol,
    builtinIncidentCaseRegistry,
    type IncidentCaseRegistry,
    implementationBundleDigest,
    type JsonValue,
    ledgerFingerprint,
    type RegisteredIncidentCase,
    registerIncidentCase,
    semanticFingerprint,
    type VerifierCheck,
    validateRegistryCatalogCorrespondence,
} from "./registry";
import {
    buildIncidentReport,
    buildScheduledIncidentReport,
    computeSelectedSetDigest,
    type IncidentCaseResult,
    incidentPoolExitCode,
    parseIncidentReport,
    parseScheduledIncidentReport,
    publishIncidentReport,
    publishScheduledIncidentReport,
    readIncidentReport,
    readScheduledIncidentReport,
    scheduledIncidentExitCode,
    scoredBaselineMismatches,
    unexpectedIncompleteResults,
} from "./report";
import {
    buildRunSnapshot,
    type RunCaseOptions,
    type RunSnapshot,
    runCaseInIsolation,
    runIncidentPool,
    type SelectedCase,
    unavailableCaseResult,
} from "./runner";
import {
    assertLoopbackProviderEndpoints,
    buildCaseEnv,
    CASE_ENV_ALLOWLIST,
    createCaseWorkspace,
    DiagnosticSink,
    destroyCaseWorkspace,
    isLoopbackUrl,
} from "./support/case-workspace";

const HEX = (fill: string): string => fill.repeat(64);
const FAIL_SIGNATURE = HEX("d");
const IMPL_DIGEST_GREEN = HEX("1");
const IMPL_DIGEST_TWO = HEX("2");
const IMPL_DIGEST_BLOCKED = HEX("3");

const testRoot = mkdtempSync(join(tmpdir(), "incident-runner-test-"));
afterAll(() => rmSync(testRoot, { recursive: true, force: true }));

interface VariantSpec {
    id: string;
    normativeChecks?: string[];
    blockedBy?: string[];
    bindingStatus?: "declared" | "live";
}

function rawVariant(spec: VariantSpec): Record<string, unknown> {
    const contract = {
        lane: "green" as const,
        applicability: { harness: "rust" as const, omitted: [] },
        normative_checks: spec.normativeChecks ?? ["check-holds", "check-durable"],
        blocked_by: spec.blockedBy ?? [],
    };
    return {
        id: spec.id,
        lane: contract.lane,
        source_claims: ["claim-demo-one"],
        applicability: contract.applicability,
        semantic_revision: {
            id: `rev-${spec.id.slice(4)}`,
            fingerprint: semanticFingerprint(contract, {}),
        },
        normative_checks: contract.normative_checks,
        verifier_binding: {
            driver: "demo#caseDriver",
            verifier: "demo#caseVerifier",
            binding_status: spec.bindingStatus ?? "live",
            invalid_state_evidence: ["crafted stale-plus-current coexistence"],
            oracle_dependencies: [],
        },
        blocked_by: contract.blocked_by,
        evidence_refs: [],
    };
}

function fixtureCatalog(): IncidentCatalog {
    return parseIncidentCatalog({
        schema: "incident-catalog/v1",
        families: [
            {
                id: "fam-demo",
                title: "Demo incident family",
                source_claims: ["claim-demo-one"],
                variants: [
                    rawVariant({ id: "var-green-one" }),
                    rawVariant({ id: "var-green-two" }),
                    rawVariant({
                        id: "var-blocked-one",
                        blockedBy: ["var-green-two"],
                    }),
                ],
            },
        ],
    });
}

function baselineEvent(eventId: string, variant: IncidentVariant): AdjudicationEvent {
    return {
        schema: ADJUDICATION_EVENT_SCHEMA,
        event_id: eventId,
        identity: variant.id,
        seq: 1,
        kind: "baseline",
        baseline_verdict: "green",
        semantic_fingerprint: variant.semantic_revision.fingerprint,
        expected_failed_checks: null,
        observation_signature: null,
        rationale: "reviewed",
        source_revision: "audit-2026-08-24",
        supersedes: null,
    };
}

interface Fixture {
    catalog: IncidentCatalog;
    events: AdjudicationEvent[];
    adjudicationLines: string[];
    implementationDigests: Map<string, string>;
}

function fixture(): Fixture {
    const catalog = fixtureCatalog();
    const variants = new Map(catalog.families[0]!.variants.map((variant) => [variant.id, variant]));
    const events = [
        baselineEvent("adj-green-one", variants.get("var-green-one")!),
        baselineEvent("adj-green-two", variants.get("var-green-two")!),
        baselineEvent("adj-blocked-one", variants.get("var-blocked-one")!),
    ];
    return {
        catalog,
        events,
        adjudicationLines: events.map((event) => JSON.stringify(event)),
        implementationDigests: new Map([
            ["var-green-one", IMPL_DIGEST_GREEN],
            ["var-green-two", IMPL_DIGEST_TWO],
            ["var-blocked-one", IMPL_DIGEST_BLOCKED],
        ]),
    };
}

function snapshotFor(data: Fixture = fixture()): RunSnapshot {
    return buildRunSnapshot({
        catalog: data.catalog,
        ledger: replayAdjudicationLedger(data.events),
        adjudicationLines: data.adjudicationLines,
        harness: "rust",
        lanes: ["green"],
        implementationDigests: data.implementationDigests,
    });
}

function selectedCase(snapshot: RunSnapshot, variantId: string): SelectedCase {
    const found = snapshot.selected.find((entry) => entry.variantId === variantId);
    if (!found) throw new Error(`fixture variant ${variantId} was not selected`);
    return found;
}

const CHILD_PRELUDE = `
import { appendFileSync } from "node:fs";
const env = process.env;
const envelopePath = env.EIDNARA_INCIDENT_ENVELOPE_PATH;
function envelopeFromEnv(overrides) {
    return Object.assign(
        {
            schema: "incident-case-envelope/v1",
            run_nonce: env.EIDNARA_INCIDENT_RUN_NONCE,
            variant_id: env.EIDNARA_INCIDENT_VARIANT_ID,
            semantic_fingerprint: env.EIDNARA_INCIDENT_SEMANTIC_FINGERPRINT,
            implementation_digest: env.EIDNARA_INCIDENT_IMPLEMENTATION_DIGEST,
            ledger_fingerprint: env.EIDNARA_INCIDENT_LEDGER_FINGERPRINT,
            baseline_event_id: env.EIDNARA_INCIDENT_BASELINE_EVENT_ID,
            preconditions: "satisfied",
            precondition_reason: null,
            blocked_by: [],
            verdict: "pass",
            failed_checks: [],
            observation_signature: null,
        },
        overrides,
    );
}
function sendRaw(text) {
    appendFileSync(envelopePath, text);
}
function sendEnvelope(overrides = {}) {
    sendRaw(JSON.stringify(envelopeFromEnv(overrides)) + "\\n");
}
`;

let childScriptCounter = 0;
function childScript(body: string): string {
    const path = join(testRoot, `fake-child-${childScriptCounter++}.ts`);
    writeFileSync(path, `${CHILD_PRELUDE}\n${body}\n`);
    return path;
}

async function runFakeChild(
    snapshot: RunSnapshot,
    selected: SelectedCase,
    body: string,
    options: Partial<RunCaseOptions> = {},
): ReturnType<typeof runCaseInIsolation> {
    return runCaseInIsolation(snapshot, selected, {
        argv: [process.execPath, childScript(body)],
        timeoutMs: 15_000,
        workspaceParentDir: testRoot,
        ...options,
    });
}

describe("semantic and implementation fingerprints", () => {
    const contract = {
        lane: "green" as const,
        applicability: { harness: "rust" as const, omitted: [] },
        normative_checks: ["check-a", "check-b"],
        blocked_by: [],
    };

    it("is insensitive to formatting and key order of structured data", () => {
        const fixturesA = JSON.parse('{"seed": 7, "topic": "recall"}') as Record<string, unknown>;
        const fixturesB = JSON.parse('{\n    "topic": "recall",\n    "seed": 7\n}') as Record<
            string,
            unknown
        >;
        expect(semanticFingerprint(contract, fixturesA)).toBe(
            semanticFingerprint(contract, fixturesB),
        );
    });

    it("changes when any owning semantic input changes", () => {
        const base = semanticFingerprint(contract, { seed: 7 });
        expect(
            semanticFingerprint({ ...contract, lane: "adjudication-only" }, { seed: 7 }),
        ).not.toBe(base);
        expect(
            semanticFingerprint({ ...contract, normative_checks: ["check-a"] }, { seed: 7 }),
        ).not.toBe(base);
        expect(semanticFingerprint({ ...contract, blocked_by: ["var-dep"] }, { seed: 7 })).not.toBe(
            base,
        );
        expect(semanticFingerprint({ ...contract, applicability: null }, { seed: 7 })).not.toBe(
            base,
        );
        expect(semanticFingerprint(contract, { seed: 8 })).not.toBe(base);
    });

    it("byte-hashes the explicit implementation file list, order-insensitively", () => {
        const root = mkdtempSync(join(testRoot, "impl-"));
        writeFileSync(join(root, "driver.ts"), "export const a = 1;\n");
        writeFileSync(join(root, "verifier.ts"), "export const b = 2;\n");
        const digest = implementationBundleDigest(root, ["driver.ts", "verifier.ts"]);
        expect(implementationBundleDigest(root, ["verifier.ts", "driver.ts"])).toBe(digest);
        writeFileSync(join(root, "driver.ts"), "export const a = 2;\n");
        expect(implementationBundleDigest(root, ["driver.ts", "verifier.ts"])).not.toBe(digest);
    });

    it("rejects non-root-confined implementation files", () => {
        const root = mkdtempSync(join(testRoot, "impl-confine-"));
        expect(() => implementationBundleDigest(root, ["../escape.ts"])).toThrow(/root-confined/);
        expect(() => implementationBundleDigest(root, ["/abs/path.ts"])).toThrow(/root-confined/);
        expect(() => implementationBundleDigest(root, [])).toThrow(/must not be empty/);
    });

    it("changes the ledger fingerprint when a baseline event is appended", () => {
        const data = fixture();
        const base = ledgerFingerprint(data.adjudicationLines);
        expect(ledgerFingerprint([...data.adjudicationLines, '{"appended":true}'])).not.toBe(base);
        expect(ledgerFingerprint(data.adjudicationLines)).toBe(base);
    });
});

function caseDriver(): Promise<JsonValue> {
    return Promise.resolve(null);
}

function caseVerifier(): VerifierCheck[] {
    return [{ id: "check-holds", passed: true }];
}

function registeredCase(
    variantId: string,
    files: string[] = ["packages/e2e-tests/package.json"],
): RegisteredIncidentCase {
    return {
        variantId,
        implementationFiles: files,
        fixtures: {},
        driver: caseDriver,
        normalizer: (raw) => raw,
        precondition: () => ({ satisfied: true }),
        verifier: caseVerifier,
        binding: { driver: caseDriver, verifier: caseVerifier },
    };
}

describe("case registry", () => {
    it("requires every live executable variant and rejects extras", () => {
        const catalog = fixtureCatalog();
        const registry: IncidentCaseRegistry = new Map();
        expect(() => validateRegistryCatalogCorrespondence(registry, catalog)).toThrow(
            /has no registered case/,
        );
        for (const variant of catalog.families[0]!.variants)
            registerIncidentCase(registry, registeredCase(variant.id));
        validateRegistryCatalogCorrespondence(registry, catalog);
        registerIncidentCase(registry, registeredCase("var-orphan"));
        expect(() => validateRegistryCatalogCorrespondence(registry, catalog)).toThrow(
            /no executable catalog variant/,
        );
    });

    it("rejects a registered case whose catalog binding is still declared", () => {
        const catalog = parseIncidentCatalog({
            schema: "incident-catalog/v1",
            families: [
                {
                    id: "fam-demo",
                    title: "Demo incident family",
                    source_claims: ["claim-demo-one"],
                    variants: [
                        rawVariant({
                            id: "var-green-one",
                            bindingStatus: "declared",
                        }),
                    ],
                },
            ],
        });
        const registry: IncidentCaseRegistry = new Map();
        registerIncidentCase(registry, registeredCase("var-green-one"));
        expect(() => validateRegistryCatalogCorrespondence(registry, catalog)).toThrow(
            /requires a live catalog verifier binding/,
        );
    });

    it("rejects a registered case bound to the wrong module symbol", () => {
        const catalog = fixtureCatalog();
        const registry: IncidentCaseRegistry = new Map();
        function otherVerifier(): VerifierCheck[] {
            return [{ id: "check-holds", passed: true }];
        }
        for (const variant of catalog.families[0]!.variants) {
            const entry = registeredCase(variant.id);
            registerIncidentCase(
                registry,
                variant.id === "var-green-two"
                    ? {
                          ...entry,
                          verifier: otherVerifier,
                          binding: {
                              driver: caseDriver,
                              verifier: otherVerifier,
                          },
                      }
                    : entry,
            );
        }
        expect(() => validateRegistryCatalogCorrespondence(registry, catalog)).toThrow(
            /binds caseDriver\/caseVerifier but carries/,
        );
    });

    it("rejects an executable callback that is not bound to the catalog symbol", () => {
        const catalog = fixtureCatalog();
        const registry: IncidentCaseRegistry = new Map();
        function impostorVerifier(): VerifierCheck[] {
            return [{ id: "check-holds", passed: true }];
        }
        // The pool executes `verifier`, so structural validation rejects metadata-only swaps that publish false results.
        for (const variant of catalog.families[0]!.variants) {
            const entry = registeredCase(variant.id);
            registerIncidentCase(
                registry,
                variant.id === "var-green-two" ? { ...entry, verifier: impostorVerifier } : entry,
            );
        }
        expect(() => validateRegistryCatalogCorrespondence(registry, catalog)).toThrow(
            /executes callbacks that are not bound to caseDriver\/caseVerifier/,
        );
    });

    it("rejects a wrapper that names the bound symbol but calls another oracle", () => {
        const catalog = fixtureCatalog();
        const registry: IncidentCaseRegistry = new Map();
        function impostorVerifier(): VerifierCheck[] {
            return [{ id: "check-holds", passed: true }];
        }
        for (const variant of catalog.families[0]!.variants) {
            const entry = registeredCase(variant.id);
            registerIncidentCase(
                registry,
                variant.id === "var-green-two"
                    ? {
                          ...entry,
                          // Source scanning cannot prove coupling when a wrapper references the bound symbol without calling it.
                          verifier: () => {
                              const unused = caseVerifier;
                              void unused;
                              return impostorVerifier();
                          },
                      }
                    : entry,
            );
        }
        expect(() => validateRegistryCatalogCorrespondence(registry, catalog)).toThrow(
            /executes callbacks that are not bound to caseDriver\/caseVerifier/,
        );
    });

    it("rejects an undeclared wrapper even when it does delegate", () => {
        const catalog = fixtureCatalog();
        const registry: IncidentCaseRegistry = new Map();
        // Delegation is indistinguishable from a callback that invokes `impostorVerifier()` by any property the validator reads, so adaptations must be declared through `adaptBoundSymbol`.
        for (const variant of catalog.families[0]!.variants) {
            const entry = registeredCase(variant.id);
            registerIncidentCase(registry, {
                ...entry,
                verifier: () => caseVerifier(),
            });
        }
        expect(() => validateRegistryCatalogCorrespondence(registry, catalog)).toThrow(
            /executes callbacks that are not bound to caseDriver\/caseVerifier/,
        );
    });

    it("accepts an adapter declared against the bound symbol", () => {
        const catalog = fixtureCatalog();
        const registry: IncidentCaseRegistry = new Map();
        // `adaptBoundSymbol` adapts the bound symbol to source-linked verifiers' normalized observations and records its source function.
        for (const variant of catalog.families[0]!.variants) {
            const entry = registeredCase(variant.id);
            registerIncidentCase(registry, {
                ...entry,
                verifier: adaptBoundSymbol(caseVerifier, (inner) => () => inner()),
            });
        }
        expect(() => validateRegistryCatalogCorrespondence(registry, catalog)).not.toThrow();
    });

    it("registers the builtin cases 1:1 against the committed catalog", () => {
        const registry = builtinIncidentCaseRegistry();
        expect(registry.size).toBe(2);
        for (const [variantId, entry] of registry) {
            expect(entry.variantId).toBe(variantId);
        }
    });

    it("rejects a registration whose fixtures drift from the catalog fingerprint", () => {
        const catalog = fixtureCatalog();
        const registry: IncidentCaseRegistry = new Map();
        for (const variant of catalog.families[0]!.variants) {
            registerIncidentCase(registry, {
                ...registeredCase(variant.id),
                fixtures: variant.id === "var-green-two" ? { drifted: true } : {},
            });
        }
        expect(() => validateRegistryCatalogCorrespondence(registry, catalog)).toThrow(
            /new semantic revision/,
        );
    });

    it("rejects duplicate registrations and unconfined file lists", () => {
        const registry: IncidentCaseRegistry = new Map();
        registerIncidentCase(registry, registeredCase("var-green-two"));
        expect(() => registerIncidentCase(registry, registeredCase("var-green-two"))).toThrow(
            /duplicate/,
        );
        expect(() =>
            registerIncidentCase(registry, registeredCase("var-x", ["../outside.ts"])),
        ).toThrow(/root-confined/);
    });
});

describe("run snapshot selection", () => {
    it("selects every applicable executable variant", () => {
        const snapshot = snapshotFor();
        expect(snapshot.selected.map((entry) => entry.variantId).sort()).toEqual([
            "var-blocked-one",
            "var-green-one",
            "var-green-two",
        ]);
        expect(snapshot.excluded).toEqual([]);
        expect(snapshot.familyCount).toBe(1);
        expect(snapshot.variantCount).toBe(3);
    });

    it("excludes a variant retired by adjudication", () => {
        const data = fixture();
        const events: AdjudicationEvent[] = [
            ...data.events,
            {
                schema: ADJUDICATION_EVENT_SCHEMA,
                event_id: "adj-green-two-retire",
                identity: "var-green-two",
                seq: 2,
                kind: "retirement",
                baseline_verdict: null,
                semantic_fingerprint: null,
                expected_failed_checks: null,
                observation_signature: null,
                rationale: "reviewed: incident retired",
                source_revision: "audit-2026-08-24",
                supersedes: null,
            },
        ];
        const snapshot = buildRunSnapshot({
            catalog: data.catalog,
            ledger: replayAdjudicationLedger(events),
            adjudicationLines: events.map((event) => JSON.stringify(event)),
            harness: "rust",
            lanes: ["green"],
            implementationDigests: data.implementationDigests,
        });
        expect(snapshot.selected.map((entry) => entry.variantId)).not.toContain("var-green-two");
        expect(snapshot.excluded.find((entry) => entry.variantId === "var-green-two")?.reason).toBe(
            "variant was retired by adjudication",
        );
    });

    it("binds each selected case to its reviewed baseline event", () => {
        const snapshot = snapshotFor();
        const two = selectedCase(snapshot, "var-green-two");
        expect(two.baselineEventId).toBe("adj-green-two");
        expect(two.baselineVerdict).toBe("green");
        expect(two.expectedFailedChecks).toEqual([]);
        expect(two.expectedObservationSignature).toBeNull();
    });

    it("filters by exact requested variants", () => {
        const data = fixture();
        const snapshot = buildRunSnapshot({
            catalog: data.catalog,
            ledger: replayAdjudicationLedger(data.events),
            adjudicationLines: data.adjudicationLines,
            harness: "rust",
            lanes: ["green"],
            variantIds: ["var-green-one", "var-green-two"],
            implementationDigests: data.implementationDigests,
        });
        expect(snapshot.selected.map((entry) => entry.variantId).sort()).toEqual([
            "var-green-one",
            "var-green-two",
        ]);
    });

    it("rejects a requested variant that no filter selected", () => {
        const data = fixture();
        const request = (variantIds: string[]) => () =>
            buildRunSnapshot({
                catalog: data.catalog,
                ledger: replayAdjudicationLedger(data.events),
                adjudicationLines: data.adjudicationLines,
                harness: "rust",
                lanes: ["green"],
                variantIds,
                implementationDigests: data.implementationDigests,
            });
        // An unknown id must fail explicitly because valid ids can otherwise leave the selection nonempty and report green for a subset.
        expect(request(["var-green-one", "var-typo-one"])).toThrow(
            /var-typo-one \(unknown variant id\)/,
        );
        expect(request(["var-green-one"])).not.toThrow();
    });

    it("fails hard on a missing baseline or registered case digest", () => {
        const data = fixture();
        const noBaseline = data.events.filter((event) => event.identity !== "var-green-two");
        expect(() =>
            buildRunSnapshot({
                catalog: data.catalog,
                ledger: replayAdjudicationLedger(noBaseline),
                adjudicationLines: noBaseline.map((event) => JSON.stringify(event)),
                harness: "rust",
                lanes: ["green"],
                implementationDigests: data.implementationDigests,
            }),
        ).toThrow(/no reviewed baseline/);
        data.implementationDigests.delete("var-green-one");
        expect(() => snapshotFor(data)).toThrow(/no registered case digest/);
    });
});

describe("isolated case execution", () => {
    const snapshot = snapshotFor();
    const green = selectedCase(snapshot, "var-green-one");
    const two = selectedCase(snapshot, "var-green-two");
    const blocked = selectedCase(snapshot, "var-blocked-one");

    it("marks green baselines expected_green on pass and regression on assertion_fail", async () => {
        const pass = await runFakeChild(snapshot, green, "sendEnvelope();");
        expect(pass.result.baseline_comparison).toBe("expected_green");
        const regression = await runFakeChild(
            snapshot,
            green,
            `sendEnvelope({ verdict: "assertion_fail", failed_checks: ["check-holds"], observation_signature: ${JSON.stringify(HEX("8"))} });`,
        );
        expect(regression.result.behavioral_verdict).toBe("assertion_fail");
        expect(regression.result.baseline_comparison).toBe("regression");
    }, 20_000);

    it("classifies unhealthy children as not_evaluated and unscored (AE2)", async () => {
        const crashThrow = await runFakeChild(
            snapshot,
            two,
            'throw new Error("driver blew up before observations");',
        );
        expect(crashThrow.result.run_health).toBe("crash");
        const forgedStdout = await runFakeChild(
            snapshot,
            two,
            "console.log(JSON.stringify(envelopeFromEnv({}))); process.exit(0);",
        );
        expect(forgedStdout.result.run_health).toBe("crash");
        expect(forgedStdout.result.reason_code).toBe("exited_without_envelope");
        const malformedJson = await runFakeChild(snapshot, two, 'sendRaw("not json at all\\n");');
        expect(malformedJson.result.run_health).toBe("malformed");
        expect(malformedJson.result.reason_code).toBe("invalid_envelope");
        const stale = await runFakeChild(
            snapshot,
            two,
            `sendEnvelope({ semantic_fingerprint: ${JSON.stringify(HEX("7"))} });`,
        );
        expect(stale.result.run_health).toBe("malformed");
        expect(stale.result.reason_code).toBe("snapshot_mismatch");
        for (const result of [
            crashThrow.result,
            forgedStdout.result,
            malformedJson.result,
            stale.result,
        ]) {
            expect(result.behavioral_verdict).toBe("not_evaluated");
            expect(result.baseline_comparison).toBe("unscored");
        }
    }, 30_000);

    it("rejects a wrong run nonce or foreign baseline event id as snapshot_mismatch", async () => {
        const wrongNonce = await runFakeChild(
            snapshot,
            two,
            'sendEnvelope({ run_nonce: "ffffffffffffffffffffffffffffffff" });',
        );
        expect(wrongNonce.result.reason_code).toBe("snapshot_mismatch");
        const wrongBaseline = await runFakeChild(
            snapshot,
            two,
            'sendEnvelope({ baseline_event_id: "adj-green-one" });',
        );
        expect(wrongBaseline.result.reason_code).toBe("snapshot_mismatch");
    }, 20_000);

    it("rejects undeclared check ids in an otherwise valid envelope", async () => {
        const { result } = await runFakeChild(
            snapshot,
            two,
            `sendEnvelope({ verdict: "assertion_fail", failed_checks: ["check-not-declared"], observation_signature: ${JSON.stringify(FAIL_SIGNATURE)} });`,
        );
        expect(result.run_health).toBe("malformed");
        expect(result.reason_code).toBe("invalid_envelope");
    }, 20_000);

    it("treats a valid envelope followed by nonzero exit as a crash", async () => {
        const { result } = await runFakeChild(snapshot, two, "sendEnvelope(); process.exit(7);");
        expect(result.run_health).toBe("crash");
        expect(result.behavioral_verdict).toBe("not_evaluated");
        expect(result.baseline_comparison).toBe("unscored");
        expect(result.reason_code).toBe("child_exit_failure");
    }, 20_000);

    it("rejects duplicate envelopes in the envelope file", async () => {
        const { result } = await runFakeChild(snapshot, two, "sendEnvelope(); sendEnvelope();");
        expect(result.run_health).toBe("malformed");
        expect(result.reason_code).toBe("duplicate_envelope");
    }, 20_000);

    it("caps the envelope file and classifies an oversized file as malformed", async () => {
        const { result } = await runFakeChild(snapshot, two, 'sendRaw("x".repeat(80 * 1024));');
        expect(result.run_health).toBe("malformed");
        expect(result.reason_code).toBe("envelope_oversized");
    }, 20_000);

    it("keeps a completed precondition failure not_evaluated without invoking the verifier", async () => {
        const reviewed = await runFakeChild(
            snapshot,
            blocked,
            'sendEnvelope({ preconditions: "failed", precondition_reason: "blocked_by_dependency", blocked_by: ["var-green-two"], verdict: null });',
        );
        expect(reviewed.result.run_health).toBe("completed");
        expect(reviewed.result.behavioral_verdict).toBe("not_evaluated");
        expect(reviewed.result.baseline_comparison).toBe("unscored");
        expect(reviewed.result.reason_code).toBe("blocked_by_dependency");
        expect(reviewed.result.blocked_by).toEqual(["var-green-two"]);
        const unreviewed = await runFakeChild(
            snapshot,
            two,
            'sendEnvelope({ preconditions: "failed", precondition_reason: "blocked_by_dependency", blocked_by: ["var-green-one"], verdict: null });',
        );
        expect(unreviewed.result.reason_code).toBe("precondition_unmet");
        expect(unreviewed.result.blocked_by).toEqual([]);

        const multiDependency = {
            ...blocked,
            blockedBy: ["var-green-two", "var-green-one"],
        };
        const subset = await runFakeChild(
            snapshot,
            multiDependency,
            'sendEnvelope({ preconditions: "failed", precondition_reason: "blocked_by_dependency", blocked_by: ["var-green-two"], verdict: null });',
        );
        expect(subset.result.reason_code).toBe("precondition_unmet");
        const exact = await runFakeChild(
            snapshot,
            multiDependency,
            'sendEnvelope({ preconditions: "failed", precondition_reason: "blocked_by_dependency", blocked_by: ["var-green-one", "var-green-two"], verdict: null });',
        );
        expect(exact.result.reason_code).toBe("blocked_by_dependency");
        expect(new Set(exact.result.blocked_by)).toEqual(
            new Set(["var-green-two", "var-green-one"]),
        );
    }, 20_000);
});

describe("case isolation", () => {
    const snapshot = snapshotFor();
    const green = selectedCase(snapshot, "var-green-one");
    const two = selectedCase(snapshot, "var-green-two");

    it("creates an owner-only workspace and deletes it at teardown", () => {
        const workspace = createCaseWorkspace(testRoot, "var-perm-check", snapshot.runNonce);
        expect(statSync(workspace.root).mode & 0o777).toBe(0o700);
        expect(statSync(workspace.store).mode & 0o777).toBe(0o700);
        expect(workspace.storeNamespace).toContain("var-perm-check");
        destroyCaseWorkspace(workspace);
        expect(existsSync(workspace.root)).toBe(false);
    });

    it("builds an allowlisted env with relocated home and temp roots", () => {
        const workspace = createCaseWorkspace(testRoot, "var-env-check", snapshot.runNonce);
        const env = buildCaseEnv(workspace, {
            PATH: "/usr/bin",
            HOME: "/real/home",
            CARGO_HOME: "/real/home/.cargo",
            AWS_SECRET_ACCESS_KEY: "canary-credential",
            HTTPS_PROXY: "http://proxy.internal:3128",
            OPENAI_API_KEY: "canary-token",
        });
        expect(env.PATH).toBe("/usr/bin");
        expect(env.HOME).toBe(workspace.home);
        expect(env.TMPDIR).toBe(workspace.tmp);
        expect(env.AWS_SECRET_ACCESS_KEY).toBeUndefined();
        expect(env.HTTPS_PROXY).toBeUndefined();
        expect(env.OPENAI_API_KEY).toBeUndefined();
        // A real `CARGO_HOME` can hold registry credentials, so the case relocates it instead of inheriting the parent's path.
        expect(env.CARGO_HOME).toBe(join(workspace.home, ".cargo"));
        const relocated = ["HOME", "TMPDIR", "TMP", "TEMP", "CARGO_HOME"];
        expect(
            Object.keys(env).every(
                (key) =>
                    (CASE_ENV_ALLOWLIST as readonly string[]).includes(key) ||
                    key.startsWith("XDG_") ||
                    relocated.includes(key),
            ),
        ).toBe(true);
        destroyCaseWorkspace(workspace);
    });

    it("strips parent credential canaries and relocates HOME for the spawned child", async () => {
        const body = `
const leaks = [];
if (env.AWS_SECRET_ACCESS_KEY !== undefined) leaks.push("credential");
if (env.HTTP_PROXY !== undefined || env.HTTPS_PROXY !== undefined) leaks.push("proxy");
if (!env.HOME || !env.HOME.startsWith(env.EIDNARA_INCIDENT_WORKSPACE_ROOT)) leaks.push("home");
if (!env.TMPDIR || !env.TMPDIR.startsWith(env.EIDNARA_INCIDENT_WORKSPACE_ROOT)) leaks.push("tmp");
if (leaks.length > 0) { console.error("leaked: " + leaks.join(",")); process.exit(9); }
sendEnvelope();`;
        const { result } = await runFakeChild(snapshot, green, body, {
            baseEnv: {
                ...process.env,
                AWS_SECRET_ACCESS_KEY: "canary-credential",
                HTTP_PROXY: "http://proxy.internal:3128",
                HTTPS_PROXY: "http://proxy.internal:3128",
                HOME: "/real/ambient/home",
            },
        });
        expect(result.run_health).toBe("completed");
        expect(result.behavioral_verdict).toBe("pass");
    }, 20_000);

    it("rejects configured non-loopback provider endpoints before spawning", async () => {
        expect(isLoopbackUrl("http://127.0.0.1:8080")).toBe(true);
        expect(isLoopbackUrl("http://localhost:8080")).toBe(true);
        expect(isLoopbackUrl("http://[::1]:8080")).toBe(true);
        expect(isLoopbackUrl("http://127.evil.com/")).toBe(false);
        expect(isLoopbackUrl("http://10.0.0.5:8080")).toBe(false);
        expect(() =>
            assertLoopbackProviderEndpoints({
                EIDNARA_E2E_PROVIDER_URL: "http://10.0.0.5:8080",
            }),
        ).toThrow(/not a loopback/);
        await expect(
            runFakeChild(snapshot, green, "sendEnvelope();", {
                providerEndpoints: {
                    EIDNARA_E2E_PROVIDER_URL: "https://api.provider.example",
                },
            }),
        ).rejects.toThrow(/not a loopback/);
        // The child receives a copied environment map, so loopback URLs alone do not prove isolation.
        // An isolation variable or stripped-proxy name must be rejected even when its URL is valid loopback.
        await expect(
            runFakeChild(snapshot, green, "sendEnvelope();", {
                providerEndpoints: { HOME: "http://127.0.0.1:19999" },
            }),
        ).rejects.toThrow(/unsafe incident case providerEndpoints key HOME/);
        await expect(
            runFakeChild(snapshot, green, "sendEnvelope();", {
                providerEndpoints: { HTTPS_PROXY: "http://127.0.0.1:19999" },
            }),
        ).rejects.toThrow(/unsafe incident case providerEndpoints key HTTPS_PROXY/);
        const loopback = await runFakeChild(snapshot, green, "sendEnvelope();", {
            providerEndpoints: {
                EIDNARA_E2E_PROVIDER_URL: "http://127.0.0.1:19999",
            },
        });
        expect(loopback.result.behavioral_verdict).toBe("pass");
    }, 20_000);

    it("rejects extraEnv overrides for isolation, identity, credentials, and proxies", async () => {
        for (const key of [
            "HOME",
            "XDG_DATA_HOME",
            "EIDNARA_INCIDENT_VARIANT_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_PROFILE",
            "OPENAI_API_KEY",
            "SSH_AUTH_SOCK",
            "HTTPS_PROXY",
        ]) {
            await expect(
                runFakeChild(snapshot, green, "sendEnvelope();", {
                    extraEnv: { [key]: "unsafe" },
                }),
            ).rejects.toThrow(new RegExp(`unsafe incident case extraEnv key ${key}`));
        }
    }, 20_000);

    it("never scores a descendant's forged envelope: the extra line surfaces as duplicate_envelope", async () => {
        // Descendants inherit `EIDNARA_INCIDENT_ENVELOPE_PATH`, so the parent cannot attribute an
        // appended line to a process. Any second line, whatever its origin, makes the case
        // malformed; the forged assertion_fail below must not become a verdict.
        const body = `
import { spawnSync } from "node:child_process";
const forged = JSON.stringify(envelopeFromEnv({ verdict: "assertion_fail", failed_checks: ["check-holds"], observation_signature: ${JSON.stringify(FAIL_SIGNATURE)} })) + "\\n";
spawnSync(process.execPath, ["-e", "require('node:fs').appendFileSync(process.env.EIDNARA_INCIDENT_ENVELOPE_PATH, process.env.FORGED)"], {
    env: { ...env, FORGED: forged },
});
sendEnvelope();`;
        const { result } = await runFakeChild(snapshot, two, body);
        expect(result.run_health).toBe("malformed");
        expect(result.reason_code).toBe("duplicate_envelope");
        expect(result.behavioral_verdict).toBe("not_evaluated");
        expect(result.baseline_comparison).toBe("unscored");
        expect(result.failed_checks).toEqual([]);
    }, 20_000);

    it("kills the process group on timeout and joins descendants before the terminal result", async () => {
        const marker = join(testRoot, `late-marker-${Date.now()}`);
        const body = `
import { spawn } from "node:child_process";
spawn(process.execPath, ["-e", "setTimeout(()=>{require('node:fs').writeFileSync(process.env.LATE_MARKER,'late')},2500)"], { stdio: "ignore", env });
setInterval(() => {}, 1000);`;
        const started = Date.now();
        const { result, diagnostics } = await runFakeChild(snapshot, two, body, {
            timeoutMs: 900,
            extraEnv: { LATE_MARKER: marker },
        });
        expect(result.run_health).toBe("timeout");
        expect(result.behavioral_verdict).toBe("not_evaluated");
        expect(result.baseline_comparison).toBe("unscored");
        expect(result.reason_code).toBe("deadline_exceeded");
        expect(diagnostics.workspaceDeleted).toBe(true);
        // Interruption must terminate the descendant before its 2500 ms write.
        await new Promise((resolveWait) =>
            setTimeout(resolveWait, Math.max(0, 3_000 - (Date.now() - started))),
        );
        expect(existsSync(marker)).toBe(false);
    }, 20_000);

    it("caps oversized diagnostics, deletes them at teardown, and keeps them out of reports", async () => {
        const body = `
for (let i = 0; i < 200; i += 1) console.log("DIAGNOSTIC-NOISE-" + "y".repeat(1024));
sendEnvelope();`;
        const { result, diagnostics } = await runFakeChild(snapshot, green, body, {
            diagnosticCapBytes: 4_096,
        });
        expect(result.behavioral_verdict).toBe("pass");
        expect(diagnostics.stdoutTruncated).toBe(true);
        expect(diagnostics.stdoutBytes).toBeLessThanOrEqual(4_096);
        expect(diagnostics.workspaceDeleted).toBe(true);
        const report = buildIncidentReport({
            runNonce: snapshot.runNonce,
            harness: snapshot.harness,
            ledgerFingerprint: snapshot.ledgerFingerprint,
            selectedSetDigest: computeSelectedSetDigest([
                [
                    result.variant_id,
                    result.semantic_fingerprint,
                    result.implementation_digest,
                    result.baseline_event_id,
                ],
            ]),
            selectedVariantIds: ["var-green-one"],
            familyCount: 1,
            results: [result],
        });
        expect(JSON.stringify(report)).not.toContain("DIAGNOSTIC-NOISE");
    }, 20_000);

    it("contains diagnostic sink failures as a static unhealthy result", async () => {
        const originalWrite = DiagnosticSink.prototype.write;
        DiagnosticSink.prototype.write = () => {
            throw new Error("private diagnostic failure");
        };
        try {
            const report = await runIncidentPool(
                snapshot,
                async (selected) =>
                    (
                        await runFakeChild(
                            snapshot,
                            selected,
                            'console.log("trigger sink"); sendEnvelope();',
                        )
                    ).result,
            );
            expect(report.results[0]?.run_health).toBe("crash");
            expect(report.results[0]?.reason_code).toBe("case_execution_failed");
            expect(JSON.stringify(report)).not.toContain("private diagnostic failure");
            expect(report.completion_marker).toBe(true);
        } finally {
            DiagnosticSink.prototype.write = originalWrite;
        }
    }, 20_000);
});

function reportInput(snapshot: RunSnapshot, results: IncidentCaseResult[]) {
    return {
        runNonce: snapshot.runNonce,
        harness: snapshot.harness,
        ledgerFingerprint: snapshot.ledgerFingerprint,
        selectedSetDigest: computeSelectedSetDigest(
            results.map((result) => [
                result.variant_id,
                result.semantic_fingerprint,
                result.implementation_digest,
                result.baseline_event_id,
            ]),
        ),
        selectedVariantIds: results.map((result) => result.variant_id),
        familyCount: new Set(results.map((result) => result.family_id)).size,
        results,
    };
}

async function completedPass(
    snapshot: RunSnapshot,
    selected: SelectedCase,
): Promise<IncidentCaseResult> {
    const { result } = await runFakeChild(snapshot, selected, "sendEnvelope();");
    return result;
}

describe("catalog-bound report", () => {
    const snapshot = snapshotFor();
    const green = selectedCase(snapshot, "var-green-one");
    const two = selectedCase(snapshot, "var-green-two");
    const blocked = selectedCase(snapshot, "var-blocked-one");

    it("rejects an empty selection", () => {
        expect(() => buildIncidentReport(reportInput(snapshot, []))).toThrow(/selection is empty/);
    });

    it("requires exactly one terminal result per selected variant", async () => {
        const result = await completedPass(snapshot, green);
        const base = reportInput(snapshot, [result]);
        expect(() => buildIncidentReport({ ...base, results: [result, result] })).toThrow(
            /duplicate terminal result/,
        );
        expect(() =>
            buildIncidentReport({
                ...base,
                selectedVariantIds: ["var-green-one", "var-green-two"],
            }),
        ).toThrow(/missing terminal result/);
        expect(() =>
            buildIncidentReport({
                ...base,
                selectedVariantIds: ["var-green-two"],
            }),
        ).toThrow(/unexpected result/);
    }, 20_000);

    it("publishes atomically: an interrupted publication is not structurally complete", async () => {
        const result = await completedPass(snapshot, green);
        const report = buildIncidentReport(reportInput(snapshot, [result]));
        const target = join(testRoot, "reports", "incident-report.json");
        publishIncidentReport(report, join(testRoot, "reports", "warmup.json"));
        writeFileSync(`${target}.tmp-dead`, JSON.stringify(report), {
            flag: "w",
        });
        expect(existsSync(target)).toBe(false);
        expect(() => readIncidentReport(target)).toThrow();
        publishIncidentReport(report, target);
        const published = readIncidentReport(target);
        expect(published.completion_marker).toBe(true);
        expect(published.evaluation_complete).toBe(true);
        expect(published.selected_set_digest).toBe(report.selected_set_digest);
    }, 20_000);

    it("publishes one atomic scheduled report for the rust harness schedule", async () => {
        const result = await completedPass(snapshot, green);
        const rust = buildIncidentReport(reportInput(snapshot, [result]));
        const scheduled = buildScheduledIncidentReport("rust", [rust]);
        expect(scheduled.variant_count).toBe(1);
        expect(scheduled.evaluation_complete).toBe(true);
        expect(scheduledIncidentExitCode(scheduled)).toBe(0);
        const target = join(testRoot, "reports", "scheduled-report.json");
        publishScheduledIncidentReport(scheduled, target);
        expect(readScheduledIncidentReport(target)).toEqual(scheduled);
        expect(() =>
            parseScheduledIncidentReport({
                ...scheduled,
                completion_marker: false,
            }),
        ).toThrow(/completion_marker/);
    }, 20_000);

    it("rejects tampered completion markers and completeness flags", async () => {
        const result = await completedPass(snapshot, green);
        const report = buildIncidentReport(reportInput(snapshot, [result]));
        const raw = JSON.parse(JSON.stringify(report)) as Record<string, unknown>;
        expect(() => parseIncidentReport({ ...raw, completion_marker: false })).toThrow(
            /completion_marker/,
        );
        expect(() => parseIncidentReport({ ...raw, evaluation_complete: false })).toThrow(
            /evaluation_complete/,
        );
        expect(() => parseIncidentReport({ ...raw, expected_count: 5 })).toThrow(/expected_count/);
        expect(() => parseIncidentReport({ ...raw, extra_field: 1 })).toThrow(/exactly/);
        const mutatedResults = structuredClone(raw) as {
            selected_set_digest: string;
            results: Array<{ implementation_digest: string }>;
        };
        mutatedResults.results[0]!.implementation_digest = HEX("9");
        expect(() => parseIncidentReport(mutatedResults)).toThrow(
            /selected_set_digest: does not match the parsed terminal result set/,
        );
    }, 20_000);

    it("publishes structurally complete facts for unhealthy runs with evaluation completeness false", async () => {
        const unavailable = unavailableCaseResult(two);
        const timeout = await runFakeChild(snapshot, green, "setInterval(() => {}, 1000);", {
            timeoutMs: 700,
        });
        const report = buildIncidentReport(reportInput(snapshot, [unavailable, timeout.result]));
        expect(report.completion_marker).toBe(true);
        expect(report.evaluation_complete).toBe(false);
        expect(report.results.every((entry) => entry.behavioral_verdict === "not_evaluated")).toBe(
            true,
        );
        expect(incidentPoolExitCode(report)).toBe(1);
    }, 20_000);

    it("excuses an incomplete result only while its blocked_by dependency still blocks", async () => {
        const regressedDependency = await runFakeChild(
            snapshot,
            two,
            `sendEnvelope({ verdict: "assertion_fail", failed_checks: ["check-holds"], observation_signature: ${JSON.stringify(FAIL_SIGNATURE)} });`,
        );
        const blockedResult = await runFakeChild(
            snapshot,
            blocked,
            'sendEnvelope({ preconditions: "failed", precondition_reason: "blocked_by_dependency", blocked_by: ["var-green-two"], verdict: null });',
        );
        const blockedReport = buildIncidentReport(
            reportInput(snapshot, [regressedDependency.result, blockedResult.result]),
        );
        expect(blockedReport.evaluation_complete).toBe(false);
        expect(unexpectedIncompleteResults(blockedReport)).toEqual([]);
        expect(scoredBaselineMismatches(blockedReport).map((entry) => entry.variant_id)).toEqual([
            "var-green-two",
        ]);
        expect(incidentPoolExitCode(blockedReport)).toBe(1);

        const missingDependencyReport = buildIncidentReport(
            reportInput(snapshot, [blockedResult.result]),
        );
        expect(
            unexpectedIncompleteResults(missingDependencyReport).map((entry) => entry.variant_id),
        ).toEqual(["var-blocked-one"]);
        expect(incidentPoolExitCode(missingDependencyReport)).toBe(1);

        // A dependent must be evaluated after a previously blocking dependency passes; dependency-id presence alone is insufficient.
        const passingDependency = await runFakeChild(
            snapshot,
            two,
            'sendEnvelope({ verdict: "pass" });',
        );
        const resolvedReport = buildIncidentReport(
            reportInput(snapshot, [passingDependency.result, blockedResult.result]),
        );
        expect(passingDependency.result.behavioral_verdict).toBe("pass");
        expect(
            unexpectedIncompleteResults(resolvedReport).map((entry) => entry.variant_id),
        ).toEqual(["var-blocked-one"]);
        expect(incidentPoolExitCode(resolvedReport)).toBe(1);

        const crashed = await runFakeChild(snapshot, green, "process.exit(3);");
        const crashReport = buildIncidentReport(
            reportInput(snapshot, [passingDependency.result, crashed.result]),
        );
        expect(unexpectedIncompleteResults(crashReport).map((entry) => entry.variant_id)).toEqual([
            "var-green-one",
        ]);
        expect(incidentPoolExitCode(crashReport)).toBe(1);
    }, 20_000);

    it("fails the command on a scored baseline mismatch and passes reviewed outcomes", async () => {
        // A failed green-lane case is a resurfaced defect.
        const regressed = await runFakeChild(
            snapshot,
            green,
            `sendEnvelope({ verdict: "assertion_fail", failed_checks: ["check-holds"], observation_signature: ${JSON.stringify(FAIL_SIGNATURE)} });`,
        );
        const twoPass = await runFakeChild(snapshot, two, "sendEnvelope();");
        const regressionReport = buildIncidentReport(
            reportInput(snapshot, [regressed.result, twoPass.result]),
        );
        expect(
            regressionReport.results.find((entry) => entry.variant_id === "var-green-one")
                ?.baseline_comparison,
        ).toBe("regression");
        expect(unexpectedIncompleteResults(regressionReport)).toEqual([]);
        expect(scoredBaselineMismatches(regressionReport).map((entry) => entry.variant_id)).toEqual(
            ["var-green-one"],
        );
        expect(incidentPoolExitCode(regressionReport)).toBe(1);

        const greenPass = await runFakeChild(snapshot, green, "sendEnvelope();");
        const reviewedReport = buildIncidentReport(
            reportInput(snapshot, [greenPass.result, twoPass.result]),
        );
        expect(reviewedReport.results.map((entry) => entry.baseline_comparison)).toEqual([
            "expected_green",
            "expected_green",
        ]);
        expect(scoredBaselineMismatches(reviewedReport)).toEqual([]);
        expect(incidentPoolExitCode(reviewedReport)).toBe(0);
    }, 30_000);

    it("continues after one case callback throws and publishes a complete static report", async () => {
        const report = await runIncidentPool(snapshot, async (selected) => {
            if (selected.variantId === "var-green-one") {
                throw new Error("private callback canary must not serialize");
            }
            return completedPass(snapshot, selected);
        });
        expect(report.results).toHaveLength(snapshot.selected.length);
        const failed = report.results.find((result) => result.variant_id === "var-green-one");
        expect(failed?.run_health).toBe("crash");
        expect(failed?.behavioral_verdict).toBe("not_evaluated");
        expect(failed?.baseline_comparison).toBe("unscored");
        expect(failed?.reason_code).toBe("case_execution_failed");
        expect(JSON.stringify(report)).not.toContain("private callback canary");
        expect(report.completion_marker).toBe(true);
    }, 30_000);

    it("runs the whole pool and publishes an evaluation-complete report", async () => {
        const report = await runIncidentPool(snapshot, async (selected) => {
            const { result } = await runFakeChild(snapshot, selected, "sendEnvelope();");
            return result;
        });
        expect(report.expected_count).toBe(3);
        expect(report.family_count).toBe(1);
        const byVariant = new Map(report.results.map((entry) => [entry.variant_id, entry]));
        for (const variantId of ["var-green-one", "var-green-two", "var-blocked-one"]) {
            expect(byVariant.get(variantId)?.baseline_comparison).toBe("expected_green");
        }
        expect(report.evaluation_complete).toBe(true);
        expect(incidentPoolExitCode(report)).toBe(0);
        expect(byVariant.get("var-green-two")?.semantic_fingerprint).toBe(two.semanticFingerprint);
        expect(byVariant.get("var-green-two")?.implementation_digest).toBe(IMPL_DIGEST_TWO);
    }, 30_000);
});

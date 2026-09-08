import { describe, expect, it } from "bun:test";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import type { IncidentCatalog, SourceInventory } from "./contract";
import { parseIncidentCatalog, parseSourceInventory } from "./contract";
import {
    assertEvidenceSnapshot,
    assertMutationReplayResults,
    changedVerifiers,
    crossCheckEvidenceInventory,
    E2E_ROOT,
    type EvidenceView,
    EXPECTED_MUTATION_ARTIFACTS,
    EXPECTED_MUTATION_RECORDS,
    loadMutationEvidence,
    mutationRecordsBoundTo,
    REPO_ROOT,
    scanSources,
    validateEvidenceAndSources,
    verifyOwnershipMatrix,
    verifySourceCompleteness,
} from "./evidence";
import { compareWithAcceptedSnapshot, splitLedgerLines, validateIncidentHistory } from "./history";

const INCIDENTS_DIR = resolve(E2E_ROOT, "incidents");

function committedInventory(): SourceInventory {
    return parseSourceInventory(
        JSON.parse(readFileSync(join(INCIDENTS_DIR, "source-inventory.json"), "utf8")),
    );
}

function committedCatalog(): IncidentCatalog {
    return parseIncidentCatalog(
        JSON.parse(readFileSync(join(INCIDENTS_DIR, "catalog.json"), "utf8")),
    );
}

function committedView(): EvidenceView {
    return loadMutationEvidence();
}

/** A record shaped like the runners' output: red under mutation, green once reverted. */
function executedRecord(name: string, overrides: Record<string, unknown> = {}) {
    return {
        name,
        applied_diff: { path: "x", before: "a", after: "b", changed: true },
        observed_failure: { exit_status: 1, output: "FAILED" },
        reverted_rerun: { exit_status: 0, output: "ok", status: "pass" },
        adequacy_finding: null,
        ...overrides,
    };
}

function findClaim(inventory: SourceInventory, claimId: string) {
    for (const item of inventory.items) {
        const claim = item.claims.find((entry) => entry.id === claimId);
        if (claim) return claim;
    }
    throw new Error(`fixture claim ${claimId} not found`);
}

function findVariant(catalog: IncidentCatalog, variantId: string) {
    for (const family of catalog.families) {
        const variant = family.variants.find((entry) => entry.id === variantId);
        if (variant) return variant;
    }
    throw new Error(`fixture variant ${variantId} not found`);
}

describe("source inventory completeness (R1)", () => {
    it("covers every scanned source item and distinct claim exactly once", () => {
        const inventory = committedInventory();
        verifySourceCompleteness(inventory, scanSources());

        const byId = new Map(inventory.items.map((item) => [item.id, item] as const));
        expect(byId.get("src-parity-findings-s2")!.claims.map((claim) => claim.id)).toEqual([
            "claim-parity-a1",
            "claim-parity-a3",
        ]);
        expect([...byId.keys()].filter((id) => id.startsWith("src-mutation-"))).toHaveLength(
            EXPECTED_MUTATION_ARTIFACTS,
        );
    });

    it("fails when a source item is removed from the inventory", () => {
        const inventory = committedInventory();
        inventory.items = inventory.items.filter((item) => item.id !== "src-parity-findings-s2");
        expect(() => verifySourceCompleteness(inventory, scanSources())).toThrow(
            /source item missing from inventory: src-parity-findings-s2/,
        );
    });

    it("fails when a single claim is removed from the inventory", () => {
        const inventory = committedInventory();
        const parity = inventory.items.find((item) => item.id === "src-parity-findings-s2")!;
        parity.claims = parity.claims.filter((claim) => claim.id !== "claim-parity-a3");
        expect(() => verifySourceCompleteness(inventory, scanSources())).toThrow(
            /source claim missing from inventory: claim-parity-a3/,
        );
    });

    it("fails when the inventory carries a claim with no live source counterpart", () => {
        const inventory = committedInventory();
        const parity = inventory.items.find((item) => item.id === "src-parity-findings-s2")!;
        parity.claims.push({
            id: "claim-parity-a99",
            content_digest: "a".repeat(64),
            disposition: "informational",
            rationale: "fabricated",
            family_links: [],
        });
        expect(() => verifySourceCompleteness(inventory, scanSources())).toThrow(
            /claim-parity-a99 has no live source counterpart/,
        );
    });

    it("rejects a free-form disposition outside the closed R2 vocabulary", () => {
        const raw = JSON.parse(
            readFileSync(join(INCIDENTS_DIR, "source-inventory.json"), "utf8"),
        ) as {
            items: Array<{ claims: Array<{ disposition: string }> }>;
        };
        raw.items[0]!.claims[0]!.disposition = "looks-suspicious";
        expect(() => parseSourceInventory(raw)).toThrow(/disposition: must be one of/);
    });

    it("rejects an edited accepted inventory row against the repository baseline", () => {
        const accepted = {
            baseLabel: "accepted",
            inventoryText: readFileSync(join(INCIDENTS_DIR, "source-inventory.json"), "utf8"),
            catalogText: readFileSync(join(INCIDENTS_DIR, "catalog.json"), "utf8"),
            adjudicationLines: splitLedgerLines(
                readFileSync(join(INCIDENTS_DIR, "adjudications.jsonl"), "utf8"),
            ),
            redactionLines: [] as string[],
        };
        const edited = JSON.parse(accepted.inventoryText) as {
            items: Array<{ claims: Array<{ rationale: string }> }>;
        };
        edited.items[0]!.claims[0]!.rationale = "quietly rewritten";
        const candidate = { ...accepted, inventoryText: JSON.stringify(edited) };
        expect(() => compareWithAcceptedSnapshot(accepted, candidate)).toThrow(
            /edited without an appended adjudication or emergency redaction/,
        );
    });
});

describe("mutation evidence normalization (R11)", () => {
    it("derives the accepted artifact and record snapshot from live files", () => {
        const view = committedView();
        assertEvidenceSnapshot(view);
        expect(view.artifacts).toHaveLength(EXPECTED_MUTATION_ARTIFACTS);
        expect(view.records).toHaveLength(EXPECTED_MUTATION_RECORDS);
        expect(new Set(view.records.map((record) => record.evidenceId)).size).toBe(
            EXPECTED_MUTATION_RECORDS,
        );
        expect(view.records.every((record) => record.shape === "mutations")).toBe(true);
    });

    it("links every record to the live verifier it challenged", () => {
        const view = committedView();
        for (const record of view.records) {
            expect(view.verifierDigests[record.verifierPath]).toMatch(/^[0-9a-f]{64}$/);
        }
        const byId = new Map(view.records.map((record) => [record.evidenceId, record] as const));
        for (const family of ["1", "2", "3"]) {
            expect(byId.get(`ev-dg-${family}-one-byte-input`)!.verifierPath).toBe(
                "crates/daemon/src/differential_goldens.rs",
            );
        }
    });

    it("agrees with the committed inventory's mutation claims", () => {
        crossCheckEvidenceInventory(committedInventory(), committedView());
    });

    it("rejects duplicated, malformed, unknown-shape, and orphan-verifier records", () => {
        const temp = mkdtempSync(join(tmpdir(), "incident-evidence-"));
        try {
            cpSync(resolve(E2E_ROOT, "mutations"), join(temp, "mutations"), { recursive: true });

            const duplicated = JSON.parse(
                readFileSync(join(temp, "mutations", "goldens-dg-3.json"), "utf8"),
            ) as { mutations: unknown[] };
            duplicated.mutations.push(structuredClone(duplicated.mutations[0]));
            writeFileSync(join(temp, "mutations", "goldens-dg-3.json"), JSON.stringify(duplicated));
            expect(() => loadMutationEvidence(temp, REPO_ROOT)).toThrow(
                /duplicate normalized evidence id ev-dg-3-one-byte-input/,
            );

            cpSync(
                resolve(E2E_ROOT, "mutations", "goldens-dg-3.json"),
                join(temp, "mutations", "goldens-dg-3.json"),
            );
            writeFileSync(
                join(temp, "mutations", "zz-unknown.json"),
                JSON.stringify({ drills: [] }),
            );
            expect(() => loadMutationEvidence(temp, REPO_ROOT)).toThrow(
                /unknown mutation artifact shape/,
            );

            writeFileSync(
                join(temp, "mutations", "zz-unknown.json"),
                JSON.stringify({
                    command: "bun test tests/rust-fm-oc-3.test.ts",
                    mutations: [{ nameless: true }],
                }),
            );
            expect(() => loadMutationEvidence(temp, REPO_ROOT)).toThrow(
                /name must be a non-empty string/,
            );

            writeFileSync(
                join(temp, "mutations", "zz-unknown.json"),
                JSON.stringify({
                    command: "bun test tests/this-verifier-does-not-exist.test.ts",
                    mutations: [executedRecord("ZZ_ORPHAN")],
                }),
            );
            expect(() => loadMutationEvidence(temp, REPO_ROOT)).toThrow(
                /links a missing verifier packages\/e2e-tests\/tests\/this-verifier-does-not-exist\.test\.ts/,
            );
        } finally {
            rmSync(temp, { recursive: true, force: true });
        }
    });

    it("rejects records the runner never drove red and green", () => {
        const temp = mkdtempSync(join(tmpdir(), "incident-evidence-"));
        try {
            cpSync(resolve(E2E_ROOT, "mutations"), join(temp, "mutations"), { recursive: true });
            const artifact = join(temp, "mutations", "zz-unexecuted.json");
            const write = (record: Record<string, unknown>) =>
                writeFileSync(
                    artifact,
                    JSON.stringify({
                        command: "bun test tests/rust-fm-oc-3.test.ts",
                        mutations: [record],
                    }),
                );
            const cases: Array<[Record<string, unknown>, RegExp]> = [
                [
                    executedRecord("ZZ", {
                        observed_failure: null,
                        reverted_rerun: null,
                        adequacy_finding: "regenerate once the runtime can start the channel",
                    }),
                    /zz-unexecuted\.json\.mutations\[0\]\.observed_failure must record/,
                ],
                [
                    executedRecord("ZZ", { observed_failure: { exit_status: 0, output: "ok" } }),
                    /observed_failure exit status 0: the mutation did not redden the drill/,
                ],
                [executedRecord("ZZ", { reverted_rerun: null }), /reverted_rerun must record/],
                [
                    executedRecord("ZZ", {
                        reverted_rerun: { exit_status: 1, output: "FAILED", status: "fail" },
                    }),
                    /reverted_rerun did not pass after the mutation was reverted/,
                ],
                [
                    executedRecord("ZZ", {
                        reverted_rerun: { exit_status: 0, output: "ok", status: "fail" },
                    }),
                    /reverted_rerun did not pass after the mutation was reverted/,
                ],
                [
                    executedRecord("ZZ", {
                        adequacy_finding:
                            "mutation did not redden the drill; investigate drill adequacy",
                    }),
                    /adequacy_finding must be null: "mutation did not redden the drill/,
                ],
            ];
            for (const [record, expected] of cases) {
                write(record);
                expect(() => loadMutationEvidence(temp, REPO_ROOT)).toThrow(expected);
            }
            write(executedRecord("ZZ"));
            expect(
                loadMutationEvidence(temp, REPO_ROOT).records.map((r) => r.evidenceId),
            ).toContain("ev-zz");
        } finally {
            rmSync(temp, { recursive: true, force: true });
        }
    });

    it("fails when a mutation link is duplicated into the inventory twice", () => {
        const inventory = committedInventory();
        const artifact = inventory.items.find((item) => item.id === "src-mutation-goldens-dg-1")!;
        artifact.claims.push({
            ...artifact.claims[0]!,
            id: "claim-mutation-dg-1-one-byte-input-copy",
        });
        expect(() => verifySourceCompleteness(inventory, scanSources())).toThrow(
            /has no live source counterpart/,
        );
    });
});

describe("ownership matrix (U3 approach 4)", () => {
    it("accepts the committed catalog only when every executable binding is live", () => {
        verifyOwnershipMatrix(committedInventory(), committedCatalog());
    });

    it("fails when an executable claim loses its owner", () => {
        const inventory = committedInventory();
        const catalog = committedCatalog();
        const family = catalog.families.find(
            (entry) => entry.id === "fam-first-render-tag-stability",
        )!;
        family.variants = family.variants.map((variant) => ({
            ...variant,
            source_claims: ["claim-parity-a1"],
        }));
        expect(() => verifyOwnershipMatrix(inventory, catalog)).toThrow(
            /executable claim claim-parity-a3 has no owner in the implementation matrix/,
        );
    });

    it("rejects a binding that names an existing Bun test instead of a scenario module", () => {
        const catalog = committedCatalog();
        const variant = findVariant(catalog, "var-parity-a1-pure-defer-stability");
        variant.verifier_binding!.driver =
            "tests/cache-invariants.test.ts#driveFirstRenderPureDeferStability";
        expect(() => verifyOwnershipMatrix(committedInventory(), catalog)).toThrow(
            /an existing Bun test alone cannot satisfy an executable binding/,
        );
    });

    it("rejects a live binding whose module or export is missing", () => {
        const catalog = committedCatalog();
        const variant = findVariant(catalog, "var-parity-a1-pure-defer-stability");
        variant.verifier_binding!.driver = "src/incident-pool/scenarios/never-written.ts#driveX";
        expect(() => verifyOwnershipMatrix(committedInventory(), catalog)).toThrow(
            /live binding names a missing module/,
        );

        const catalog2 = committedCatalog();
        const variant2 = findVariant(catalog2, "var-parity-a1-pure-defer-stability");
        variant2.verifier_binding!.driver =
            "src/incident-pool/scenarios/source-linked-regressions.ts#driveSomethingElse";
        expect(() => verifyOwnershipMatrix(committedInventory(), catalog2)).toThrow(
            /does not export function driveSomethingElse/,
        );
    });

    it("rejects comment-only text that looks like an exported function", () => {
        const relative = `src/incident-pool/scenarios/comment-only-${Date.now()}.ts`;
        const absolute = resolve(E2E_ROOT, relative);
        writeFileSync(absolute, "// export function driveFirstRenderPureDeferStability() {}\n");
        try {
            const catalog = committedCatalog();
            const variant = findVariant(catalog, "var-parity-a1-pure-defer-stability");
            variant.verifier_binding!.driver = `${relative}#driveFirstRenderPureDeferStability`;
            expect(() => verifyOwnershipMatrix(committedInventory(), catalog)).toThrow(
                /does not export function driveFirstRenderPureDeferStability/,
            );
        } finally {
            rmSync(absolute, { force: true });
        }
    });

    it("rejects any declared executable binding after rollout", () => {
        const catalog = committedCatalog();
        const variant = findVariant(catalog, "var-parity-a1-pure-defer-stability");
        variant.verifier_binding!.binding_status = "declared";
        variant.verifier_binding!.driver =
            "src/incident-pool/scenarios/source-linked-regressions.ts#driveNotWrittenYet";
        expect(() => verifyOwnershipMatrix(committedInventory(), catalog)).toThrow(
            /requires a live verifier binding/,
        );
    });

    it("requires reciprocal inventory and family ownership links", () => {
        const inventoryMissing = committedInventory();
        findClaim(inventoryMissing, "claim-parity-a1").family_links = [];
        expect(() => verifyOwnershipMatrix(inventoryMissing, committedCatalog())).toThrow(
            /lacks reciprocal inventory family_link/,
        );

        const catalogMissing = committedCatalog();
        const family = catalogMissing.families.find(
            (entry) => entry.id === "fam-first-render-tag-stability",
        )!;
        family.source_claims = [];
        expect(() => verifyOwnershipMatrix(committedInventory(), catalogMissing)).toThrow(
            /lacks reciprocal family source_claim/,
        );
    });

    it("rejects giving an unsupported claim an executable target (AE3)", () => {
        const inventory = committedInventory();
        const parity = inventory.items.find((item) => item.id === "src-parity-findings-s2")!;
        parity.claims.push({
            id: "claim-parity-unsupported",
            content_digest: "b".repeat(64),
            disposition: "unsupported",
            rationale: "wording only; missing evidence",
            family_links: [],
        });
        const catalog = committedCatalog();
        const variant = findVariant(catalog, "var-parity-a1-pure-defer-stability");
        variant.source_claims = [...variant.source_claims, "claim-parity-unsupported"];
        expect(() => verifyOwnershipMatrix(inventory, catalog)).toThrow(
            /unsupported claim claim-parity-unsupported must not have an executable target/,
        );
    });
});

describe("verifier-change mutation replay gate (R14)", () => {
    it("requires no replay while verifier bytes match the accepted digests", () => {
        const view = committedView();
        expect(changedVerifiers(view.verifierDigests, view.verifierDigests)).toEqual([]);
    });

    it("fails contributor verification when a bound mutation no longer produces the expected red result", () => {
        const view = committedView();
        const verifier = "crates/daemon/src/differential_goldens.rs";
        const changed = changedVerifiers(view.verifierDigests, {
            ...view.verifierDigests,
            [verifier]: "0".repeat(64),
        });
        expect(changed).toEqual([verifier]);

        const bound = mutationRecordsBoundTo(view, verifier);
        expect(bound.map((record) => record.evidenceId).sort()).toEqual([
            "ev-dg-1-one-byte-input",
            "ev-dg-2-one-byte-input",
            "ev-dg-3-one-byte-input",
        ]);

        assertMutationReplayResults(view, verifier, {
            "ev-dg-1-one-byte-input": true,
            "ev-dg-2-one-byte-input": true,
            "ev-dg-3-one-byte-input": true,
        });
        expect(() =>
            assertMutationReplayResults(view, verifier, {
                "ev-dg-1-one-byte-input": true,
                "ev-dg-2-one-byte-input": true,
                "ev-dg-3-one-byte-input": false,
            }),
        ).toThrow(/ev-dg-3-one-byte-input did not produce the expected red result/);
        expect(() =>
            assertMutationReplayResults(view, verifier, {
                "ev-dg-1-one-byte-input": true,
            }),
        ).toThrow(/did not produce the expected red result/);
    });
});

describe("committed repository state", () => {
    it("validates the whole populated inventory, catalog, ledger, and evidence together", () => {
        const state = validateIncidentHistory({
            inventoryText: readFileSync(join(INCIDENTS_DIR, "source-inventory.json"), "utf8"),
            catalogText: readFileSync(join(INCIDENTS_DIR, "catalog.json"), "utf8"),
            adjudicationLines: splitLedgerLines(
                readFileSync(join(INCIDENTS_DIR, "adjudications.jsonl"), "utf8"),
            ),
            redactionLines: splitLedgerLines(
                readFileSync(join(INCIDENTS_DIR, "emergency-redactions.jsonl"), "utf8"),
            ),
        });
        const view = validateEvidenceAndSources(state.inventory, state.catalog);
        expect(view.records).toHaveLength(EXPECTED_MUTATION_RECORDS);

        for (const family of state.catalog.families) {
            for (const variant of family.variants) {
                if (variant.lane === "adjudication-only") continue;
                const baseline = state.ledger.byIdentity.get(variant.id)?.latestBaseline;
                expect(baseline?.semantic_fingerprint).toBe(variant.semantic_revision.fingerprint);
                expect(baseline?.baseline_verdict).toBe("green");
                expect(variant.applicability).toEqual({ harness: "rust", omitted: [] });
            }
        }
    });

    it("fails when the catalog references an orphan source claim", () => {
        const inventoryText = readFileSync(join(INCIDENTS_DIR, "source-inventory.json"), "utf8");
        const catalog = JSON.parse(readFileSync(join(INCIDENTS_DIR, "catalog.json"), "utf8")) as {
            families: Array<{ source_claims: string[] }>;
        };
        catalog.families[0]!.source_claims.push("claim-orphan-ghost");
        expect(() =>
            validateIncidentHistory({
                inventoryText,
                catalogText: JSON.stringify(catalog),
                adjudicationLines: splitLedgerLines(
                    readFileSync(join(INCIDENTS_DIR, "adjudications.jsonl"), "utf8"),
                ),
                redactionLines: [],
            }),
        ).toThrow(/unknown source claim claim-orphan-ghost/);
    });
});

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import {
    LEGACY_IDENTITY_RE,
    NOT_APPLICABLE,
    sha256,
    validateShape,
    verify,
    type CheckKind,
    type Context,
} from "./check";

const digest = "a".repeat(64);
const commit = "b".repeat(40);
const blob = "c".repeat(40);
const repoRoot = join(import.meta.dir, "../..");

type Json = Record<string, unknown>;

const cleanSourceStatus = {
    exercised: "yes",
    check_status: "audited",
    portfolio_verdict: "pass",
    known_violation: false,
};

const valid: Record<CheckKind, Json> = {
    receipt: {
        schema_version: 1,
        wave: "U2",
        sources: [{ repo: "commons", commit }],
        catalogs: [{ repo: "commons", commit }],
        readiness: "migration/upstream-readiness.json",
        registry: "migration/registry.json",
        waivers: "migration/waves/U2/waivers.json",
        scope: [{ repo: "commons", path: "crates/cortexkit-lease" }],
        property_impact: "migration/waves/U2/property-impact.json",
        architecture_impact: "migration/waves/U2/architecture-impact.json",
        files: [
            {
                source: { repo: "commons", path: "crates/cortexkit-lease/src/lib.rs", blob_sha: blob },
                destination: "crates/lease/src/lib.rs",
                destination_sha256: digest,
                transformation: "adapted",
                class: "human-authored",
                review_evidence: { doc_rigor: "review/doc-rigor/U2-lease-lib.json" },
            },
        ],
        gates: { tests: "pass", release: "pass" },
        known_red: [
            {
                gate: "source-release",
                kind: "release",
                status: "not_run",
                source_repo: "commons",
                justification: "Owner-hosted source release gate unavailable",
            },
        ],
    },
    registry: {
        schema_version: 1,
        entries: [
            {
                kind: "identity",
                value: ".mc-host-coordination",
                class: "frozen-durable",
                rationale: "Existing writer exclusion path",
                evidence: ["release/mc-host-release.json"],
            },
            {
                kind: "identity",
                value: "cortexkit-lease",
                class: "renamed",
                rename_to: "lease",
                rationale: "Crate name",
                evidence: ["Cargo.toml"],
            },
            {
                kind: "typescript",
                path: "packages/opencode-plugin/src/index.ts",
                class: "permanent",
                owner: "ahrav",
                tier: "harness",
                contract_test: "packages/e2e-tests/mode-manifest.json",
                rust_parity_anchor: "daemon::routes::dispatch",
                rationale: "Harness adapter",
            },
            {
                kind: "family",
                name: "kernel-store",
                class: "retained-authoritative-baseline",
                rationale: "Semantic kernel store",
                literals: ["core.sqlite"],
                paths: ["<data-root>/core.sqlite"],
                mismatch_behavior: "refuse-without-mutation",
                probe: "PRAGMA application_id",
                baseline_source: "U4",
                restore_policy: "current-format backup",
            },
            {
                kind: "family",
                name: "opencode-db",
                class: "foreign",
                rationale: "Harness owned",
                literals: ["opencode.db"],
                paths: [],
                mismatch_behavior: "skip-and-report",
                probe: "schema probe",
            },
            { kind: "authored", path: "crates/lease/src/new.rs", rationale: "authored" },
        ],
    },
    waivers: {
        schema_version: 1,
        wave: "U2",
        waivers: [
            {
                id: "W-U2-1",
                gate: "release",
                kind: "release",
                owner: "ahrav",
                approver: "ahrav",
                bead_id: "magic-context-abc",
                created_at: "2026-09-02",
                expires_by_wave: "U3",
                closure_condition: "owner-hosted chain repointed",
                evidence: ["migration/waves/U2/receipt.json"],
            },
        ],
    },
    "property-catalog": {
        schema_version: 1,
        part: "shared-primitives",
        source: "catalog.md",
        source_sha256: digest,
        records: [
            {
                slug: "lease-single-writer",
                type: "safety",
                reachability: "default-production",
                status: "active",
                exercised: { state: "yes", note: "cross-process test" },
                guarantee: "At most one writer holds the lease.",
                check: { semantics: "always", condition: "active_writers <= 1" },
                fault_timing: "contending process",
                required_faults: "same lease key",
                confidence: { level: "high", evidence: "[evidence](evidence/single-writer.md)" },
                existing_check: "crates/lease/src/lib.rs tests",
                impact: "two writers corrupt the store",
                open_questions: [],
            },
        ],
    },
    "property-impact": {
        schema_version: 1,
        wave: "U2",
        provenance: [{ repo: "commons", source_commit: commit, catalog_commit: commit }],
        destination_commit: commit,
        touched_files: ["crates/lease/src/lib.rs", "crates/lease/src/key.rs"],
        records: [
            {
                slug: "lease-single-writer",
                classification: "core",
                disposition: "pass",
                relationship: "mapped",
                files: ["crates/lease/src/lib.rs"],
                source_status: { ...cleanSourceStatus },
                strategy_decision: "cross-process integration test",
                audit_verdict: "pass",
                evidence_digest: digest,
                code_hash: digest,
                check_hash: digest,
                target_configurations: ["linux-x64"],
                evidence_attempts: 1,
            },
            {
                slug: "lease-key-derivation",
                classification: "carried-forward",
                relationship: "mapped",
                files: ["crates/lease/src/key.rs"],
                provenance: `commons@${commit}`,
                source_status: { exercised: "partial", check_status: "unaudited", portfolio_verdict: "not-evaluated", known_violation: false },
                destination_status: { exercised: "partial", check_status: "unaudited", portfolio_verdict: "not-evaluated", known_violation: false },
                check_pointer: "crates/lease/src/key.rs#L10",
                evidence_pointer: "docs/properties/shared-primitives/evidence/lease-key-derivation.md",
            },
        ],
    },
    "architecture-impact": {
        schema_version: 1,
        wave: "U2",
        reports: [
            {
                phase: "pre-port",
                iteration: 0,
                analyzed: { repo: "commons", commit, scope_hash: digest },
                report_hash: digest,
                skill_sha256: digest,
                candidates: [],
            },
            {
                phase: "post-integration",
                iteration: 1,
                analyzed: { repo: "eidnara", commit, scope_hash: digest },
                report_hash: digest,
                skill_sha256: digest,
                candidates: [
                    {
                        title: "Deepen lease module",
                        strength: "Strong",
                        origin: "original-scope",
                        decision: "accepted",
                        modules: ["crates/lease"],
                        interface: "Lease::acquire",
                        implementation: "filesystem and process exclusion policy",
                        deletion_test: {
                            concentrates_complexity: true,
                            rationale: "Deleting module leaks exclusion policy into every caller.",
                        },
                        benefits: { locality: true, leverage: true, testability: true },
                        claims_flexibility: true,
                        adapters: ["filesystem", "in-memory test"],
                        specialist_routes: ["cohesion-coupling-and-modularity"],
                        final_verdict: "keep one interface and absorb policy",
                        implementation_evidence: "review/U2/deepen-lease.json",
                        property_impact: "migration/waves/U2/property-impact.json",
                        affected_properties: ["lease-single-writer"],
                    },
                ],
            },
        ],
    },
};

function copy<K extends CheckKind>(kind: K): Json {
    return structuredClone(valid[kind]);
}

function files(receipt: Json): Json[] {
    return receipt.files as Json[];
}

function records(value: Json): Json[] {
    return value.records as Json[];
}

function candidate(value: Json): Json {
    const reports = value.reports as Json[];
    return (reports[1]!.candidates as Json[])[0]!;
}

function cli(args: string[]): { status: number | null; stdout: string; stderr: string } {
    const result = spawnSync("bun", ["scripts/eidnara-migration/check.ts", ...args], {
        cwd: repoRoot,
        encoding: "utf8",
        timeout: 60_000,
    });
    return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

describe("shape: complete fixtures", () => {
    for (const kind of Object.keys(valid) as CheckKind[]) {
        test(`${kind} accepts a complete fixture`, () => {
            expect(validateShape(kind, copy(kind))).toEqual([]);
        });
    }

    test("schema and digest formats fail closed", () => {
        const schema = copy("registry");
        schema.schema_version = 2;
        expect(validateShape("registry", schema)).toContain("$.schema_version must equal 1");
        const receipt = copy("receipt");
        files(receipt)[0]!.destination_sha256 = "not-a-digest";
        expect(validateShape("receipt", receipt)).toContain("$.files[0].destination_sha256 must be a lowercase SHA-256 digest");
    });
});

describe("shape: receipt", () => {
    test("rejects duplicate destinations", () => {
        const value = copy("receipt");
        files(value).push(structuredClone(files(value)[0]!));
        expect(validateShape("receipt", value)).toContain("$.files[1].destination is duplicated");
    });

    test("rejects malformed gate states and empty gate sets", () => {
        const malformed = copy("receipt");
        malformed.gates = { tests: ["pass"] };
        expect(validateShape("receipt", malformed)).toContain("$.gates.tests must be one of: pass, fail, cannot_run, not_run");
        const empty = copy("receipt");
        empty.gates = {};
        expect(validateShape("receipt", empty)).toContain("$.gates must declare at least one blocking gate");
    });

    test("validates known-red inheritance separately", () => {
        const malformed = copy("receipt");
        malformed.known_red = "not-an-array";
        expect(validateShape("receipt", malformed)).toContain("$.known_red must be an array");

        const duplicate = copy("receipt");
        (duplicate.known_red as Json[]).push(structuredClone((duplicate.known_red as Json[])[0]!));
        expect(validateShape("receipt", duplicate)).toContain("$.known_red[1].gate is duplicated");

        const overlap = copy("receipt");
        (overlap.known_red as Json[])[0]!.gate = "release";
        expect(validateShape("receipt", overlap)).toContain("$.known_red[0].gate is also declared as a blocking gate");

        const prototypeName = copy("receipt");
        (prototypeName.known_red as Json[])[0]!.gate = "toString";
        expect(validateShape("receipt", prototypeName)).toEqual([]);

        const nonwaivable = copy("receipt");
        (nonwaivable.known_red as Json[])[0]!.kind = "architecture";
        expect(validateShape("receipt", nonwaivable)).toContain("$.known_red[0].kind is nonwaivable");
    });

    test("scopes the source-free exception and not-applicable literals to U1 and U8", () => {
        const u1 = copy("receipt");
        u1.wave = "U1";
        u1.sources = [];
        u1.catalogs = [];
        u1.scope = [];
        u1.property_impact = NOT_APPLICABLE;
        u1.architecture_impact = NOT_APPLICABLE;
        files(u1)[0] = {
            source: null,
            destination: "migration/registry.json",
            destination_sha256: digest,
            transformation: "authored",
            class: "new-authored",
            review_evidence: { design_review: "plan U1", negative_tests: "check.test.ts" },
        };
        expect(validateShape("receipt", u1)).toEqual([]);

        const u2 = copy("receipt");
        u2.sources = [];
        expect(validateShape("receipt", u2)).toContain("$.sources must contain at least one repository commit");

        const u2NotApplicable = copy("receipt");
        u2NotApplicable.property_impact = NOT_APPLICABLE;
        expect(validateShape("receipt", u2NotApplicable)).toContain("$.property_impact may be not-applicable only for waves U1, U8");

        const u1WithImpact = structuredClone(u1);
        u1WithImpact.architecture_impact = "migration/waves/U1/architecture-impact.json";
        expect(validateShape("receipt", u1WithImpact)).toContain("$.architecture_impact must be not-applicable for wave U1");
    });

    test("rejects missing source commit, unknown class, and a source string", () => {
        const noCommit = copy("receipt");
        (noCommit.sources as Json[])[0]!.commit = undefined;
        expect(validateShape("receipt", noCommit)).toContain("$.sources[0].commit must be a non-empty string");

        const unknownClass = copy("receipt");
        files(unknownClass)[0]!.class = "mystery";
        expect(validateShape("receipt", unknownClass)).toContain(
            "$.files[0].class must be one of: human-authored, generated, contract-generated, predecessor-captured, new-authored",
        );

        const noneString = copy("receipt");
        files(noneString)[0]!.source = "none";
        expect(validateShape("receipt", noneString)).toContain("$.files[0].source must be an object");

        const missingSource = copy("receipt");
        delete files(missingSource)[0]!.source;
        expect(validateShape("receipt", missingSource)).toContain("$.files[0].source must be present (null for authored files)");
    });

    test("human-authored file without doc-rigor evidence fails (AE1)", () => {
        const value = copy("receipt");
        files(value)[0]!.review_evidence = {};
        expect(validateShape("receipt", value)).toContain("$.files[0].review_evidence.doc_rigor must be a non-empty string");
    });

    test("contract-generated file without semantic review fails even when the generator ran (AE2)", () => {
        const value = copy("receipt");
        const file = files(value)[0]!;
        file.class = "contract-generated";
        file.transformation = "generated";
        file.review_evidence = { generator: "bun scripts/generate-release-manifest.ts" };
        expect(validateShape("receipt", value)).toContain("$.files[0].review_evidence.semantic_review must be a non-empty string");
    });

    test("new-authored files need source null, authored transformation, design review, and negative tests", () => {
        const withSource = copy("receipt");
        const file = files(withSource)[0]!;
        file.class = "new-authored";
        file.transformation = "authored";
        file.review_evidence = { design_review: "x", negative_tests: "y" };
        expect(validateShape("receipt", withSource)).toEqual(
            expect.arrayContaining([
                "$.files[0].source must be null for new-authored files",
                "$.files[0].transformation authored requires source null",
            ]),
        );

        const wrongTransformation = copy("receipt");
        const nullFile = files(wrongTransformation)[0]!;
        nullFile.class = "new-authored";
        nullFile.source = null;
        nullFile.transformation = "verbatim";
        nullFile.review_evidence = { design_review: "x" };
        expect(validateShape("receipt", wrongTransformation)).toEqual(
            expect.arrayContaining([
                "$.files[0].transformation must be authored when source is null",
                "$.files[0].review_evidence.negative_tests must be a non-empty string",
            ]),
        );

        const humanNull = copy("receipt");
        files(humanNull)[0]!.source = null;
        expect(validateShape("receipt", humanNull)).toContain("$.files[0].source may be null only for new-authored or generated files");
    });

    test("predecessor-captured files must be verbatim with capture evidence", () => {
        const value = copy("receipt");
        const file = files(value)[0]!;
        file.class = "predecessor-captured";
        file.transformation = "adapted";
        file.review_evidence = { captured_at_commit: commit };
        expect(validateShape("receipt", value)).toEqual(
            expect.arrayContaining([
                "$.files[0].transformation must be verbatim for predecessor-captured files",
                "$.files[0].review_evidence.capture_command must be a non-empty string",
            ]),
        );
    });

    test("scope entries and file sources must name a pinned repository", () => {
        const value = copy("receipt");
        (value.scope as Json[]).push({ repo: "magic-context", path: "crates/mc-host" });
        expect(validateShape("receipt", value)).toContain("$.scope[1].repo magic-context has no pinned source commit");
        const empty = copy("receipt");
        empty.scope = [];
        expect(validateShape("receipt", empty)).toContain("$.scope must declare at least one source directory");
    });
});

describe("shape: registry", () => {
    test("rejects an empty inventory and duplicate values", () => {
        expect(validateShape("registry", { schema_version: 1, entries: [] })).toContain("$.entries must contain at least one entry");
        const value = copy("registry");
        (value.entries as Json[]).push(structuredClone((value.entries as Json[])[0]!));
        expect(validateShape("registry", value)).toContain("$.entries[6].value is duplicated");
    });

    test("renamed identities need a legacy-free rename target", () => {
        const value = copy("registry");
        const renamed = (value.entries as Json[])[1]!;
        delete renamed.rename_to;
        expect(validateShape("registry", value)).toContain("$.entries[1].rename_to is required for renamed identities");
        renamed.rename_to = "mc-lease";
        expect(validateShape("registry", value)).toContain("$.entries[1].rename_to retains a legacy identity");
        const frozen = (value.entries as Json[])[0]!;
        frozen.rename_to = "x";
        expect(validateShape("registry", value)).toContain("$.entries[0].rename_to is only valid for renamed identities");
    });

    test("identity and family evidence cannot be empty", () => {
        const value = copy("registry");
        (value.entries as Json[])[0]!.evidence = [];
        expect(validateShape("registry", value)).toContain("$.entries[0].evidence must contain at least one entry");
    });

    test("unknown R11 class fails and family literals are owned once", () => {
        const value = copy("registry");
        const family = (value.entries as Json[])[3]!;
        family.class = "mystery";
        expect(validateShape("registry", value)).toContain(
            "$.entries[3].class must be one of: retained-authoritative-baseline, retained-derived-projection, retained-coordination-state, foreign, planned, absent-by-design, component-of-family, test-only",
        );
        const duplicateLiteral = copy("registry");
        ((duplicateLiteral.entries as Json[])[4]!.literals as string[]).push("core.sqlite");
        expect(validateShape("registry", duplicateLiteral)).toContain("$.entries[4].literals contains core.sqlite, already owned by family kernel-store");
    });

    test("family classes carry their class contract", () => {
        const authoritative = copy("registry");
        const family = (authoritative.entries as Json[])[3]!;
        family.mismatch_behavior = "rebuild";
        delete family.probe;
        delete family.baseline_source;
        expect(validateShape("registry", authoritative)).toEqual(
            expect.arrayContaining([
                "$.entries[3].mismatch_behavior must refuse or quarantine for an authoritative family",
                "$.entries[3].probe is required for an authoritative family",
                "$.entries[3].baseline_source is required for an authoritative family",
            ]),
        );

        const foreign = copy("registry");
        const store = (foreign.entries as Json[])[4]!;
        store.baseline_source = "nope";
        store.restore_policy = "nope";
        expect(validateShape("registry", foreign)).toEqual(
            expect.arrayContaining([
                "$.entries[4].baseline_source is not valid for a foreign store",
                "$.entries[4].restore_policy is not valid for a foreign store",
            ]),
        );

        const derived = copy("registry");
        (derived.entries as Json[]).push({
            kind: "family",
            name: "window-reports",
            class: "retained-derived-projection",
            rationale: "ledger",
            literals: ["window-reports.jsonl"],
            paths: [],
            mismatch_behavior: "rebuild",
        });
        expect(validateShape("registry", derived)).toContain("$.entries[6].rebuild_contract must be deterministic or provider-dependent");
    });

    test("typescript entries are closed classifications, never globs", () => {
        const value = copy("registry");
        const ts = (value.entries as Json[])[2]!;
        ts.path = "packages/opencode-plugin/src/*.ts";
        delete ts.rust_parity_anchor;
        expect(validateShape("registry", value)).toEqual(
            expect.arrayContaining([
                "$.entries[2].path must name one file, not a glob",
                "$.entries[2].rust_parity_anchor must be a non-empty string",
            ]),
        );
        const transitional = copy("registry");
        (transitional.entries as Json[])[2] = {
            kind: "typescript",
            path: "packages/opencode-plugin/src/storage.ts",
            class: "transitional",
            rationale: "moves to daemon",
        };
        expect(validateShape("registry", transitional)).toEqual(
            expect.arrayContaining([
                "$.entries[2].bead_id must be a non-empty string",
                "$.entries[2].deletion_condition must be a non-empty string",
            ]),
        );
    });

    test("legacy identity regex matches whole tokens only", () => {
        expect(LEGACY_IDENTITY_RE.test("check.ts")).toBe(false);
        expect(LEGACY_IDENTITY_RE.test("mcu-board")).toBe(false);
        expect(LEGACY_IDENTITY_RE.test("ck-mc-host")).toBe(true);
        expect(LEGACY_IDENTITY_RE.test("MC_HOST_RING_PROFILE")).toBe(true);
        expect(LEGACY_IDENTITY_RE.test("cortexkit/magic-context")).toBe(true);
        expect(LEGACY_IDENTITY_RE.test("eidnara-host")).toBe(false);
    });
});

describe("shape: waivers", () => {
    test("expired waiver fails and no waiver may reach U8 (AE30)", () => {
        const expired = copy("waivers");
        (expired.waivers as Json[])[0]!.expires_by_wave = "U2";
        expect(validateShape("waivers", expired)).toContain("$.waivers[0] expired: expires_by_wave U2 is not after wave U2");
        const late = copy("waivers");
        late.wave = "U8";
        (late.waivers as Json[])[0]!.expires_by_wave = "U8";
        expect(validateShape("waivers", late)).toEqual(
            expect.arrayContaining(["$.waivers must be empty for wave U8", "$.waivers[0].expires_by_wave may not reach U8"]),
        );
    });

    test("property and architecture gates are nonwaivable", () => {
        const value = copy("waivers");
        (value.waivers as Json[])[0]!.kind = "property";
        expect(validateShape("waivers", value)).toContain("$.waivers[0].kind property is nonwaivable");
    });

    test("waivers need owner, approver, bead, closure condition, and evidence", () => {
        const value = copy("waivers");
        const waiver = (value.waivers as Json[])[0]!;
        delete waiver.approver;
        delete waiver.bead_id;
        waiver.evidence = [];
        waiver.created_at = "yesterday";
        expect(validateShape("waivers", value)).toEqual(
            expect.arrayContaining([
                "$.waivers[0].approver must be a non-empty string",
                "$.waivers[0].bead_id must be a non-empty string",
                "$.waivers[0].evidence must contain at least one entry",
                "$.waivers[0].created_at must be an ISO-8601 UTC date",
            ]),
        );
    });
});

describe("shape: property catalog index", () => {
    test("rejects invalidated record without unreachability evidence and duplicate slugs", () => {
        const value = copy("property-catalog");
        const record = records(value)[0]!;
        record.status = "invalidated";
        expect(validateShape("property-catalog", value)).toContain("$.records[0].unreachability_evidence must be a non-empty string");
        const duplicate = copy("property-catalog");
        records(duplicate).push(structuredClone(records(duplicate)[0]!));
        expect(validateShape("property-catalog", duplicate)).toContain("$.records[1].slug is duplicated");
    });

    test("requires METHOD enum vocabulary", () => {
        const value = copy("property-catalog");
        const record = records(value)[0]!;
        (record.check as Json).semantics = "always(!X)";
        (record.exercised as Json).state = "no";
        expect(validateShape("property-catalog", value)).toEqual(
            expect.arrayContaining([
                "$.records[0].check.semantics must be one of: always, always-or-unreached, sometimes, reachable, unreachable",
                "$.records[0].exercised.state must be one of: yes, partial, not-yet",
            ]),
        );
    });
});

describe("shape: property impact", () => {
    test("uncovered touched file blocks the wave and names discovery scope (AE13)", () => {
        const value = copy("property-impact");
        (value.touched_files as string[]).push("crates/lease/src/fence.rs");
        expect(validateShape("property-impact", value)).toContain(
            "$.touched_files has uncovered file: crates/lease/src/fence.rs; run property discovery for it before approval",
        );
    });

    test("blocked core record blocks; excluded records do not cover files", () => {
        const value = copy("property-impact");
        records(value)[0]!.disposition = "blocked";
        expect(validateShape("property-impact", value)).toContain("$.records[0] blocks the wave");

        const excluded = copy("property-impact");
        const record = records(excluded)[0]!;
        record.classification = "excluded";
        record.isolation_evidence = "Subsystem is absent by design";
        expect(validateShape("property-impact", excluded)).toContain(
            "$.touched_files has uncovered file: crates/lease/src/lib.rs; run property discovery for it before approval",
        );
    });

    test("rejects empty closures, unaudited checks, and duplicate slugs", () => {
        const empty = copy("property-impact");
        empty.touched_files = [];
        empty.records = [];
        expect(validateShape("property-impact", empty)).toEqual(
            expect.arrayContaining(["$.touched_files must contain at least one file", "$.records must contain at least one disposition"]),
        );
        const unaudited = copy("property-impact");
        records(unaudited)[0]!.audit_verdict = "vacuous";
        expect(validateShape("property-impact", unaudited)).toContain("$.records[0].audit_verdict must equal pass");
        const duplicate = copy("property-impact");
        records(duplicate).push(structuredClone(records(duplicate)[0]!));
        expect(validateShape("property-impact", duplicate)).toContain("$.records[2].slug is duplicated");
    });

    test("core record with not-yet, partial, unaudited, blocked, inconclusive, or violation source status needs new evidence (AE14)", () => {
        const statuses: Json[] = [
            { ...cleanSourceStatus, exercised: "not-yet" },
            { ...cleanSourceStatus, exercised: "partial" },
            { ...cleanSourceStatus, check_status: "unaudited" },
            { ...cleanSourceStatus, portfolio_verdict: "PARTIAL" },
            { ...cleanSourceStatus, portfolio_verdict: "BLOCKED" },
            { ...cleanSourceStatus, portfolio_verdict: "INCONCLUSIVE" },
            { ...cleanSourceStatus, known_violation: true },
        ];
        for (const status of statuses) {
            const value = copy("property-impact");
            records(value)[0]!.source_status = status;
            const errors = validateShape("property-impact", value);
            expect(errors.some((error) => error.includes("needs new discriminating evidence"))).toBe(true);
            records(value)[0]!.new_evidence = { digest, description: "cross-binary fence contention test" };
            expect(validateShape("property-impact", value)).toEqual([]);
        }
    });

    test("carried-forward record keeps source status verbatim (AE25)", () => {
        const value = copy("property-impact");
        const record = records(value)[1]!;
        (record.destination_status as Json).exercised = "yes";
        expect(validateShape("property-impact", value)).toContain(
            "$.records[1] carried-forward record changed status; destination_status must equal source_status",
        );
        record.provenance = "commons";
        expect(validateShape("property-impact", value)).toContain("$.records[1].provenance must have the form <repo>@<sha>");
    });

    test("catalog provenance must match the wave source commit", () => {
        const value = copy("property-impact");
        (value.provenance as Json[])[0]!.catalog_commit = "d".repeat(40);
        expect(validateShape("property-impact", value)[0]).toContain("differs from source_commit");
    });

    test("two failed evidence attempts require a scope decision", () => {
        const value = copy("property-impact");
        const record = records(value)[0]!;
        record.disposition = "blocked";
        record.evidence_attempts = 2;
        expect(validateShape("property-impact", value)).toContain("$.records[0] needs a scope decision after 2 failed evidence attempts");
        value.scope_decisions = [{ slug: "lease-single-writer", decision: "mechanism-left-scope", evidence: "lease fencing stays in commons this wave" }];
        expect(validateShape("property-impact", value)).toEqual(["$.records[0] blocks the wave"]);
    });

    test("invalidated records need historical evidence plus unreachability", () => {
        const value = copy("property-impact");
        records(value).push({
            slug: "old-mechanism",
            classification: "invalidated",
            relationship: "isolated",
            files: [],
            historical_evidence: "docs/properties/shared-primitives/evidence/old.md",
        });
        expect(validateShape("property-impact", value)).toContain("$.records[2].unreachability_evidence must be a non-empty string");
    });
});

describe("shape: architecture impact", () => {
    test("unresolved original-scope Strong candidates block (AE19)", () => {
        const value = copy("architecture-impact");
        const item = candidate(value);
        item.decision = "unresolved";
        expect(validateShape("architecture-impact", value)).toContain(
            "$.reports[1].candidates[0] is an original-scope Strong candidate that is neither accepted nor rejected",
        );
        item.decision = "recorded";
        expect(validateShape("architecture-impact", value)).toContain(
            "$.reports[1].candidates[0] is an original-scope Strong candidate that is neither accepted nor rejected",
        );
    });

    test("loop-created Strong candidates may be recorded with a bead", () => {
        const value = copy("architecture-impact");
        const item = candidate(value);
        item.origin = "loop-created";
        item.decision = "recorded";
        expect(validateShape("architecture-impact", value)).toContain("$.reports[1].candidates[0].bead_id must be a non-empty string");
        item.bead_id = "eidnara-1";
        expect(validateShape("architecture-impact", value)).toEqual([]);
    });

    test("hypothetical flexibility and missing benefits are rejected (AE20, AE21)", () => {
        const value = copy("architecture-impact");
        candidate(value).adapters = ["filesystem"];
        expect(validateShape("architecture-impact", value)).toContain("$.reports[1].candidates[0] claims flexibility without two current adapters");
        const noBenefit = copy("architecture-impact");
        candidate(noBenefit).benefits = { locality: false, leverage: false, testability: false };
        expect(validateShape("architecture-impact", noBenefit)).toContain("$.reports[1].candidates[0] has no locality, leverage, or testability benefit");
        const shallow = copy("architecture-impact");
        (candidate(shallow).deletion_test as Json).concentrates_complexity = false;
        expect(validateShape("architecture-impact", shallow)).toContain("$.reports[1].candidates[0] fails the deletion test");
        const typed = copy("architecture-impact");
        candidate(typed).benefits = { locality: true, leverage: "yes", testability: 42 };
        expect(validateShape("architecture-impact", typed)).toEqual(
            expect.arrayContaining([
                "$.reports[1].candidates[0].benefits.leverage must be a boolean",
                "$.reports[1].candidates[0].benefits.testability must be a boolean",
            ]),
        );
    });

    test("requires both phases, bounded iterations, and accepted-change evidence", () => {
        const missingPhase = copy("architecture-impact");
        (missingPhase.reports as unknown[]).splice(0, 1);
        expect(validateShape("architecture-impact", missingPhase)).toContain("$.reports is missing pre-port phase");

        const third = copy("architecture-impact");
        (third.reports as Json[])[1]!.iteration = 3;
        expect(validateShape("architecture-impact", third)[0]).toContain("must be 1 or 2 for post-integration");

        const incomplete = copy("architecture-impact");
        const item = candidate(incomplete);
        item.specialist_routes = [];
        delete item.implementation_evidence;
        item.affected_properties = [];
        expect(validateShape("architecture-impact", incomplete)).toEqual(
            expect.arrayContaining([
                "$.reports[1].candidates[0].implementation_evidence must be a non-empty string",
                "$.reports[1].candidates[0].specialist_routes must contain at least one route",
                "$.reports[1].candidates[0].affected_properties must contain at least one entry",
            ]),
        );
    });

    test("third original-scope Strong candidate requires an escalation record", () => {
        const value = copy("architecture-impact");
        const base = candidate(value);
        const rejected = (title: string): Json => ({
            ...structuredClone(base),
            title,
            decision: "rejected",
            rationale: "moves complexity to callers",
            claims_flexibility: false,
        });
        ((value.reports as Json[])[1]!.candidates as Json[]).push(rejected("Second"), rejected("Third"));
        expect(validateShape("architecture-impact", value)).toContain(
            "$.escalation is required: 3 original-scope Strong candidates in one wave",
        );
        value.escalation = { candidate: "Third", decision: "deferred-with-bead", bead_id: "eidnara-2", rationale: "scope decision recorded" };
        expect(validateShape("architecture-impact", value)).toEqual([]);
    });
});

describe("evidence: git-backed receipts", () => {
    let work: string;
    let source: string;
    let destination: string;
    let sourceCommit: string;
    let leaseBlob: string;
    let keyBlob: string;
    let ctx: Context;

    function gitIn(cwd: string, args: string[]): string {
        const result = spawnSync("git", args, { cwd, encoding: "utf8" });
        if (result.status !== 0) throw new Error(`git ${args.join(" ")}: ${result.stderr}`);
        return result.stdout.trim();
    }

    function write(path: string, content: string): void {
        mkdirSync(dirname(path), { recursive: true });
        writeFileSync(path, content);
    }

    beforeAll(() => {
        work = mkdtempSync(join(tmpdir(), "eidnara-check-"));
        source = join(work, "commons");
        destination = join(work, "eidnara");
        mkdirSync(source);
        gitIn(source, ["init", "-q", "-b", "main"]);
        gitIn(source, ["config", "user.email", "t@example.com"]);
        gitIn(source, ["config", "user.name", "t"]);
        write(join(source, "crates/cortexkit-lease/src/lib.rs"), "pub fn lease() {}\n");
        write(join(source, "crates/cortexkit-lease/src/key.rs"), "pub struct LeaseKey;\n");
        write(join(source, "crates/cortexkit-lease/Cargo.toml"), "[package]\nname = \"cortexkit-lease\"\n");
        write(
            join(source, ".beads/issues.jsonl"),
            `${JSON.stringify({ id: "commons-abc", status: "closed" })}\n${JSON.stringify({ id: "commons-open", status: "open" })}\n`,
        );
        gitIn(source, ["add", "-A"]);
        gitIn(source, ["commit", "-q", "-m", "seed"]);
        sourceCommit = gitIn(source, ["rev-parse", "HEAD"]);
        leaseBlob = gitIn(source, ["rev-parse", `HEAD:crates/cortexkit-lease/src/lib.rs`]);
        keyBlob = gitIn(source, ["rev-parse", `HEAD:crates/cortexkit-lease/src/key.rs`]);

        write(join(destination, "crates/lease/src/lib.rs"), "pub fn lease() {}\n");
        write(join(destination, "crates/lease/src/key.rs"), "pub struct LeaseKey; // adapted\n");
        write(join(destination, "crates/lease/Cargo.toml"), "[package]\nname = \"lease\"\n");
        write(join(destination, "docs/properties/shared-primitives/evidence/lease-key-derivation.md"), "# evidence\n");
        write(
            join(destination, "migration/upstream-readiness.json"),
            JSON.stringify({
                schema_version: 1,
                waves: Object.fromEntries(
                    ["U1", "U2", "U3", "U4", "U5", "U7", "U8"].map((wave) => [
                        wave,
                        {
                            repo: "commons",
                            bead_ids: wave === "U2" ? ["commons-abc"] : [],
                            required_status: "closed",
                            acceptance_check: "test",
                        },
                    ]),
                ),
            }),
        );
        write(join(destination, "migration/registry.json"), JSON.stringify(valid.registry));
        write(join(destination, "migration/waves/U2/waivers.json"), JSON.stringify({ schema_version: 1, wave: "U2", waivers: [] }));
        write(join(destination, "migration/waves/U2/property-impact.json"), JSON.stringify(impactFor(sourceCommit)));
        write(join(destination, "migration/waves/U2/architecture-impact.json"), JSON.stringify(valid["architecture-impact"]));
        ctx = { root: destination, checkouts: { commons: source } };
    });

    afterAll(() => {
        rmSync(work, { recursive: true, force: true });
    });

    function impactFor(pin: string): Json {
        const impact = copy("property-impact");
        (impact.provenance as Json[])[0] = { repo: "commons", source_commit: pin, catalog_commit: pin };
        return impact;
    }

    function receipt(): Json {
        return {
            schema_version: 1,
            wave: "U2",
            sources: [{ repo: "commons", commit: sourceCommit }],
            catalogs: [{ repo: "commons", commit: sourceCommit }],
            readiness: "migration/upstream-readiness.json",
            registry: "migration/registry.json",
            waivers: "migration/waves/U2/waivers.json",
            scope: [{ repo: "commons", path: "crates/cortexkit-lease" }],
            excluded: [{ repo: "commons", path: "crates/cortexkit-lease/Cargo.toml", reason: "regenerated as crates/lease/Cargo.toml" }],
            property_impact: "migration/waves/U2/property-impact.json",
            architecture_impact: "migration/waves/U2/architecture-impact.json",
            files: [
                {
                    source: { repo: "commons", path: "crates/cortexkit-lease/src/lib.rs", blob_sha: leaseBlob },
                    destination: "crates/lease/src/lib.rs",
                    destination_sha256: sha256("pub fn lease() {}\n"),
                    transformation: "verbatim",
                    class: "human-authored",
                    review_evidence: { doc_rigor: "review/U2/lib.json" },
                },
                {
                    source: { repo: "commons", path: "crates/cortexkit-lease/src/key.rs", blob_sha: keyBlob },
                    destination: "crates/lease/src/key.rs",
                    destination_sha256: sha256("pub struct LeaseKey; // adapted\n"),
                    transformation: "adapted",
                    class: "human-authored",
                    review_evidence: { doc_rigor: "review/U2/key.json" },
                },
                {
                    source: null,
                    destination: "crates/lease/Cargo.toml",
                    destination_sha256: sha256("[package]\nname = \"lease\"\n"),
                    transformation: "generated",
                    class: "generated",
                    review_evidence: { regeneration: "cargo metadata" },
                },
            ],
            gates: { tests: "pass" },
        };
    }

    test("a complete synthetic wave passes every evidence check", () => {
        expect(verify("receipt", receipt(), ctx)).toEqual([]);
    });

    test("stale source blob hash marks the receipt stale (AE6)", () => {
        const value = receipt();
        (files(value)[0]!.source as Json).blob_sha = "e".repeat(40);
        const errors = verify("receipt", value, ctx);
        expect(errors.some((error) => error.startsWith("$.files[0].source.blob_sha is stale"))).toBe(true);
    });

    test("stale destination hash and missing destination fail", () => {
        const value = receipt();
        files(value)[0]!.destination_sha256 = digest;
        expect(verify("receipt", value, ctx).some((error) => error.startsWith("$.files[0].destination_sha256 is stale"))).toBe(true);
        const missing = receipt();
        files(missing)[0]!.destination = "crates/lease/src/missing.rs";
        expect(verify("receipt", missing, ctx)).toContain(
            "$.files[0].destination crates/lease/src/missing.rs does not exist in the destination checkout",
        );
    });

    test("verbatim files must have byte-identical source and destination", () => {
        const value = receipt();
        files(value)[1]!.transformation = "verbatim";
        expect(verify("receipt", value, ctx).some((error) => error.includes("is verbatim but destination bytes differ"))).toBe(true);
    });

    test("scope file missing from the receipt fails and names the path (AE26)", () => {
        const value = receipt();
        files(value).splice(1, 1);
        expect(verify("receipt", value, ctx)).toContain(
            "$.scope commons:crates/cortexkit-lease has file missing from receipt: crates/cortexkit-lease/src/key.rs",
        );
    });

    test("unpinned source path is rejected", () => {
        const value = receipt();
        (files(value)[0]!.source as Json).path = "crates/cortexkit-lease/src/nope.rs";
        expect(verify("receipt", value, ctx).some((error) => error.includes("is not a blob at commons@"))).toBe(true);
    });

    test("open readiness bead refuses the SHA pin (AE32)", () => {
        const value = receipt();
        const readinessPath = join(destination, "migration/upstream-readiness.json");
        const original = JSON.parse(require("node:fs").readFileSync(readinessPath, "utf8")) as Json;
        const modified = structuredClone(original);
        ((modified.waves as Json).U2 as Json).bead_ids = ["commons-abc", "commons-open", "commons-missing"];
        writeFileSync(readinessPath, JSON.stringify(modified));
        try {
            const errors = verify("receipt", value, ctx);
            expect(errors.some((error) => error.includes("beads not closed: commons-open=open, commons-missing=missing"))).toBe(true);
        } finally {
            writeFileSync(readinessPath, JSON.stringify(original));
        }
    });

    test("non-pass gate blocks unless a valid waiver names it", () => {
        const value = receipt();
        (value.gates as Json).release = "cannot_run";
        expect(verify("receipt", value, ctx)).toContain("$.gates.release blocks the wave with status cannot_run");
        const waiversPath = join(destination, "migration/waves/U2/waivers.json");
        writeFileSync(waiversPath, JSON.stringify(valid.waivers));
        try {
            expect(verify("receipt", value, ctx)).toEqual([]);
        } finally {
            writeFileSync(waiversPath, JSON.stringify({ schema_version: 1, wave: "U2", waivers: [] }));
        }
    });

    test("new-authored file needs a registry authored entry", () => {
        const value = receipt();
        files(value).push({
            source: null,
            destination: "crates/lease/src/lib.rs",
            destination_sha256: sha256("pub fn lease() {}\n"),
            transformation: "authored",
            class: "new-authored",
            review_evidence: { design_review: "x", negative_tests: "y" },
        });
        files(value).splice(0, 1);
        expect(verify("receipt", value, ctx)).toContain(
            "$.files[2] is new-authored but the registry has no authored entry for crates/lease/src/lib.rs",
        );
    });

    test("carried-forward pointers must resolve in the destination tree (AE25)", () => {
        const impact = impactFor(sourceCommit);
        records(impact)[1]!.check_pointer = "crates/lease/src/gone.rs#L1";
        const errors = verify("property-impact", impact, ctx);
        expect(errors).toContain(
            "$.records[1].check_pointer crates/lease/src/gone.rs#L1 does not resolve in the destination tree; reclassify the record as core or excluded",
        );
        expect(verify("property-impact", impactFor(sourceCommit), ctx)).toEqual([]);
    });

    test("registry scans catch unfrozen legacy identities and unowned persistent literals (AE7)", () => {
        write(join(destination, "crates/lease/src/paths.rs"), 'const ROOT: &str = "cortexkit";\nconst DB: &str = "mystery.db";\nconst OK: &str = "core.sqlite";\n');
        write(join(destination, "crates/lease/src/old.rs"), 'use cortexkit_lease::Lease;\n');
        try {
            const registry = copy("registry");
            let errors = verify("registry", registry, ctx);
            expect(errors).toEqual(
                expect.arrayContaining([
                    "crates/lease/src/paths.rs:1: legacy identity not frozen by the registry",
                    'crates/lease/src/paths.rs: persistent literal "mystery.db" has no family entry in the registry',
                ]),
            );
            expect(errors.some((error) => error.startsWith("crates/lease/src/old.rs:1"))).toBe(true);
            (registry.entries as Json[]).push({
                kind: "identity",
                value: "cortexkit",
                class: "frozen-durable",
                rationale: "managed dir",
                evidence: ["x"],
            });
            errors = verify("registry", registry, ctx);
            expect(errors).not.toContain("crates/lease/src/paths.rs:1: legacy identity not frozen by the registry");
        } finally {
            rmSync(join(destination, "crates/lease/src/paths.rs"));
            rmSync(join(destination, "crates/lease/src/old.rs"));
        }
    });

    test("renamed identities are caught before a retained substring can excuse them (AE7)", () => {
        write(join(destination, "crates/lease/src/stale.rs"), "use cortexkit_lease::Lease;\nconst DB: &str = \"mc-store.db\";\nconst FENCE: &str = \"cortexkit_fence\";\n");
        try {
            const registry = copy("registry");
            (registry.entries as Json[]).push(
                { kind: "identity", value: "cortexkit", class: "frozen-durable", rationale: "managed dir", evidence: ["x"] },
                { kind: "identity", value: "mc-store.db", class: "frozen-durable", rationale: "module store file", evidence: ["x"] },
                { kind: "identity", value: "mc-store", class: "renamed", rename_to: "memory-store", rationale: "crate", evidence: ["x"] },
            );
            const errors = verify("registry", registry, ctx);
            expect(errors).toContain("crates/lease/src/stale.rs:1: renamed identity cortexkit-lease still present");
            expect(errors.some((error) => error.startsWith("crates/lease/src/stale.rs:2"))).toBe(false);
            expect(errors.some((error) => error.startsWith("crates/lease/src/stale.rs:3"))).toBe(false);
        } finally {
            rmSync(join(destination, "crates/lease/src/stale.rs"));
        }
    });

    test("registered fixtures are exempt from the identity scan and must stay verbatim", () => {
        const fixturePath = "crates/lease/tests/golden/vectors.json";
        const fixtureBytes = '{"provenance": "magic-context: inject.ts"}\n';
        write(join(destination, fixturePath), fixtureBytes);
        try {
            const registry = copy("registry");
            let errors = verify("registry", registry, ctx);
            expect(errors).toContain(`${fixturePath}:1: legacy identity not frozen by the registry`);
            (registry.entries as Json[]).push({
                kind: "fixture",
                path: fixturePath,
                role: "byte-stable",
                rationale: "golden vectors",
                evidence: ["migration/waves/U2/receipt.json"],
            });
            errors = verify("registry", registry, ctx);
            expect(errors.some((error) => error.startsWith(`${fixturePath}:`))).toBe(false);

            (registry.entries as Json[]).push({
                kind: "fixture",
                path: "crates/lease/tests/golden/missing.json",
                role: "generator",
                rationale: "generator",
                evidence: ["x"],
            });
            expect(validateShape("registry", registry)).toContain("$.entries[7].fixture must be a non-empty string");
            ((registry.entries as Json[])[7] as Json).fixture = fixturePath;
            errors = verify("registry", registry, ctx);
            expect(errors).toContain("fixture crates/lease/tests/golden/missing.json is registered but does not exist in the destination");

            (registry.entries as Json[]).pop();
            writeFileSync(join(destination, "migration/registry.json"), JSON.stringify(registry));
            const value = receipt();
            files(value).push({
                source: { repo: "commons", path: "crates/cortexkit-lease/tests/golden/vectors.json", blob_sha: blob },
                destination: fixturePath,
                destination_sha256: sha256(fixtureBytes),
                transformation: "adapted",
                class: "human-authored",
                review_evidence: { doc_rigor: "x" },
            });
            expect(verify("receipt", value, ctx)).toContain(
                `$.files[${files(value).length - 1}] is a byte-stable fixture but its transformation is adapted, not verbatim`,
            );
        } finally {
            rmSync(join(destination, fixturePath));
            writeFileSync(join(destination, "migration/registry.json"), JSON.stringify(valid.registry));
        }
    });

    test("a generator fixture must target a registered byte-stable fixture", () => {
        const registry = copy("registry");
        (registry.entries as Json[]).push({
            kind: "fixture",
            path: "scripts/gen.ts",
            role: "generator",
            fixture: "crates/lease/tests/golden/unregistered.json",
            rationale: "generator",
            evidence: ["x"],
        });
        expect(validateShape("registry", registry)).toContain(
            "$.entries[6].fixture crates/lease/tests/golden/unregistered.json is not a registered byte-stable fixture",
        );
        // A registered fixture whose role is not byte-stable is rejected too.
        (registry.entries as Json[]).push({
            kind: "fixture",
            path: "release/registry-gate.json",
            role: "external-record",
            rationale: "external record",
            evidence: ["x"],
        });
        ((registry.entries as Json[])[6] as Json).fixture = "release/registry-gate.json";
        expect(validateShape("registry", registry)).toContain(
            "$.entries[6].fixture release/registry-gate.json is not a registered byte-stable fixture",
        );
    });

    test("shipped TypeScript needs a permanent or transitional registry entry", () => {
        write(join(destination, "packages/opencode-plugin/src/index.ts"), "export {};\n");
        write(join(destination, "packages/opencode-plugin/src/extra.ts"), "export {};\n");
        try {
            const errors = verify("registry", copy("registry"), ctx);
            expect(errors).toContain("packages/opencode-plugin/src/extra.ts: TypeScript file has no registry classification");
            expect(errors).not.toContain("packages/opencode-plugin/src/index.ts: TypeScript file has no registry classification");
        } finally {
            rmSync(join(destination, "packages"), { recursive: true, force: true });
        }
    });
});

describe("cli", () => {
    test("every subcommand accepts its complete shape-only fixture where no evidence is needed", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-check-cli-"));
        try {
            for (const kind of ["waivers", "property-catalog", "architecture-impact"] as CheckKind[]) {
                const path = join(root, `${kind}.json`);
                writeFileSync(path, `${JSON.stringify(valid[kind])}\n`);
                const result = cli([kind, path, "--root", root]);
                expect(result.status, `${kind}: ${result.stderr}`).toBe(0);
                expect(result.stdout).toContain(`${kind}: PASS`);
            }
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("fails closed on invalid, missing, and extra inputs", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-check-cli-invalid-"));
        try {
            const invalidPath = join(root, "invalid.json");
            writeFileSync(invalidPath, `${JSON.stringify({ schema_version: 1, entries: [] })}\n`);
            const invalid = cli(["receipt", invalidPath, "--root", root]);
            expect(invalid.status).toBe(1);
            expect(invalid.stderr).toContain("$.wave must be a non-empty string");

            const missing = cli(["registry", join(root, "missing.json"), "--root", root]);
            expect(missing.status).toBe(2);
            expect(missing.stderr).toContain("failed to read");

            const extra = cli(["registry", invalidPath, "extra"]);
            expect(extra.status).toBe(2);
            expect(extra.stderr).toContain("usage:");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});

/**
 *
 *
 * The scanner normalizes `mutations[].name` and `mutation_records[].id` into one evidence view.
 * The scanner preserves the raw mutation artifacts.
 * Each mutation record identifies the verifier it challenged.
 * The live scan must produce exactly 3 artifacts and 3 records.
 * The scanner extracts stable source-item and source-claim identities from the named incident sources.
 * Each extracted source item and claim includes a content digest.
 * Each extracted source identity must match exactly one committed inventory entry.
 * Each executable claim must be owned by an executable catalog variant.
 * Each driver/verifier binding must name a scenario module.
 * A `live` binding must resolve to a real scenario-module export.
 * A `declared` binding must name a scenario module that does not exist.
 * A bare Bun test cannot satisfy a driver/verifier binding.
 *    binding.
 *
 * A verifier change gates on mutation replay:
 * `mutationRecordsBoundTo` returns the records bound to verifiers whose bytes changed.
 * The replay runner processes records from `mutationRecordsBoundTo` serially, and `assertMutationReplayResults` fails unless every replayed mutation produces the expected red result.
 */

import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import * as ts from "typescript";
import { validateCommittedMatrix } from "../../scripts/validate-shm-hardening-matrix";
import type { IncidentCatalog, IncidentVariant, SourceInventory } from "./contract";
import { EXECUTABLE_LANES } from "./contract";
import { rowDigest } from "./history";

export const E2E_ROOT = resolve(import.meta.dir, "..", "..");
export const REPO_ROOT = resolve(E2E_ROOT, "..", "..");

export const EXPECTED_MUTATION_ARTIFACTS = 3;
export const EXPECTED_MUTATION_RECORDS = 3;

/** The parity findings are an external document; the inventory binds each finding by the digest of its claim wording, and the wording constants are the scanned source bytes. */
export const PARITY_SOURCE_PATH = "parity-findings:s2";
export const PARITY_A1_WORDING =
    "parity A1: first-render tag activation keeps pure-defer growth byte-stable, with zero prefix busts across six low-pressure turns";
export const PARITY_A3_WORDING =
    "parity A3: an aged real ctx_reduce tool-use and tool-result pair survives pure-defer growth past the protected window with zero prefix busts and stays on the final wire";

function sha256(text: string): string {
    return createHash("sha256").update(text, "utf8").digest("hex");
}

export function slugify(text: string): string {
    return text
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-+|-+$/g, "");
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

export interface MutationEvidenceRecord {
    /** `evidenceId` uses the normalized `ev-<slug>` format. */
    evidenceId: string;
    /** `claimId` uses the matching `claim-mutation-<slug>` format. */
    claimId: string;
    /** `artifactPath` is relative to the e2e package root. */
    artifactPath: string;
    /** `rawName` preserves the raw `mutations[].name` or `mutation_records[].id` value. */
    rawName: string;
    shape: "mutations" | "mutation_records";
    /** `verifierPath` is the repo-relative path of the challenged verifier. */
    verifierPath: string;
    /** `replayCommand` is the committed command that replays this mutation. */
    replayCommand: string;
    /** `recordDigest` detects drift in the raw record object. */
    recordDigest: string;
}

export interface MutationEvidenceArtifact {
    path: string;
    contentDigest: string;
    records: MutationEvidenceRecord[];
}

export interface EvidenceView {
    artifacts: MutationEvidenceArtifact[];
    records: MutationEvidenceRecord[];
    /** `verifierDigests` maps each repo-relative verifier path to the SHA-256 digest of its current bytes. */
    verifierDigests: Record<string, string>;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Fail closed because the fixed-ring matrix has no unresolved state. */
function assertDeferralStillPermitted(label: string): void {
    const outcome = validateCommittedMatrix().outcome;
    throw new Error(
        `${label} is deferred, but the fixed-ring matrix is ${outcome}: this claim needs a real mutation record instead of a deferral`,
    );
}

function requireString(value: unknown, label: string): string {
    if (typeof value !== "string" || value.trim().length === 0) {
        throw new Error(`${label} must be a non-empty string`);
    }
    return value;
}

/** Verifier evidence requires a failing mutated drill, a passing reverted rerun, and no adequacy finding. */
function requireExecutedResults(rawRecord: Record<string, unknown>, label: string): void {
    const observed = rawRecord.observed_failure;
    if (!isRecord(observed) || !Number.isInteger(observed.exit_status)) {
        throw new Error(`${label}.observed_failure must record the mutated drill's exit status`);
    }
    if (observed.exit_status === 0) {
        throw new Error(
            `${label}.observed_failure exit status 0: the mutation did not redden the drill`,
        );
    }
    const reverted = rawRecord.reverted_rerun;
    if (!isRecord(reverted) || !Number.isInteger(reverted.exit_status)) {
        throw new Error(`${label}.reverted_rerun must record the reverted drill's exit status`);
    }
    if (reverted.exit_status !== 0 || reverted.status !== "pass") {
        throw new Error(`${label}.reverted_rerun did not pass after the mutation was reverted`);
    }
    if (rawRecord.adequacy_finding !== null) {
        throw new Error(
            `${label}.adequacy_finding must be null: ${JSON.stringify(rawRecord.adequacy_finding)}`,
        );
    }
}

const E2E_TEST_PATH_RE = /(?:^|[\s'"])((?:tests|scripts)\/[\w./-]+\.ts)/;
/** `cargo test -p <crate> --test <target>` resolves to `<crate>/tests/<target>.rs`.
 *  `<crate>/tests/<target>.rs`. */
const CARGO_INTEGRATION_RE = /cargo test -p ([\w-]+) --test ([\w-]+)/;
/** `cargo test -p <crate> --lib <module>::…::<test>` resolves to the source file for `<module>`.
 * */
const CARGO_UNIT_RE = /cargo test -p ([\w-]+) --lib ([\w:]+)/;
/* */
const PACKAGE_SRC_TEST_PATH_RE = /(?:^|[\s'"])(src\/[\w./-]+\.test\.ts)/;

/**
 * Rust verifier paths follow Cargo's target layout.
 * */
function verifierFromCommand(repoRoot: string, command: string, label: string): string {
    if (command.startsWith("cargo test -p daemon")) {
        return "crates/daemon/src/differential_goldens.rs";
    }
    const integration = command.match(CARGO_INTEGRATION_RE);
    if (integration) {
        return `crates/${integration[1]}/tests/${integration[2]}.rs`;
    }
    const unit = command.match(CARGO_UNIT_RE);
    if (unit) {
        // verifierFromCommand removes the test function and trailing `tests` module because the remaining module path names the source file.
        const segments = unit[2].split("::");
        segments.pop();
        if (segments.at(-1) === "tests") segments.pop();
        if (segments.length > 0) {
            return `crates/${unit[1]}/src/${segments.join("/")}.rs`;
        }
    }
    const match = command.match(E2E_TEST_PATH_RE);
    if (match) return `packages/e2e-tests/${match[1]}`;
    // The resolver checks every package because a `src/`-relative command path does not name its owning package.
    // Ambiguous package-relative paths fail instead of selecting the first package.
    const packageRelative = command.match(PACKAGE_SRC_TEST_PATH_RE);
    if (packageRelative) {
        const hits = readdirSync(resolve(repoRoot, "packages"))
            .map((pkg) => `packages/${pkg}/${packageRelative[1]}`)
            .filter((candidate) => existsSync(resolve(repoRoot, candidate)));
        if (hits.length === 1) return hits[0];
        if (hits.length > 1) {
            throw new Error(
                `${label}: ${JSON.stringify(packageRelative[1])} exists in more than one package (${hits.join(", ")})`,
            );
        }
    }
    throw new Error(`${label}: cannot resolve a verifier from command ${JSON.stringify(command)}`);
}

/**
 * */
function verifierFromMustFail(
    repoRoot: string,
    rerunCommand: string,
    mustFail: string,
    label: string,
): string {
    const candidates = [...rerunCommand.matchAll(/tests\/[\w./-]+\.test\.ts/g)].map((m) => m[0]);
    if (candidates.length === 0) {
        throw new Error(`${label}: reverted_rerun_command names no test files`);
    }
    for (const candidate of candidates) {
        const path = resolve(repoRoot, "packages/e2e-tests", candidate);
        if (existsSync(path) && readFileSync(path, "utf8").includes(mustFail)) {
            return `packages/e2e-tests/${candidate}`;
        }
    }
    throw new Error(`${label}: no candidate test file contains must_fail id ${mustFail}`);
}

export function loadMutationEvidence(
    e2eRoot: string = E2E_ROOT,
    repoRoot: string = REPO_ROOT,
): EvidenceView {
    const mutationsDir = resolve(e2eRoot, "mutations");
    const files = readdirSync(mutationsDir)
        .filter((name) => name.endsWith(".json"))
        .sort();

    const artifacts: MutationEvidenceArtifact[] = [];
    const evidenceIds = new Set<string>();
    for (const file of files) {
        const artifactPath = `mutations/${file}`;
        const text = readFileSync(resolve(mutationsDir, file), "utf8");
        let raw: unknown;
        try {
            raw = JSON.parse(text) as unknown;
        } catch (error) {
            throw new Error(`${artifactPath} is not valid JSON: ${String(error)}`);
        }
        if (!isRecord(raw)) throw new Error(`${artifactPath} must be a JSON object`);

        const records: MutationEvidenceRecord[] = [];
        let declaredRecords = 0;
        if (Array.isArray(raw.mutations)) {
            declaredRecords = raw.mutations.length;
            const proven = raw.mutations.filter(
                (rawRecord) => !isRecord(rawRecord) || rawRecord.status !== "deferred",
            );
            const command =
                proven.length > 0 ? requireString(raw.command, `${artifactPath}.command`) : "";
            for (const [index, rawRecord] of raw.mutations.entries()) {
                const label = `${artifactPath}.mutations[${index}]`;
                if (!isRecord(rawRecord)) throw new Error(`${label} must be an object`);
                if (rawRecord.status === "deferred") {
                    requireString(rawRecord.reason, `${label}.reason`);
                    assertDeferralStillPermitted(label);
                    continue;
                }
                const name = requireString(rawRecord.name, `${label}.name`);
                requireExecutedResults(rawRecord, label);
                records.push({
                    evidenceId: `ev-${slugify(name)}`,
                    claimId: `claim-mutation-${slugify(name)}`,
                    artifactPath,
                    rawName: name,
                    shape: "mutations",
                    verifierPath: verifierFromCommand(repoRoot, command, label),
                    replayCommand: command,
                    recordDigest: rowDigest(rawRecord),
                });
            }
        } else if (Array.isArray(raw.mutation_records)) {
            declaredRecords = raw.mutation_records.length;
            for (const [index, rawRecord] of raw.mutation_records.entries()) {
                const label = `${artifactPath}.mutation_records[${index}]`;
                if (!isRecord(rawRecord)) throw new Error(`${label} must be an object`);
                const id = requireString(rawRecord.id, `${label}.id`);
                const mustFail = requireString(rawRecord.must_fail, `${label}.must_fail`);
                const rerun = requireString(
                    rawRecord.reverted_rerun_command,
                    `${label}.reverted_rerun_command`,
                );
                requireExecutedResults(rawRecord, label);
                records.push({
                    evidenceId: `ev-${slugify(id)}`,
                    claimId: `claim-mutation-${slugify(id)}`,
                    artifactPath,
                    rawName: id,
                    shape: "mutation_records",
                    verifierPath: verifierFromMustFail(repoRoot, rerun, mustFail, label),
                    replayCommand: rerun,
                    recordDigest: rowDigest(rawRecord),
                });
            }
        } else {
            throw new Error(
                `${artifactPath}: unknown mutation artifact shape (expected mutations[] or mutation_records[])`,
            );
        }

        // is malformed.
        if (declaredRecords === 0)
            throw new Error(`${artifactPath}: artifact contains no mutation records`);
        for (const record of records) {
            if (evidenceIds.has(record.evidenceId)) {
                throw new Error(
                    `duplicate normalized evidence id ${record.evidenceId} (${record.artifactPath})`,
                );
            }
            evidenceIds.add(record.evidenceId);
        }
        artifacts.push({
            path: artifactPath,
            contentDigest: sha256(text),
            records,
        });
    }

    const records = artifacts.flatMap((artifact) => artifact.records);
    const verifierDigests: Record<string, string> = {};
    for (const record of records) {
        if (record.verifierPath in verifierDigests) continue;
        const path = resolve(repoRoot, record.verifierPath);
        if (!existsSync(path)) {
            throw new Error(
                `evidence record ${record.evidenceId} links a missing verifier ${record.verifierPath}`,
            );
        }
        verifierDigests[record.verifierPath] = sha256(readFileSync(path, "utf8"));
    }
    return { artifacts, records, verifierDigests };
}

/**
 * */
export function assertEvidenceSnapshot(view: EvidenceView): void {
    if (view.artifacts.length !== EXPECTED_MUTATION_ARTIFACTS) {
        throw new Error(
            `expected ${EXPECTED_MUTATION_ARTIFACTS} mutation artifacts, found ${view.artifacts.length}`,
        );
    }
    if (view.records.length !== EXPECTED_MUTATION_RECORDS) {
        throw new Error(
            `expected ${EXPECTED_MUTATION_RECORDS} mutation records, found ${view.records.length}`,
        );
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

export interface ScannedClaim {
    id: string;
    digest: string;
}

export interface ScannedItem {
    id: string;
    sourcePath: string;
    digest: string;
    claims: ScannedClaim[];
}

/* */
export function scanSources(
    repoRoot: string = REPO_ROOT,
    e2eRoot: string = E2E_ROOT,
): ScannedItem[] {
    const items: ScannedItem[] = [
        {
            id: "src-parity-findings-s2",
            sourcePath: PARITY_SOURCE_PATH,
            digest: sha256(`${PARITY_A1_WORDING}\n${PARITY_A3_WORDING}`),
            claims: [
                { id: "claim-parity-a1", digest: sha256(PARITY_A1_WORDING) },
                { id: "claim-parity-a3", digest: sha256(PARITY_A3_WORDING) },
            ],
        },
    ];

    const view = loadMutationEvidence(e2eRoot, repoRoot);
    for (const artifact of view.artifacts) {
        items.push({
            id: `src-mutation-${slugify(artifact.path.replace(/^mutations\//, "").replace(/\.json$/, ""))}`,
            sourcePath: `packages/e2e-tests/${artifact.path}`,
            digest: artifact.contentDigest,
            claims: artifact.records.map((record) => ({
                id: record.claimId,
                digest: record.recordDigest,
            })),
        });
    }
    return items;
}

/* */
export function verifySourceCompleteness(inventory: SourceInventory, scanned: ScannedItem[]): void {
    const inventoryItems = new Map(inventory.items.map((item) => [item.id, item] as const));
    for (const item of scanned) {
        const committed = inventoryItems.get(item.id);
        if (!committed) throw new Error(`source item missing from inventory: ${item.id}`);
        if (committed.source_path !== item.sourcePath) {
            throw new Error(
                `source item ${item.id} path drifted: ${committed.source_path} != ${item.sourcePath}`,
            );
        }
        if (committed.content_digest !== item.digest) {
            throw new Error(`source item ${item.id} content drifted from its accepted digest`);
        }
        const committedClaims = new Map(
            committed.claims.map((claim) => [claim.id, claim] as const),
        );
        for (const claim of item.claims) {
            const committedClaim = committedClaims.get(claim.id);
            if (!committedClaim)
                throw new Error(`source claim missing from inventory: ${claim.id}`);
            if (committedClaim.content_digest !== claim.digest) {
                throw new Error(
                    `source claim ${claim.id} content drifted from its accepted digest`,
                );
            }
        }
        for (const claimId of committedClaims.keys()) {
            if (!item.claims.some((claim) => claim.id === claimId)) {
                throw new Error(`inventory claim ${claimId} has no live source counterpart`);
            }
        }
        if (committed.claims.length !== item.claims.length) {
            throw new Error(`source item ${item.id} claim count drifted`);
        }
    }
    for (const itemId of inventoryItems.keys()) {
        if (!scanned.some((item) => item.id === itemId)) {
            throw new Error(`inventory item ${itemId} has no live source counterpart`);
        }
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

const EXECUTABLE_DISPOSITIONS = new Set([
    "executable_accepted_behavior",
    "executable_fixed_regression",
    "executable_known_defect",
]);

const SCENARIO_BINDING_RE = /^(src\/incident-pool\/scenarios\/[\w-]+\.ts)#([A-Za-z][A-Za-z0-9]*)$/;

function parseBinding(reference: string, label: string): { path: string; symbol: string } {
    const match = reference.match(SCENARIO_BINDING_RE);
    if (!match) {
        throw new Error(
            `${label}: binding ${JSON.stringify(reference)} must be a scenario module reference ` +
                "(src/incident-pool/scenarios/<module>.ts#<export>); an existing Bun test alone cannot satisfy an executable binding",
        );
    }
    return { path: match[1]!, symbol: match[2]! };
}

/**
 *
 */
const parsedModules = new Map<string, ts.SourceFile>();

function parsedModule(absolute: string): ts.SourceFile {
    const cached = parsedModules.get(absolute);
    if (cached) return cached;
    const source = ts.createSourceFile(
        absolute,
        readFileSync(absolute, "utf8"),
        ts.ScriptTarget.Latest,
        true,
    );
    parsedModules.set(absolute, source);
    return source;
}

function checkBindingLiveness(
    e2eRoot: string,
    variantId: string,
    status: "declared" | "live",
    reference: string,
): void {
    const { path, symbol } = parseBinding(reference, `variant ${variantId}`);
    const absolute = resolve(e2eRoot, path);
    if (status === "declared") {
        if (existsSync(absolute)) {
            throw new Error(
                `variant ${variantId}: binding_status is declared but ${path} exists; flip the binding to live`,
            );
        }
        return;
    }
    if (!existsSync(absolute)) {
        throw new Error(`variant ${variantId}: live binding names a missing module ${path}`);
    }
    const source = parsedModule(absolute);
    const exported = (node: ts.Node): boolean =>
        ts.canHaveModifiers(node) &&
        ts.getModifiers(node)?.some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword) ===
            true;
    const hasFunctionExport = source.statements.some((statement) => {
        if (
            ts.isFunctionDeclaration(statement) &&
            exported(statement) &&
            statement.name?.text === symbol
        ) {
            return true;
        }
        if (!ts.isVariableStatement(statement) || !exported(statement)) {
            return false;
        }
        return statement.declarationList.declarations.some(
            (declaration) =>
                ts.isIdentifier(declaration.name) &&
                declaration.name.text === symbol &&
                declaration.initializer !== undefined &&
                (ts.isArrowFunction(declaration.initializer) ||
                    ts.isFunctionExpression(declaration.initializer)),
        );
    });
    if (!hasFunctionExport) {
        throw new Error(
            `variant ${variantId}: live binding ${path} does not export function ${symbol}`,
        );
    }
}

export function verifyOwnershipMatrix(
    inventory: SourceInventory,
    catalog: IncidentCatalog,
    e2eRoot: string = E2E_ROOT,
): void {
    const claims = new Map(
        inventory.items.flatMap((item) => item.claims.map((claim) => [claim.id, claim] as const)),
    );
    const families = new Map(catalog.families.map((family) => [family.id, family] as const));
    const executableVariantsByClaim = new Map<string, IncidentVariant[]>();
    for (const family of catalog.families) {
        for (const variant of family.variants) {
            if (!EXECUTABLE_LANES.includes(variant.lane)) continue;
            for (const claimId of variant.source_claims) {
                const list = executableVariantsByClaim.get(claimId) ?? [];
                list.push(variant);
                executableVariantsByClaim.set(claimId, list);
            }
        }
    }

    for (const family of catalog.families) {
        for (const claimId of family.source_claims) {
            const claim = claims.get(claimId);
            if (!claim?.family_links.includes(family.id)) {
                throw new Error(
                    `family ${family.id} source claim ${claimId} lacks reciprocal inventory family_link`,
                );
            }
        }
    }

    for (const item of inventory.items) {
        for (const claim of item.claims) {
            for (const familyId of claim.family_links) {
                const family = families.get(familyId);
                if (!family?.source_claims.includes(claim.id)) {
                    throw new Error(
                        `inventory claim ${claim.id} family_link ${familyId} lacks reciprocal family source_claim`,
                    );
                }
            }
            const owners = executableVariantsByClaim.get(claim.id) ?? [];
            if (EXECUTABLE_DISPOSITIONS.has(claim.disposition)) {
                if (claim.family_links.length === 0 || owners.length === 0) {
                    throw new Error(
                        `executable claim ${claim.id} has no owner in the implementation matrix (no executable variant references it)`,
                    );
                }
            } else if (claim.disposition === "unsupported" && owners.length > 0) {
                throw new Error(`unsupported claim ${claim.id} must not have an executable target`);
            }
        }
    }

    for (const family of catalog.families) {
        for (const variant of family.variants) {
            const binding = variant.verifier_binding;
            if (EXECUTABLE_LANES.includes(variant.lane)) {
                if (binding?.binding_status !== "live") {
                    throw new Error(
                        `executable variant ${variant.id} requires a live verifier binding`,
                    );
                }
            }
            if (binding === null) continue;
            checkBindingLiveness(e2eRoot, variant.id, binding.binding_status, binding.driver);
            checkBindingLiveness(e2eRoot, variant.id, binding.binding_status, binding.verifier);
        }
    }
}

/* */
export function crossCheckEvidenceInventory(inventory: SourceInventory, view: EvidenceView): void {
    const evidenceClaims = new Set(view.records.map((record) => record.claimId));
    const inventoryClaims = new Set(
        inventory.items
            .filter((item) => item.id.startsWith("src-mutation-"))
            .flatMap((item) => item.claims.map((claim) => claim.id)),
    );
    for (const claimId of evidenceClaims) {
        if (!inventoryClaims.has(claimId))
            throw new Error(`mutation record ${claimId} missing from inventory`);
    }
    for (const claimId of inventoryClaims) {
        if (!evidenceClaims.has(claimId))
            throw new Error(`inventory mutation claim ${claimId} has no live record`);
    }
    if (inventoryClaims.size !== EXPECTED_MUTATION_RECORDS) {
        throw new Error(
            `inventory carries ${inventoryClaims.size} mutation claims; expected ${EXPECTED_MUTATION_RECORDS}`,
        );
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/* */
export function changedVerifiers(
    acceptedDigests: Record<string, string>,
    currentDigests: Record<string, string>,
): string[] {
    const changed: string[] = [];
    const paths = new Set([...Object.keys(acceptedDigests), ...Object.keys(currentDigests)]);
    for (const path of paths) {
        if (acceptedDigests[path] !== currentDigests[path]) changed.push(path);
    }
    return changed.sort();
}

export function mutationRecordsBoundTo(
    view: EvidenceView,
    verifierPath: string,
): MutationEvidenceRecord[] {
    return view.records.filter((record) => record.verifierPath === verifierPath);
}

/**
 *
 *
 */
export function boundVerifierFiles(catalog: IncidentCatalog): string[] {
    const paths = new Set<string>();
    for (const family of catalog.families) {
        for (const variant of family.variants) {
            if (!EXECUTABLE_LANES.includes(variant.lane)) continue;
            const binding = variant.verifier_binding;
            if (!binding) continue;
            for (const reference of [binding.driver, binding.verifier]) {
                const path = reference.split("#")[0]?.trim() ?? "";
                if (
                    path.length === 0 ||
                    path.startsWith("/") ||
                    path.split(/[\\/]/).includes("..")
                ) {
                    throw new Error(
                        `variant ${variant.id} verifier binding ${reference} is not a confined relative path`,
                    );
                }
                paths.add(path);
            }
        }
    }
    return [...paths].sort();
}

/**
 */
export function boundVerifierDigests(
    catalog: IncidentCatalog,
    e2eRoot: string = E2E_ROOT,
): Record<string, string> {
    const digests: Record<string, string> = {};
    for (const path of boundVerifierFiles(catalog)) {
        const absolute = resolve(e2eRoot, path);
        if (!existsSync(absolute)) {
            throw new Error(`catalog binds a missing verifier module packages/e2e-tests/${path}`);
        }
        digests[`packages/e2e-tests/${path}`] = sha256(readFileSync(absolute, "utf8"));
    }
    return digests;
}

/**
 */
export function assertMutationReplayResults(
    view: EvidenceView,
    verifierPath: string,
    replayProducedExpectedRed: Record<string, boolean>,
): void {
    const bound = mutationRecordsBoundTo(view, verifierPath);
    if (bound.length === 0)
        throw new Error(`no mutation records are bound to verifier ${verifierPath}`);
    const failures: string[] = [];
    for (const record of bound) {
        if (replayProducedExpectedRed[record.evidenceId] !== true) failures.push(record.evidenceId);
    }
    if (failures.length > 0) {
        throw new Error(
            `changed verifier ${verifierPath} failed mutation replay: ${failures.join(", ")} did not produce the expected red result`,
        );
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

export function validateEvidenceAndSources(
    inventory: SourceInventory,
    catalog: IncidentCatalog,
    repoRoot: string = REPO_ROOT,
    e2eRoot: string = E2E_ROOT,
): EvidenceView {
    const view = loadMutationEvidence(e2eRoot, repoRoot);
    assertEvidenceSnapshot(view);
    verifySourceCompleteness(inventory, scanSources(repoRoot, e2eRoot));
    crossCheckEvidenceInventory(inventory, view);
    verifyOwnershipMatrix(inventory, catalog, e2eRoot);
    return view;
}

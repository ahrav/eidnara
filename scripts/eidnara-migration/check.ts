import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export type CheckKind =
    | "receipt"
    | "registry"
    | "waivers"
    | "property-catalog"
    | "property-impact"
    | "architecture-impact";

export const CHECK_KINDS: readonly CheckKind[] = [
    "receipt",
    "registry",
    "waivers",
    "property-catalog",
    "property-impact",
    "architecture-impact",
];

export const WAVES = ["U1", "U2", "U3", "U4", "U5", "U7", "U8"] as const;
export type Wave = (typeof WAVES)[number];

const CONTROL_ONLY_WAVES: readonly string[] = ["U1", "U8"];

export const NOT_APPLICABLE = "not-applicable";

export const FILE_CLASSES = [
    "human-authored",
    "generated",
    "contract-generated",
    "captured",
    "new-authored",
] as const;

export const TRANSFORMATIONS = ["verbatim", "renamed", "adapted", "generated", "authored"] as const;

export const GATE_STATES = ["pass", "fail", "cannot_run", "not_run"] as const;

export const IDENTITY_CLASSES = ["frozen-durable", "external-protocol", "third-party"] as const;

export const TYPESCRIPT_CLASSES = ["permanent", "transitional", "excluded"] as const;

export const FAMILY_CLASSES = [
    "retained-authoritative-baseline",
    "retained-derived-projection",
    "retained-coordination-state",
    "foreign",
    "planned",
    "absent-by-design",
    "component-of-family",
    "test-only",
] as const;

export const MISMATCH_BEHAVIORS = [
    "refuse-without-mutation",
    "quarantine-and-rebootstrap",
    "rebuild",
    "recreate",
    "skip-and-report",
    "none",
] as const;

export const WAIVER_KINDS = ["release", "parity", "repo", "other"] as const;
export const NONWAIVABLE_KINDS = ["architecture", "property"] as const;

export const PROPERTY_CLASSIFICATIONS = ["core", "carried-forward", "excluded", "invalidated"] as const;

export const CHECK_SEMANTICS = [
    "always",
    "always-or-unreached",
    "sometimes",
    "reachable",
    "unreachable",
] as const;

type JsonObject = Record<string, unknown>;

const SHA256_RE = /^[0-9a-f]{64}$/;
const COMMIT_RE = /^[0-9a-f]{7,64}$/;
const BLOB_RE = /^[0-9a-f]{40}$|^[0-9a-f]{64}$/;
const PROVENANCE_RE = /^[a-z0-9][a-z0-9-]*@[0-9a-f]{7,64}$/;
const ISO_DATE_RE = /^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}:\d{2}(\.\d+)?Z)?$/;

// A persistent filename in every string form of the scanned language. Rust strings are
// `"…"`, `b"…"`, `r"…"`, `r#"…"#`, and `br#"…"#`; a backtick in Rust is a doc-comment code
// span, not a string. TypeScript strings are `"…"`, `'…'`, and the template form `` `…` ``.
const PERSISTENT_NAME = String.raw`\.(?:db|sqlite|bin|lock|jsonl|handle)`;
export const PERSISTENT_LITERAL_RE: Record<".rs" | ".ts", RegExp> = {
    ".rs": new RegExp(String.raw`(?:\b(?:br|b|r)#*)?"([^"\n]*${PERSISTENT_NAME})"#*`, "g"),
    ".ts": new RegExp(String.raw`"([^"\n]*${PERSISTENT_NAME})"|'([^'\n]*${PERSISTENT_NAME})'|` + "`([^`\\n]*" + PERSISTENT_NAME + ")`", "g"),
};

// Every Eidnara-owned SQL family has exactly one baseline; a version ledger or a
// runner that upgrades one schema into another has no place in destination code.
export const MIGRATION_MACHINERY_RE =
    /schema_migrations|\bMIGRATIONS\b|LATEST_MIGRATION_VERSION|BOOTSTRAP_MIGRATION_VERSION|ensureColumn|\bMigration \{|\bfn migrate\b|run_migrations/g;

export const FIXTURE_ROLES = ["byte-stable", "generator", "external-record"] as const;

function isObject(value: unknown): value is JsonObject {
    return value !== null && typeof value === "object" && !Array.isArray(value);
}

function requireObject(value: unknown, path: string, errors: string[]): JsonObject | undefined {
    if (!isObject(value)) {
        errors.push(`${path} must be an object`);
        return undefined;
    }
    return value;
}

function requireString(object: JsonObject, key: string, path: string, errors: string[]): string | undefined {
    const value = object[key];
    if (typeof value !== "string" || value.trim() === "") {
        errors.push(`${path}.${key} must be a non-empty string`);
        return undefined;
    }
    return value;
}

function optionalString(object: JsonObject, key: string, path: string, errors: string[]): string | undefined {
    if (object[key] === undefined) return undefined;
    return requireString(object, key, path, errors);
}

function requireBoolean(object: JsonObject, key: string, path: string, errors: string[]): boolean | undefined {
    const value = object[key];
    if (typeof value !== "boolean") {
        errors.push(`${path}.${key} must be a boolean`);
        return undefined;
    }
    return value;
}

function requireInteger(
    object: JsonObject,
    key: string,
    path: string,
    errors: string[],
    minimum = 0,
): number | undefined {
    const value = object[key];
    if (typeof value !== "number" || !Number.isInteger(value) || value < minimum) {
        errors.push(`${path}.${key} must be an integer >= ${minimum}`);
        return undefined;
    }
    return value;
}

function requireArray(object: JsonObject, key: string, path: string, errors: string[]): unknown[] {
    const value = object[key];
    if (!Array.isArray(value)) {
        errors.push(`${path}.${key} must be an array`);
        return [];
    }
    return value;
}

function requireStringArray(
    object: JsonObject,
    key: string,
    path: string,
    errors: string[],
    minimum = 0,
): string[] {
    const values = requireArray(object, key, path, errors);
    const strings: string[] = [];
    values.forEach((value, index) => {
        if (typeof value !== "string" || value.trim() === "") {
            errors.push(`${path}.${key}[${index}] must be a non-empty string`);
        } else {
            strings.push(value);
        }
    });
    if (strings.length < minimum) {
        errors.push(`${path}.${key} must contain at least one entry`);
    }
    return strings;
}

function requireEnum(
    object: JsonObject,
    key: string,
    allowed: readonly string[],
    path: string,
    errors: string[],
): string | undefined {
    const value = requireString(object, key, path, errors);
    if (value !== undefined && !allowed.includes(value)) {
        errors.push(`${path}.${key} must be one of: ${allowed.join(", ")}`);
        return undefined;
    }
    return value;
}

function requireDigest(object: JsonObject, key: string, path: string, errors: string[]): string | undefined {
    const value = requireString(object, key, path, errors);
    if (value !== undefined && !SHA256_RE.test(value)) {
        errors.push(`${path}.${key} must be a lowercase SHA-256 digest`);
        return undefined;
    }
    return value;
}

function requireCommit(object: JsonObject, key: string, path: string, errors: string[]): string | undefined {
    const value = requireString(object, key, path, errors);
    if (value !== undefined && !COMMIT_RE.test(value)) {
        errors.push(`${path}.${key} must be a hexadecimal commit id`);
        return undefined;
    }
    return value;
}

function requireWave(object: JsonObject, key: string, path: string, errors: string[]): Wave | undefined {
    const value = requireEnum(object, key, WAVES, path, errors);
    return value as Wave | undefined;
}

function validateSchemaVersion(root: JsonObject, errors: string[]): void {
    if (root.schema_version !== 1) errors.push("$.schema_version must equal 1");
}

function waveIndex(wave: string): number {
    return WAVES.indexOf(wave as Wave);
}

interface RepoCommit {
    repo: string;
    commit: string;
}

function validateRepoCommits(value: unknown, path: string, errors: string[], allowEmpty: boolean): RepoCommit[] {
    if (!Array.isArray(value)) {
        errors.push(`${path} must be an array of repository commits`);
        return [];
    }
    if (value.length === 0 && !allowEmpty) {
        errors.push(`${path} must contain at least one repository commit`);
    }
    const result: RepoCommit[] = [];
    const repos = new Set<string>();
    value.forEach((entry, index) => {
        const itemPath = `${path}[${index}]`;
        const item = requireObject(entry, itemPath, errors);
        if (item === undefined) return;
        const repo = requireString(item, "repo", itemPath, errors);
        const commit = requireCommit(item, "commit", itemPath, errors);
        if (repo !== undefined) {
            if (repos.has(repo)) errors.push(`${itemPath}.repo is duplicated`);
            repos.add(repo);
        }
        if (repo !== undefined && commit !== undefined) result.push({ repo, commit });
    });
    return result;
}

interface ReceiptFile {
    path: string;
    source: { repo: string; blob_sha: string } | null;
    destination: string;
    destination_sha256: string | undefined;
    transformation: string | undefined;
    class: string | undefined;
}

interface ReceiptShape {
    wave: Wave | undefined;
    sources: RepoCommit[];
    /// Source trees whose every blob must be listed in `files` or `excluded`.
    scope: { repo: string; tree: string }[];
    excluded: { repo: string; blob_sha: string }[];
    files: ReceiptFile[];
    readiness: string | undefined;
    registry: string | undefined;
    waivers: string | undefined;
    propertyImpact: string | undefined;
    architectureImpact: string | undefined;
    gates: Record<string, string>;
}

function validateReceiptFile(entry: unknown, path: string, errors: string[]): ReceiptFile | undefined {
    const file = requireObject(entry, path, errors);
    if (file === undefined) return undefined;
    const classification = requireEnum(file, "class", FILE_CLASSES, path, errors);
    const transformation = requireEnum(file, "transformation", TRANSFORMATIONS, path, errors);
    const destination = requireString(file, "destination", path, errors);
    const destinationSha = requireDigest(file, "destination_sha256", path, errors);
    const review = requireObject(file.review_evidence, `${path}.review_evidence`, errors);

    let source: ReceiptFile["source"] = null;
    if (!Object.hasOwn(file, "source")) {
        errors.push(`${path}.source must be present (null for authored files)`);
    } else if (file.source === null) {
        if (classification !== "new-authored" && classification !== "generated" && classification !== "contract-generated") {
            errors.push(`${path}.source may be null only for new-authored or generated files`);
        }
        if (classification === "new-authored" && transformation !== "authored") {
            errors.push(`${path}.transformation must be authored when source is null`);
        }
    } else {
        const sourceObject = requireObject(file.source, `${path}.source`, errors);
        if (sourceObject !== undefined) {
            const repo = requireString(sourceObject, "repo", `${path}.source`, errors);
            const blob = requireString(sourceObject, "blob_sha", `${path}.source`, errors);
            if (blob !== undefined && !BLOB_RE.test(blob)) {
                errors.push(`${path}.source.blob_sha must be a git blob id`);
            }
            if (repo !== undefined && blob !== undefined) {
                source = { repo, blob_sha: blob };
            }
        }
        if (classification === "new-authored") {
            errors.push(`${path}.source must be null for new-authored files`);
        }
        if (transformation === "authored") {
            errors.push(`${path}.transformation authored requires source null`);
        }
    }

    if (review !== undefined) {
        const reviewPath = `${path}.review_evidence`;
        switch (classification) {
            case "human-authored":
                requireString(review, "doc_rigor", reviewPath, errors);
                break;
            case "generated":
                requireString(review, "regeneration", reviewPath, errors);
                break;
            case "contract-generated":
                requireString(review, "generator", reviewPath, errors);
                requireString(review, "semantic_review", reviewPath, errors);
                break;
            case "captured":
                requireCommit(review, "captured_at_commit", reviewPath, errors);
                requireString(review, "capture_command", reviewPath, errors);
                if (transformation !== "verbatim") {
                    errors.push(`${path}.transformation must be verbatim for captured files`);
                }
                break;
            case "new-authored":
                requireString(review, "design_review", reviewPath, errors);
                requireString(review, "negative_tests", reviewPath, errors);
                break;
            default:
                break;
        }
    }

    if (destination === undefined) return undefined;
    return {
        path,
        source,
        destination,
        destination_sha256: destinationSha,
        transformation,
        class: classification,
    };
}

function validateGates(root: JsonObject, errors: string[]): Record<string, string> {
    const result: Record<string, string> = {};
    const gates = requireObject(root.gates, "$.gates", errors);
    if (gates === undefined) return result;
    if (Object.keys(gates).length === 0) {
        errors.push("$.gates must declare at least one blocking gate");
    }
    for (const [name, value] of Object.entries(gates)) {
        if (typeof value !== "string" || !GATE_STATES.includes(value as (typeof GATE_STATES)[number])) {
            errors.push(`$.gates.${name} must be one of: ${GATE_STATES.join(", ")}`);
            continue;
        }
        result[name] = value;
    }
    return result;
}

function validateKnownRed(root: JsonObject, gates: Record<string, string>, errors: string[]): void {
    if (root.known_red === undefined) return;
    const seen = new Set<string>();
    requireArray(root, "known_red", "$", errors).forEach((entry, index) => {
        const path = `$.known_red[${index}]`;
        const knownRed = requireObject(entry, path, errors);
        if (knownRed === undefined) return;
        const gate = requireString(knownRed, "gate", path, errors);
        const kind = requireEnum(knownRed, "kind", [...WAIVER_KINDS, ...NONWAIVABLE_KINDS], path, errors);
        requireEnum(knownRed, "status", ["fail", "cannot_run", "not_run"], path, errors);
        requireString(knownRed, "justification", path, errors);
        requireString(knownRed, "source_repo", path, errors);
        if (kind !== undefined && (NONWAIVABLE_KINDS as readonly string[]).includes(kind)) {
            errors.push(`${path}.kind is nonwaivable`);
        }
        if (gate !== undefined) {
            if (seen.has(gate)) errors.push(`${path}.gate is duplicated`);
            seen.add(gate);
            if (Object.hasOwn(gates, gate)) {
                errors.push(`${path}.gate is also declared as a blocking gate`);
            }
        }
    });
}

function validateReceiptShape(root: JsonObject, errors: string[]): ReceiptShape {
    validateSchemaVersion(root, errors);
    const wave = requireWave(root, "wave", "$", errors);
    const controlOnly = wave !== undefined && CONTROL_ONLY_WAVES.includes(wave);

    const sources = validateRepoCommits(root.sources, "$.sources", errors, controlOnly);
    validateRepoCommits(root.catalogs, "$.catalogs", errors, controlOnly);
    const sourceRepos = new Set(sources.map((source) => source.repo));

    const readiness = requireString(root, "readiness", "$", errors);
    const registry = requireString(root, "registry", "$", errors);
    const waivers = requireString(root, "waivers", "$", errors);

    const impactField = (key: string): string | undefined => {
        const value = requireString(root, key, "$", errors);
        if (value === NOT_APPLICABLE && !controlOnly) {
            errors.push(`$.${key} may be ${NOT_APPLICABLE} only for waves ${CONTROL_ONLY_WAVES.join(", ")}`);
            return undefined;
        }
        if (value !== NOT_APPLICABLE && controlOnly && value !== undefined) {
            errors.push(`$.${key} must be ${NOT_APPLICABLE} for wave ${wave}`);
            return undefined;
        }
        return value;
    };
    const propertyImpact = impactField("property_impact");
    const architectureImpact = impactField("architecture_impact");

    const scope: ReceiptShape["scope"] = [];
    requireArray(root, "scope", "$", errors).forEach((entry, index) => {
        const path = `$.scope[${index}]`;
        const item = requireObject(entry, path, errors);
        if (item === undefined) return;
        const repo = requireString(item, "repo", path, errors);
        const tree = requireString(item, "tree", path, errors);
        if (tree !== undefined && !BLOB_RE.test(tree)) errors.push(`${path}.tree must be a git tree id`);
        if (repo !== undefined && !sourceRepos.has(repo)) {
            errors.push(`${path}.repo ${repo} has no pinned source commit`);
        }
        if (repo !== undefined && tree !== undefined) scope.push({ repo, tree });
    });
    if (!controlOnly && scope.length === 0) {
        errors.push("$.scope must declare at least one source tree");
    }

    const excluded: ReceiptShape["excluded"] = [];
    if (root.excluded !== undefined) {
        requireArray(root, "excluded", "$", errors).forEach((entry, index) => {
            const path = `$.excluded[${index}]`;
            const item = requireObject(entry, path, errors);
            if (item === undefined) return;
            const repo = requireString(item, "repo", path, errors);
            const blob = requireString(item, "blob_sha", path, errors);
            requireString(item, "reason", path, errors);
            if (blob !== undefined && !BLOB_RE.test(blob)) errors.push(`${path}.blob_sha must be a git blob id`);
            if (repo !== undefined && blob !== undefined) excluded.push({ repo, blob_sha: blob });
        });
    }

    const destinations = new Set<string>();
    const files: ReceiptFile[] = [];
    const rawFiles = requireArray(root, "files", "$", errors);
    if (rawFiles.length === 0) errors.push("$.files must contain at least one file");
    rawFiles.forEach((entry, index) => {
        const file = validateReceiptFile(entry, `$.files[${index}]`, errors);
        if (file === undefined) return;
        if (destinations.has(file.destination)) errors.push(`${file.path}.destination is duplicated`);
        destinations.add(file.destination);
        if (file.source !== null && !sourceRepos.has(file.source.repo)) {
            errors.push(`${file.path}.source.repo ${file.source.repo} has no pinned source commit`);
        }
        files.push(file);
    });

    const gates = validateGates(root, errors);
    validateKnownRed(root, gates, errors);

    return {
        wave,
        sources,
        scope,
        excluded,
        files,
        readiness,
        registry,
        waivers,
        propertyImpact,
        architectureImpact,
        gates,
    };
}

interface RegistryShape {
    identities: { value: string; class: string }[];
    typescript: { path: string; class: string }[];
    families: { name: string; class: string; literals: string[] }[];
    authored: string[];
    fixtures: { path: string; role: string }[];
}

function validateIdentityEntry(entry: JsonObject, path: string, errors: string[]): { value: string; class: string } | undefined {
    const value = requireString(entry, "value", path, errors);
    const classification = requireEnum(entry, "class", IDENTITY_CLASSES, path, errors);
    requireString(entry, "rationale", path, errors);
    requireStringArray(entry, "evidence", path, errors, 1);
    if (value === undefined || classification === undefined) return undefined;
    return { value, class: classification };
}

function validateTypescriptEntry(entry: JsonObject, path: string, errors: string[]): { path: string; class: string } | undefined {
    const filePath = requireString(entry, "path", path, errors);
    const classification = requireEnum(entry, "class", TYPESCRIPT_CLASSES, path, errors);
    requireString(entry, "rationale", path, errors);
    if (filePath !== undefined && filePath.includes("*")) {
        errors.push(`${path}.path must name one file, not a glob`);
    }
    if (classification === "permanent") {
        requireString(entry, "owner", path, errors);
        requireString(entry, "tier", path, errors);
        requireString(entry, "contract_test", path, errors);
        requireString(entry, "rust_parity_anchor", path, errors);
    }
    if (classification === "transitional") {
        requireString(entry, "bead_id", path, errors);
        requireString(entry, "rust_replacement", path, errors);
        requireString(entry, "parity_proof", path, errors);
        requireString(entry, "deletion_condition", path, errors);
    }
    if (filePath === undefined || classification === undefined) return undefined;
    return { path: filePath, class: classification };
}

function validateFamilyEntry(
    entry: JsonObject,
    path: string,
    errors: string[],
): { name: string; class: string; literals: string[] } | undefined {
    const name = requireString(entry, "name", path, errors);
    const classification = requireEnum(entry, "class", FAMILY_CLASSES, path, errors);
    requireString(entry, "rationale", path, errors);
    const literalMinimum = classification === "planned" || classification === "absent-by-design" ? 0 : 1;
    const literals = requireStringArray(entry, "literals", path, errors, literalMinimum);
    requireStringArray(entry, "paths", path, errors);
    const mismatch = requireEnum(entry, "mismatch_behavior", MISMATCH_BEHAVIORS, path, errors);
    const probe = optionalString(entry, "probe", path, errors);
    const baselineSource = optionalString(entry, "baseline_source", path, errors);
    const rebuild = optionalString(entry, "rebuild_contract", path, errors);
    const parent = optionalString(entry, "family", path, errors);
    const restore = optionalString(entry, "restore_policy", path, errors);

    switch (classification) {
        case "retained-authoritative-baseline":
            if (mismatch !== "refuse-without-mutation" && mismatch !== "quarantine-and-rebootstrap") {
                errors.push(`${path}.mismatch_behavior must refuse or quarantine for an authoritative family`);
            }
            if (probe === undefined) errors.push(`${path}.probe is required for an authoritative family`);
            if (baselineSource === undefined) errors.push(`${path}.baseline_source is required for an authoritative family`);
            if (restore === undefined) errors.push(`${path}.restore_policy is required for an authoritative family`);
            break;
        case "retained-derived-projection":
            if (mismatch !== "rebuild") errors.push(`${path}.mismatch_behavior must be rebuild for a derived projection`);
            if (rebuild !== "deterministic" && rebuild !== "provider-dependent") {
                errors.push(`${path}.rebuild_contract must be deterministic or provider-dependent`);
            }
            if (baselineSource !== undefined) errors.push(`${path}.baseline_source is not valid for a derived projection`);
            break;
        case "retained-coordination-state":
            if (mismatch !== "recreate") errors.push(`${path}.mismatch_behavior must be recreate for coordination state`);
            break;
        case "foreign":
            if (mismatch !== "skip-and-report") errors.push(`${path}.mismatch_behavior must be skip-and-report for a foreign store`);
            if (probe === undefined) errors.push(`${path}.probe is required for a foreign store`);
            if (baselineSource !== undefined) errors.push(`${path}.baseline_source is not valid for a foreign store`);
            if (restore !== undefined) errors.push(`${path}.restore_policy is not valid for a foreign store`);
            break;
        case "component-of-family":
            if (parent === undefined) errors.push(`${path}.family is required for a family component`);
            break;
        case "planned":
        case "absent-by-design":
        case "test-only":
            if (mismatch !== "none") errors.push(`${path}.mismatch_behavior must be none for class ${classification}`);
            break;
        default:
            break;
    }
    if (name === undefined || classification === undefined) return undefined;
    return { name, class: classification, literals };
}

function validateRegistryShape(root: JsonObject, errors: string[]): RegistryShape {
    validateSchemaVersion(root, errors);
    const shape: RegistryShape = { identities: [], typescript: [], families: [], authored: [], fixtures: [] };
    const identityValues = new Set<string>();
    const typescriptPaths = new Set<string>();
    const familyNames = new Set<string>();
    const literals = new Map<string, string>();
    const authoredPaths = new Set<string>();
    const fixturePaths = new Set<string>();
    const generatorTargets: { target: string; path: string }[] = [];

    const entries = requireArray(root, "entries", "$", errors);
    if (entries.length === 0) errors.push("$.entries must contain at least one entry");
    entries.forEach((raw, index) => {
        const path = `$.entries[${index}]`;
        const entry = requireObject(raw, path, errors);
        if (entry === undefined) return;
        const kind = requireEnum(entry, "kind", ["identity", "typescript", "family", "authored", "fixture"], path, errors);
        switch (kind) {
            case "identity": {
                const identity = validateIdentityEntry(entry, path, errors);
                if (identity === undefined) return;
                if (identityValues.has(identity.value)) errors.push(`${path}.value is duplicated`);
                identityValues.add(identity.value);
                shape.identities.push(identity);
                break;
            }
            case "typescript": {
                const ts = validateTypescriptEntry(entry, path, errors);
                if (ts === undefined) return;
                if (typescriptPaths.has(ts.path)) errors.push(`${path}.path is duplicated`);
                typescriptPaths.add(ts.path);
                shape.typescript.push(ts);
                break;
            }
            case "family": {
                const family = validateFamilyEntry(entry, path, errors);
                if (family === undefined) return;
                if (familyNames.has(family.name)) errors.push(`${path}.name is duplicated`);
                familyNames.add(family.name);
                for (const literal of family.literals) {
                    const owner = literals.get(literal);
                    if (owner !== undefined) {
                        errors.push(`${path}.literals contains ${literal}, already owned by family ${owner}`);
                    }
                    literals.set(literal, family.name);
                }
                shape.families.push(family);
                break;
            }
            case "authored": {
                const authoredPath = requireString(entry, "path", path, errors);
                requireString(entry, "rationale", path, errors);
                if (authoredPath === undefined) return;
                if (authoredPaths.has(authoredPath)) errors.push(`${path}.path is duplicated`);
                authoredPaths.add(authoredPath);
                shape.authored.push(authoredPath);
                break;
            }
            case "fixture": {
                const fixturePath = requireString(entry, "path", path, errors);
                const role = requireEnum(entry, "role", FIXTURE_ROLES, path, errors);
                requireString(entry, "rationale", path, errors);
                requireStringArray(entry, "evidence", path, errors, 1);
                if (role === "generator") {
                    const target = requireString(entry, "fixture", path, errors);
                    if (target !== undefined) generatorTargets.push({ target, path });
                }
                if (fixturePath === undefined || role === undefined) return;
                if (fixturePaths.has(fixturePath)) errors.push(`${path}.path is duplicated`);
                fixturePaths.add(fixturePath);
                shape.fixtures.push({ path: fixturePath, role });
                break;
            }
            default:
                break;
        }
    });
    // Each generator target must name a registered byte-stable fixture.
    const byteStable = new Set(
        shape.fixtures.filter((fixture) => fixture.role === "byte-stable").map((fixture) => fixture.path),
    );
    for (const { target, path } of generatorTargets) {
        if (!byteStable.has(target)) {
            errors.push(`${path}.fixture ${target} is not a registered byte-stable fixture`);
        }
    }
    return shape;
}

interface WaiverShape {
    wave: Wave | undefined;
    gates: Set<string>;
}

function validateWaiversShape(root: JsonObject, errors: string[]): WaiverShape {
    validateSchemaVersion(root, errors);
    const wave = requireWave(root, "wave", "$", errors);
    const gates = new Set<string>();
    const ids = new Set<string>();
    const waivers = requireArray(root, "waivers", "$", errors);
    if (wave === "U8" && waivers.length > 0) {
        errors.push("$.waivers must be empty for wave U8");
    }
    waivers.forEach((raw, index) => {
        const path = `$.waivers[${index}]`;
        const waiver = requireObject(raw, path, errors);
        if (waiver === undefined) return;
        const id = requireString(waiver, "id", path, errors);
        const gate = requireString(waiver, "gate", path, errors);
        const kind = requireEnum(waiver, "kind", [...WAIVER_KINDS, ...NONWAIVABLE_KINDS], path, errors);
        requireString(waiver, "owner", path, errors);
        requireString(waiver, "approver", path, errors);
        requireString(waiver, "bead_id", path, errors);
        const created = requireString(waiver, "created_at", path, errors);
        const expires = requireWave(waiver, "expires_by_wave", path, errors);
        requireString(waiver, "closure_condition", path, errors);
        requireStringArray(waiver, "evidence", path, errors, 1);
        if (created !== undefined && !ISO_DATE_RE.test(created)) {
            errors.push(`${path}.created_at must be an ISO-8601 UTC date`);
        }
        if (kind !== undefined && (NONWAIVABLE_KINDS as readonly string[]).includes(kind)) {
            errors.push(`${path}.kind ${kind} is nonwaivable`);
        }
        if (wave !== undefined && expires !== undefined && waveIndex(expires) <= waveIndex(wave)) {
            errors.push(`${path} expired: expires_by_wave ${expires} is not after wave ${wave}`);
        }
        if (expires === "U8") {
            errors.push(`${path}.expires_by_wave may not reach U8`);
        }
        if (id !== undefined) {
            if (ids.has(id)) errors.push(`${path}.id is duplicated`);
            ids.add(id);
        }
        if (gate !== undefined) {
            if (gates.has(gate)) errors.push(`${path}.gate is duplicated`);
            gates.add(gate);
        }
    });
    return { wave, gates };
}

function validatePropertyCatalogShape(root: JsonObject, errors: string[]): void {
    validateSchemaVersion(root, errors);
    requireString(root, "part", "$", errors);
    requireString(root, "source", "$", errors);
    requireDigest(root, "source_sha256", "$", errors);
    const seen = new Set<string>();
    const records = requireArray(root, "records", "$", errors);
    if (records.length === 0) errors.push("$.records must contain at least one property");
    records.forEach((entry, index) => {
        const path = `$.records[${index}]`;
        const record = requireObject(entry, path, errors);
        if (record === undefined) return;
        const slug = requireString(record, "slug", path, errors);
        requireEnum(record, "type", ["safety", "liveness", "reachability"], path, errors);
        requireEnum(record, "reachability", ["default-production", "explicit-config-only", "test-only"], path, errors);
        const status = requireEnum(record, "status", ["active", "invalidated"], path, errors);
        const exercised = requireObject(record.exercised, `${path}.exercised`, errors);
        if (exercised !== undefined) {
            requireEnum(exercised, "state", ["yes", "partial", "not-yet"], `${path}.exercised`, errors);
            requireString(exercised, "note", `${path}.exercised`, errors);
        }
        requireString(record, "guarantee", path, errors);
        const check = requireObject(record.check, `${path}.check`, errors);
        if (check !== undefined) {
            requireEnum(check, "semantics", CHECK_SEMANTICS, `${path}.check`, errors);
            requireString(check, "condition", `${path}.check`, errors);
        }
        requireString(record, "fault_timing", path, errors);
        requireString(record, "required_faults", path, errors);
        const confidence = requireObject(record.confidence, `${path}.confidence`, errors);
        if (confidence !== undefined) {
            requireEnum(confidence, "level", ["high", "medium", "low"], `${path}.confidence`, errors);
            requireString(confidence, "evidence", `${path}.confidence`, errors);
        }
        requireString(record, "existing_check", path, errors);
        requireString(record, "impact", path, errors);
        requireStringArray(record, "open_questions", path, errors);
        if (slug !== undefined) {
            if (seen.has(slug)) errors.push(`${path}.slug is duplicated`);
            seen.add(slug);
        }
        if (status === "invalidated") {
            requireString(record, "unreachability_evidence", path, errors);
        }
    });
}

interface PropertyImpactShape {
    pointers: { path: string; pointer: string }[];
    /// Core records whose hashes must match the destination bytes they claim to cover.
    cores: {
        path: string;
        files: string[];
        check_pointer: string | undefined;
        evidence_pointer: string | undefined;
        code_hash: string | undefined;
        check_hash: string | undefined;
        evidence_digest: string | undefined;
        new_evidence_digest: string | undefined;
    }[];
}

const SOURCE_EXERCISED = ["yes", "partial", "not-yet"] as const;
const SOURCE_CHECK_STATUS = ["audited", "unaudited", "none"] as const;
const SOURCE_VERDICTS = ["pass", "PARTIAL", "BLOCKED", "INCONCLUSIVE", "not-evaluated"] as const;

interface SourceStatus {
    exercised: string | undefined;
    check_status: string | undefined;
    portfolio_verdict: string | undefined;
    known_violation: boolean | undefined;
}

function validateSourceStatus(record: JsonObject, key: string, path: string, errors: string[]): SourceStatus | undefined {
    const status = requireObject(record[key], `${path}.${key}`, errors);
    if (status === undefined) return undefined;
    const statusPath = `${path}.${key}`;
    return {
        exercised: requireEnum(status, "exercised", SOURCE_EXERCISED, statusPath, errors),
        check_status: requireEnum(status, "check_status", SOURCE_CHECK_STATUS, statusPath, errors),
        portfolio_verdict: requireEnum(status, "portfolio_verdict", SOURCE_VERDICTS, statusPath, errors),
        known_violation: requireBoolean(status, "known_violation", statusPath, errors),
    };
}

function sourceStatusIsClean(status: SourceStatus): boolean {
    return (
        status.exercised === "yes" &&
        status.check_status === "audited" &&
        (status.portfolio_verdict === "pass" || status.portfolio_verdict === "not-evaluated") &&
        status.known_violation === false
    );
}

function sameSourceStatus(a: JsonObject | undefined, b: JsonObject | undefined): boolean {
    if (a === undefined || b === undefined) return false;
    const keys = ["exercised", "check_status", "portfolio_verdict", "known_violation"];
    return keys.every((key) => a[key] === b[key]);
}

function validatePropertyImpactShape(root: JsonObject, errors: string[]): PropertyImpactShape {
    validateSchemaVersion(root, errors);
    requireWave(root, "wave", "$", errors);
    const pointers: PropertyImpactShape["pointers"] = [];

    const provenance = requireArray(root, "provenance", "$", errors);
    if (provenance.length === 0) errors.push("$.provenance must contain at least one repository");
    provenance.forEach((entry, index) => {
        const path = `$.provenance[${index}]`;
        const item = requireObject(entry, path, errors);
        if (item === undefined) return;
        requireString(item, "repo", path, errors);
        const source = requireCommit(item, "source_commit", path, errors);
        const catalog = requireCommit(item, "catalog_commit", path, errors);
        if (source !== undefined && catalog !== undefined && source !== catalog) {
            errors.push(`${path} catalog_commit ${catalog} differs from source_commit ${source}; reconcile the catalog before selecting proofs`);
        }
    });
    requireCommit(root, "destination_commit", "$", errors);

    const touched = requireStringArray(root, "touched_files", "$", errors);
    if (touched.length === 0) errors.push("$.touched_files must contain at least one file");

    const scopeDecisions = new Map<string, string>();
    if (root.scope_decisions !== undefined) {
        requireArray(root, "scope_decisions", "$", errors).forEach((entry, index) => {
            const path = `$.scope_decisions[${index}]`;
            const item = requireObject(entry, path, errors);
            if (item === undefined) return;
            const slug = requireString(item, "slug", path, errors);
            const decision = requireEnum(item, "decision", ["mechanism-left-scope", "subsystem-dropped"], path, errors);
            requireString(item, "evidence", path, errors);
            if (slug !== undefined && decision !== undefined) scopeDecisions.set(slug, decision);
        });
    }

    const covered = new Set<string>();
    const seen = new Set<string>();
    const cores: PropertyImpactShape["cores"] = [];
    const records = requireArray(root, "records", "$", errors);
    if (records.length === 0) errors.push("$.records must contain at least one disposition");
    records.forEach((entry, index) => {
        const path = `$.records[${index}]`;
        const record = requireObject(entry, path, errors);
        if (record === undefined) return;
        const slug = requireString(record, "slug", path, errors);
        const classification = requireEnum(record, "classification", PROPERTY_CLASSIFICATIONS, path, errors);
        requireEnum(record, "relationship", ["mapped", "isolated"], path, errors);
        const files = requireStringArray(record, "files", path, errors);
        if (slug !== undefined) {
            if (seen.has(slug)) errors.push(`${path}.slug is duplicated`);
            seen.add(slug);
        }

        switch (classification) {
            case "core": {
                files.forEach((file) => covered.add(file));
                const disposition = requireEnum(record, "disposition", ["pass", "blocked"], path, errors);
                const source = validateSourceStatus(record, "source_status", path, errors);
                requireString(record, "strategy_decision", path, errors);
                const auditVerdict = requireEnum(record, "audit_verdict", ["pass", "fail", "vacuous", "pending"], path, errors);
                // Every digest names the file it is compared against: `evidence_digest` is
                // the evidence record, `check_hash` the file the check pointer names, and
                // `code_hash` the covered files. A digest without a file is never compared.
                const evidenceDigest = requireDigest(record, "evidence_digest", path, errors);
                const evidencePointer = requireString(record, "evidence_pointer", path, errors);
                const codeHash = requireDigest(record, "code_hash", path, errors);
                const checkPointer = requireString(record, "check_pointer", path, errors);
                if (checkPointer !== undefined && !checkPointer.includes("#")) {
                    errors.push(`${path}.check_pointer must name the check as path#check`);
                }
                const checkHash = requireDigest(record, "check_hash", path, errors);
                if (checkPointer !== undefined) pointers.push({ path: `${path}.check_pointer`, pointer: checkPointer });
                if (evidencePointer !== undefined) pointers.push({ path: `${path}.evidence_pointer`, pointer: evidencePointer });
                const newEvidence = isObject(record.new_evidence) ? record.new_evidence : undefined;
                cores.push({
                    path,
                    files,
                    check_pointer: checkPointer,
                    evidence_pointer: evidencePointer,
                    code_hash: codeHash,
                    check_hash: checkHash,
                    evidence_digest: evidenceDigest,
                    new_evidence_digest: typeof newEvidence?.digest === "string" ? newEvidence.digest : undefined,
                });
                const targets = requireStringArray(record, "target_configurations", path, errors);
                const attempts = requireInteger(record, "evidence_attempts", path, errors);
                if (targets.length === 0) {
                    errors.push(`${path}.target_configurations must contain at least one target`);
                }
                if (source !== undefined && !sourceStatusIsClean(source)) {
                    const fresh = requireObject(record.new_evidence, `${path}.new_evidence`, errors);
                    if (fresh === undefined) {
                        errors.push(`${path} core record with source status ${describeSourceStatus(source)} needs new discriminating evidence`);
                    } else {
                        requireDigest(fresh, "digest", `${path}.new_evidence`, errors);
                        requireString(fresh, "description", `${path}.new_evidence`, errors);
                    }
                }
                if (auditVerdict !== undefined && auditVerdict !== "pass") {
                    errors.push(`${path}.audit_verdict must equal pass`);
                }
                if (disposition !== "pass") {
                    errors.push(`${path} blocks the wave`);
                    if (attempts !== undefined && attempts >= 2 && slug !== undefined && !scopeDecisions.has(slug)) {
                        errors.push(`${path} needs a scope decision after ${attempts} failed evidence attempts`);
                    }
                }
                break;
            }
            case "carried-forward": {
                files.forEach((file) => covered.add(file));
                const provenanceValue = requireString(record, "provenance", path, errors);
                if (provenanceValue !== undefined && !PROVENANCE_RE.test(provenanceValue)) {
                    errors.push(`${path}.provenance must have the form <repo>@<sha>`);
                }
                validateSourceStatus(record, "source_status", path, errors);
                validateSourceStatus(record, "destination_status", path, errors);
                const sourceStatus = isObject(record.source_status) ? record.source_status : undefined;
                const destinationStatus = isObject(record.destination_status) ? record.destination_status : undefined;
                if (sourceStatus !== undefined && destinationStatus !== undefined && !sameSourceStatus(sourceStatus, destinationStatus)) {
                    errors.push(`${path} carried-forward record changed status; destination_status must equal source_status`);
                }
                const check = requireString(record, "check_pointer", path, errors);
                const evidence = requireString(record, "evidence_pointer", path, errors);
                if (check !== undefined) pointers.push({ path: `${path}.check_pointer`, pointer: check });
                if (evidence !== undefined) pointers.push({ path: `${path}.evidence_pointer`, pointer: evidence });
                break;
            }
            case "invalidated": {
                files.forEach((file) => covered.add(file));
                requireString(record, "historical_evidence", path, errors);
                requireString(record, "unreachability_evidence", path, errors);
                break;
            }
            case "excluded":
                requireString(record, "isolation_evidence", path, errors);
                break;
            default:
                break;
        }
    });
    for (const file of touched) {
        if (!covered.has(file)) {
            errors.push(`$.touched_files has uncovered file: ${file}; run property discovery for it before approval`);
        }
    }
    return { pointers, cores };
}

function describeSourceStatus(status: SourceStatus): string {
    const parts = [
        `exercised=${status.exercised ?? "?"}`,
        `check=${status.check_status ?? "?"}`,
        `verdict=${status.portfolio_verdict ?? "?"}`,
    ];
    if (status.known_violation) parts.push("known-violation");
    return parts.join(",");
}

function validateArchitectureCandidate(
    candidateEntry: unknown,
    path: string,
    errors: string[],
): { title: string | undefined; strength: string | undefined; origin: string | undefined; decision: string | undefined } {
    const candidate = requireObject(candidateEntry, path, errors);
    if (candidate === undefined) return { title: undefined, strength: undefined, origin: undefined, decision: undefined };
    const strength = requireEnum(candidate, "strength", ["Strong", "Worth exploring", "Speculative"], path, errors);
    const decision = requireEnum(candidate, "decision", ["accepted", "rejected", "recorded", "unresolved"], path, errors);
    const origin = requireEnum(candidate, "origin", ["original-scope", "loop-created"], path, errors);
    const title = requireString(candidate, "title", path, errors);
    requireStringArray(candidate, "modules", path, errors);
    requireString(candidate, "interface", path, errors);
    requireString(candidate, "implementation", path, errors);
    const deletion = requireObject(candidate.deletion_test, `${path}.deletion_test`, errors);
    if (deletion !== undefined) {
        requireBoolean(deletion, "concentrates_complexity", `${path}.deletion_test`, errors);
        requireString(deletion, "rationale", `${path}.deletion_test`, errors);
    }
    const benefits = requireObject(candidate.benefits, `${path}.benefits`, errors);
    let hasBenefit = false;
    if (benefits !== undefined) {
        const flags = ["locality", "leverage", "testability"].map((key) =>
            requireBoolean(benefits, key, `${path}.benefits`, errors),
        );
        hasBenefit = flags.some((flag) => flag === true);
    }
    const claimsFlexibility = requireBoolean(candidate, "claims_flexibility", path, errors);
    const adapters = requireStringArray(candidate, "adapters", path, errors);
    const routes = requireStringArray(candidate, "specialist_routes", path, errors);

    if (strength === "Strong" && decision !== "accepted" && decision !== "rejected") {
        if (origin === "original-scope") {
            errors.push(`${path} is an original-scope Strong candidate that is neither accepted nor rejected`);
        } else if (decision === "recorded") {
            requireString(candidate, "bead_id", path, errors);
        } else {
            errors.push(`${path} is a loop-created Strong candidate that must be accepted, rejected, or recorded with a bead`);
        }
    }
    if (decision === "accepted") {
        requireString(candidate, "final_verdict", path, errors);
        requireString(candidate, "implementation_evidence", path, errors);
        requireString(candidate, "property_impact", path, errors);
        requireStringArray(candidate, "affected_properties", path, errors, 1);
        if (routes.length === 0) {
            errors.push(`${path}.specialist_routes must contain at least one route`);
        }
        if (!hasBenefit) {
            errors.push(`${path} has no locality, leverage, or testability benefit`);
        }
        if (deletion?.concentrates_complexity !== true) {
            errors.push(`${path} fails the deletion test`);
        }
        if (strength !== "Strong") {
            errors.push(`${path} accepted candidate must be Strong: deletion test and interface metric both pass`);
        }
    }
    if (decision === "rejected") requireString(candidate, "rationale", path, errors);
    if (claimsFlexibility === true && adapters.length < 2) {
        errors.push(`${path} claims flexibility without two current adapters`);
    }
    return { title, strength, origin, decision };
}

function validateArchitectureImpactShape(root: JsonObject, errors: string[]): void {
    validateSchemaVersion(root, errors);
    requireWave(root, "wave", "$", errors);
    let prePort = 0;
    const postIterations = new Set<number>();
    const originalStrong = new Set<string>();
    const reports = requireArray(root, "reports", "$", errors);
    reports.forEach((entry, reportIndex) => {
        const reportPath = `$.reports[${reportIndex}]`;
        const report = requireObject(entry, reportPath, errors);
        if (report === undefined) return;
        const phase = requireEnum(report, "phase", ["pre-port", "post-integration"], reportPath, errors);
        const iteration = requireInteger(report, "iteration", reportPath, errors);
        if (phase === "pre-port") {
            prePort += 1;
            if (prePort > 1) errors.push(`${reportPath}.phase pre-port is duplicated`);
            if (iteration !== undefined && iteration !== 0) errors.push(`${reportPath}.iteration must be 0 for pre-port`);
        } else if (phase === "post-integration" && iteration !== undefined) {
            if (iteration < 1 || iteration > 2) {
                errors.push(`${reportPath}.iteration must be 1 or 2 for post-integration; a third iteration needs an escalation record instead`);
            }
            if (postIterations.has(iteration)) errors.push(`${reportPath}.iteration ${iteration} is duplicated`);
            postIterations.add(iteration);
        }
        const analyzed = requireObject(report.analyzed, `${reportPath}.analyzed`, errors);
        if (analyzed !== undefined) {
            requireString(analyzed, "repo", `${reportPath}.analyzed`, errors);
            requireCommit(analyzed, "commit", `${reportPath}.analyzed`, errors);
            requireDigest(analyzed, "scope_hash", `${reportPath}.analyzed`, errors);
            // A post-integration report judges the destination code, so it names the module
            // directories it covers and the digest of their tracked contents.
            if (phase === "post-integration") {
                requireStringArray(analyzed, "modules", `${reportPath}.analyzed`, errors, 1);
                requireDigest(analyzed, "modules_hash", `${reportPath}.analyzed`, errors);
            }
        }
        requireDigest(report, "report_hash", reportPath, errors);
        requireDigest(report, "skill_sha256", reportPath, errors);
        requireArray(report, "candidates", reportPath, errors).forEach((candidateEntry, index) => {
            const summary = validateArchitectureCandidate(candidateEntry, `${reportPath}.candidates[${index}]`, errors);
            if (summary.strength === "Strong" && summary.origin === "original-scope" && summary.title !== undefined) {
                originalStrong.add(summary.title);
            }
        });
    });
    if (prePort === 0) errors.push("$.reports is missing pre-port phase");
    if (postIterations.size === 0) errors.push("$.reports is missing post-integration phase");

    const escalation = root.escalation;
    if (originalStrong.size >= 3 && escalation === undefined) {
        errors.push(`$.escalation is required: ${originalStrong.size} original-scope Strong candidates in one wave`);
    }
    if (escalation !== undefined) {
        const item = requireObject(escalation, "$.escalation", errors);
        if (item !== undefined) {
            requireString(item, "candidate", "$.escalation", errors);
            requireEnum(item, "decision", ["mechanism-left-scope", "subsystem-dropped", "deferred-with-bead"], "$.escalation", errors);
            requireString(item, "bead_id", "$.escalation", errors);
            requireString(item, "rationale", "$.escalation", errors);
        }
    }
}

export function validateShape(kind: CheckKind, value: unknown): string[] {
    const errors: string[] = [];
    const root = requireObject(value, "$", errors);
    if (root === undefined) return errors;
    switch (kind) {
        case "receipt":
            validateReceiptShape(root, errors);
            break;
        case "registry":
            validateRegistryShape(root, errors);
            break;
        case "waivers":
            validateWaiversShape(root, errors);
            break;
        case "property-catalog":
            validatePropertyCatalogShape(root, errors);
            break;
        case "property-impact":
            validatePropertyImpactShape(root, errors);
            break;
        case "architecture-impact":
            validateArchitectureImpactShape(root, errors);
            break;
    }
    return errors;
}

export interface Context {
    root: string;
    checkouts: Record<string, string>;
}

export function defaultContext(root: string): Context {
    return { root, checkouts: {} };
}

/// A source repository alias resolves to `--checkout <alias>=<dir>` when given, otherwise to a
/// sibling directory of the destination checkout named after the alias.
function checkoutFor(ctx: Context, repo: string): string {
    return ctx.checkouts[repo] ?? join(dirname(ctx.root), repo);
}

function git(cwd: string, args: string[]): { ok: true; stdout: string } | { ok: false; error: string } {
    const result = spawnSync("git", args, { cwd, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
    if (result.error) return { ok: false, error: String(result.error) };
    if (result.status !== 0) return { ok: false, error: result.stderr.trim() || `git exited ${result.status}` };
    return { ok: true, stdout: result.stdout };
}

export function sha256(bytes: Buffer | string): string {
    return createHash("sha256").update(bytes).digest("hex");
}

function readJson(path: string, errors: string[], label: string): JsonObject | undefined {
    if (!existsSync(path)) {
        errors.push(`${label} ${path} does not exist`);
        return undefined;
    }
    try {
        const value = JSON.parse(readFileSync(path, "utf8")) as unknown;
        if (!isObject(value)) {
            errors.push(`${label} ${path} must contain a JSON object`);
            return undefined;
        }
        return value;
    } catch (error) {
        errors.push(`${label} ${path} is not valid JSON: ${String(error)}`);
        return undefined;
    }
}

function listFiles(dir: string, skip: (rel: string) => boolean = () => false): string[] {
    const out: string[] = [];
    const walk = (current: string): void => {
        for (const entry of readdirSync(current, { withFileTypes: true })) {
            const full = join(current, entry.name);
            const rel = relative(dir, full);
            if (skip(rel)) continue;
            if (entry.isDirectory()) walk(full);
            else if (entry.isFile()) out.push(rel);
        }
    };
    if (existsSync(dir) && statSync(dir).isDirectory()) walk(dir);
    return out.sort();
}

const SKIP_DIRS = new Set([".git", "node_modules", "target", "dist", ".worktrees"]);

function skipVendored(rel: string): boolean {
    return rel.split("/").some((segment) => SKIP_DIRS.has(segment));
}

/**
 * A pointer is `path` or `path#check`, where `check` names a declared test in the file: a
 * `fn check` in Rust or a `test("check"`, `it("check"`, or `describe("check"` in TypeScript.
 * A line number would keep resolving after the cited check moved, so it is not an anchor.
 * Returns the reason a pointer does not resolve, or undefined when it does.
 */
export function pointerProblem(root: string, pointer: string): string | undefined {
    const hash = pointer.indexOf("#");
    const filePart = hash === -1 ? pointer : pointer.slice(0, hash);
    const anchor = hash === -1 ? undefined : pointer.slice(hash + 1);
    if (filePart === "") return "names no file";
    const full = resolve(root, filePart);
    // A prefix comparison would admit a sibling directory whose name extends the root's;
    // the relative path decides containment.
    const inside = relative(resolve(root), full);
    if (inside === "" || inside.startsWith("..") || isAbsolute(inside)) return "escapes the destination root";
    if (!existsSync(full) || !statSync(full).isFile()) return "does not resolve in the destination tree";
    if (anchor === undefined) return undefined;
    if (/^L\d+$/.test(anchor)) return "uses a line anchor; name the check instead";
    if (!/^[A-Za-z_][A-Za-z0-9_ -]*$/.test(anchor)) return `has an anchor "${anchor}" that is not a check name`;
    const text = readFileSync(full, "utf8");
    const declared = filePart.endsWith(".rs")
        ? new RegExp(`^\\s*(?:pub(?:\\([^)]*\\))?\\s+)?(?:async\\s+)?fn\\s+${anchor}\\b`, "m").test(text)
        : filePart.endsWith(".ts")
          ? new RegExp(`\\b(?:test|it|describe)\\(\\s*"${anchor.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}"`).test(text)
          : false;
    return declared ? undefined : `names ${anchor}, which ${filePart} does not declare`;
}


/// Object types for `ids` in one `git cat-file --batch-check` call; missing ids are absent.
function objectTypes(checkout: string, ids: Iterable<string>): Map<string, string> | string {
    const input = [...ids].join("\n");
    if (input === "") return new Map();
    const result = spawnSync("git", ["cat-file", "--batch-check"], { cwd: checkout, input: `${input}\n`, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
    if (result.error) return String(result.error);
    if (result.status !== 0) return result.stderr.trim() || `git exited ${result.status}`;
    const types = new Map<string, string>();
    for (const line of result.stdout.split("\n")) {
        const [id, type] = line.trim().split(/\s+/);
        if (id !== undefined && type !== undefined && type !== "missing") types.set(id, type);
    }
    return types;
}

/// Tree ids that are part of the pinned commit's own snapshot, including its root tree.
function snapshotTrees(checkout: string, commit: string): Set<string> | string {
    const root = git(checkout, ["rev-parse", `${commit}^{tree}`]);
    if (!root.ok) return root.error;
    const listed = git(checkout, ["ls-tree", "-r", "-d", "--object-only", commit]);
    if (!listed.ok) return listed.error;
    const trees = new Set(listed.stdout.split("\n").map((line) => line.trim()).filter((line) => line !== ""));
    trees.add(root.stdout.trim());
    return trees;
}

/// Blob ids under a tree with the number of paths each occupies. Two paths with identical
/// bytes share one blob id, so a set would let one receipt entry account for both files.
function treeBlobs(checkout: string, tree: string): Map<string, number> | string {
    const result = git(checkout, ["ls-tree", "-r", "--object-only", tree]);
    if (!result.ok) return result.error;
    const counts = new Map<string, number>();
    for (const line of result.stdout.split("\n")) {
        const blob = line.trim();
        if (blob !== "") counts.set(blob, (counts.get(blob) ?? 0) + 1);
    }
    return counts;
}

function blobBytes(checkout: string, blob: string): Buffer | string {
    const result = spawnSync("git", ["cat-file", "blob", blob], { cwd: checkout, maxBuffer: 256 * 1024 * 1024 });
    if (result.error) return String(result.error);
    if (result.status !== 0) return result.stderr.toString().trim() || `git exited ${result.status}`;
    return result.stdout;
}

function verifyReceipt(shape: ReceiptShape, ctx: Context, errors: string[]): void {
    const wave = shape.wave;
    if (wave === undefined) return;

    let registryShape: RegistryShape | undefined;
    if (shape.registry !== undefined) {
        const registry = readJson(join(ctx.root, shape.registry), errors, "$.registry");
        if (registry !== undefined) {
            const registryErrors: string[] = [];
            registryShape = validateRegistryShape(registry, registryErrors);
            registryErrors.forEach((error) => errors.push(`$.registry: ${error}`));
        }
    }
    let waiverGates = new Set<string>();
    // A waiver expires when the wave it names has landed, whichever receipt is being
    // checked: the waiver's own wave is where it was written, not where it stops applying.
    const landed = landedWave(ctx.root, wave);
    if (shape.waivers !== undefined) {
        const waivers = readJson(join(ctx.root, shape.waivers), errors, "$.waivers");
        if (waivers !== undefined) {
            const waiverErrors: string[] = [];
            const waiverShape = validateWaiversShape(waivers, waiverErrors);
            waiverErrors.forEach((error) => errors.push(`$.waivers: ${error}`));
            if (waiverShape.wave !== undefined && waiverShape.wave !== wave) {
                errors.push(`$.waivers names wave ${waiverShape.wave}, receipt is ${wave}`);
            }
            for (const expired of expiredWaivers(waivers, landed)) {
                errors.push(`$.waivers ${expired.id} expired: expires_by_wave ${expired.expires} and wave ${landed} has landed; close the gate or record a new waiver`);
                waiverShape.gates.delete(expired.gate);
            }
            waiverGates = waiverShape.gates;
        }
    }
    for (const earlier of earlierWaiverFiles(ctx.root, wave)) {
        for (const expired of expiredWaivers(earlier.waivers, wave)) {
            errors.push(`${earlier.path} ${expired.id} expired: expires_by_wave ${expired.expires} is not after wave ${wave}; close the gate before recording this wave`);
        }
    }
    for (const [name, status] of Object.entries(shape.gates)) {
        if (status === "pass") continue;
        if (waiverGates.has(name)) continue;
        errors.push(`$.gates.${name} blocks the wave with status ${status}`);
    }

    if (shape.propertyImpact !== undefined && shape.propertyImpact !== NOT_APPLICABLE) {
        const impact = readJson(join(ctx.root, shape.propertyImpact), errors, "$.property_impact");
        if (impact !== undefined) {
            const impactErrors = verifyKind("property-impact", impact, ctx);
            impactErrors.forEach((error) => errors.push(`$.property_impact: ${error}`));
            if (impact.wave !== wave) errors.push(`$.property_impact names wave ${String(impact.wave)}, receipt is ${wave}`);
            // The impact's own touched list is written by the same hand as its records, so
            // the receipt's destinations decide which implementation files it must cover.
            const declared = new Set(Array.isArray(impact.touched_files) ? impact.touched_files.filter((entry): entry is string => typeof entry === "string") : []);
            for (const file of shape.files) {
                if (!/^crates\/.*\.(?:rs|sql)$/.test(file.destination)) continue;
                if (!declared.has(file.destination)) {
                    errors.push(`$.property_impact.touched_files omits ${file.destination}, which this receipt changes`);
                }
            }
        }
    }
    if (shape.architectureImpact !== undefined && shape.architectureImpact !== NOT_APPLICABLE) {
        const impact = readJson(join(ctx.root, shape.architectureImpact), errors, "$.architecture_impact");
        if (impact !== undefined) {
            const impactErrors = verifyKind("architecture-impact", impact, ctx);
            impactErrors.forEach((error) => errors.push(`$.architecture_impact: ${error}`));
            if (impact.wave !== wave) errors.push(`$.architecture_impact names wave ${String(impact.wave)}, receipt is ${wave}`);
        }
    }

    if (shape.readiness !== undefined) {
        verifyReadiness(shape, ctx, errors);
    }

    for (const file of shape.files) {
        const full = join(ctx.root, file.destination);
        if (!existsSync(full) || !statSync(full).isFile()) {
            errors.push(`${file.path}.destination ${file.destination} does not exist in the destination checkout`);
            continue;
        }
        const actual = sha256(readFileSync(full));
        if (file.destination_sha256 !== undefined && actual !== file.destination_sha256) {
            errors.push(`${file.path}.destination_sha256 is stale: destination ${file.destination} hashes to ${actual}`);
        }
        if (file.class === "new-authored" && registryShape !== undefined && !registryShape.authored.includes(file.destination)) {
            errors.push(`${file.path} is new-authored but the registry has no authored entry for ${file.destination}`);
        }
        const fixtureRole = registryShape?.fixtures.find((fixture) => fixture.path === file.destination)?.role;
        if (fixtureRole === "byte-stable" && file.transformation !== "verbatim" && file.transformation !== "authored") {
            errors.push(`${file.path} is a byte-stable fixture but its transformation is ${file.transformation ?? "missing"}, not verbatim or authored`);
        }
    }

    const commitByRepo = new Map(shape.sources.map((source) => [source.repo, source.commit]));
    const reachableCache = new Map<string, Set<string> | undefined>();
    // Every object reachable from the pinned commit, so a blob is verified as part of that commit
    // without naming where it lived in the source tree.
    const reachableFor = (repo: string): Set<string> | undefined => {
        if (reachableCache.has(repo)) return reachableCache.get(repo);
        const commit = commitByRepo.get(repo);
        let result: Set<string> | undefined;
        if (commit !== undefined) {
            const checkout = checkoutFor(ctx, repo);
            if (!existsSync(checkout)) {
                errors.push(`no checkout is available for source repository ${repo} at ${checkout}`);
            } else {
                const listed = git(checkout, ["rev-list", "--objects", "--no-object-names", commit]);
                if (!listed.ok) {
                    errors.push(`git rev-list failed for ${repo}@${commit}: ${listed.error}`);
                } else {
                    result = new Set(listed.stdout.split("\n").map((line) => line.trim()).filter((line) => line !== ""));
                }
            }
        }
        reachableCache.set(repo, result);
        return result;
    };

    // A commit or tree id is reachable too, so the type is checked as well as the reach.
    const typesByRepo = new Map<string, Map<string, string>>();
    for (const repo of commitByRepo.keys()) {
        const checkout = checkoutFor(ctx, repo);
        if (!existsSync(checkout)) continue;
        const ids = shape.files.flatMap((file) => (file.source?.repo === repo ? [file.source.blob_sha] : []));
        const types = objectTypes(checkout, ids);
        if (typeof types === "string") errors.push(`git cat-file --batch-check failed for ${repo}: ${types}`);
        else typesByRepo.set(repo, types);
    }

    for (const file of shape.files) {
        if (file.source === null) continue;
        const reachable = reachableFor(file.source.repo);
        if (reachable === undefined) continue;
        if (!reachable.has(file.source.blob_sha)) {
            errors.push(`${file.path}.source.blob_sha ${file.source.blob_sha} is not reachable from ${file.source.repo}@${commitByRepo.get(file.source.repo)}`);
            continue;
        }
        const type = typesByRepo.get(file.source.repo)?.get(file.source.blob_sha);
        if (type !== undefined && type !== "blob") {
            errors.push(`${file.path}.source.blob_sha ${file.source.blob_sha} is a ${type}, not a blob`);
            continue;
        }
        if (file.transformation === "verbatim" && file.destination_sha256 !== undefined) {
            const bytes = blobBytes(checkoutFor(ctx, file.source.repo), file.source.blob_sha);
            if (typeof bytes === "string") {
                errors.push(`git cat-file failed for ${file.source.repo} blob ${file.source.blob_sha}: ${bytes}`);
            } else if (sha256(bytes) !== file.destination_sha256) {
                errors.push(`${file.path} is verbatim but destination bytes differ from source blob ${file.source.blob_sha}`);
            }
        }
    }

    // Every blob under a scoped source tree is either carried or explicitly excluded, so a
    // file dropped from a source crate cannot pass unnoticed.
    const disposed = new Map<string, number>();
    const dispose = (repo: string, blob: string) => {
        const key = `${repo}\u0000${blob}`;
        disposed.set(key, (disposed.get(key) ?? 0) + 1);
    };
    for (const file of shape.files) {
        if (file.source !== null) dispose(file.source.repo, file.source.blob_sha);
    }
    for (const entry of shape.excluded) dispose(entry.repo, entry.blob_sha);
    // Required multiplicities are summed over every scoped tree before the comparison: one
    // disposition of a blob that sits in two trees accounts for one path, not two.
    const required = new Map<string, { paths: number; scopes: string[] }>();
    const snapshotCache = new Map<string, Set<string> | undefined>();
    shape.scope.forEach((entry, index) => {
        const checkout = checkoutFor(ctx, entry.repo);
        if (!existsSync(checkout)) return;
        const commit = commitByRepo.get(entry.repo);
        if (commit === undefined) return;
        // A tree from an older commit is reachable too, so the tree must sit in the pinned
        // commit's own snapshot or a stale crate version could pass as complete.
        if (!snapshotCache.has(entry.repo)) {
            const trees = snapshotTrees(checkout, commit);
            if (typeof trees === "string") {
                errors.push(`git ls-tree failed for ${entry.repo}@${commit}: ${trees}`);
                snapshotCache.set(entry.repo, undefined);
            } else {
                snapshotCache.set(entry.repo, trees);
            }
        }
        const snapshot = snapshotCache.get(entry.repo);
        if (snapshot !== undefined && !snapshot.has(entry.tree)) {
            errors.push(`$.scope[${index}].tree ${entry.tree} is not a tree of ${entry.repo}@${commit}`);
            return;
        }
        const blobs = treeBlobs(checkout, entry.tree);
        if (typeof blobs === "string") {
            errors.push(`git ls-tree failed for ${entry.repo} tree ${entry.tree}: ${blobs}`);
            return;
        }
        if (blobs.size === 0) errors.push(`$.scope[${index}].tree ${entry.tree} lists no blobs`);
        for (const [blob, paths] of blobs) {
            const key = `${entry.repo}\u0000${blob}`;
            const seen = required.get(key) ?? { paths: 0, scopes: [] };
            seen.paths += paths;
            seen.scopes.push(`$.scope[${index}]`);
            required.set(key, seen);
        }
    });
    for (const [key, { paths, scopes }] of required) {
        const [repo, blob] = key.split("\u0000") as [string, string];
        const accounted = disposed.get(key) ?? 0;
        if (accounted === 0) {
            errors.push(`${scopes.join(",")} ${repo} has blob ${blob} missing from the receipt`);
        } else if (accounted < paths) {
            errors.push(`${scopes.join(",")} ${repo} has blob ${blob} at ${paths} paths but the receipt disposes of it ${accounted} time(s)`);
        }
    }
}

/// The highest wave with a receipt under the destination root, or `current` when none is
/// higher; a receipt being written counts as landed for its own waivers.
function landedWave(root: string, current: string): string {
    let highest = current;
    const waves = join(root, "migration", "waves");
    if (!existsSync(waves)) return highest;
    for (const wave of readdirSync(waves)) {
        if (waveIndex(wave) === -1 || !existsSync(join(waves, wave, "receipt.json"))) continue;
        if (waveIndex(wave) > waveIndex(highest)) highest = wave;
    }
    return highest;
}

/// Waiver files of waves earlier than `current`, read as they are on disk.
function earlierWaiverFiles(root: string, current: string): { path: string; waivers: JsonObject }[] {
    const out: { path: string; waivers: JsonObject }[] = [];
    const waves = join(root, "migration", "waves");
    if (!existsSync(waves)) return out;
    for (const wave of readdirSync(waves).sort()) {
        if (waveIndex(wave) === -1 || waveIndex(wave) >= waveIndex(current)) continue;
        const path = join("migration", "waves", wave, "waivers.json");
        if (!existsSync(join(root, path))) continue;
        try {
            const waivers = JSON.parse(readFileSync(join(root, path), "utf8")) as JsonObject;
            if (isObject(waivers)) out.push({ path, waivers });
        } catch {
            // A malformed waiver file fails its own wave's check.
        }
    }
    return out;
}

/// Waivers in `waivers` whose `expires_by_wave` is not after `wave`.
function expiredWaivers(waivers: JsonObject, wave: string): { id: string; gate: string; expires: string }[] {
    const out: { id: string; gate: string; expires: string }[] = [];
    for (const entry of Array.isArray(waivers.waivers) ? waivers.waivers : []) {
        if (!isObject(entry) || typeof entry.expires_by_wave !== "string" || waveIndex(entry.expires_by_wave) === -1) continue;
        if (waveIndex(entry.expires_by_wave) <= waveIndex(wave)) {
            out.push({
                id: typeof entry.id === "string" ? entry.id : "<unnamed>",
                gate: typeof entry.gate === "string" ? entry.gate : "",
                expires: entry.expires_by_wave,
            });
        }
    }
    return out;
}

/// Destinations named by every wave receipt under the destination root.
function receiptDestinations(root: string): Set<string> {
    const out = new Set<string>();
    const waves = join(root, "migration", "waves");
    if (!existsSync(waves)) return out;
    for (const wave of readdirSync(waves)) {
        const path = join(waves, wave, "receipt.json");
        if (!existsSync(path)) continue;
        try {
            const receipt = JSON.parse(readFileSync(path, "utf8")) as JsonObject;
            for (const entry of Array.isArray(receipt.files) ? receipt.files : []) {
                if (isObject(entry) && typeof entry.destination === "string") out.add(entry.destination);
            }
        } catch {
            // A malformed receipt fails its own check; the registry check does not double-report it.
        }
    }
    return out;
}

/**
 * Digest of a set of files: one `path\n<sha256 of bytes>\n` line per file, sorted by path.
 * Each file is hashed on its own, so bytes moving across a file boundary change the digest;
 * a plain concatenation would not see them move.
 */
export function fileSetDigest(entries: Iterable<[string, Buffer]>): string {
    const lines = [...entries].map(([path, bytes]) => `${path}\n${sha256(bytes)}\n`);
    lines.sort();
    return sha256(lines.join(""));
}

/**
 * Digest of every tracked file under the named module directories in the checked tree, in
 * the `fileSetDigest` form. Tracked means listed by `git ls-files`; a plain directory walk
 * would let build output and editor files change the digest.
 */
export function modulesHash(root: string, modules: string[], path: string, errors: string[]): string | undefined {
    const directories = [...new Set(modules)].sort();
    const entries: [string, Buffer][] = [];
    for (const directory of directories) {
        const full = join(root, directory);
        if (!existsSync(full) || !statSync(full).isDirectory()) {
            errors.push(`${path} names ${directory}, which is not a directory in the destination tree`);
            return undefined;
        }
        const listed = git(root, ["ls-files", "-z", "--", directory]);
        if (!listed.ok) {
            errors.push(`${path}: git ls-files failed for ${directory}: ${listed.error}`);
            return undefined;
        }
        for (const file of listed.stdout.split("\0").filter((entry) => entry !== "")) {
            const fileFull = join(root, file);
            if (!existsSync(fileFull) || !statSync(fileFull).isFile()) {
                errors.push(`${path}: tracked file ${file} is not a file in the destination tree`);
                return undefined;
            }
            entries.push([file, readFileSync(fileFull)]);
        }
    }
    if (entries.length === 0) {
        errors.push(`${path} covers no tracked files`);
        return undefined;
    }
    return fileSetDigest(entries);
}

/// `destination_commit` must be an ancestor of the checked-out destination, so impact records
/// cannot describe an unrelated tree. Skipped when the destination is not a git checkout.
function verifyDestinationCommit(commit: string, path: string, ctx: Context, errors: string[]): void {
    if (!existsSync(join(ctx.root, ".git"))) return;
    const result = spawnSync("git", ["merge-base", "--is-ancestor", commit, "HEAD"], { cwd: ctx.root, encoding: "utf8" });
    if (result.error) {
        errors.push(`${path}: git merge-base failed: ${String(result.error)}`);
    } else if (result.status !== 0) {
        errors.push(`${path} ${commit} is not an ancestor of the destination HEAD`);
    }
}

function verifyReadiness(shape: ReceiptShape, ctx: Context, errors: string[]): void {
    if (shape.readiness === undefined || shape.wave === undefined) return;
    const readiness = readJson(join(ctx.root, shape.readiness), errors, "$.readiness");
    if (readiness === undefined) return;
    const readinessErrors: string[] = [];
    validateReadinessShape(readiness, readinessErrors);
    readinessErrors.forEach((error) => errors.push(`$.readiness: ${error}`));
    if (readinessErrors.length > 0) return;
    const waves = readiness.waves as JsonObject;
    const entry = waves[shape.wave];
    if (!isObject(entry)) {
        errors.push(`$.readiness has no entry for wave ${shape.wave}`);
        return;
    }
    const beadIds = entry.bead_ids as string[];
    if (beadIds.length === 0) return;
    const repo = entry.repo as string;
    const required = entry.required_status as string;
    const commit = shape.sources.find((source) => source.repo === repo)?.commit;
    if (commit === undefined) {
        errors.push(`$.readiness wave ${shape.wave} needs a pinned ${repo} commit to read bead state`);
        return;
    }
    const checkout = checkoutFor(ctx, repo);
    if (!existsSync(checkout)) {
        errors.push(`no checkout is available for readiness repository ${repo} at ${checkout}`);
        return;
    }
    const exportPath = (entry.beads_export as string | undefined) ?? ".beads/issues.jsonl";
    const shown = git(checkout, ["show", `${commit}:${exportPath}`]);
    if (!shown.ok) {
        errors.push(`cannot read ${exportPath} at ${repo}@${commit}: ${shown.error}`);
        return;
    }
    const statuses = new Map<string, string>();
    for (const line of shown.stdout.split("\n")) {
        if (line.trim() === "") continue;
        try {
            const issue = JSON.parse(line) as JsonObject;
            if (typeof issue.id === "string" && typeof issue.status === "string") statuses.set(issue.id, issue.status);
        } catch {
            errors.push(`${exportPath} at ${repo}@${commit} has a malformed line`);
            return;
        }
    }
    const open = beadIds.filter((id) => statuses.get(id) !== required);
    if (open.length > 0) {
        errors.push(
            `$.readiness refuses to pin ${repo}@${commit} for wave ${shape.wave}: beads not ${required}: ${open
                .map((id) => `${id}=${statuses.get(id) ?? "missing"}`)
                .join(", ")}`,
        );
    }
}

export function validateReadinessShape(root: JsonObject, errors: string[]): void {
    validateSchemaVersion(root, errors);
    const waves = requireObject(root.waves, "$.waves", errors);
    if (waves !== undefined) {
        for (const [wave, raw] of Object.entries(waves)) {
            const path = `$.waves.${wave}`;
            if (!WAVES.includes(wave as Wave)) {
                errors.push(`${path} is not a known wave`);
                continue;
            }
            const entry = requireObject(raw, path, errors);
            if (entry === undefined) continue;
            requireString(entry, "repo", path, errors);
            requireStringArray(entry, "bead_ids", path, errors);
            requireEnum(entry, "required_status", ["closed"], path, errors);
            requireString(entry, "acceptance_check", path, errors);
            optionalString(entry, "beads_export", path, errors);
            optionalString(entry, "landed_commit", path, errors);
        }
        for (const wave of WAVES) {
            if (!Object.hasOwn(waves, wave)) errors.push(`$.waves is missing ${wave}`);
        }
    }
    if (root.non_gating !== undefined) {
        requireArray(root, "non_gating", "$", errors).forEach((raw, index) => {
            const path = `$.non_gating[${index}]`;
            const entry = requireObject(raw, path, errors);
            if (entry === undefined) return;
            requireString(entry, "bead_id", path, errors);
            requireString(entry, "rationale", path, errors);
        });
    }
}





function verifyRegistry(shape: RegistryShape, ctx: Context, errors: string[]): void {
    const pinned = receiptDestinations(ctx.root);
    for (const fixture of shape.fixtures) {
        const full = join(ctx.root, fixture.path);
        if (!existsSync(full) || !statSync(full).isFile()) {
            errors.push(`fixture ${fixture.path} is registered but does not exist in the destination`);
        }
        // Only a receipt entry pins bytes, so a byte-stable fixture outside every receipt is unpinned.
        if (fixture.role === "byte-stable" && !pinned.has(fixture.path)) {
            errors.push(`fixture ${fixture.path} is byte-stable but no receipt pins its bytes`);
        }
    }

    const literalOwners = new Set(shape.families.flatMap((family) => family.literals));
    for (const tree of ["crates", "packages"]) {
        const treeRoot = join(ctx.root, tree);
        if (!existsSync(treeRoot)) continue;
        for (const rel of listFiles(treeRoot, skipVendored)) {
            const parts = rel.split("/");
            if (parts[1] !== "src") continue;
            if (/(^|[./_-])tests?([./_-]|$)/.test(rel)) continue;
            const language = rel.endsWith(".rs") ? ".rs" : rel.endsWith(".ts") ? ".ts" : undefined;
            if (language === undefined) continue;
            const text = readFileSync(join(treeRoot, rel), "utf8");
            for (const match of text.matchAll(PERSISTENT_LITERAL_RE[language])) {
                const literal = match[1] ?? match[2] ?? match[3];
                if (literal === undefined || literalOwners.has(literal)) continue;
                errors.push(`${tree}/${rel}: persistent literal "${literal}" has no family entry in the registry`);
            }
            text.split("\n").forEach((line, index) => {
                for (const match of line.matchAll(MIGRATION_MACHINERY_RE)) {
                    errors.push(`${tree}/${rel}:${index + 1}: migration machinery "${match[0]}"; a family has one baseline and no version ledger`);
                }
            });
        }
    }

    const packagesRoot = join(ctx.root, "packages");
    if (existsSync(packagesRoot)) {
        const byPath = new Map(shape.typescript.map((entry) => [entry.path, entry.class]));
        for (const rel of listFiles(packagesRoot, skipVendored)) {
            if (!rel.endsWith(".ts") && !rel.endsWith(".tsx")) continue;
            const path = `packages/${rel}`;
            const cls = byPath.get(path);
            if (cls === undefined) errors.push(`${path}: TypeScript file has no registry classification`);
            else if (cls === "excluded") errors.push(`${path}: TypeScript file is classified excluded but exists in the destination`);
        }
    }
}

function verifyKind(kind: CheckKind, root: JsonObject, ctx: Context): string[] {
    const errors: string[] = [];
    switch (kind) {
        case "receipt": {
            const shape = validateReceiptShape(root, errors);
            if (errors.length === 0) verifyReceipt(shape, ctx, errors);
            break;
        }
        case "registry": {
            const shape = validateRegistryShape(root, errors);
            if (errors.length === 0) verifyRegistry(shape, ctx, errors);
            break;
        }
        case "waivers":
            validateWaiversShape(root, errors);
            break;
        case "property-catalog":
            validatePropertyCatalogShape(root, errors);
            break;
        case "property-impact": {
            const shape = validatePropertyImpactShape(root, errors);
            if (errors.length === 0 && typeof root.destination_commit === "string") {
                verifyDestinationCommit(root.destination_commit, "$.destination_commit", ctx, errors);
            }
            // Evidence binds to bytes, not to a commit: a core record's hashes must equal the
            // hashes of the files it covers in the checked tree.
            for (const core of shape.cores) {
                // A missing covered file is itself an error; code_hash is only compared over the
                // complete file set.
                const entries: [string, Buffer][] = [];
                let complete = true;
                for (const file of core.files) {
                    const full = join(ctx.root, file);
                    if (existsSync(full) && statSync(full).isFile()) entries.push([file, readFileSync(full)]);
                    else {
                        complete = false;
                        errors.push(`${core.path}.files lists ${file}, which is not a file in the destination tree; the code_hash cannot be verified`);
                    }
                }
                if (complete) {
                    const actual = fileSetDigest(entries);
                    if (core.code_hash !== undefined && actual !== core.code_hash) {
                        errors.push(`${core.path}.code_hash is stale: the covered files hash to ${actual}; regenerate the record against the checked tree`);
                    }
                }
                const checkFile = core.check_pointer?.split("#")[0];
                if (checkFile !== undefined) {
                    const full = join(ctx.root, checkFile);
                    if (existsSync(full) && statSync(full).isFile()) {
                        const actual = sha256(readFileSync(full));
                        if (core.check_hash !== undefined && actual !== core.check_hash) {
                            errors.push(`${core.path}.check_hash is stale: ${checkFile} hashes to ${actual}; regenerate the record against the checked tree`);
                        }
                    } else {
                        errors.push(`${core.path}.check_pointer names ${checkFile}, which is not a file in the destination tree; the check_hash cannot be verified`);
                    }
                }
                if (core.evidence_pointer !== undefined) {
                    const full = join(ctx.root, core.evidence_pointer);
                    if (existsSync(full) && statSync(full).isFile()) {
                        const actual = sha256(readFileSync(full));
                        if (core.evidence_digest !== undefined && actual !== core.evidence_digest) {
                            errors.push(`${core.path}.evidence_digest is stale: ${core.evidence_pointer} hashes to ${actual}; regenerate the record against the checked tree`);
                        }
                    } else {
                        errors.push(`${core.path}.evidence_pointer names ${core.evidence_pointer}, which is not a file in the destination tree; the evidence_digest cannot be verified`);
                    }
                }
                if (core.new_evidence_digest !== undefined && core.evidence_digest !== undefined && core.new_evidence_digest !== core.evidence_digest) {
                    errors.push(`${core.path}.new_evidence.digest ${core.new_evidence_digest} does not equal evidence_digest; both name the same evidence record`);
                }
            }
            for (const pointer of shape.pointers) {
                const problem = pointerProblem(ctx.root, pointer.pointer);
                if (problem !== undefined) {
                    errors.push(`${pointer.path} ${pointer.pointer} ${problem}; reclassify the record as core or excluded`);
                }
            }
            break;
        }
        case "architecture-impact":
            validateArchitectureImpactShape(root, errors);
            if (errors.length === 0) {
                (Array.isArray(root.reports) ? root.reports : []).forEach((entry, index) => {
                    if (!isObject(entry) || entry.phase !== "post-integration" || !isObject(entry.analyzed)) return;
                    if (typeof entry.analyzed.commit === "string") {
                        verifyDestinationCommit(entry.analyzed.commit, `$.reports[${index}].analyzed.commit`, ctx, errors);
                    }
                    if (Array.isArray(entry.analyzed.modules) && typeof entry.analyzed.modules_hash === "string") {
                        const actual = modulesHash(ctx.root, entry.analyzed.modules as string[], `$.reports[${index}].analyzed.modules`, errors);
                        if (actual !== undefined && actual !== entry.analyzed.modules_hash) {
                            errors.push(`$.reports[${index}].analyzed.modules_hash is stale: the covered modules hash to ${actual}; re-run the review against the checked tree`);
                        }
                    }
                });
            }
            break;
    }
    return errors;
}

export function verify(kind: CheckKind, value: unknown, ctx: Context): string[] {
    const errors: string[] = [];
    const root = requireObject(value, "$", errors);
    if (root === undefined) return errors;
    return verifyKind(kind, root, ctx);
}

function usage(): string {
    return `usage: bun scripts/eidnara-migration/check.ts <${CHECK_KINDS.join("|")}> <json-path> [--root <dir>] [--checkout <repo>=<dir>]...`;
}

export function run(argv: string[], defaultRoot: string): number {
    const positional: string[] = [];
    let root = defaultRoot;
    const checkoutOverrides: Record<string, string> = {};
    for (let index = 0; index < argv.length; index += 1) {
        const arg = argv[index];
        if (arg === "--root") {
            const value = argv[index + 1];
            if (value === undefined) {
                console.error(usage());
                return 2;
            }
            root = resolve(value);
            index += 1;
        } else if (arg === "--checkout") {
            const value = argv[index + 1];
            const eq = value?.indexOf("=") ?? -1;
            if (value === undefined || eq <= 0) {
                console.error(usage());
                return 2;
            }
            checkoutOverrides[value.slice(0, eq)] = resolve(value.slice(eq + 1));
            index += 1;
        } else if (arg !== undefined) {
            positional.push(arg);
        }
    }
    const [kind, path] = positional;
    if (kind === undefined || path === undefined || positional.length !== 2 || !CHECK_KINDS.includes(kind as CheckKind)) {
        console.error(usage());
        return 2;
    }
    let value: unknown;
    try {
        value = JSON.parse(readFileSync(path, "utf8")) as unknown;
    } catch (error) {
        console.error(`failed to read ${path}: ${String(error)}`);
        return 2;
    }
    const ctx = defaultContext(root);
    Object.assign(ctx.checkouts, checkoutOverrides);
    const errors = verify(kind as CheckKind, value, ctx);
    if (errors.length > 0) {
        errors.forEach((error) => console.error(error));
        return 1;
    }
    console.log(`${kind}: PASS (${path})`);
    return 0;
}

if (import.meta.main) {
    const scriptRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
    process.exit(run(process.argv.slice(2), scriptRoot));
}

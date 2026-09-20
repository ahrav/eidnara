// Usage: `bun scripts/forbid-test-support-dependencies.ts` (exit 1 on a hit).

export const DEV_ONLY_PACKAGES: ReadonlySet<string> = new Set(["eval-core"]);

/** The evaluator core's closed normal dependency set (`docs/evaluator.md`). */
export const EVAL_CORE_DEPENDENCIES: ReadonlySet<string> = new Set([
    "context-core",
    "serde",
    "serde_json",
    "sha2",
]);

/**
 * Paths a sans-I/O core must not name: product crates and the std effect
 * modules, whether written as a full path (`std::fs::read`) or as a member
 * of a brace-grouped `use std::{...}` list.
 */
const STD_EFFECT_MODULES = "fs|path|process|time|net|env|io";
const PRODUCT_CRATES = "kernel|daemon|retrieval|storage|memory_store|host_runtime|rusqlite|tokio";
const FORBIDDEN_CORE_SOURCE = new RegExp(
    [
        `\\b(${PRODUCT_CRATES})::`,
        `\\buse (${PRODUCT_CRATES})\\b`,
        `\\bstd::(${STD_EFFECT_MODULES})\\b`,
        `\\bstd::\\{[^;]*(?:[{,]\\s*|::)(${STD_EFFECT_MODULES})\\b`,
    ].join("|"),
    "g",
);

/** The directory the source fence scans, relative to the workspace root. */
export const EVAL_CORE_SOURCE_DIR = "crates/eval-core/src";

export interface MetadataDependency {
    name: string;
    /** The alias a `package = "name"` rename gives the dependency; feature entries use it. */
    rename?: string | null;
    kind: "dev" | "build" | null;
    features?: string[];
    target?: string | null;
}

export interface MetadataPackage {
    name: string;
    source: string | null;
    dependencies: MetadataDependency[];
    features?: Record<string, string[]>;
}

export interface CargoMetadata {
    packages: MetadataPackage[];
    workspace_root: string;
}

/** A feature or `pkg/feature` entry that turns on test-support, with the feature that holds it. */
interface TestSupportHit {
    /** The feature in whose table the hit sits (or the hit itself when `entry` is null). */
    feature: string;
    /** The requested feature whose closure reached `feature`. */
    root: string;
    /** A `pkg/test-support` entry, or null when `feature` is itself a test-support feature. */
    entry: string | null;
}

function tableName(dep: MetadataDependency): string {
    const table = dep.kind === "build" ? "build-dependencies" : "dependencies";
    return dep.target ? `target.${dep.target}.${table}` : table;
}

/** A reached feature: bare for the scanned package, `pkg/feature` for another local package. */
interface Reached {
    name: string;
    /** The requested feature whose closure reached it. */
    root: string;
    entries: string[];
}

/** Splits `pkg/feature` and `pkg?/feature`; null for a same-package entry. */
function forwarded(entry: string): [string, string] | null {
    const slash = entry.indexOf("/");
    if (slash < 0) return null;
    return [entry.slice(0, slash).replace(/\?$/, ""), entry.slice(slash + 1)];
}

/** The package `owner`'s feature entries call `alias`: its rename, else the name itself. */
function dependencyPackage(local: Map<string, MetadataPackage>, owner: string, alias: string) {
    const dep = local.get(owner)?.dependencies.find((d) => (d.rename ?? d.name) === alias);
    return dep?.name ?? alias;
}

/**
 * Features reachable from `roots` through `pkg`'s feature table and, via
 * `other/feature` entries, through other local packages' tables. `dep:x`
 * enables the optional dependency `x` and suppresses its implicit feature, so it
 * is not a feature node; the dependency's own requested features are scanned on
 * its edge. A `pkg/test-support` entry is a hit on its own and is not followed.
 */
function featureClosure(
    local: Map<string, MetadataPackage>,
    pkg: string,
    roots: string[],
): Reached[] {
    const reached = new Map<string, Reached>();
    const pending: [string, string, string][] = roots.map((root) => [pkg, root, root]);
    while (pending.length > 0) {
        const [owner, feature, root] = pending.pop() as [string, string, string];
        const table = local.get(owner)?.features ?? {};
        const key = `${owner}/${feature}`;
        if (reached.has(key) || !(feature in table)) continue;
        const entries = table[feature] ?? [];
        reached.set(key, { name: owner === pkg ? feature : key, root, entries });
        for (const entry of entries) {
            if (entry.startsWith("dep:")) continue;
            const other = forwarded(entry);
            if (other === null) pending.push([owner, entry, root]);
            else if (!entry.endsWith("/test-support")) {
                pending.push([dependencyPackage(local, owner, other[0]), other[1], root]);
            }
        }
    }
    return [...reached.values()];
}

/**
 * Every way the closure of `roots` turns on test-support: a reached feature named
 * `*test-support`, or a reached feature whose table forwards `pkg/test-support`.
 */
function testSupportReach(
    local: Map<string, MetadataPackage>,
    pkg: string,
    roots: string[],
): TestSupportHit[] {
    const hits: TestSupportHit[] = [];
    for (const { name, root, entries } of featureClosure(local, pkg, roots)) {
        if (name.endsWith("test-support")) hits.push({ feature: name, root, entry: null });
        for (const entry of entries) {
            if (entry.endsWith("/test-support")) hits.push({ feature: name, root, entry });
        }
    }
    return hits;
}

export function forbiddenDependencyEdges(metadata: CargoMetadata): string[] {
    const findings = new Set<string>();
    const local = new Map(
        metadata.packages.filter((pkg) => pkg.source === null).map((pkg) => [pkg.name, pkg]),
    );
    for (const pkg of local.values()) {
        if (pkg.name === "eval-core") {
            const normal = new Set(
                pkg.dependencies.filter((dep) => dep.kind !== "dev").map((dep) => dep.name),
            );
            const unexpected = [...normal].filter((name) => !EVAL_CORE_DEPENDENCIES.has(name));
            const missing = [...EVAL_CORE_DEPENDENCIES].filter((name) => !normal.has(name));
            for (const name of unexpected) {
                findings.add(`eval-core [dependencies] names ${name} outside its closed set`);
            }
            for (const name of missing) {
                findings.add(`eval-core [dependencies] lacks ${name} from its closed set`);
            }
        }
        for (const dep of pkg.dependencies) {
            if (dep.kind === "dev") continue;
            const table = `${pkg.name} [${tableName(dep)}]`;
            const edge = `${table} ${dep.name}`;
            const requested = dep.features ?? [];
            const testSupport = requested.filter((feature) => feature.endsWith("test-support"));
            if (testSupport.length > 0) {
                findings.add(`${edge} enables ${testSupport.join(", ")}`);
            }
            if (DEV_ONLY_PACKAGES.has(dep.name)) {
                findings.add(`${table} depends on dev-only package ${dep.name}`);
            }
            // A requested feature can forward to test-support under another name,
            // in the target or in a local package it forwards to; the target's
            // own `default` is reported once below, on the target.
            for (const hit of testSupportReach(local, dep.name, requested)) {
                if (hit.entry !== null) {
                    findings.add(`${edge} reaches ${hit.entry} through ${hit.feature}`);
                } else if (hit.feature !== hit.root) {
                    findings.add(`${edge} enables ${hit.feature} through ${hit.root}`);
                }
            }
        }
        for (const hit of testSupportReach(local, pkg.name, ["default"])) {
            findings.add(
                hit.entry === null
                    ? `${pkg.name} [features] default enables ${hit.feature}`
                    : `${pkg.name} [features] default reaches ${hit.entry} through ${hit.feature}`,
            );
        }
    }
    return [...findings].sort();
}

/**
 * An empty source set is itself a finding: a scan that matched no files
 * proves nothing about the crate, so the gate must not pass on it.
 */
export function forbiddenCoreSources(sources: Record<string, string>): string[] {
    const entries = Object.entries(sources);
    if (entries.length === 0) {
        return [`${EVAL_CORE_SOURCE_DIR}: no source files scanned`];
    }
    const findings: string[] = [];
    // Match the whole file, not each line: a rustfmt-wrapped `use std::{`
    // group names its effect module several lines below the `std::` prefix.
    for (const [path, text] of entries) {
        const lines = text.split("\n");
        const reported = new Set<number>();
        for (const match of text.matchAll(FORBIDDEN_CORE_SOURCE)) {
            const line = text.slice(0, match.index).split("\n").length;
            if (reported.has(line)) continue;
            reported.add(line);
            findings.push(`${path}:${line}: ${lines[line - 1]!.trim()}`);
        }
    }
    return findings.sort();
}

if (import.meta.main) {
    const proc = Bun.spawnSync(["cargo", "metadata", "--locked", "--format-version", "1"], {
        stdout: "pipe",
        stderr: "inherit",
    });
    if (proc.exitCode !== 0) {
        console.error(`cargo metadata exited ${proc.exitCode}`);
        process.exit(2);
    }
    let metadata: CargoMetadata;
    try {
        metadata = JSON.parse(proc.stdout.toString()) as CargoMetadata;
    } catch (error) {
        console.error(`cargo metadata produced invalid JSON: ${String(error)}`);
        process.exit(2);
    }
    const findings = forbiddenDependencyEdges(metadata);
    if (findings.length > 0) {
        console.error("test-only code reachable from a production dependency table:");
        for (const finding of findings) console.error(`  ${finding}`);
        process.exit(1);
    }
    // `cargo metadata` names the workspace root, so the scan does not depend
    // on the directory the script was launched from.
    const sources: Record<string, string> = {};
    const glob = new Bun.Glob(`${EVAL_CORE_SOURCE_DIR}/**/*.rs`);
    for (const path of glob.scanSync(metadata.workspace_root)) {
        sources[path] = await Bun.file(`${metadata.workspace_root}/${path}`).text();
    }
    const leaks = forbiddenCoreSources(sources);
    if (leaks.length > 0) {
        console.error("eval-core source names a product crate or a std effect module:");
        for (const leak of leaks) console.error(`  ${leak}`);
        process.exit(1);
    }
    const local = metadata.packages.filter((pkg) => pkg.source === null).length;
    console.log(
        `dependency tables: ${local} local packages, no test-support or dev-only edges; eval-core closed set and ${Object.keys(sources).length} source files clean`,
    );
}

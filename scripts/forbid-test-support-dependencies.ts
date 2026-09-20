// Usage: `bun scripts/forbid-test-support-dependencies.ts` (exit 1 on a hit).

export const DEV_ONLY_PACKAGES: ReadonlySet<string> = new Set(["eval-core"]);

export interface MetadataDependency {
    name: string;
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
}

type FeatureTable = Record<string, string[]>;

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

/**
 * Feature names reachable from `roots` through one package's feature table, each
 * mapped to the root that reached it first. `dep:x` entries name the optional
 * dependency `x`, whose implicit feature shares its name; `pkg/feature` entries
 * belong to another package's table and stop here.
 */
function featureClosure(features: FeatureTable, roots: string[]): Map<string, string> {
    const reached = new Map<string, string>();
    const pending: [string, string][] = roots.map((root) => [root, root]);
    while (pending.length > 0) {
        const [feature, root] = pending.pop() as [string, string];
        if (reached.has(feature) || !(feature in features)) continue;
        reached.set(feature, root);
        for (const entry of features[feature] ?? []) {
            if (!entry.includes("/")) pending.push([entry.replace(/^dep:/, ""), root]);
        }
    }
    return reached;
}

/**
 * Every way the closure of `roots` turns on test-support: a reached feature named
 * `*test-support`, or a reached feature whose table forwards `pkg/test-support`.
 */
function testSupportReach(features: FeatureTable, roots: string[]): TestSupportHit[] {
    const hits: TestSupportHit[] = [];
    for (const [feature, root] of featureClosure(features, roots)) {
        if (feature.endsWith("test-support")) hits.push({ feature, root, entry: null });
        for (const entry of features[feature] ?? []) {
            if (entry.endsWith("/test-support")) hits.push({ feature, root, entry });
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
            // A requested feature can forward to test-support under another name;
            // the target's own `default` is reported once below, on the target.
            const target = local.get(dep.name);
            for (const hit of testSupportReach(target?.features ?? {}, requested)) {
                if (hit.entry !== null) {
                    findings.add(`${edge} reaches ${hit.entry} through ${hit.feature}`);
                } else if (hit.feature !== hit.root) {
                    findings.add(`${edge} enables ${hit.feature} through ${hit.root}`);
                }
            }
        }
        for (const hit of testSupportReach(pkg.features ?? {}, ["default"])) {
            findings.add(
                hit.entry === null
                    ? `${pkg.name} [features] default enables ${hit.feature}`
                    : `${pkg.name} [features] default reaches ${hit.entry} through ${hit.feature}`,
            );
        }
    }
    return [...findings].sort();
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
    const metadata = JSON.parse(proc.stdout.toString()) as CargoMetadata;
    const findings = forbiddenDependencyEdges(metadata);
    if (findings.length > 0) {
        console.error("test-only code reachable from a production dependency table:");
        for (const finding of findings) console.error(`  ${finding}`);
        process.exit(1);
    }
    const local = metadata.packages.filter((pkg) => pkg.source === null).length;
    console.log(
        `dependency tables: ${local} local packages, no test-support or dev-only edges`,
    );
}

// Usage: `bun scripts/forbid-test-support-dependencies.ts` (exit 1 on a hit).

export const DEV_ONLY_PACKAGES: ReadonlySet<string> = new Set(["eval-core"]);

/** The evaluator core's closed normal dependency set (`docs/evaluator.md`). */
export const EVAL_CORE_DEPENDENCIES: ReadonlySet<string> = new Set([
    "context-core",
    "serde",
    "serde_json",
    "sha2",
]);

/** Paths a sans-I/O core must not name: product crates and the std effect modules. */
const FORBIDDEN_CORE_SOURCE =
    /\b(kernel|daemon|retrieval|storage|memory_store|host_runtime|rusqlite|tokio)::|\buse (kernel|daemon|retrieval|storage|memory_store|host_runtime|rusqlite|tokio)\b|\bstd::(fs|path|process|time|net|env|io)\b/;

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

function tableName(dep: MetadataDependency): string {
    const table = dep.kind === "build" ? "build-dependencies" : "dependencies";
    return dep.target ? `target.${dep.target}.${table}` : table;
}

/** Feature names reachable from `default` through the package's own feature table. */
function defaultFeatureClosure(features: Record<string, string[]>): Set<string> {
    const reached = new Set<string>();
    const pending = ["default"];
    while (pending.length > 0) {
        const feature = pending.pop() as string;
        if (reached.has(feature) || !(feature in features)) continue;
        reached.add(feature);
        for (const entry of features[feature] ?? []) {
            if (!entry.includes("/")) pending.push(entry.replace(/^dep:/, ""));
        }
    }
    return reached;
}

export function forbiddenDependencyEdges(metadata: CargoMetadata): string[] {
    const findings: string[] = [];
    for (const pkg of metadata.packages) {
        if (pkg.source !== null) continue;
        if (pkg.name === "eval-core") {
            const normal = new Set(
                pkg.dependencies.filter((dep) => dep.kind !== "dev").map((dep) => dep.name),
            );
            const unexpected = [...normal].filter((name) => !EVAL_CORE_DEPENDENCIES.has(name));
            const missing = [...EVAL_CORE_DEPENDENCIES].filter((name) => !normal.has(name));
            for (const name of unexpected) {
                findings.push(`eval-core [dependencies] names ${name} outside its closed set`);
            }
            for (const name of missing) {
                findings.push(`eval-core [dependencies] lacks ${name} from its closed set`);
            }
        }
        for (const dep of pkg.dependencies) {
            if (dep.kind === "dev") continue;
            const testSupport = (dep.features ?? []).filter((feature) =>
                feature.endsWith("test-support"),
            );
            if (testSupport.length > 0) {
                findings.push(
                    `${pkg.name} [${tableName(dep)}] ${dep.name} enables ${testSupport.join(", ")}`,
                );
            }
            if (DEV_ONLY_PACKAGES.has(dep.name)) {
                findings.push(
                    `${pkg.name} [${tableName(dep)}] depends on dev-only package ${dep.name}`,
                );
            }
        }
        const features = pkg.features ?? {};
        for (const feature of defaultFeatureClosure(features)) {
            for (const entry of features[feature] ?? []) {
                if (entry.endsWith("/test-support")) {
                    findings.push(
                        `${pkg.name} [features] default reaches ${entry} through ${feature}`,
                    );
                }
            }
        }
    }
    return findings.sort();
}

/** Lines of evaluator-core source that reach a product crate or a std effect module. */
export function forbiddenCoreSources(sources: Record<string, string>): string[] {
    const findings: string[] = [];
    for (const [path, text] of Object.entries(sources)) {
        text.split("\n").forEach((line, index) => {
            if (FORBIDDEN_CORE_SOURCE.test(line)) {
                findings.push(`${path}:${index + 1}: ${line.trim()}`);
            }
        });
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
    const metadata = JSON.parse(proc.stdout.toString()) as CargoMetadata;
    const findings = forbiddenDependencyEdges(metadata);
    if (findings.length > 0) {
        console.error("test-only code reachable from a production dependency table:");
        for (const finding of findings) console.error(`  ${finding}`);
        process.exit(1);
    }
    const sources: Record<string, string> = {};
    const glob = new Bun.Glob("crates/eval-core/src/**/*.rs");
    for (const path of glob.scanSync(".")) {
        sources[path] = await Bun.file(path).text();
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

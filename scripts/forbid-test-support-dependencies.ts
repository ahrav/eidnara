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

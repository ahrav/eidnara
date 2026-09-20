import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readdirSync, realpathSync, writeFileSync } from "node:fs";
import { isAbsolute, join, posix, relative, resolve } from "node:path";

interface Node {
    path: string;
    source_root: string;
    source_path: string;
    kind: string;
    mode: number;
    size_bytes: number;
    sha256: string;
    dependencies: Array<{ path: string; kind: string }>;
}

interface Manifest extends Record<string, unknown> {
    entrypoint: string;
    nodes: Node[];
}

function safeRelative(path: string): boolean {
    return path.length > 0 && !/[\\\u0000-\u001f\u007f]/.test(path) && !isAbsolute(path)
        && path.split("/").every((part) => part !== "" && part !== "." && part !== "..");
}

function parseManifest(value: unknown): Manifest {
    if (value === null || typeof value !== "object" || !("nodes" in value) || !Array.isArray(value.nodes)
        || !("entrypoint" in value) || typeof value.entrypoint !== "string") {
        throw new Error("Closure manifest requires an entrypoint and nodes array");
    }
    for (const node of value.nodes) {
        if (node === null || typeof node !== "object" || !Array.isArray(node.dependencies)
            || ![node.path, node.source_root, node.source_path, node.kind, node.sha256].every((field) => typeof field === "string")
            || !Number.isSafeInteger(node.mode) || !Number.isSafeInteger(node.size_bytes)
            || !node.dependencies.every((edge: unknown) => edge !== null && typeof edge === "object" && "path" in edge && typeof edge.path === "string" && "kind" in edge && typeof edge.kind === "string")) {
            throw new Error("Malformed closure node");
        }
    }
    return value as Manifest;
}

const DEVELOPMENT_DIRECTORIES = new Set(["docs", "examples", "test", "tests", "__tests__"]);
const MODULE = /\.(js|cjs|mjs)$/;
const DATA = /\.(json|txt|wasm|tiktoken)$/;

/** Inventories the entrypoint's installed npm runtime tree, including nested
 * production dependencies. Variable-specifier imports cannot be proven complete
 * by a static import walk. Every admitted byte is hash-pinned; no package script
 * executes, and existing pins must still match. Development trees and unrelated
 * user/extension directories are excluded. The root's finite_dynamic edges
 * describe a conservative transitive inventory, not immediate import sites. */
export function completeRuntimeClosure(input: unknown, sourceRoots: Record<string, string>): {
    manifest: Manifest;
    added: string[];
} {
    const manifest = structuredClone(parseManifest(input));
    const nodes = new Map(manifest.nodes.map((node) => [node.path, node]));
    if (nodes.size !== manifest.nodes.length) throw new Error("Duplicate closure node");
    const entrypoint = nodes.get(manifest.entrypoint);
    if (!entrypoint || !safeRelative(entrypoint.source_path) || entrypoint.path !== entrypoint.source_path) {
        throw new Error("Runtime completion requires an unchanged relative entrypoint path");
    }
    const parts = entrypoint.source_path.split("/");
    const npm = parts.indexOf("node_modules");
    const packageName = parts[npm + 1];
    if (npm < 0 || !packageName) throw new Error("Entrypoint is not inside an installed npm package");
    const packageRoot = parts.slice(0, npm + (packageName.startsWith("@") ? 3 : 2)).join("/");
    const root = sourceRoots[entrypoint.source_root];
    if (!root || !isAbsolute(root)) throw new Error(`Missing absolute source root: ${entrypoint.source_root}`);
    const realRoot = realpathSync(root);
    const pending = [packageRoot];
    const added: string[] = [];
    const dependencies = new Set(entrypoint.dependencies.map((edge) => edge.path));
    while (pending.length > 0) {
        const directory = pending.pop();
        if (directory === undefined) break;
        for (const entry of readdirSync(join(realRoot, directory), { withFileTypes: true })) {
            if (entry.name.startsWith(".")) continue;
            const path = posix.join(directory, entry.name);
            if (entry.isDirectory()) {
                if (!DEVELOPMENT_DIRECTORIES.has(entry.name)) pending.push(path);
                continue;
            }
            if (!MODULE.test(entry.name) && !DATA.test(entry.name) && !entry.name.endsWith(".node")) continue;
            if (/\.(test|spec)\.[cm]?js$/.test(entry.name) || entry.name === "package-lock.json" || entry.name === "npm-shrinkwrap.json") continue;
            const source = join(realRoot, path);
            const stat = lstatSync(source);
            const rel = relative(realRoot, realpathSync(source));
            if (!stat.isFile() || stat.isSymbolicLink() || rel.startsWith("..") || isAbsolute(rel)) {
                throw new Error(`Runtime member is not a regular in-tree file: ${path}`);
            }
            if (stat.size > 256 * 1024 * 1024) throw new Error(`Runtime member exceeds 256 MiB: ${path}`);
            const bytes = readFileSync(source);
            const sha256 = createHash("sha256").update(bytes).digest("hex");
            const previous = nodes.get(path);
            if (previous) {
                if (previous.sha256 !== sha256 || previous.size_bytes !== bytes.length || previous.source_root !== entrypoint.source_root) {
                    throw new Error(`Existing runtime pin differs: ${path}`);
                }
                continue;
            }
            if (nodes.size >= 65536) throw new Error("Runtime closure exceeds 65536 nodes");
            nodes.set(path, { path, source_path:path, source_root:entrypoint.source_root,
                kind:entry.name.endsWith(".node") ? "native_addon" : MODULE.test(entry.name) ? "module" : "data",
                mode:0o600, size_bytes:bytes.length, sha256, dependencies:[] });
            if (!dependencies.has(path)) {
                entrypoint.dependencies.push({ path, kind:entry.name.endsWith(".node") ? "native" : "finite_dynamic" });
                dependencies.add(path);
            }
            added.push(path);
        }
    }
    entrypoint.dependencies.sort((left,right) => left.path < right.path ? -1 : left.path > right.path ? 1 : 0);
    manifest.nodes = [...nodes.values()].sort((left,right) => left.path < right.path ? -1 : left.path > right.path ? 1 : 0);
    return { manifest, added:added.sort() };
}

/** Space=2 matches Rust's serde_json::to_value followed by to_vec_pretty for
 * these string/integer-only manifests; space=0 keeps release files compact.
 * The Rust manifest_digest test remains the digest authority. */
export function canonicalManifest(manifest: Manifest, space: 0 | 2 = 2): string {
    const keys = new Set<string>();
    const collectKeys = (value: unknown): void => {
        if (Array.isArray(value)) {
            for (const child of value) collectKeys(child);
        } else if (value !== null && typeof value === "object") {
            for (const [key, child] of Object.entries(value)) { keys.add(key); collectKeys(child); }
        }
    };
    collectKeys(manifest);
    return JSON.stringify(manifest, [...keys].sort(), space);
}

if (import.meta.main) {
    const [file, ...args] = process.argv.slice(2);
    if (!file) throw new Error("Usage: bun scripts/complete-harness-runtime.ts <manifest> <source-root=absolute-path>... [--write]");
    const roots: Record<string, string> = Object.create(null);
    for (const arg of args.filter((arg) => arg !== "--write")) {
        const equals = arg.indexOf("=");
        if (equals <= 0 || roots[arg.slice(0, equals)] !== undefined) throw new Error("Expected distinct source-root=absolute-path arguments");
        roots[arg.slice(0, equals)] = arg.slice(equals + 1);
    }
    let input: unknown;
    try { input = JSON.parse(readFileSync(resolve(file), "utf8")); }
    catch (cause) { throw new Error("Cannot read closure manifest JSON", { cause }); }
    const result = completeRuntimeClosure(input, roots);
    const canonical = canonicalManifest(result.manifest);
    if (Buffer.byteLength(canonical) > 16 * 1024 * 1024) throw new Error("Canonical manifest exceeds 16 MiB");
    if (args.includes("--write")) writeFileSync(resolve(file), `${canonicalManifest(result.manifest, 0)}\n`);
    console.log(JSON.stringify({ addedCount:result.added.length, added:result.added.slice(0,40), omittedFromDisplay:Math.max(0,result.added.length-40), nodes:result.manifest.nodes.length, sha256:createHash("sha256").update(canonical).digest("hex"), written:args.includes("--write") }, null, 2));
}

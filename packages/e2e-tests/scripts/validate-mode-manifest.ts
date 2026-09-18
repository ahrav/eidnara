#!/usr/bin/env bun

import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { Glob } from "bun";
import ts from "typescript";

export const E2E_ROOT = resolve(import.meta.dir, "..");
export const MANIFEST_PATH = resolve(E2E_ROOT, "mode-manifest.json");
const TEST_GLOB = "tests/**/*.test.ts";

export const TIERS = ["rust-only", "pi-smoke"] as const;
export type Tier = (typeof TIERS)[number];
export type Mode = "rust";

export interface ModeManifestEntry {
    path: string;
    tier: Tier;
    rationale: string;
    contract_refs: string[];
    /**
     * Why every test in the file is skipped. Required exactly when the file has no live test,
     * so `bun test` exiting 0 on the file is a recorded decision rather than silent coverage
     * loss, and the marker leaves with the skips.
     */
    quarantined?: string;
}

export interface ModeManifest {
    schema: number;
    header: string;
    entries: ModeManifestEntry[];
}

export interface ValidationResult {
    manifest: ModeManifest;
    files: string[];
}

function enumerateTestFiles(): string[] {
    const glob = new Glob(TEST_GLOB);
    return [...glob.scanSync({ cwd: E2E_ROOT, onlyFiles: true })].sort();
}

function parseJsonFile<T>(path: string, parse: (raw: unknown) => T): T {
    let raw: unknown;
    try {
        raw = JSON.parse(readFileSync(path, "utf8")) as unknown;
    } catch (error) {
        throw new Error(`could not read ${path}: ${String(error)}`);
    }
    return parse(raw);
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function validateEntry(value: unknown, index: number): ModeManifestEntry {
    if (!isRecord(value)) throw new Error(`entry ${index} is not an object`);
    const requiredKeys = ["path", "tier", "rationale", "contract_refs"];
    const optionalKeys = ["quarantined"];
    const actualKeys = Object.keys(value).sort();
    if (
        requiredKeys.some((key) => !actualKeys.includes(key)) ||
        actualKeys.some((key) => !requiredKeys.includes(key) && !optionalKeys.includes(key))
    ) {
        throw new Error(
            `entry ${index} must contain ${requiredKeys.join(", ")} and at most ${optionalKeys.join(", ")}; got ${actualKeys.join(", ")}`,
        );
    }

    const path = value.path;
    if (typeof path !== "string" || path.length === 0) {
        throw new Error(`entry ${index} has an invalid path`);
    }
    const tier = value.tier;
    if (typeof tier !== "string" || !TIERS.includes(tier as Tier)) {
        throw new Error(`entry ${index} has invalid classification ${JSON.stringify(tier)}`);
    }
    const rationale = value.rationale;
    if (typeof rationale !== "string" || rationale.trim().length === 0) {
        throw new Error(`entry ${index} must have a rationale`);
    }
    const contractRefs = value.contract_refs;
    if (
        !Array.isArray(contractRefs) ||
        contractRefs.length === 0 ||
        contractRefs.some((ref) => typeof ref !== "string" || ref.trim().length === 0)
    ) {
        throw new Error(
            `entry ${index} (${path}) must have a non-empty string-array contract_refs`,
        );
    }

    const quarantined = value.quarantined;
    if (
        quarantined !== undefined &&
        (typeof quarantined !== "string" || quarantined.trim() === "")
    ) {
        throw new Error(`entry ${index} (${path}) quarantined must be a non-empty reason`);
    }

    return {
        path,
        tier: tier as Tier,
        rationale,
        contract_refs: [...contractRefs] as string[],
        ...(quarantined === undefined ? {} : { quarantined }),
    };
}

const TEST_CALLEES = new Set(["it", "test"]);
const SUITE_CALLEES = new Set(["describe"]);
/** Unconditional disabling only: `skipIf` is the tier's documented environment gate, not a quarantine. */
const DISABLING_MODIFIERS = new Set(["skip", "todo"]);

/** The bare name plus any modifier chain of a `bun:test` callee, or `null` for anything else. */
function testCallee(callee: ts.Expression): { name: string; modifiers: string[] } | null {
    const modifiers: string[] = [];
    let current: ts.Expression = callee;
    // `it.each([...])("name", fn)` calls the result of `each`; the callee is the inner expression.
    if (ts.isCallExpression(current)) current = current.expression;
    while (ts.isPropertyAccessExpression(current)) {
        modifiers.unshift(current.name.text);
        current = current.expression;
    }
    if (!ts.isIdentifier(current)) return null;
    return { name: current.text, modifiers };
}

/**
 * Whether `source` registers at least one test `bun test` may run: an `it`/`test` call not
 * under an unconditional `skip`/`todo` and not inside a `describe.skip`/`describe.todo` suite.
 */
export function hasLiveTests(path: string, source: string): boolean {
    const parsed = ts.createSourceFile(
        path,
        source,
        ts.ScriptTarget.Latest,
        true,
        ts.ScriptKind.TS,
    );
    let live = false;
    const visit = (node: ts.Node): void => {
        if (live) return;
        if (ts.isCallExpression(node)) {
            const callee = testCallee(node.expression);
            if (callee !== null) {
                const disabled = callee.modifiers.some((modifier) =>
                    DISABLING_MODIFIERS.has(modifier),
                );
                if (SUITE_CALLEES.has(callee.name) && disabled) return;
                if (TEST_CALLEES.has(callee.name) && !disabled) {
                    live = true;
                    return;
                }
            }
        }
        ts.forEachChild(node, visit);
    };
    visit(parsed);
    return live;
}

/**
 * A manifest path must be a real test module, not merely a file that exists: the source has to
 * parse as TypeScript and import `bun:test`, so a stray or truncated file cannot count as coverage.
 */
export function validateTestSource(path: string, source: string): void {
    const transpiled = ts.transpileModule(source, {
        fileName: path,
        reportDiagnostics: true,
        compilerOptions: { target: ts.ScriptTarget.Latest, module: ts.ModuleKind.ESNext },
    });
    const syntaxErrors = (transpiled.diagnostics ?? []).map((diagnostic) =>
        ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"),
    );
    if (syntaxErrors.length > 0) {
        throw new Error(`${path} does not parse as TypeScript: ${syntaxErrors.join("; ")}`);
    }
    const parsed = ts.createSourceFile(
        path,
        source,
        ts.ScriptTarget.Latest,
        true,
        ts.ScriptKind.TS,
    );
    const importsBunTest = parsed.statements.some(
        (statement) =>
            ts.isImportDeclaration(statement) &&
            ts.isStringLiteral(statement.moduleSpecifier) &&
            statement.moduleSpecifier.text === "bun:test",
    );
    if (!importsBunTest) {
        throw new Error(`${path} does not import bun:test`);
    }
}

export function validateManifestDocument(
    raw: unknown,
    expectedFiles: string[] = enumerateTestFiles(),
    readSource: (path: string) => string = (path) => readFileSync(resolve(E2E_ROOT, path), "utf8"),
): ValidationResult {
    if (
        !isRecord(raw) ||
        raw.schema !== 1 ||
        typeof raw.header !== "string" ||
        !Array.isArray(raw.entries)
    ) {
        throw new Error("mode manifest must be an object with schema: 1, header, and entries");
    }

    const entries = raw.entries.map(validateEntry);
    const expectedSet = new Set(expectedFiles);
    const seen = new Map<string, number>();
    for (const entry of entries) {
        seen.set(entry.path, (seen.get(entry.path) ?? 0) + 1);
        if (!expectedSet.has(entry.path)) {
            throw new Error(`dead or out-of-scope manifest path: ${entry.path}`);
        }
        if (!existsSync(resolve(E2E_ROOT, entry.path))) {
            throw new Error(`manifest path does not exist: ${entry.path}`);
        }
        if (!entry.path.startsWith("tests/") || !entry.path.endsWith(".test.ts")) {
            throw new Error(`manifest path is not under ${TEST_GLOB}: ${entry.path}`);
        }
        if (seen.get(entry.path)! > 1) {
            throw new Error(`duplicate manifest entry: ${entry.path}`);
        }
        const source = readSource(entry.path);
        validateTestSource(entry.path, source);
        const live = hasLiveTests(entry.path, source);
        if (!live && entry.quarantined === undefined) {
            throw new Error(
                `${entry.path} has no live tests and no quarantined reason; every case is skipped`,
            );
        }
        if (live && entry.quarantined !== undefined) {
            throw new Error(`${entry.path} is marked quarantined but has live tests`);
        }
    }

    const missing = expectedFiles.filter((path) => !seen.has(path));
    if (missing.length > 0) {
        throw new Error(`missing manifest entries: ${missing.join(", ")}`);
    }

    return {
        manifest: { schema: 1, header: raw.header, entries },
        files: expectedFiles,
    };
}

export function validateModeManifest(): ValidationResult {
    const validation = parseJsonFile(MANIFEST_PATH, (raw) => validateManifestDocument(raw));
    if (filesForMode(validation, "rust").length === 0) {
        throw new Error("rust manifest selector is empty");
    }
    return validation;
}

export function filesForMode(validation: ValidationResult, mode: Mode): string[] {
    if (mode !== "rust") throw new Error(`unsupported mode ${String(mode)}`);
    return validation.manifest.entries.map((entry) => entry.path).sort();
}

function parseArgs(args: string[]): { mode?: Mode } {
    let mode: Mode | undefined;
    for (let index = 0; index < args.length; index += 1) {
        const arg = args[index];
        if (arg === "--mode") {
            const value = args[++index];
            if (value !== "rust") throw new Error("--mode must be rust");
            mode = value;
        } else if (arg === "--help" || arg === "-h") {
            console.log("Usage: validate-mode-manifest.ts [--mode rust]");
            process.exit(0);
        } else {
            throw new Error(`unknown argument: ${arg}`);
        }
    }
    return { mode };
}

if (import.meta.main) {
    try {
        const { mode } = parseArgs(Bun.argv.slice(2));
        const validation = validateModeManifest();
        if (mode) {
            for (const path of filesForMode(validation, mode)) console.log(path);
        } else {
            const quarantined = validation.manifest.entries.filter(
                (entry) => entry.quarantined !== undefined,
            );
            console.log(
                `validated ${validation.files.length} e2e test entries, ${quarantined.length} quarantined`,
            );
            for (const entry of quarantined) {
                console.log(`  quarantined ${entry.path}: ${entry.quarantined}`);
            }
        }
    } catch (error) {
        console.error(`mode manifest validation failed: ${String(error)}`);
        process.exit(1);
    }
}

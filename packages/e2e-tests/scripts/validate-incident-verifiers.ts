#!/usr/bin/env bun

import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
    EXECUTABLE_LANES,
    type IncidentCatalog,
    parseIncidentCatalog,
} from "../src/incident-pool/contract";
import {
    boundVerifierDigests,
    E2E_ROOT,
    type EvidenceView,
    loadMutationEvidence,
    REPO_ROOT,
} from "../src/incident-pool/evidence";
import {
    canonicalJson,
    compareWithAcceptedSnapshot,
    type HistorySnapshot,
} from "../src/incident-pool/history";
import {
    builtinIncidentCaseRegistry,
    validateRegistryCatalogCorrespondence,
} from "../src/incident-pool/registry";
import {
    deriveTrustedAcceptedCommit,
    type GitRunner,
    loadHistorySnapshot,
    loadHistorySnapshotFromGit,
} from "./validate-incident-history";

function git(
    args: string[],
    cwd: string,
): {
    status: number;
    stdout: string;
    stderr: string;
} {
    const result = Bun.spawnSync({
        cmd: ["git", ...args],
        cwd,
        stdout: "pipe",
        stderr: "pipe",
    });
    return {
        status: result.exitCode,
        stdout: result.stdout.toString(),
        stderr: result.stderr.toString(),
    };
}

/** Accepted paths absent from the current tree are `unbound`; paths present in both with different bytes are `changed`. Paths only the current tree binds have no accepted bytes to drift from. */
function digestDrift(
    acceptedDigests: Record<string, string>,
    currentDigests: Record<string, string>,
): { changed: string[]; unbound: string[] } {
    const changed: string[] = [];
    const unbound: string[] = [];
    for (const [path, accepted] of Object.entries(acceptedDigests)) {
        const current = currentDigests[path];
        if (current === undefined) unbound.push(path);
        else if (current !== accepted) changed.push(path);
    }
    return { changed: changed.sort(), unbound: unbound.sort() };
}

/** Removing a record exempts its verifier from replay, so an accepted verifier that no current record binds is rejected. */
export function assertBoundVerifierBytesUnchanged(
    acceptedDigests: Record<string, string>,
    currentDigests: Record<string, string>,
): void {
    const { changed, unbound } = digestDrift(acceptedDigests, currentDigests);
    if (unbound.length > 0) {
        throw new Error(
            `mutation records no longer bind accepted verifiers: ${unbound.join(", ")}`,
        );
    }
    if (changed.length > 0) {
        throw new Error(
            `bound verifiers changed without recorded mutation replay support: ${changed.join(", ")}`,
        );
    }
}

/**
 *
 * A catalog with no accepted bindings treats every current binding as new.
 * Reject modules bound only by the accepted catalog; otherwise removing a binding exempts an executable verifier from this gate.
 */
export function assertCatalogBoundVerifierBytesUnchanged(
    acceptedDigests: Record<string, string>,
    currentDigests: Record<string, string>,
): void {
    const { changed, unbound } = digestDrift(acceptedDigests, currentDigests);
    if (unbound.length > 0) {
        throw new Error(
            `catalog no longer binds accepted executable verifiers: ${unbound.join(", ")}`,
        );
    }
    if (changed.length > 0) {
        throw new Error(
            `catalog-bound executable verifiers changed without recorded replay support: ${changed.join(", ")}`,
        );
    }
}

function trustedCiCommit(gitRunner: GitRunner): string {
    const eventName = process.env.GITHUB_EVENT_NAME;
    const eventPath = process.env.GITHUB_EVENT_PATH;
    const githubSha = process.env.GITHUB_SHA;
    const githubRef = process.env.GITHUB_REF;
    if (!eventName || !eventPath || !githubSha || !githubRef) {
        throw new Error("trusted verifier CI validation requires GitHub event environment");
    }
    let event: unknown;
    try {
        event = JSON.parse(readFileSync(eventPath, "utf8")) as unknown;
    } catch {
        throw new Error("could not read trusted GitHub event payload");
    }
    return deriveTrustedAcceptedCommit({
        eventName,
        event,
        githubSha,
        githubRef,
        githubRefProtected: process.env.GITHUB_REF_PROTECTED ?? "false",
        repoRoot: REPO_ROOT,
        git: gitRunner,
    });
}

/** Rebinding a variant to another module or symbol is a verifier change even when every previously bound path keeps its bytes, so the gate compares each accepted variant's whole binding. */
export function assertCatalogBindingsUnchanged(
    acceptedBindings: Record<string, string>,
    currentBindings: Record<string, string>,
): void {
    const { changed, unbound } = digestDrift(acceptedBindings, currentBindings);
    if (unbound.length > 0) {
        throw new Error(
            `accepted executable variants no longer bind a verifier: ${unbound.join(", ")}`,
        );
    }
    if (changed.length > 0) {
        throw new Error(
            `executable variants rebound their verifier without recorded replay support: ${changed.join(", ")}`,
        );
    }
}

/** One canonical string per executable variant: driver and verifier references plus the sorted oracle dependencies. */
export function catalogBindings(catalog: IncidentCatalog): Record<string, string> {
    const bindings: Record<string, string> = {};
    for (const family of catalog.families) {
        for (const variant of family.variants) {
            if (!EXECUTABLE_LANES.includes(variant.lane) || !variant.verifier_binding) continue;
            const binding = variant.verifier_binding;
            bindings[variant.id] = [
                binding.driver,
                binding.verifier,
                ...[...binding.oracle_dependencies].sort(),
            ].join("\n");
        }
    }
    return bindings;
}

/** Rebinding one record to another verifier while sibling records keep the old path leaves every path digest unchanged, so the gate also compares each accepted record's own binding. */
export function assertMutationBindingsUnchanged(
    acceptedBindings: Record<string, string>,
    currentBindings: Record<string, string>,
): void {
    const { changed, unbound } = digestDrift(acceptedBindings, currentBindings);
    if (unbound.length > 0) {
        throw new Error(`accepted mutation records vanished: ${unbound.join(", ")}`);
    }
    if (changed.length > 0) {
        throw new Error(
            `mutation records rebound their verifier without recorded replay support: ${changed.join(", ")}`,
        );
    }
}

/** One canonical string per evidence record: verifier path, sorted included fixtures, and the replay command. */
export function mutationBindings(view: EvidenceView): Record<string, string> {
    const bindings: Record<string, string> = {};
    for (const record of view.records) {
        bindings[record.evidenceId] = [
            record.verifierPath,
            ...[...record.fixturePaths].sort(),
            record.replayCommand,
        ].join("\n");
    }
    return bindings;
}

interface TrustedVerifierState {
    mutationDigests: Record<string, string>;
    mutationBindings: Record<string, string>;
    catalogBoundDigests: Record<string, string>;
    catalogBindings: Record<string, string>;
}

/* */
function readVerifierState(worktree: string, repoRoot: string): TrustedVerifierState {
    const e2eRoot = resolve(worktree, "packages/e2e-tests");
    const catalogPath = resolve(e2eRoot, "incidents", "catalog.json");
    // A tree without catalog.json binds no executable verifiers.
    // Such a tree contributes no accepted bytes, so every current binding is new.
    // Deleting the current catalog leaves accepted bindings without counterparts.
    const catalog = existsSync(catalogPath)
        ? parseIncidentCatalog(JSON.parse(readFileSync(catalogPath, "utf8")) as unknown)
        : null;
    const catalogBoundDigests = catalog ? boundVerifierDigests(catalog, e2eRoot) : {};
    // Without mutations/, mutation records bind no verifiers.
    // Deleting the current directory leaves every accepted mutation-bound verifier without a counterpart, which `assertBoundVerifierBytesUnchanged` rejects.
    const evidence = existsSync(resolve(e2eRoot, "mutations"))
        ? loadMutationEvidence(e2eRoot, repoRoot)
        : null;
    return {
        mutationDigests: evidence?.verifierDigests ?? {},
        mutationBindings: evidence ? mutationBindings(evidence) : {},
        catalogBoundDigests,
        catalogBindings: catalog ? catalogBindings(catalog) : {},
    };
}

function loadTrustedEvidence(baseCommit: string): TrustedVerifierState {
    const parent = mkdtempSync(join(tmpdir(), "incident-verifier-base-"));
    const worktree = join(parent, "tree");
    const added = git(["worktree", "add", "--detach", worktree, baseCommit], REPO_ROOT);
    if (added.status !== 0) {
        rmSync(parent, { recursive: true, force: true });
        throw new Error(
            `could not create trusted verifier worktree: ${added.stderr.trim() || `git worktree add exited ${added.status}`}`,
        );
    }
    let evidence: TrustedVerifierState;
    try {
        evidence = readVerifierState(worktree, worktree);
    } catch (error) {
        // Suppress cleanup errors when evidence loading fails so they do not replace the evidence error.
        cleanupTrustedWorktree(worktree, parent);
        throw error;
    }
    // Cleanup failures propagate after evidence loads.
    const cleanupError = cleanupTrustedWorktree(worktree, parent);
    if (cleanupError) throw cleanupError;
    return evidence;
}

/* */
function cleanupTrustedWorktree(worktree: string, parent: string): Error | null {
    const removed = git(["worktree", "remove", "--force", worktree], REPO_ROOT);
    rmSync(parent, { recursive: true, force: true });
    if (removed.status !== 0) {
        return new Error(
            `could not remove trusted verifier worktree: ${removed.stderr.trim() || `git worktree remove exited ${removed.status}`}`,
        );
    }
    return null;
}

function runCatalogSuite(args: string[], cwd: string): ReturnType<GitRunner> {
    const result = Bun.spawnSync({
        cmd: [process.execPath, ...args],
        cwd,
        env: { ...process.env, NO_COLOR: "1", FORCE_COLOR: "0" },
        stdout: "pipe",
        stderr: "pipe",
        timeout: 120_000,
        maxBuffer: 4 * 1024 * 1024,
    });
    return {
        status: result.exitCode,
        stdout: result.stdout.toString(),
        stderr: result.stderr.toString(),
    };
}

/** Catalog-only replay admission. Mutation bytes and per-record bindings retain their own gates. */
export function replayCatalogVerifierChanges(
    acceptedSnapshot: HistorySnapshot,
    currentSnapshot: HistorySnapshot,
    acceptedDigests: Record<string, string>,
    currentDigests: Record<string, string>,
    run: GitRunner = runCatalogSuite,
): void {
    const { changed, unbound } = digestDrift(acceptedDigests, currentDigests);
    if (unbound.length > 0) {
        throw new Error(
            `catalog no longer binds accepted executable verifiers: ${unbound.join(", ")}`,
        );
    }
    const { accepted, candidate } = compareWithAcceptedSnapshot(acceptedSnapshot, currentSnapshot);
    const beforeBindings = catalogBindings(accepted.catalog);
    const afterBindings = catalogBindings(candidate.catalog);
    const vanished = digestDrift(beforeBindings, afterBindings).unbound;
    if (vanished.length > 0) {
        throw new Error(
            `accepted executable variants no longer bind a verifier: ${vanished.join(", ")}`,
        );
    }
    const changedPaths = new Set([
        ...changed,
        ...Object.keys(currentDigests).filter((path) => acceptedDigests[path] === undefined),
    ]);
    const currentVariants = new Map(
        candidate.catalog.families.flatMap((family) =>
            family.variants.map((variant) => [variant.id, variant] as const),
        ),
    );
    const suites = new Set<string>();
    const appended = candidate.events.slice(accepted.events.length);
    for (const before of accepted.catalog.families.flatMap((family) => family.variants)) {
        if (!EXECUTABLE_LANES.includes(before.lane)) continue;
        const beforeBinding = before.verifier_binding;
        const after = currentVariants.get(before.id);
        const binding = after?.verifier_binding;
        if (!beforeBinding || !after || !binding) {
            throw new Error(`accepted executable variant ${before.id} no longer binds a verifier`);
        }
        if (before.normative_checks.some((check) => !after.normative_checks.includes(check))) {
            throw new Error(`variant ${before.id} removed an accepted normative check`);
        }
        if (
            beforeBinding.oracle_dependencies.some(
                (path) => !binding.oracle_dependencies.includes(path),
            )
        ) {
            throw new Error(`variant ${before.id} removed an accepted oracle binding`);
        }
        const paths = [beforeBindings[before.id], afterBindings[before.id]]
            .flatMap((references) => references.split("\n"))
            .map((reference) => `packages/e2e-tests/${reference.split("#")[0]}`);
        if (
            !paths.some((path) => changedPaths.has(path)) &&
            canonicalJson(before) === canonicalJson(after)
        )
            continue;
        const baseline = candidate.ledger.byIdentity.get(after.id)?.latestBaseline;
        if (
            !baseline ||
            before.semantic_revision.id === after.semantic_revision.id ||
            !appended.some(
                (event) =>
                    event.event_id === baseline.event_id &&
                    event.kind === "baseline" &&
                    event.semantic_fingerprint === after.semantic_revision.fingerprint,
            )
        ) {
            throw new Error(
                `variant ${before.id} requires an appended fingerprint-bound baseline and distinct semantic revision for replay`,
            );
        }
        for (const reference of [
            beforeBinding.driver,
            beforeBinding.verifier,
            binding.driver,
            binding.verifier,
        ]) {
            const module = reference.split("#")[0];
            const suite = module.replace(/\.ts$/, ".test.ts");
            if (suite === module || !existsSync(resolve(E2E_ROOT, suite))) {
                throw new Error(`variant ${before.id} missing required regression suite ${suite}`);
            }
            suites.add(suite);
        }
        for (const module of binding.oracle_dependencies) {
            if (!changedPaths.has(`packages/e2e-tests/${module}`)) continue;
            const suite = module.replace(/\.ts$/, ".test.ts");
            if (suite !== module && existsSync(resolve(E2E_ROOT, suite))) suites.add(suite);
        }
    }
    validateRegistryCatalogCorrespondence(builtinIncidentCaseRegistry(), candidate.catalog);
    const suiteDigests = (): Record<string, string> =>
        Object.fromEntries(
            [...suites].sort().map((suite) => [
                suite,
                createHash("sha256")
                    .update(readFileSync(resolve(E2E_ROOT, suite)))
                    .digest("hex"),
            ]),
        );
    const beforeSuites = suiteDigests();
    for (const suite of [...suites].sort()) {
        const result = run(["test", `./${suite}`, "--max-concurrency", "1"], E2E_ROOT);
        // Bun's summary is required even on exit zero: missing/empty/skipped suites are not replay evidence.
        const passes = result.stderr.match(/^\s*(\d+) pass\s*$/m);
        const failures = result.stderr.match(/^\s*(\d+) fail\s*$/m);
        if (
            result.status !== 0 ||
            !passes ||
            Number(passes[1]) < 1 ||
            !failures ||
            Number(failures[1]) !== 0 ||
            /^\s*[1-9]\d* (?:skip|todo)\s*$/m.test(result.stderr)
        ) {
            throw new Error(
                `catalog regression replay failed or executed no successful tests: ${suite} (exit ${result.status})`,
            );
        }
        // Only static paths, counts and digests leave the gate, never raw test diagnostics.
        console.log(`catalog replay ${suite}: ${passes[1]} passed`);
    }
    if (suites.size > 0) {
        const afterSnapshot = loadHistorySnapshot(
            resolve(E2E_ROOT, "incidents"),
            currentSnapshot.baseLabel,
        );
        if (
            canonicalJson(afterSnapshot) !== canonicalJson(currentSnapshot) ||
            canonicalJson(boundVerifierDigests(candidate.catalog)) !==
                canonicalJson(currentDigests) ||
            canonicalJson(suiteDigests()) !== canonicalJson(beforeSuites)
        ) {
            throw new Error("catalog replay inputs changed during replay");
        }
        for (const [path, digest] of Object.entries(currentDigests).sort()) {
            console.log(`catalog replay digest ${path}: ${digest}`);
        }
    }
}

export function validateIncidentVerifiers(baseCommit: string): number {
    const accepted = loadTrustedEvidence(baseCommit);
    const current = readVerifierState(REPO_ROOT, REPO_ROOT);
    assertBoundVerifierBytesUnchanged(accepted.mutationDigests, current.mutationDigests);
    assertMutationBindingsUnchanged(accepted.mutationBindings, current.mutationBindings);
    replayCatalogVerifierChanges(
        loadHistorySnapshotFromGit(REPO_ROOT, baseCommit, git),
        loadHistorySnapshot(resolve(E2E_ROOT, "incidents"), baseCommit),
        accepted.catalogBoundDigests,
        current.catalogBoundDigests,
    );
    if (canonicalJson(readVerifierState(REPO_ROOT, REPO_ROOT)) !== canonicalJson(current)) {
        throw new Error("bound verifier bytes or bindings changed during replay");
    }
    return (
        Object.keys(accepted.mutationDigests).length +
        Object.keys(accepted.catalogBoundDigests).length
    );
}

function main(args: string[]): void {
    const ci = args.length === 1 && args[0] === "--ci";
    const local = args.length === 2 && args[0] === "--base";
    if (!ci && !local) {
        throw new Error("usage: validate-incident-verifiers.ts --ci | --base <commit>");
    }
    const baseCommit = ci ? trustedCiCommit(git) : args[1]!;
    const count = validateIncidentVerifiers(baseCommit);
    console.log(`validated ${count} bound verifier files against ${baseCommit}`);
}

if (import.meta.main) {
    try {
        main(process.argv.slice(2));
    } catch (error) {
        console.error(error instanceof Error ? error.message : String(error));
        process.exit(1);
    }
}

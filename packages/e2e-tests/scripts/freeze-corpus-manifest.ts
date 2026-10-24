/**
 * Usage:
 *   bun packages/e2e-tests/scripts/freeze-corpus-manifest.ts \
 *     --repo <checkout> --commit <sha> --name <name> --upstream <url> --license <spdx> \
 *     --layout <layout> --instructions <condition> [--exclude <path>:<line>:<reason>]... \
 *     --out crates/daemon/testdata/corpus/repos/<name>.json
 *
 * The manifest reads source content from the resolved commit's tree; command-line options supply
 * metadata and exclusions. Instruction documents keep their text with each `--exclude` line
 * blanked; other regular-file blobs retain their size and UTF-8 validity, and symlinks retain
 * their targets.
 */

import { parseArgs } from "node:util";

const { writeFileSync } = process.getBuiltinModule("node:fs");

const INSTRUCTION_DOCUMENTS = new Set([
    "AGENTS.md",
    "CLAUDE.md",
    ".cursorrules",
    "ARCHITECTURE.md",
    "STRUCTURE.md",
]);

interface Exclusion {
    path: string;
    line: number;
    reason: string;
}

function git(repo: string, args: string[]): Buffer {
    const proc = Bun.spawnSync(["git", "-C", repo, ...args], { stdout: "pipe", stderr: "pipe" });
    if (proc.exitCode !== 0) {
        throw new Error(`git ${args.join(" ")} exited ${proc.exitCode}: ${proc.stderr}`);
    }
    return Buffer.from(proc.stdout);
}

function parseExclusion(spec: string): Exclusion {
    const [, path, line, reason] = /^(.+?):(\d+):(.+)$/.exec(spec) ?? [];
    if (path === undefined || reason === undefined) {
        throw new Error(`--exclude ${spec}: expected <path>:<line>:<reason>`);
    }
    return { path, line: Number(line), reason };
}

function basename(path: string): string {
    return path.slice(path.lastIndexOf("/") + 1);
}

function isUtf8(bytes: Buffer): boolean {
    try {
        new TextDecoder("utf-8", { fatal: true }).decode(bytes);
        return true;
    } catch {
        return false;
    }
}

function blank(text: string, path: string, exclusions: readonly Exclusion[]): string {
    const lines = text.split("\n");
    for (const exclusion of exclusions.filter((e) => e.path === path)) {
        if (exclusion.line < 1 || exclusion.line > lines.length) {
            throw new Error(`--exclude ${path}:${exclusion.line}: no such line`);
        }
        lines[exclusion.line - 1] = "";
    }
    return lines.join("\n");
}

const { values } = parseArgs({
    options: {
        repo: { type: "string" },
        commit: { type: "string" },
        name: { type: "string" },
        upstream: { type: "string" },
        license: { type: "string" },
        layout: { type: "string" },
        instructions: { type: "string" },
        exclude: { type: "string", multiple: true },
        out: { type: "string" },
    },
});
function required(
    key: "repo" | "commit" | "name" | "upstream" | "license" | "layout" | "instructions",
): string {
    const value = values[key];
    if (!value) throw new Error(`--${key} is required`);
    return value;
}
const repo = required("repo");
const name = required("name");
const layout = required("layout");
const instructions = required("instructions");
const upstream = { repository: required("upstream"), license: required("license") };
const commit = git(repo, ["rev-parse", `${required("commit")}^{commit}`])
    .toString()
    .trim();
const exclusions = (values.exclude ?? []).map(parseExclusion);

const documents: { path: string; text: string }[] = [];
const files: ({ path: string; size: number; utf8: boolean } | { path: string; symlink: string })[] =
    [];
const listing = new TextDecoder("utf-8", { fatal: true }).decode(
    git(repo, ["ls-tree", "-r", "-z", "--full-tree", commit]),
);
for (const record of listing.split("\0").filter((r) => r.length > 0)) {
    const tab = record.indexOf("\t");
    const [mode, type, oid] = record.slice(0, tab).split(" ");
    const path = record.slice(tab + 1);
    if (type !== "blob" || oid === undefined) {
        throw new Error(`${path}: ${type} entries are not supported`);
    }
    const bytes = git(repo, ["cat-file", "blob", oid]);
    if (mode === "120000") {
        files.push({ path, symlink: bytes.toString("utf8") });
    } else if (INSTRUCTION_DOCUMENTS.has(basename(path))) {
        if (!isUtf8(bytes)) throw new Error(`${path}: instruction document is not UTF-8`);
        documents.push({ path, text: blank(bytes.toString("utf8"), path, exclusions) });
    } else {
        files.push({ path, size: bytes.length, utf8: isUtf8(bytes) });
    }
}
for (const exclusion of exclusions) {
    if (!documents.some((d) => d.path === exclusion.path)) {
        throw new Error(`--exclude ${exclusion.path}: not an instruction document`);
    }
}
const byPath = (a: { path: string }, b: { path: string }) =>
    Buffer.compare(Buffer.from(a.path), Buffer.from(b.path));
documents.sort(byPath);
files.sort(byPath);

const list = (items: readonly unknown[]) =>
    items.length === 0
        ? "[]"
        : `[\n${items.map((i) => `    ${JSON.stringify(i)}`).join(",\n")}\n  ]`;
const manifest = `{
  "schema": "eidnara.source-corpus/v1",
  "name": ${JSON.stringify(name)},
  "layout": ${JSON.stringify(layout)},
  "instructions": ${JSON.stringify(instructions)},
  "upstream": ${JSON.stringify({ repository: upstream.repository, commit, license: upstream.license })},
  "exclusions": ${list(exclusions)},
  "documents": ${list(documents)},
  "files": ${list(files)}
}
`;
if (values.out) writeFileSync(values.out, manifest);
else process.stdout.write(manifest);

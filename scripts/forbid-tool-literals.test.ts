import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

const SCRIPT = join(import.meta.dir, "forbid-tool-literals.sh");

const FILE_PATH = '"filePath"';
const MCP_NAME = '"mcp_foo"';

let repo: string;

function git(...args: string[]): void {
    const result = spawnSync("git", args, { cwd: repo, encoding: "utf8" });
    if (result.status !== 0) throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
}

function write(path: string, content: string): void {
    mkdirSync(dirname(join(repo, path)), { recursive: true });
    writeFileSync(join(repo, path), content);
}

function scan(...args: string[]): { status: number | null; stderr: string } {
    const result = spawnSync("bash", [SCRIPT, ...args], { cwd: repo, encoding: "utf8" });
    return { status: result.status, stderr: result.stderr };
}

/** Tracks every file, then reports the full-tree scan and the staged scan of the same change. */
function both(): [
    { status: number | null; stderr: string },
    { status: number | null; stderr: string },
] {
    git("add", "-A");
    const staged = scan("--staged");
    git("commit", "-q", "--no-verify", "-m", "case");
    return [scan(), staged];
}

const WITH_TEST_MODULE = (literal: string, after: string) => `fn production() {}

#[cfg(test)]
mod tests {
    #[test]
    fn reads() {
        let input = serde_json::json!({ ${literal}: "a.rs" });
        if true {
            drop(input);
        }
    }
}
${after}`;

beforeEach(() => {
    repo = mkdtempSync(join(tmpdir(), "forbid-tool-literals-"));
    git("init", "-q");
    git("config", "user.email", "t@example.invalid");
    git("config", "user.name", "t");
    write("crates/daemon/src/lib.rs", 'pub fn path_key() -> &\'static str { "path" }\n');
    git("add", "-A");
    git("commit", "-q", "--no-verify", "-m", "base");
});

afterEach(() => {
    rmSync(repo, { recursive: true, force: true });
});

describe("forbid-tool-literals.sh", () => {
    test("passes on a tree with only generic keys", () => {
        expect(scan().status).toBe(0);
        expect(scan("--staged").status).toBe(0);
    });

    test("names the file and line of a distinctive literal in production source", () => {
        write("crates/daemon/src/boundary.rs", `fn a() {}\nconst K: &str = ${FILE_PATH};\n`);
        for (const { status, stderr } of both()) {
            expect(status).toBe(1);
            expect(stderr).toContain("crates/daemon/src/boundary.rs:2:");
        }
    });

    test("catches an MCP-prefixed name and a nested production module", () => {
        write("crates/daemon/src/codec/opencode.rs", `const N: &str = ${MCP_NAME};\n`);
        for (const { status, stderr } of both()) {
            expect(status).toBe(1);
            expect(stderr).toContain("crates/daemon/src/codec/opencode.rs:1:");
        }
    });

    test("skips the vocabulary module, test files, tests directories, and testdata", () => {
        write("crates/daemon/src/tool_vocabulary.rs", `const K: &str = ${FILE_PATH};\n`);
        write("crates/daemon/src/boundary_tests.rs", `const K: &str = ${FILE_PATH};\n`);
        write("crates/daemon/src/tests/fixture.rs", `const K: &str = ${FILE_PATH};\n`);
        write("crates/daemon/src/testdata/fixture.rs", `const K: &str = ${FILE_PATH};\n`);
        write("crates/daemon/tests/selection.rs", `const K: &str = ${FILE_PATH};\n`);
        write("crates/daemon/testdata/golden.json", `{${FILE_PATH}: "a.rs"}\n`);
        for (const { status } of both()) expect(status).toBe(0);
    });

    test("skips a top-level test module and catches a literal after its closing brace", () => {
        write("crates/daemon/src/boundary.rs", WITH_TEST_MODULE(FILE_PATH, ""));
        for (const { status } of both()) expect(status).toBe(0);
        write(
            "crates/daemon/src/boundary.rs",
            WITH_TEST_MODULE(FILE_PATH, `const K: &str = ${FILE_PATH};\n`),
        );
        for (const { status, stderr } of both()) {
            expect(status).toBe(1);
            expect(stderr).toContain("crates/daemon/src/boundary.rs:13:");
            expect(stderr).not.toContain("boundary.rs:7:");
        }
    });

    test("scans from the repository root when run from a subdirectory", () => {
        write("crates/daemon/src/boundary.rs", `const K: &str = ${FILE_PATH};\n`);
        git("add", "-A");
        for (const args of [["--staged"], []]) {
            const result = spawnSync("bash", [SCRIPT, ...args], {
                cwd: join(repo, "crates"),
                encoding: "utf8",
            });
            expect(result.status).toBe(1);
            expect(result.stderr).toContain("crates/daemon/src/boundary.rs:1:");
        }
    });

    test("counts an added line that begins with a plus sign", () => {
        write("crates/daemon/src/boundary.rs", `fn a() {}\n++ 1;\nconst K: &str = ${FILE_PATH};\n`);
        git("add", "-A");
        const { status, stderr } = scan("--staged");
        expect(status).toBe(1);
        expect(stderr).toContain("crates/daemon/src/boundary.rs:3:");
    });

    test("reports a tool failure outside a git checkout", () => {
        const outside = mkdtempSync(join(tmpdir(), "forbid-tool-literals-nogit-"));
        const result = spawnSync("bash", [SCRIPT], {
            cwd: outside,
            encoding: "utf8",
            env: { ...process.env, GIT_CEILING_DIRECTORIES: dirname(outside) },
        });
        rmSync(outside, { recursive: true, force: true });
        expect(result.status).toBe(2);
    });
});

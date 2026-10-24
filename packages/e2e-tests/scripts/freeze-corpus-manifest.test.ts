import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const SCRIPT = join(import.meta.dir, "freeze-corpus-manifest.ts");
const temporaryRoots: string[] = [];

afterEach(() => {
    for (const root of temporaryRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function run(cwd: string, cmd: string[], env: Record<string, string | undefined> = {}): string {
    const proc = Bun.spawnSync(cmd, {
        cwd,
        env: {
            ...Object.fromEntries(
                Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_")),
            ),
            ...env,
        },
        stdout: "pipe",
        stderr: "pipe",
    });
    if (proc.exitCode !== 0) {
        throw new Error(`${cmd.join(" ")} exited ${proc.exitCode}: ${proc.stderr}`);
    }
    return proc.stdout.toString();
}

const GIT_IDENTITY = {
    GIT_AUTHOR_NAME: "corpus",
    GIT_AUTHOR_EMAIL: "corpus@example.invalid",
    GIT_COMMITTER_NAME: "corpus",
    GIT_COMMITTER_EMAIL: "corpus@example.invalid",
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_CONFIG_GLOBAL: "/dev/null",
};

function repository(text: string): { root: string; head: string } {
    const root = mkdtempSync(join(tmpdir(), "eidnara-freeze-corpus-"));
    temporaryRoots.push(root);
    run(root, ["git", "init", "-q", "-b", "main"], GIT_IDENTITY);
    writeFileSync(join(root, "AGENTS.md"), text);
    run(root, ["git", "add", "AGENTS.md"], GIT_IDENTITY);
    run(root, ["git", "commit", "-q", "-m", "init"], GIT_IDENTITY);
    return { root, head: run(root, ["git", "rev-parse", "HEAD"], GIT_IDENTITY).trim() };
}

describe("freeze-corpus-manifest", () => {
    it("reads the --repo tree when the caller's shell points GIT_DIR at another repository", () => {
        const wanted = repository("# wanted\n");
        const other = repository("# other\n");
        const out = join(wanted.root, "..", `manifest-${process.pid}.json`);
        temporaryRoots.push(out);
        run(
            wanted.root,
            [
                "bun",
                SCRIPT,
                "--repo",
                wanted.root,
                "--commit",
                "HEAD",
                "--name",
                "wanted",
                "--upstream",
                "https://example.invalid/wanted",
                "--license",
                "MIT",
                "--layout",
                "non_git",
                "--instructions",
                "root_only",
                "--out",
                out,
            ],
            { ...GIT_IDENTITY, GIT_DIR: join(other.root, ".git"), GIT_WORK_TREE: other.root },
        );
        const manifest = JSON.parse(readFileSync(out, "utf8"));
        expect(manifest.upstream.commit).toBe(wanted.head);
        expect(manifest.documents).toEqual([{ path: "AGENTS.md", text: "# wanted\n" }]);
    });
});

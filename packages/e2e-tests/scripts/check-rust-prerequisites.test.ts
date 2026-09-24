import { afterEach, describe, expect, it } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DAEMON_EXAMPLES, DIRECT_HOST_FIXTURE } from "../src/rust-runner/daemon-examples";
import { detectRustPrerequisites } from "./check-rust-prerequisites";

const temporaryRoots: string[] = [];
const channelAvailable = () => ({ available: true });

afterEach(() => {
    for (const root of temporaryRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

/** A workspace whose fake `cargo` lists every daemon example except those named in `without`. */
function fakeWorkspace(without: string[] = []): { root: string; bin: string } {
    const parent = mkdtempSync(join(tmpdir(), "eidnara-rust-prereq-"));
    temporaryRoots.push(parent);
    const root = join(parent, "repo");
    const bin = join(root, "bin");
    mkdirSync(bin, { recursive: true });
    writeFileSync(join(root, "Cargo.toml"), "[workspace]\nmembers = []\n");
    const metadata = JSON.stringify({
        packages: [
            {
                name: "daemon",
                targets: DAEMON_EXAMPLES.filter((e) => !without.includes(e.example)).map((e) => ({
                    name: e.example,
                    kind: ["example"],
                })),
            },
        ],
    });
    const cargo = join(bin, "cargo");
    writeFileSync(cargo, `#!/bin/sh\nprintf '%s\\n' '${metadata}'\n`);
    chmodSync(cargo, 0o755);
    return { root, bin };
}

function executable(path: string): void {
    mkdirSync(join(path, ".."), { recursive: true });
    writeFileSync(path, "#!/bin/sh\nexit 0\n");
    chmodSync(path, 0o755);
}

describe("Rust direct-host prerequisite detector", () => {
    it("resolves a workspace that has every example target but no prebuilt binary", () => {
        const { root, bin } = fakeWorkspace();

        const result = detectRustPrerequisites({
            repoRoot: root,
            env: { PATH: bin },
            channelProbe: channelAvailable,
        });

        expect(result).toEqual({ ok: true, missing: [], binaries: {} });
    });

    it("resolves pre-built workspace binaries without building", () => {
        const { root, bin } = fakeWorkspace();
        const fixture = join(root, "target", "debug", "examples", "direct_host_fixture");
        executable(fixture);

        const result = detectRustPrerequisites({
            repoRoot: root,
            allowBuild: false,
            env: { PATH: bin },
            channelProbe: channelAvailable,
        });

        expect(result).toEqual({
            ok: true,
            missing: [],
            binaries: { [DIRECT_HOST_FIXTURE.prebuiltEnv]: fixture },
        });
    });

    it("uses a valid prebuilt override as-is", () => {
        const { root, bin } = fakeWorkspace();
        const prebuilt = join(root, "elsewhere", "eval_runner");
        executable(prebuilt);

        const result = detectRustPrerequisites({
            repoRoot: root,
            env: { PATH: bin, EIDNARA_E2E_EVAL_RUNNER_BIN: prebuilt },
            channelProbe: channelAvailable,
        });

        expect(result).toEqual({
            ok: true,
            missing: [],
            binaries: { EIDNARA_E2E_EVAL_RUNNER_BIN: prebuilt },
        });
    });

    it("rejects a workspace without an example target, naming the example", () => {
        const { root, bin } = fakeWorkspace(["eval_runner"]);
        const result = detectRustPrerequisites({
            repoRoot: root,
            env: { PATH: bin },
            channelProbe: channelAvailable,
        });
        expect(result.ok).toBe(false);
        expect(result.missing).toEqual(["cargo workspace: eval_runner example is unavailable"]);
    });

    it("reports a set but non-executable prebuilt override instead of ignoring it", () => {
        const { root, bin } = fakeWorkspace();
        const stale = join(root, "no-such-binary");
        const result = detectRustPrerequisites({
            repoRoot: root,
            env: { PATH: bin, EIDNARA_E2E_DIRECT_HOST_FIXTURE_BIN: stale },
            channelProbe: channelAvailable,
        });
        expect(result.ok).toBe(false);
        expect(result.missing).toContain(
            `EIDNARA_E2E_DIRECT_HOST_FIXTURE_BIN=${stale} is not an executable file`,
        );
    });

    it("reports an unavailable shared-memory channel as a missing prerequisite", () => {
        const { root, bin } = fakeWorkspace();
        const result = detectRustPrerequisites({
            repoRoot: root,
            env: { PATH: bin },
            channelProbe: () => ({ available: false, reason: "runtime_mechanism_unavailable" }),
        });
        expect(result.ok).toBe(false);
        expect(result.missing).toEqual([
            "shared-memory channel unavailable on this runtime: runtime_mechanism_unavailable",
        ]);
    });
});

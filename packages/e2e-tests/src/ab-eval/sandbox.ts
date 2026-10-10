import { spawnSync } from "node:child_process";
import { realpathSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { ensurePiInstall } from "../bedrock-peer/harness-runtime";

const REPO_ROOT = resolve(import.meta.dir, "../../../..");

/** Paths reach the generated shell script unquoted, so each must be one plain word. */
const PLAIN_PATH = /^\/[A-Za-z0-9._@+/-]*$/;

function plainPath(path: string): string {
    if (!PLAIN_PATH.test(path)) {
        throw new Error(`sandbox path ${JSON.stringify(path)} must match ${PLAIN_PATH}`);
    }
    return path;
}

function within(path: string, dir: string): boolean {
    return path === dir || path.startsWith(dir.endsWith("/") ? dir : `${dir}/`);
}

/**
 * The sandbox masks `/tmp`, the invoking user's home, and the run root with empty tmpfs mounts.
 * The list omits a path inside another listed path, since the outer mount already covers it.
 */
export function maskedPaths(runRoot: string): string[] {
    const candidates = [...new Set(["/tmp", realpathSync(homedir()), runRoot])].sort(
        (a, b) => a.length - b.length,
    );
    const masked: string[] = [];
    for (const path of candidates) {
        if (!masked.some((outer) => within(path, outer))) masked.push(path);
    }
    return masked;
}

/** The sandbox restores kept paths after masking, so a run root inside one would stay readable. */
export function assertRunRootMaskable(runRoot: string, shared: readonly string[]): void {
    const keeper = shared.find((path) => within(runRoot, path));
    if (keeper !== undefined) {
        throw new Error(
            `the run directory ${runRoot} is inside ${keeper}, which every sandboxed arm can read; pass --out outside it`,
        );
    }
}

/**
 * The namespace's procfs exposes processes in the arm's PID namespace. Namespace PID 1 accepts
 * SIGTERM from an ancestor PID namespace only if PID 1 has installed a SIGTERM handler. dash runs
 * the last command of a `-c` script through `exec`, so the trailing `exit 0` keeps `setpriv` a
 * child of the shell. Exec preserves ignored signal dispositions, so `env --default-signal`
 * restores SIGINT and SIGTERM defaults for the harness. With a non-root invoking UID,
 * `--no-new-privs` keeps `sudo` unprivileged across exec.
 */
function sandboxScript(hidden: readonly string[]): string {
    const mounts = hidden
        .map((dir) => `mount -t tmpfs -o mode=1777 tmpfs ${plainPath(dir)}`)
        .join("\n");
    return `#!/bin/sh
AB_PATH="$PATH" exec sudo -n --preserve-env unshare --mount --pid --fork --mount-proc --propagation private /bin/sh -c '
set -e
keep=""
while [ "$1" != "--" ]; do keep="$keep $1"; shift; done
shift
st=$(mktemp -d /dev/shm/abst.XXXXXX)
for p in $keep; do mkdir -p "$st$p"; mount --rbind "$p" "$st$p"; done
${mounts}
for p in $keep; do mkdir -p "$p"; mount --rbind "$st$p" "$p"; done
for p in $keep; do umount -R "$st$p" || true; done
find "$st" -depth -type d -empty -delete || true
export PATH="$AB_PATH"
setpriv --reuid=${process.getuid?.() ?? 0} --regid=${process.getgid?.() ?? 0} --init-groups --no-new-privs --inh-caps=-all --bounding-set=-all -- env --default-signal=INT,TERM "$@"
exit 0' sh "$@"
`;
}

export function nodeRoot(): string {
    return resolve(realpathSync(Bun.which("node") as string), "../..");
}

export function opencodeBinary(): string {
    const found = Bun.which("opencode");
    if (!found) throw new Error("opencode is not on PATH");
    return realpathSync(found);
}

export function sharedKeep(): string[] {
    return [REPO_ROOT, nodeRoot(), ensurePiInstall(), resolve(opencodeBinary(), "../../..")];
}

export function sandboxKeep(armRoot: string): string[] {
    return [...sharedKeep(), armRoot].map(plainPath);
}

export function sandboxPath(): string {
    return [join(nodeRoot(), "bin"), "/usr/local/bin", "/usr/bin", "/bin"].join(":");
}

export function ensureSandboxScript(runRoot: string): string {
    const root = realpathSync(runRoot);
    const path = join(root, "sandbox.sh");
    writeFileSync(path, sandboxScript(maskedPaths(root)), { mode: 0o755 });
    return path;
}

export function sandboxAvailable(): boolean {
    return spawnSync("sudo", ["-n", "true"]).status === 0 && Bun.which("unshare") !== null;
}

import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

// `process.getBuiltinModule` returns the `node:fs` module object. Under Bun, a static `import`
// of `node:fs` adds about 4 ms to process startup.
const { existsSync, mkdirSync, realpathSync, renameSync, statSync, writeFileSync } =
    process.getBuiltinModule("node:fs");

export function publishJsonAtomically(
    value: unknown,
    path: string,
    options?: { mode?: number },
): void {
    mkdirSync(dirname(path), { recursive: true });
    const suffix = Buffer.from(crypto.getRandomValues(new Uint8Array(6))).toString("hex");
    const temp = `${path}.tmp-${suffix}`;
    writeFileSync(temp, `${JSON.stringify(value, null, 4)}\n`, {
        mode: options?.mode ?? 0o644,
    });
    renameSync(temp, path);
}

/** The repository root: private evaluation artifacts never land beneath it. */
const REPOSITORY_ROOT = resolve(import.meta.dir, "../../..");

/**
 * Publishes `value` as `<dir>/<name>` with mode `0600` in an owner-only directory outside the
 * repository. The directory is created `0700` when absent; an existing one must already be
 * `0700` and owned by this user. A `..` segment or a name that is not a plain file name is
 * refused.
 */
export function publishPrivateJson(value: unknown, dir: string, name: string): string {
    if (!/^[A-Za-z0-9._-]+$/.test(name) || name.startsWith(".")) {
        throw new Error(`${name} is not a plain file label`);
    }
    if (dir.split(/[\\/]/).includes("..")) throw new Error(`${dir} names a parent directory`);
    const target = resolve(dir);
    let existing = target;
    while (!existsSync(existing)) existing = dirname(existing);
    const real = join(realpathSync(existing), relative(existing, target));
    const inside = relative(realpathSync(REPOSITORY_ROOT), real);
    if (
        inside === "" ||
        (inside !== ".." && !inside.startsWith(`..${sep}`) && !isAbsolute(inside))
    ) {
        throw new Error(`${real} is inside the repository`);
    }
    mkdirSync(real, { recursive: true, mode: 0o700 });
    const stat = statSync(real);
    if ((stat.mode & 0o777) !== 0o700 || stat.uid !== process.getuid?.()) {
        throw new Error(`${real} is not an owner-only directory`);
    }
    const path = join(real, name);
    publishJsonAtomically(value, path, { mode: 0o600 });
    return path;
}

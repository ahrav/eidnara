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
 * Every existing ancestor of `real` is owned by this user or root and is either closed to
 * group and other writes or sticky, so no other local user can rename the checked directory
 * away between its check and the write. It runs before creation and again after, so a
 * component another user created in between is refused rather than trusted.
 */
function requireTrustedAncestors(real: string): void {
    const uid = process.getuid?.();
    let existing = real;
    while (!existsSync(existing)) existing = dirname(existing);
    for (let ancestor = existing; ; ancestor = dirname(ancestor)) {
        const stat = statSync(ancestor);
        const ownedByTrusted = stat.uid === uid || stat.uid === 0;
        const othersWrite = (stat.mode & 0o022) !== 0 && (stat.mode & 0o1000) === 0;
        if (!ownedByTrusted || othersWrite) {
            throw new Error(`${ancestor} is writable by others or not owned by this user or root`);
        }
        if (ancestor === dirname(ancestor)) break;
    }
}

/**
 * The path `dir` resolves to once its longest existing prefix is canonicalized through
 * symlinks; the publisher writes there, so callers compare directories by this path.
 */
export function realDirectory(dir: string): string {
    if (dir.split(/[\\/]/).includes("..")) throw new Error(`${dir} names a parent directory`);
    const target = resolve(dir);
    let existing = target;
    while (!existsSync(existing)) existing = dirname(existing);
    return join(realpathSync(existing), relative(existing, target));
}

/**
 * Publishes `value` as `<dir>/<name>` with mode `0600` in an owner-only directory outside the
 * repository. The directory is created `0700` when absent. An existing directory must be `0700`
 * and owned by this user, every existing ancestor must be trusted, and `name` must be a plain
 * file name.
 */
export function publishPrivateJson(value: unknown, dir: string, name: string): string {
    if (!/^[A-Za-z0-9._-]+$/.test(name) || name.startsWith(".")) {
        throw new Error(`${name} is not a plain file label`);
    }
    const real = realDirectory(dir);
    const inside = relative(realpathSync(REPOSITORY_ROOT), real);
    if (
        inside === "" ||
        (inside !== ".." && !inside.startsWith(`..${sep}`) && !isAbsolute(inside))
    ) {
        throw new Error(`${real} is inside the repository`);
    }
    requireTrustedAncestors(real);
    mkdirSync(real, { recursive: true, mode: 0o700 });
    requireTrustedAncestors(real);
    const stat = statSync(real);
    if ((stat.mode & 0o777) !== 0o700 || stat.uid !== process.getuid?.()) {
        throw new Error(`${real} is not an owner-only directory`);
    }
    const path = join(real, name);
    publishJsonAtomically(value, path, { mode: 0o600 });
    return path;
}

import { lstat, readlink, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import hostRelease from "../../../release/host-release.json";
import { ProviderError } from "./errors";

export const managedLayout = {
    managedSubtree: hostRelease.layout.managed_subtree,
    runtimeDirectory: hostRelease.layout.runtime_directory,
    storageSubdirectory: hostRelease.layout.storage_subdirectory,
} as const;

export interface ResolveProviderPathOptions {
    allowMissing: boolean;
    homeDirectory?: string;
    dataDirectory?: string;
    cwd?: string;
}

export async function resolveAndFenceProviderPath(
    configuredPath: string,
    options: ResolveProviderPathOptions,
): Promise<string> {
    const { home, dataDirectory } = await resolveFenceRoots(options);
    const expanded = configuredPath.startsWith("~/")
        ? join(home, configuredPath.slice(2))
        : configuredPath === "~"
          ? home
          : configuredPath;
    const absolute = isAbsolute(expanded)
        ? resolve(expanded)
        : resolve(options.cwd ?? process.cwd(), expanded);
    const canonical = await canonicalPath(absolute, options.allowMissing);
    if (isFencedPath(canonical, dataDirectory)) {
        throw new ProviderError("fenced_path", `Refusing fenced path: ${canonical}`);
    }
    return canonical;
}

export async function revalidateProviderPath(
    canonicalPath: string,
    options: ResolveProviderPathOptions,
): Promise<string> {
    const revalidated = await resolveAndFenceProviderPath(canonicalPath, options);
    if (revalidated !== canonicalPath) {
        throw new ProviderError(
            "fenced_path",
            `Refusing path changed after fence check: ${canonicalPath}`,
        );
    }
    return revalidated;
}

async function resolveFenceRoots(
    options: ResolveProviderPathOptions,
): Promise<{ home: string; dataDirectory: string }> {
    const configuredHomePath =
        absoluteOverride("homeDirectory", options.homeDirectory) ??
        absoluteOrNull(process.env.HOME) ??
        homedir();
    let home: string;
    try {
        home = await realpath(configuredHomePath);
    } catch (error) {
        throw fsError(configuredHomePath, error);
    }

    // Runtime storage is rooted at XDG_DATA_HOME. An absent, relative, or
    // empty environment value falls back to $HOME/.local/share, the same rule
    // the host applies in `crates/host-runtime/src/instance.rs`, so that root
    // is always fenced. The root is canonicalized here before checking paths, commentlint: allow(JUDGE)
    // so a symlinked data directory is checked by its real path.
    const configuredDataDirectory =
        absoluteOverride("dataDirectory", options.dataDirectory) ??
        absoluteOrNull(process.env.XDG_DATA_HOME) ??
        join(home, ".local", "share");
    const dataDirectory = await canonicalPath(configuredDataDirectory, true);
    return { home, dataDirectory };
}

/** A relative override is refused rather than resolved against cwd, which
 *  would move the fence roots with the working directory. */
function absoluteOverride(name: string, value: string | undefined): string | null {
    if (!value) return null;
    if (!isAbsolute(value)) {
        throw new ProviderError("invalid_option", `${name} must be an absolute path: ${value}`);
    }
    return value;
}

/** The env value participates only when it names an absolute path;
 *  relative and empty values are ignored rather than joined to cwd. */
function absoluteOrNull(value: string | undefined): string | null {
    if (!value || !isAbsolute(value)) return null;
    return value;
}

/** Matches the kernel's `SYMLOOP_MAX` (40 on Linux) that `realpath` enforces. commentlint: allow(JUDGE)
 *  A dangling target bypasses `realpath`'s symlink limit, so this walk enforces
 *  the bound. A target that normalizes back to its own link would otherwise
 *  recurse forever. */
const MAX_SYMLINK_HOPS = 40;

async function canonicalPath(
    path: string,
    allowMissing: boolean,
    hopsRemaining = MAX_SYMLINK_HOPS,
): Promise<string> {
    try {
        return await realpath(path);
    } catch (error) {
        if (!allowMissing || !isMissingError(error)) {
            throw fsError(path, error);
        }

        const suffix: string[] = [];
        let candidate = path;
        while (true) {
            try {
                const metadata = await lstat(candidate);
                if (metadata.isSymbolicLink()) {
                    if (hopsRemaining === 0) {
                        throw fsError(path, new Error("too many levels of symbolic links"));
                    }
                    // A relative target's `..` resolves against the real parent commentlint: allow(JUDGE)
                    // directory, not a symlinked lexical parent.
                    const [target, realParent] = await Promise.all([
                        readlink(candidate),
                        realpath(dirname(candidate)),
                    ]);
                    const resolvedTarget = resolve(realParent, target);
                    return canonicalPath(join(resolvedTarget, ...suffix), true, hopsRemaining - 1);
                }
            } catch (candidateError) {
                if (candidateError instanceof ProviderError) throw candidateError;
                if (!isMissingError(candidateError)) {
                    throw fsError(path, candidateError);
                }
            }

            const parent = dirname(candidate);
            if (parent === candidate) {
                throw fsError(path, error);
            }
            suffix.unshift(basename(candidate));
            candidate = parent;
            try {
                return join(await realpath(candidate), ...suffix);
            } catch (parentError) {
                if (!isMissingError(parentError)) {
                    throw fsError(path, parentError);
                }
            }
        }
    }
}

export function isFencedPath(canonicalPath: string, dataDirectory: string): boolean {
    const eidnaraRoot = join(resolve(dataDirectory), managedLayout.managedSubtree);
    const relativeToEidnara = relative(eidnaraRoot, canonicalPath);
    const insideEidnara =
        relativeToEidnara !== "" &&
        relativeToEidnara !== ".." &&
        !relativeToEidnara.startsWith(`..${sep}`) &&
        !isAbsolute(relativeToEidnara);
    const root = insideEidnara ? relativeToEidnara.split(sep)[0] : undefined;
    const inFencedRoot =
        root === managedLayout.runtimeDirectory || root === managedLayout.storageSubdirectory;
    const name = basename(canonicalPath);
    const fencedBasename = name.includes("binding-key") || name.endsWith(".handle");
    return inFencedRoot || fencedBasename;
}

function fsError(path: string, error: unknown): ProviderError {
    const message = error instanceof Error ? error.message : String(error);
    return new ProviderError("unreadable_path", `Could not read ${path}: ${message}`);
}

function isMissingError(error: unknown): boolean {
    return (
        error !== null &&
        typeof error === "object" &&
        "code" in error &&
        ((error as { code?: unknown }).code === "ENOENT" ||
            (error as { code?: unknown }).code === "ENOTDIR")
    );
}

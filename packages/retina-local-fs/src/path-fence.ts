import { lstat, readlink, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import hostRelease from "../../../release/host-release.json";
import { fsError, isMissingError, ProviderError } from "./errors";

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
    const { expanded, dataDirectory } = await resolveFenceRoots(configuredPath, options);
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

/** `dataDirectory` mirrors the host's data-root rule in `crates/host-runtime/src/instance.rs`.
 *  `HOME` is resolved only for a `~` path or the home-derived fallback.
 *  The host never consults `HOME` beside an absolute XDG_DATA_HOME.
 *  Refusing an absolute path over an unusable `HOME` would reject an environment the host accepts. */
async function resolveFenceRoots(
    configuredPath: string,
    options: ResolveProviderPathOptions,
): Promise<{ expanded: string; dataDirectory: string }> {
    const configuredDataDirectory =
        absoluteOverride("dataDirectory", options.dataDirectory) ??
        absoluteOrNull(process.env.XDG_DATA_HOME);
    const usesHome = configuredPath === "~" || configuredPath.startsWith("~/");
    if (!usesHome && configuredDataDirectory !== null) {
        return {
            expanded: configuredPath,
            dataDirectory: await canonicalPath(configuredDataDirectory, true),
        };
    }

    const home = await resolveHome(options);
    const expanded = usesHome
        ? configuredPath === "~"
            ? home
            : join(home, configuredPath.slice(2))
        : configuredPath;
    const dataDirectory = await canonicalPath(
        configuredDataDirectory ?? join(home, ".local", "share"),
        true,
    );
    return { expanded, dataDirectory };
}

/** `homedir()` returns a set `HOME` verbatim, so a relative `HOME` is rejected here; falling through would anchor it to cwd.
 *  A missing `HOME` is tolerated because the host accepts any absolute `HOME` and creates the tree beneath it on first run. */
async function resolveHome(options: ResolveProviderPathOptions): Promise<string> {
    const configuredHomePath =
        absoluteOverride("homeDirectory", options.homeDirectory) ??
        absoluteOverride("HOME", process.env.HOME) ??
        homedir();
    return canonicalPath(configuredHomePath, true);
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

/** Matches the kernel's `SYMLOOP_MAX` (40 on Linux) that `realpath` enforces.
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
                    // A relative target's `..` resolves against the real parent
                    // directory, not a symlinked lexical parent.
                    const [target, realParent] = await Promise.all([
                        readlink(candidate),
                        realpath(dirname(candidate)),
                    ]);
                    const resolvedTarget = resolve(realParent, target);
                    return await canonicalPath(
                        join(resolvedTarget, ...suffix),
                        true,
                        hopsRemaining - 1,
                    );
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

/** Both arguments must already be canonical: the comparison is lexical, so a
 *  symlinked spelling of either side would place a fenced path outside the root. */
export function isFencedPath(canonicalPath: string, canonicalDataDirectory: string): boolean {
    const eidnaraRoot = join(canonicalDataDirectory, managedLayout.managedSubtree);
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
    const fencedBasename =
        name.includes("binding-key") || name.endsWith(".handle") || name.endsWith(".lease");
    return inFencedRoot || fencedBasename;
}

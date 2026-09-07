import { lstat, readlink, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import hostRelease from "../../../release/host-release.json";
import { fsError, isMissingError, ProviderError } from "./errors";

export const managedLayout = {
    managedSubtree: hostRelease.layout.managed_subtree,
    runtimeDirectory: hostRelease.layout.runtime_directory,
    connectionFile: hostRelease.layout.connection_file,
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
    if (isFencedPath(canonical, home, dataDirectory)) {
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
    // A relative or empty HOME is ignored like the daemon ignores it;
    // os.homedir() reads passwd and is always absolute.
    const configuredHomePath = resolve(
        options.homeDirectory ?? absoluteOrNull(process.env.HOME) ?? homedir(),
    );
    let home: string;
    try {
        home = await realpath(configuredHomePath);
    } catch (error) {
        throw fsError(configuredHomePath, error);
    }

    // Runtime storage is rooted at XDG_DATA_HOME. An absent, relative, or
    // empty environment value falls back to $HOME/.local/share, the same rule
    // the host applies in `crates/host-runtime/src/instance.rs`, so that root
    // is always fenced. An empty explicit override counts as absent. The root
    // is canonicalized here before checking paths, so a symlinked data
    // directory is checked by its real path.
    const configuredDataDirectory =
        options.dataDirectory ||
        absoluteOrNull(process.env.XDG_DATA_HOME) ||
        join(home, ".local", "share");
    const dataDirectory = await canonicalPath(resolve(configuredDataDirectory), true);
    return { home, dataDirectory };
}

/** The env value participates only when it names an absolute path;
 *  relative and empty values are ignored rather than joined to cwd. */
function absoluteOrNull(value: string | undefined): string | null {
    if (!value || !isAbsolute(value)) return null;
    return value;
}

async function canonicalPath(path: string, allowMissing: boolean): Promise<string> {
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
                    const target = await readlink(candidate);
                    const resolvedTarget = resolve(dirname(candidate), target);
                    return canonicalPath(join(resolvedTarget, ...suffix), true);
                }
            } catch (candidateError) {
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

export function isFencedPath(
    canonicalPath: string,
    homeDirectory: string,
    dataDirectory = absoluteOrNull(process.env.XDG_DATA_HOME) ??
        join(resolve(homeDirectory), ".local", "share"),
): boolean {
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

export class ProviderError extends Error {
    constructor(
        readonly code: string,
        message: string,
    ) {
        super(message);
        this.name = "ProviderError";
    }
}

export function fsError(path: string, error: unknown): ProviderError {
    const message = error instanceof Error ? error.message : String(error);
    return new ProviderError("unreadable_path", `Could not read ${path}: ${message}`);
}

/** Only ENOENT names a path that can still be created. ENOTDIR means a regular
 *  file sits where a directory is needed, so a descendant of it is impossible,
 *  not missing. */
export function isMissingError(error: unknown): boolean {
    return hasErrnoCode(error, "ENOENT");
}

export function hasErrnoCode(error: unknown, ...codes: string[]): boolean {
    return (
        typeof error === "object" &&
        error !== null &&
        "code" in error &&
        typeof error.code === "string" &&
        codes.includes(error.code)
    );
}

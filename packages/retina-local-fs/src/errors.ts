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

export function isMissingError(error: unknown): boolean {
    return hasErrnoCode(error, "ENOENT", "ENOTDIR");
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

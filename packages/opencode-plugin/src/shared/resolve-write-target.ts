import { realpathSync } from "node:fs";

/**
 * Resolves symlinks so atomic replacement updates the target instead of replacing the link.
 * An absent path resolves to itself so the file is created there.
 */
export function resolveWriteTarget(filePath: string): string {
    try {
        return realpathSync(filePath);
    } catch {
        return filePath;
    }
}

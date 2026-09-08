import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";

import { stripJsonComments } from "../shared/jsonc-parser";

export interface SubstituteInput {
    /** Raw config text before JSONC parsing. */
    text: string;
    /**
     * Resolves relative `{file:...}` references.
     * Pass `undefined` for virtual inputs.
     * When `configPath` is undefined, relative `{file:}` paths resolve against `cwd`.
     * Pass `configPath` when a backing file exists.
     */
    configPath?: string;
    /**
     * Project-level config files leave `{env:}` and `{file:}` tokens literal to prevent secret expansion.
     */
    isProjectConfig?: boolean;
}

export interface SubstituteResult {
    /* */
    text: string;
    /**
     * Warnings cover missing environment variables, unreadable files, and tokens replaced with an empty string.
     */
    warnings: string[];
}

const ENV_PATTERN = /\{env:([^}]+)\}/g;
const FILE_PATTERN = /\{file:([^}]+)\}/g;
/** A file token whose path may embed complete `{env:...}` groups; `FILE_PATTERN` only needs to detect the token's presence. */
const NESTED_FILE_PATTERN = /\{file:((?:[^{}]|\{(?!env:)|\{env:[^{}]*\})+)\}/g;
const PLACEHOLDER_PATTERN = /\uE000eidnara:(\d+)\uE000/g;

/** `path.relative` applies the platform's separator and case rules, so a descendant is detected on Windows as well as POSIX. commentlint: allow(JUDGE) */
function isWithinDirectory(dir: string, candidate: string): boolean {
    const rel = relative(dir, candidate);
    return rel === "" || (rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel));
}

/**
 * User-level configs warn, rather than block, when `{file:}` resolves under these directories.
 */
function sensitiveFilePathReason(resolvedPath: string): string | null {
    const home = homedir();
    const sensitiveDirs: Array<{ dir: string; label: string }> = [
        { dir: resolve(home, ".ssh"), label: "SSH keys" },
        { dir: resolve(home, ".aws"), label: "AWS credentials" },
        { dir: resolve(home, ".gnupg"), label: "GnuPG keyring" },
        { dir: resolve(home, ".config", "gh"), label: "GitHub CLI auth" },
    ];
    for (const { dir, label } of sensitiveDirs) {
        if (isWithinDirectory(dir, resolvedPath)) {
            return label;
        }
    }
    return null;
}

/**
 *
 *   - `{env:VAR}` → `process.env.VAR` (trimmed key), JSON-escaped for safe inlining, empty string when missing
 *   - `{file:~/path}` → contents of `~/path`, JSON-escaped for safe inlining
 * `{file:./rel}` and `{file:rel}` resolve against `dirname(configPath)`, or `cwd` when `configPath` is undefined.
 *   - `{file:/abs}` → resolved as absolute
 *
 * Missing values produce warnings instead of errors.
 *
 */
export function substituteConfigVariables(input: SubstituteInput): SubstituteResult {
    const warnings: string[] = [];
    let text = input.text;

    if (input.isProjectConfig) {
        // Scan comment-stripped text so a documented token in a `//` or `/* */`
        // comment does not raise the security warning; the returned text stays
        // unchanged.
        const scanText = stripJsonComments(text);
        const hasEnvTokens = ENV_PATTERN.test(scanText);
        const hasFileTokens = FILE_PATTERN.test(scanText);
        ENV_PATTERN.lastIndex = 0;
        FILE_PATTERN.lastIndex = 0;
        if (hasEnvTokens || hasFileTokens) {
            const tokenTypes = [
                hasEnvTokens ? "{env:}" : undefined,
                hasFileTokens ? "{file:}" : undefined,
            ]
                .filter(Boolean)
                .join(" and ");
            warnings.push(
                `Project-level config no longer supports ${tokenTypes} tokens for security reasons; leaving tokens literal. Move secret expansion to user-level config.`,
            );
        }
        return { text, warnings };
    }

    // Strip JSONC comments before substitution to prevent tokens in comments from triggering environment or file reads.
    text = stripJsonComments(text);

    // Substituted values go in as placeholders and come back out in one final pass, so replacement text is never rescanned: an environment value that spells `{file:...}` stays a value, and file contents that spell `{env:...}` stay contents. The delimiter is a private-use code point. commentlint: allow(JUDGE)
    const substitutions: string[] = [];
    const placeholder = (value: string): string => {
        substitutions.push(value);
        return `\uE000eidnara:${substitutions.length - 1}\uE000`;
    };

    const envValue = (rawName: string): string | undefined => {
        const varName = rawName.trim();
        const value = varName ? process.env[varName] : undefined;
        if (value === undefined || value === "") {
            warnings.push(
                `Environment variable ${varName} is not set (referenced via {env:${varName}}); using empty string`,
            );
            return undefined;
        }
        return value;
    };

    const configDir = input.configPath ? dirname(input.configPath) : process.cwd();

    // A file token's path may embed `{env:...}` groups, and each group's value is a raw path fragment that may itself contain `}`; matching the groups as units keeps such a value from ending the token early. commentlint: allow(JUDGE)
    text = text.replace(
        NESTED_FILE_PATTERN,
        (token, rawPath: string, index: number, source: string) => {
            const lineStart = source.lastIndexOf("\n", index - 1) + 1;
            const prefix = source.slice(lineStart, index).trimStart();
            if (prefix.startsWith("//")) return token;

            let filePath = rawPath
                .replace(ENV_PATTERN, (_, rawName: string) => envValue(rawName) ?? "")
                .trim();
            if (filePath.startsWith("~/")) {
                filePath = resolve(homedir(), filePath.slice(2));
            } else if (!isAbsolute(filePath)) {
                filePath = resolve(configDir, filePath);
            }

            // Inlining a sensitive file exposes its contents in the substituted config.
            const sensitiveReason = sensitiveFilePathReason(filePath);
            if (sensitiveReason) {
                warnings.push(
                    `${token} resolves to a sensitive path (${sensitiveReason}: ${filePath}); ` +
                        "inlining its contents into config — make sure this is intentional.",
                );
            }

            if (!existsSync(filePath)) {
                warnings.push(
                    `File not found for ${token} (resolved to ${filePath}); using empty string`,
                );
                return "";
            }

            let contents: string;
            try {
                contents = readFileSync(filePath, "utf-8").trim();
            } catch (error) {
                const message = error instanceof Error ? error.message : String(error);
                warnings.push(
                    `Failed to read file for ${token} (${filePath}): ${message}; using empty string`,
                );
                return "";
            }

            if (contents === "") {
                warnings.push(`File for ${token} (${filePath}) is empty; using empty string`);
                return "";
            }

            // JSON-escape substitutions so quotes, backslashes, and line breaks survive JSONC parsing.
            // `slice(1, -1)` removes `JSON.stringify`'s outer quotes so the substitution remains inside the caller's string literal.
            return placeholder(JSON.stringify(contents).slice(1, -1));
        },
    );

    text = text.replace(ENV_PATTERN, (_, rawName: string) => {
        const value = envValue(rawName);
        return value === undefined ? "" : placeholder(JSON.stringify(value).slice(1, -1));
    });

    text = text.replace(
        PLACEHOLDER_PATTERN,
        (_, index: string) => substitutions[Number(index)] ?? "",
    );
    return { text, warnings };
}

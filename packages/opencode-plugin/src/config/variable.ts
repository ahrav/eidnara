import { existsSync, readFileSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";

import { stripJsoncComments } from "../shared/jsonc-parser";

/**
 * The environment's home takes precedence over the account database, so a harness or test can point every home-relative path at a scratch directory; Bun's `os.homedir()` does not re-read `HOME` after startup. commentlint: allow(JUDGE)
 */
function homeDir(): string {
    if (process.platform === "win32") {
        return process.env.USERPROFILE || process.env.HOME || homedir();
    }
    return process.env.HOME || homedir();
}

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
     * Warnings cover missing environment variables, unreadable files, tokens replaced with an empty string, and sensitive-path advisories.
     */
    warnings: string[];
    /**
     * The subset of `warnings` where a token was replaced with an empty string or left unresolved.
     * A sensitive-path advisory is not a failure: the file was read and inlined.
     */
    failures: string[];
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

function realPathOrSelf(path: string): string {
    try {
        return realpathSync.native(path);
    } catch {
        return path;
    }
}

/** User-level configs warn, rather than block, when `{file:}` resolves under these directories. Each directory is compared by its spelled path and by its real path, so a home or credential directory that is itself a symlink still catches a candidate given by its real location. commentlint: allow(JUDGE) */
function sensitiveFilePathReason(resolvedPath: string): string | null {
    const home = homeDir();
    const sensitiveDirs: Array<{ dir: string; label: string }> = [
        { dir: resolve(home, ".ssh"), label: "SSH keys" },
        { dir: resolve(home, ".aws"), label: "AWS credentials" },
        { dir: resolve(home, ".gnupg"), label: "GnuPG keyring" },
        { dir: resolve(home, ".config", "gh"), label: "GitHub CLI auth" },
    ];
    for (const { dir, label } of sensitiveDirs) {
        if (isWithinDirectory(dir, resolvedPath)) return label;
        const realDir = realPathOrSelf(dir);
        if (realDir !== dir && isWithinDirectory(realDir, resolvedPath)) return label;
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
    const failures: string[] = [];
    const fail = (message: string): void => {
        warnings.push(message);
        failures.push(message);
    };
    let text = input.text;

    if (input.isProjectConfig) {
        // Scan comment-stripped text so a documented token in a `//` or `/* */`
        // comment does not raise the security warning; the returned text stays
        // unchanged.
        const scanText = stripJsoncComments(text);
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
            fail(
                `Project-level config no longer supports ${tokenTypes} tokens for security reasons; leaving tokens literal. Move secret expansion to user-level config.`,
            );
        }
        return { text, warnings, failures };
    }

    // Strip JSONC comments before substitution to prevent tokens in comments from triggering environment or file reads.
    text = stripJsoncComments(text);

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
            fail(
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

            // A missing fragment must not leave a shorter path that names some other existing file (`{file:{env:DIR}/secret}` with `DIR` unset would read `/secret`), so the whole token yields the empty string the env warning already announced. commentlint: allow(JUDGE)
            let nestedEnvMissing = false;
            let nestedEnvExpanded = false;
            let filePath = rawPath
                .replace(ENV_PATTERN, (_, rawName: string) => {
                    const value = envValue(rawName);
                    if (value === undefined) nestedEnvMissing = true;
                    else nestedEnvExpanded = true;
                    return value ?? "";
                })
                .trim();
            if (nestedEnvMissing) return "";
            if (filePath.startsWith("~/")) {
                filePath = resolve(homeDir(), filePath.slice(2));
            } else if (!isAbsolute(filePath)) {
                filePath = resolve(configDir, filePath);
            }
            // `token` is the literal text, so it names the `{env:...}` group rather than its value; the
            // resolved path carries that value and is withheld from every warning about this token.
            const shownPath = nestedEnvExpanded
                ? "path withheld: it contains an {env:} expansion"
                : filePath;

            // Inlining a sensitive file exposes its contents in the substituted config. The spelled path is classified before the existence check so the warning fires whether or not the file is there. commentlint: allow(JUDGE)
            const warnSensitive = (reason: string, target: string): void => {
                warnings.push(
                    `${token} resolves to a sensitive path (${reason}: ${target}); ` +
                        "inlining its contents into config — make sure this is intentional.",
                );
            };
            const spelledReason = sensitiveFilePathReason(filePath);
            if (spelledReason) warnSensitive(spelledReason, shownPath);

            if (!existsSync(filePath)) {
                fail(`File not found for ${token} (resolved to ${shownPath}); using empty string`);
                return "";
            }

            // The read follows symlinks, so an existing file's real path is classified too; a link elsewhere into a credential directory is still a credential read. commentlint: allow(JUDGE)
            if (!spelledReason) {
                const realPath = realPathOrSelf(filePath);
                const realReason = realPath === filePath ? null : sensitiveFilePathReason(realPath);
                if (realReason) {
                    warnSensitive(
                        realReason,
                        nestedEnvExpanded ? shownPath : `${filePath} -> ${realPath}`,
                    );
                }
            }

            let contents: string;
            try {
                contents = readFileSync(filePath, "utf-8").trim();
            } catch (error) {
                // Node embeds the path in `error.message`; an expanded token keeps only `error.code`.
                const code = (error as NodeJS.ErrnoException | undefined)?.code;
                const message = nestedEnvExpanded
                    ? (code ?? "read error")
                    : error instanceof Error
                      ? error.message
                      : String(error);
                fail(
                    `Failed to read file for ${token} (${shownPath}): ${message}; using empty string`,
                );
                return "";
            }

            if (contents === "") {
                fail(`File for ${token} (${shownPath}) is empty; using empty string`);
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
    return { text, warnings, failures };
}

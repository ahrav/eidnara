import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, sep } from "node:path";

import { substituteConfigVariables } from "./variable";

describe("substituteConfigVariables", () => {
    const ORIGINAL_ENV = { ...process.env };
    let tmpDir: string;

    beforeEach(() => {
        tmpDir = mkdtempSync(join(tmpdir(), "eidnara-variable-test-"));
    });

    afterEach(() => {
        try {
            rmSync(tmpDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
        } catch {
            /* */
        }
        process.env = { ...ORIGINAL_ENV };
    });

    describe("env substitution", () => {
        it("replaces {env:VAR} with process.env value", () => {
            process.env.EIDNARA_TEST_KEY = "sk-real-value";
            const input = `{ "api_key": "{env:EIDNARA_TEST_KEY}" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "sk-real-value" }`);
            expect(result.warnings).toHaveLength(0);
        });

        it("trims whitespace inside env token", () => {
            process.env.EIDNARA_TEST_KEY = "trimmed-value";
            const input = `{ "api_key": "{env: EIDNARA_TEST_KEY }" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "trimmed-value" }`);
            expect(result.warnings).toHaveLength(0);
        });

        it("JSON-escapes quotes in env values so JSONC parsing survives", () => {
            process.env.EIDNARA_QUOTED = 'sk-"quoted"-value';
            const input = `{ "api_key": "{env:EIDNARA_QUOTED}" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "sk-\\"quoted\\"-value" }`);
            expect(JSON.parse(result.text).api_key).toBe('sk-"quoted"-value');
            expect(result.warnings).toHaveLength(0);
        });

        it("JSON-escapes newlines in env values so JSONC parsing survives", () => {
            process.env.EIDNARA_MULTILINE = "line1\nline2";
            const input = `{ "api_key": "{env:EIDNARA_MULTILINE}" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "line1\\nline2" }`);
            expect(JSON.parse(result.text).api_key).toBe("line1\nline2");
            expect(result.warnings).toHaveLength(0);
        });

        it("prevents env values from injecting sibling JSON keys", () => {
            process.env.EIDNARA_INJECT = 'abc", "provider": "off';
            const input = `{ "api_key": "{env:EIDNARA_INJECT}" }`;

            const result = substituteConfigVariables({ text: input });
            const parsed = JSON.parse(result.text);

            expect(parsed.api_key).toBe('abc", "provider": "off');
            expect(parsed.provider).toBeUndefined();
            expect(result.warnings).toHaveLength(0);
        });

        it("emits warning and empty string for missing env var", () => {
            delete process.env.EIDNARA_MISSING_VAR;
            const input = `{ "api_key": "{env:EIDNARA_MISSING_VAR}" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "" }`);
            expect(result.warnings).toHaveLength(1);
            expect(result.warnings[0]).toContain("EIDNARA_MISSING_VAR");
            expect(result.warnings[0]).toContain("not set");
        });

        it("emits warning for empty-string env var", () => {
            process.env.EIDNARA_EMPTY = "";
            const input = `{ "api_key": "{env:EIDNARA_EMPTY}" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "" }`);
            expect(result.warnings).toHaveLength(1);
        });

        it("passes {env:} literally through (matches OpenCode regex: at least one char required)", () => {
            const input = `{ "api_key": "{env:}" }`;

            const result = substituteConfigVariables({ text: input });

            // `{env:}` passes through because env tokens require a nonempty name.
            expect(result.text).toBe(input);
            expect(result.warnings).toHaveLength(0);
        });

        it("handles multiple env tokens in one text", () => {
            process.env.EIDNARA_A = "alpha";
            process.env.EIDNARA_B = "beta";
            const input = `{ "a": "{env:EIDNARA_A}", "b": "{env:EIDNARA_B}", "c": "{env:EIDNARA_MISSING}" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "a": "alpha", "b": "beta", "c": "" }`);
            expect(result.warnings).toHaveLength(1);
            expect(result.warnings[0]).toContain("EIDNARA_MISSING");
        });
    });

    describe("file substitution", () => {
        it("inlines file contents for absolute path", () => {
            const keyFile = join(tmpDir, "key.txt");
            writeFileSync(keyFile, "sk-from-file\n");
            const input = `{ "api_key": "{file:${keyFile}}" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "sk-from-file" }`);
            expect(result.warnings).toHaveLength(0);
        });

        it("resolves relative path against configPath directory", () => {
            const keyFile = join(tmpDir, "key.txt");
            writeFileSync(keyFile, "relative-value");
            const configPath = join(tmpDir, "eidnara.jsonc");

            const input = `{ "api_key": "{file:./key.txt}" }`;
            const result = substituteConfigVariables({ text: input, configPath });

            expect(result.text).toBe(`{ "api_key": "relative-value" }`);
            expect(result.warnings).toHaveLength(0);
        });

        it("resolves relative path without leading ./", () => {
            const keyFile = join(tmpDir, "key.txt");
            writeFileSync(keyFile, "no-dot-slash");
            const configPath = join(tmpDir, "eidnara.jsonc");

            const input = `{ "api_key": "{file:key.txt}" }`;
            const result = substituteConfigVariables({ text: input, configPath });

            expect(result.text).toBe(`{ "api_key": "no-dot-slash" }`);
        });

        it("expands ~/ to home directory", () => {
            const input = `{ "marker": "{file:~/__eidnara-never-exists-${Date.now()}}" }`;

            const result = substituteConfigVariables({ text: input });

            // Missing-file warnings report the homedir-resolved path.
            expect(result.warnings).toHaveLength(1);
            expect(result.warnings[0]).toContain(homedir() + sep);
        });

        it("JSON-escapes quotes and newlines in file contents", () => {
            const keyFile = join(tmpDir, "multiline.txt");
            writeFileSync(keyFile, 'line1 with "quote"\nline2');
            const input = `{ "value": "{file:${keyFile}}" }`;

            const result = substituteConfigVariables({ text: input });

            // JSON escaping preserves the enclosing JSONC string's validity.
            expect(result.text).toBe(`{ "value": "line1 with \\"quote\\"\\nline2" }`);
            const parsed = JSON.parse(result.text);
            expect(parsed.value).toBe('line1 with "quote"\nline2');
        });

        it("emits warning and empty string for missing file", () => {
            const missing = join(tmpDir, "never-exists.txt");
            const input = `{ "api_key": "{file:${missing}}" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "" }`);
            expect(result.warnings).toHaveLength(1);
            expect(result.warnings[0]).toContain("not found");
            expect(result.warnings[0]).toContain(missing);
        });

        it("emits warning and empty string for an empty file", () => {
            const emptyFile = join(tmpDir, "empty.txt");
            writeFileSync(emptyFile, "");
            const input = `{ "api_key": "{file:${emptyFile}}" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "" }`);
            expect(result.warnings).toHaveLength(1);
            expect(result.warnings[0]).toContain("is empty");
            expect(result.warnings[0]).toContain(emptyFile);
        });

        it("emits warning and empty string for a whitespace-only file", () => {
            const blankFile = join(tmpDir, "blank.txt");
            writeFileSync(blankFile, "  \n\t\n");
            const input = `{ "api_key": "{file:${blankFile}}" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "" }`);
            expect(result.warnings).toHaveLength(1);
            expect(result.warnings[0]).toContain("is empty");
        });

        it("passes {file:} literally through (matches OpenCode regex: at least one char required)", () => {
            const input = `{ "api_key": "{file:}" }`;

            const result = substituteConfigVariables({ text: input });

            // `{file:}` is not a valid token and passes through literally.
            expect(result.text).toBe(input);
            expect(result.warnings).toHaveLength(0);
        });

        it("suppresses {file:} expansion inside // line comments", () => {
            const keyFile = join(tmpDir, "key.txt");
            writeFileSync(keyFile, "should-not-appear");
            const input = [
                `{`,
                `    // see docs: {file:${keyFile}}`,
                `    "other": "value"`,
                `}`,
            ].join("\n");

            const result = substituteConfigVariables({ text: input });

            expect(result.text).not.toContain(`{file:${keyFile}}`);
            expect(result.text).not.toContain("should-not-appear");
            expect(result.warnings).toHaveLength(0);
        });

        it("suppresses {file:} expansion inside block comments", () => {
            const keyFile = join(tmpDir, "key.txt");
            writeFileSync(keyFile, "should-not-appear");
            const input = [
                `{`,
                `    /* see docs: {file:${keyFile}} */`,
                `    "other": "value"`,
                `}`,
            ].join("\n");

            const result = substituteConfigVariables({ text: input });

            expect(result.text).not.toContain(`{file:${keyFile}}`);
            expect(result.text).not.toContain("should-not-appear");
            expect(result.warnings).toHaveLength(0);
        });

        it("still expands {file:} tokens inside strings that contain URL comment markers", () => {
            const keyFile = join(tmpDir, "key.txt");
            writeFileSync(keyFile, "token-value");
            const input = `{ "endpoint": "https://example.test/*literal*/{file:${keyFile}}" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(
                `{ "endpoint": "https://example.test/*literal*/token-value" }`,
            );
            expect(JSON.parse(result.text).endpoint).toBe(
                "https://example.test/*literal*/token-value",
            );
            expect(result.warnings).toHaveLength(0);
        });
    });

    describe("combined substitution", () => {
        it("handles env and file tokens together", () => {
            process.env.EIDNARA_COMBINED = "env-val";
            const keyFile = join(tmpDir, "combined.txt");
            writeFileSync(keyFile, "file-val");

            const input = `{ "e": "{env:EIDNARA_COMBINED}", "f": "{file:${keyFile}}" }`;
            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "e": "env-val", "f": "file-val" }`);
            expect(result.warnings).toHaveLength(0);
        });

        it("env tokens inside {file:} path expand before file read", () => {
            // Environment substitution runs before file substitution.
            // The file pass resolves paths emitted by env substitution.
            // substitution.
            process.env.EIDNARA_FILE_DIR = tmpDir;
            const keyFile = join(tmpDir, "indirect.txt");
            writeFileSync(keyFile, "indirect-value");

            const input = `{ "api_key": "{file:{env:EIDNARA_FILE_DIR}/indirect.txt}" }`;
            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "indirect-value" }`);
        });

        it("env values inside {file:} stay raw paths even when the directory name needs JSON escaping", () => {
            const quotedDir = join(tmpDir, 'a"b\\c');
            mkdirSync(quotedDir);
            writeFileSync(join(quotedDir, "secret.txt"), "quoted-dir-value");
            process.env.EIDNARA_FILE_DIR = quotedDir;

            const input = `{ "api_key": "{file:{env:EIDNARA_FILE_DIR}/secret.txt}", "dir": "{env:EIDNARA_FILE_DIR}" }`;
            const result = substituteConfigVariables({ text: input });

            // The file token read the directory verbatim; the standalone env token is still JSON-escaped for the string literal.
            expect(result.text).toBe(
                `{ "api_key": "quoted-dir-value", "dir": "${JSON.stringify(quotedDir).slice(1, -1)}" }`,
            );
            expect(result.warnings).toHaveLength(0);
        });

        it("a closing brace in an env-supplied directory does not end the {file:} token early", () => {
            const bracedDir = join(tmpDir, "a}b");
            mkdirSync(bracedDir);
            writeFileSync(join(bracedDir, "secret.txt"), "braced-dir-value");
            process.env.EIDNARA_FILE_DIR = bracedDir;

            const input = `{ "api_key": "{file:{env:EIDNARA_FILE_DIR}/secret.txt}" }`;
            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "braced-dir-value" }`);
            expect(result.warnings).toHaveLength(0);
        });

        it("an env value that spells a {file:} token is inlined as text, never read as a file", () => {
            const keyFile = join(tmpDir, "must-not-be-read.txt");
            writeFileSync(keyFile, "leaked");
            process.env.EIDNARA_TEST_KEY = `{file:${keyFile}}`;

            const input = `{ "api_key": "{env:EIDNARA_TEST_KEY}" }`;
            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "api_key": "{file:${keyFile}}" }`);
            expect(result.text).not.toContain("leaked");
            expect(result.warnings).toHaveLength(0);
        });

        it("file contents that spell an {env:} token are inlined as text, never expanded", () => {
            process.env.EIDNARA_TEST_KEY = "must-not-expand";
            const keyFile = join(tmpDir, "template.txt");
            writeFileSync(keyFile, "{env:EIDNARA_TEST_KEY}");

            const input = `{ "template": "{file:${keyFile}}" }`;
            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(`{ "template": "{env:EIDNARA_TEST_KEY}" }`);
            expect(result.warnings).toHaveLength(0);
        });

        it("a missing env inside a {file:} token empties the whole token instead of reading the shortened path", () => {
            // With the directory fragment gone, the remainder would name `<configDir>/secret.txt`, which exists here.
            writeFileSync(join(tmpDir, "secret.txt"), "must-not-be-read");
            delete process.env.EIDNARA_MISSING_DIR;

            const input = `{ "api_key": "{file:{env:EIDNARA_MISSING_DIR}/secret.txt}" }`;
            const result = substituteConfigVariables({
                text: input,
                configPath: join(tmpDir, "eidnara.jsonc"),
            });

            expect(result.text).toBe(`{ "api_key": "" }`);
            expect(result.text).not.toContain("must-not-be-read");
            expect(result.warnings).toHaveLength(1);
            expect(result.warnings[0]).toContain("EIDNARA_MISSING_DIR is not set");
        });
    });

    describe("no-op cases", () => {
        it("returns text unchanged when no tokens present", () => {
            const input = `{ "api_key": "literal-value", "provider": "openai-compatible" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(input);
            expect(result.warnings).toHaveLength(0);
        });

        it("leaves partial patterns like {env alone", () => {
            const input = `{ "note": "this {env is not a token" }`;

            const result = substituteConfigVariables({ text: input });

            expect(result.text).toBe(input);
            expect(result.warnings).toHaveLength(0);
        });
    });

    describe("project-level config", () => {
        it("leaves tokens literal and warns once", () => {
            process.env.EIDNARA_PROJECT = "must-not-expand";
            const input = `{ "a": "{env:EIDNARA_PROJECT}", "b": "{file:./key.txt}" }`;

            const result = substituteConfigVariables({ text: input, isProjectConfig: true });

            expect(result.text).toBe(input);
            expect(result.warnings).toHaveLength(1);
            expect(result.warnings[0]).toContain("{env:} and {file:}");
        });

        it("does not warn for tokens that appear only inside comments", () => {
            const input = [
                `{`,
                `    // use {env:API_KEY} in the user config`,
                `    /* or {file:~/key.txt} */`,
                `    "a": "literal"`,
                `}`,
            ].join("\n");

            const result = substituteConfigVariables({ text: input, isProjectConfig: true });

            // The comments survive in the output; only the warning scan ignores them.
            expect(result.text).toBe(input);
            expect(result.warnings).toHaveLength(0);
        });

        it("still warns when a real token sits beside a commented one", () => {
            const input = [
                `{`,
                `    // {file:~/docs.txt} is documented here`,
                `    "a": "{env:REAL_TOKEN}"`,
                `}`,
            ].join("\n");

            const result = substituteConfigVariables({ text: input, isProjectConfig: true });

            expect(result.text).toBe(input);
            expect(result.warnings).toHaveLength(1);
            expect(result.warnings[0]).toContain("{env:}");
            expect(result.warnings[0]).not.toContain("{file:}");
        });
    });

    describe("sensitive-path warnings", () => {
        it("warns when a user {file:} resolves into ~/.ssh", () => {
            const input = `{ "key": "{file:~/.ssh/id_rsa}" }`;
            const result = substituteConfigVariables({ text: input });
            // Whether the file exists or not, the sensitive-path warning fires
            // The sensitive-path warning precedes the existence check.
            expect(result.warnings.some((w) => w.includes("sensitive path"))).toBe(true);
            expect(result.warnings.some((w) => w.includes("SSH keys"))).toBe(true);
        });

        it("does NOT warn for an ordinary {file:} path", () => {
            const input = `{ "prompt": "{file:~/notes/context.md}" }`;
            const result = substituteConfigVariables({ text: input });
            expect(result.warnings.some((w) => w.includes("sensitive path"))).toBe(false);
        });

        it("does NOT warn for a sibling whose name merely extends a sensitive directory", () => {
            const input = `{ "prompt": "{file:~/.ssh-backup/notes.md}" }`;
            const result = substituteConfigVariables({ text: input });
            expect(result.warnings.some((w) => w.includes("sensitive path"))).toBe(false);
        });

        it("warns when the path reaches a sensitive directory through a parent segment", () => {
            const input = `{ "key": "{file:~/notes/../.aws/credentials}" }`;
            const result = substituteConfigVariables({ text: input });
            expect(result.warnings.some((w) => w.includes("AWS credentials"))).toBe(true);
        });

        it("warns when a symlink outside the credential directories points into one", () => {
            // `homedir()` follows HOME, so the credential directories live under the temp home for this test.
            process.env.HOME = tmpDir;
            mkdirSync(join(tmpDir, ".ssh"));
            writeFileSync(join(tmpDir, ".ssh", "id_rsa"), "private-key");
            const link = join(tmpDir, "innocent-link");
            symlinkSync(join(tmpDir, ".ssh", "id_rsa"), link);

            const result = substituteConfigVariables({ text: `{ "key": "{file:${link}}" }` });

            expect(result.text).toBe(`{ "key": "private-key" }`);
            const warning = result.warnings.find((w) => w.includes("sensitive path"));
            expect(warning).toContain("SSH keys");
            expect(warning).toContain(`${link} -> `);
        });
    });
});

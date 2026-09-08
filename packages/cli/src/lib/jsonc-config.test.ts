import { describe, expect, it } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { stringify } from "comment-json";

import { readJsoncConfig, readJsoncConfigForUpdate, readJsoncLenient } from "./jsonc-config";

describe("readJsoncConfigForUpdate", () => {
    it("returns a tree whose mutations serialize with the original comments", () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-cli-jsonc-update-"));
        const path = join(directory, "config.jsonc");
        writeFileSync(path, `{\n  // keep me\n  "items": ["a"] /* trailing */\n}\n`);

        try {
            const tree = readJsoncConfigForUpdate(path);
            (tree.items as unknown[]).push("b");
            const written = stringify(tree, null, 2);
            expect(written).toContain("// keep me");
            expect(written).toContain("/* trailing */");
            expect(written).toContain('"b"');
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });

    it("throws on an array document root and on prototype-pollution keys", () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-cli-jsonc-update-"));
        const arrayRoot = join(directory, "array.json");
        const polluted = join(directory, "polluted.json");
        writeFileSync(arrayRoot, `[1, 2]`);
        writeFileSync(polluted, `{"constructor": {"prototype": {"x": 1}}}`);

        try {
            expect(() => readJsoncConfigForUpdate(arrayRoot)).toThrow(
                "expected a JSON object at the document root",
            );
            expect(() => readJsoncConfigForUpdate(polluted)).toThrow("prototype-pollution");
            expect(readJsoncConfigForUpdate(join(directory, "missing.json"))).toEqual({});
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });

    it("rejects a scalar document root and a malformed string escape", () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-cli-jsonc-scalar-"));
        const scalarRoot = join(directory, "scalar.json");
        const surrogate = join(directory, "surrogate.json");
        writeFileSync(scalarRoot, `"disabled"`);
        writeFileSync(surrogate, `{"label": "\\ud800"}`);

        try {
            expect(() => readJsoncConfigForUpdate(scalarRoot)).toThrow(
                "expected a JSON object at the document root",
            );
            expect(() => readJsoncConfigForUpdate(surrogate)).toThrow("Invalid JSONC");
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });

    it("rejects malformed UTF-8 instead of rewriting it as U+FFFD", () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-cli-jsonc-utf8-"));
        const path = join(directory, "bad-utf8.json");
        writeFileSync(
            path,
            Buffer.concat([Buffer.from('{"label": "'), Buffer.from([0xff]), Buffer.from('"}')]),
        );

        try {
            expect(() => readJsoncConfigForUpdate(path)).toThrow();
            expect(readJsoncConfig(path).kind).toBe("parse-error");
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });

    it("refuses to rewrite a file whose numeric literal parsing rounded", () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-cli-jsonc-update-"));
        const path = join(directory, "big.json");

        try {
            for (const literal of ["9007199254740993", "0.123456789012345678901", "1e-400"]) {
                writeFileSync(path, `{"n": ${literal}, "plugin": []}`);
                expect(() => readJsoncConfigForUpdate(path)).toThrow("parser rounded");
                // Reading for diagnostics still works; only the rewrite is refused.
                expect(readJsoncConfig(path).kind).toBe("parsed");
            }
            // An overflowing literal is already invalid JSON to the shared parser.
            writeFileSync(path, `{"n": 1e400, "plugin": []}`);
            expect(() => readJsoncConfigForUpdate(path)).toThrow();
            for (const literal of ["9007199254740991", "1.0", "1e3", "-0.5", "2.5E-3", "0"]) {
                writeFileSync(path, `{"n": ${literal}, "plugin": []}`);
                expect(readJsoncConfigForUpdate(path)).toMatchObject({ n: Number(literal) });
            }
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });

    it.if(process.platform !== "win32")("refuses a FIFO instead of blocking on it", () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-cli-jsonc-fifo-"));
        const fifo = join(directory, "config.json");
        execFileSync("mkfifo", [fifo]);

        try {
            const started = performance.now();
            const result = readJsoncConfig(fifo);
            expect(performance.now() - started).toBeLessThan(2_000);
            expect(result.kind).toBe("parse-error");
            if (result.kind === "parse-error") {
                expect(result.error.message).toContain("not a regular file");
            }
            expect(() => readJsoncConfigForUpdate(fifo)).toThrow("not a regular file");
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });
});

describe("readJsoncConfig prototype-pollution hardening", () => {
    it("refuses dangerous keys recursively before config mutation", () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-cli-jsonc-"));
        const path = join(directory, "config.jsonc");
        writeFileSync(
            path,
            `{
                "nested": { "prototype": { "hidden": true } },
                "items": [{ "__proto__": { "plugin": ["attacker"] } }]
            }`,
        );

        try {
            const result = readJsoncConfig(path);
            expect(result.kind).toBe("parse-error");
            if (result.kind === "parse-error") {
                expect(result.error.message).toContain("prototype-pollution");
            }
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });

    it("rejects a document whose root prototype was overridden", () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-cli-jsonc-"));
        const path = join(directory, "config.jsonc");
        writeFileSync(path, `{ "__proto__": { "polluted": true }, "a": 1 }`);

        try {
            const result = readJsoncConfig(path);
            expect(result.kind).toBe("parse-error");
            expect(({} as { polluted?: boolean }).polluted).toBeUndefined();
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });
});

describe("readJsoncConfigForUpdate comment round-trip", () => {
    it("returns a tree that stringifies with the file's comments intact", () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-cli-jsonc-"));
        const path = join(directory, "config.jsonc");
        writeFileSync(
            path,
            [
                "{",
                "  // leading comment",
                '  "historian": { "model": "a" }, // trailing comment',
                "  /* block comment */",
                '  "packages": [ /* inside array */ "one" ]',
                "}",
            ].join("\n"),
        );

        try {
            const config = readJsoncConfigForUpdate(path);
            expect(config).toEqual({ historian: { model: "a" }, packages: ["one"] });
            config.added = true;
            const written = stringify(config, null, 2);
            expect(written).toContain("// leading comment");
            expect(written).toContain("// trailing comment");
            expect(written).toContain("/* block comment */");
            expect(written).toContain("/* inside array */");
            expect(written).toContain('"added": true');
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });
});

describe("readJsoncConfig I/O failure boundary", () => {
    it("reports an existing-but-unreadable path as parse-error instead of throwing", () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-cli-jsonc-io-"));
        // A directory passes the existsSync probe but readFileSync(EISDIR) fails.
        const path = join(directory, "config.jsonc");
        mkdirSync(path);

        try {
            const result = readJsoncConfig(path);
            expect(result.kind).toBe("parse-error");
            if (result.kind === "parse-error") {
                expect(result.error.path).toBe(path);
            }
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });

    it("keeps readJsoncLenient lenient on read failures (returns parseError, never throws)", () => {
        const directory = mkdtempSync(join(tmpdir(), "eidnara-cli-jsonc-io-"));
        const path = join(directory, "settings.json");
        mkdirSync(path);

        try {
            const result = readJsoncLenient(path);
            expect(result.value).toEqual({});
            expect(typeof result.parseError).toBe("string");
        } finally {
            rmSync(directory, { recursive: true, force: true });
        }
    });
});

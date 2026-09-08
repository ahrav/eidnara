import { afterEach, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import {
    chmodSync,
    existsSync,
    mkdirSync,
    mkdtempSync,
    rmSync,
    symlinkSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";
import { BootstrapError } from "./bootstrap";
import { prepareManagedLaunchTarget, resolveManagedPayloadDir } from "./owner";

const roots: string[] = [];

function tempRoot(): string {
    const root = mkdtempSync(join(tmpdir(), "eidnara-host-owner-"));
    roots.push(root);
    return root;
}

afterEach(() => {
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function sha256(bytes: Buffer | string): string {
    return createHash("sha256").update(bytes).digest("hex");
}

interface Fixture {
    root: string;
    dataRoot: string;
    parentRoot: string;
    packageDir: string;
    manifestPath: string;
    manifest: Record<string, unknown>;
    manifestText: string;
    launcherDigest: string;
    manifestDigest: string;
}

function writeManifest(f: Pick<Fixture, "manifestPath">, manifest: unknown): string {
    const text = `${JSON.stringify(manifest, null, 2)}\n`;
    writeFileSync(f.manifestPath, text);
    return text;
}

function fixture(): Fixture {
    const root = tempRoot();
    const dataRoot = join(root, "data");
    const parentRoot = join(root, "parent");
    const packageDir = join(parentRoot, "node_modules", "@eidnara", "host-linux-x64-gnu");
    const launcher = Buffer.from("\x7fELF qualified launcher\n");
    const model = Buffer.from("qualified model bytes\n");
    const launcherDigest = sha256(launcher);
    const manifest = {
        schema: "eidnara.payload-manifest/v1",
        release: { id: "eidnara-host-release", version: "0.1.0" },
        release_contract_sha256: "1".repeat(64),
        production_inputs_lock_sha256: "2".repeat(64),
        mode: "production",
        package: {
            name: "@eidnara/host-linux-x64-gnu",
            version: "0.1.0",
            target: "linux-x64-gnu",
        },
        platform_floor: { glibc: "2.34" },
        synapse: "qualified",
        launcher: "payload/bin/eidnara-host",
        files: [
            {
                path: "payload/bin/eidnara-host",
                type: "file",
                size: launcher.length,
                mode: "755",
                sha256: launcherDigest,
            },
            {
                path: "payload/model/model.onnx",
                type: "file",
                size: model.length,
                mode: "644",
                sha256: sha256(model),
            },
        ],
    };
    mkdirSync(join(packageDir, "payload", "bin"), { recursive: true, mode: 0o700 });
    mkdirSync(join(packageDir, "payload", "model"), { recursive: true, mode: 0o700 });
    writeFileSync(
        join(packageDir, "package.json"),
        JSON.stringify({
            name: "@eidnara/host-linux-x64-gnu",
            version: "0.1.0",
        }),
    );
    writeFileSync(join(packageDir, "payload", "bin", "eidnara-host"), launcher, {
        mode: 0o755,
    });
    writeFileSync(join(packageDir, "payload", "model", "model.onnx"), model, {
        mode: 0o644,
    });
    const manifestPath = join(packageDir, "payload-manifest.json");
    const manifestText = writeManifest({ manifestPath }, manifest);
    return {
        root,
        dataRoot,
        parentRoot,
        packageDir,
        manifestPath,
        manifest,
        manifestText,
        launcherDigest,
        manifestDigest: sha256(manifestText.slice(0, -1)),
    };
}

function prepare(f: Fixture, allowStaging: boolean, parentRoot = f.parentRoot) {
    return prepareManagedLaunchTarget({
        dataRoot: f.dataRoot,
        declaringParentRoot: parentRoot,
        target: "linux-x64-gnu",
        allowStaging,
    });
}

function addPayloadFile(
    f: Pick<Fixture, "packageDir">,
    relPath: string,
    text: string,
): Record<string, unknown> {
    const bytes = Buffer.from(text);
    writeFileSync(join(f.packageDir, relPath), bytes, { mode: 0o644 });
    return { path: relPath, type: "file", size: bytes.length, mode: "644", sha256: sha256(bytes) };
}

describe("managed lifecycle owner", () => {
    test("a verified package stages one retained descriptor keyed by its launcher digest", () => {
        const f = fixture();
        const target = prepare(f, true);

        expect(target?.kind).toBe("retained-fd");
        expect(target?.retained.path).toContain(f.launcherDigest);
        expect(target?.payloadDir).toBe(f.packageDir);
    });

    test("the manifest digest is over the file's bytes with one trailing newline stripped", () => {
        const f = fixture();
        expect(prepare(f, true)?.payloadManifestDigest).toBe(f.manifestDigest);
        expect(f.manifestDigest).not.toBe(sha256(f.manifestText));

        // `crates/daemon/src/bin/eidnara-host.rs` strips one trailing newline before it digests, so a manifest without one digests the same.
        writeFileSync(f.manifestPath, f.manifestText.slice(0, -1));
        expect(prepare(f, true)?.payloadManifestDigest).toBe(f.manifestDigest);
    });

    test("observation reuses the retained bootstrap and never stages", () => {
        const f = fixture();
        expect(prepare(f, false)).toBeNull();

        const staged = prepare(f, true);
        expect(staged).not.toBeNull();

        const observed = prepare(f, false);
        expect(observed?.kind).toBe("retained-fd");
        expect(observed?.retained.path).toBe(staged?.retained.path);
    });

    test("an absent package is missing, not invalid", () => {
        const f = fixture();
        expect(() => prepare(f, true, join(f.root, "missing-parent"))).toThrow(BootstrapError);
        try {
            prepare(f, true, join(f.root, "missing-parent"));
        } catch (error) {
            expect((error as BootstrapError).reason).toBe("native_payload_missing");
        }
    });

    test("a package whose identity is outside the release contract is invalid", () => {
        const f = fixture();
        writeFileSync(
            join(f.packageDir, "package.json"),
            JSON.stringify({ name: "@eidnara/host-linux-x64-gnu", version: "0.2.0" }),
        );
        expect(() => prepare(f, true)).toThrow(/release contract/);

        writeFileSync(
            join(f.packageDir, "package.json"),
            JSON.stringify({ name: "@other/payload", version: "0.1.0" }),
        );
        expect(() => prepare(f, true)).toThrow(/release contract/);
    });

    test("a manifest with the wrong schema, identity, or launcher fails closed without staging", () => {
        const f = fixture();
        writeManifest(f, { ...f.manifest, schema: "eidnara.payload-manifest/v2" });
        expect(() => prepare(f, true)).toThrow(/schema/);

        writeManifest(f, {
            ...f.manifest,
            package: { ...(f.manifest.package as object), target: "linux-arm64-gnu" },
        });
        expect(() => prepare(f, true)).toThrow(/identity/);

        writeManifest(f, { ...f.manifest, launcher: "payload/bin/other" });
        expect(() => prepare(f, true)).toThrow(/identity/);

        writeManifest(f, {
            ...f.manifest,
            files: (f.manifest.files as unknown[]).slice(1),
        });
        expect(() => prepare(f, true)).toThrow(/launcher/);
    });

    test("the manifest key set is exactly the daemon's trusted-mode field set", () => {
        const f = fixture();
        const { synapse: _dropped, ...withoutSynapse } = f.manifest;
        writeManifest(f, withoutSynapse);
        expect(() => prepare(f, true)).toThrow(/keys do not match/);

        writeManifest(f, { ...f.manifest, extra: true });
        expect(() => prepare(f, true)).toThrow(/keys do not match/);

        writeManifest(f, { ...f.manifest, release: { id: "eidnara-host-release" } });
        expect(() => prepare(f, true)).toThrow(/keys do not match/);

        const files = f.manifest.files as Record<string, unknown>[];
        writeManifest(f, { ...f.manifest, files: [{ ...files[0], owner: "root" }, files[1]] });
        expect(() => prepare(f, true)).toThrow(/keys do not match/);
        expect(existsSync(f.dataRoot)).toBe(false);
    });

    test("a manifest outside the release identity or production mode fails closed", () => {
        const f = fixture();
        writeManifest(f, { ...f.manifest, release: { id: "other-release", version: "0.1.0" } });
        expect(() => prepare(f, true)).toThrow(/release identity/);

        writeManifest(f, {
            ...f.manifest,
            release: { id: "eidnara-host-release", version: "0.2.0" },
        });
        expect(() => prepare(f, true)).toThrow(/release identity/);

        writeManifest(f, { ...f.manifest, mode: "development" });
        expect(() => prepare(f, true)).toThrow(/release identity/);

        writeManifest(f, { ...f.manifest, release_contract_sha256: "not-a-digest" });
        expect(() => prepare(f, true)).toThrow(/release identity/);
        expect(existsSync(f.dataRoot)).toBe(false);
    });

    test("manifest bytes that are not valid UTF-8 are malformed, not repaired", () => {
        const f = fixture();
        // 0xff cannot start a UTF-8 sequence; a lenient decoder would substitute U+FFFD inside the string and parse on.
        const bytes = Buffer.from(f.manifestText, "utf8");
        const at = bytes.indexOf(Buffer.from('"qualified"')) + 1;
        bytes[at] = 0xff;
        writeFileSync(f.manifestPath, bytes);
        expect(() => prepare(f, true)).toThrow(/malformed/);
    });

    test("a manifest that repeats a key is rejected as serde would reject it", () => {
        const f = fixture();
        // `JSON.parse` keeps the last `mode`, so the parsed value is identical to the fixture's.
        const dup = f.manifestText.replace(
            '"mode": "production",',
            '"mode": "production",\n  "mode": "production",',
        );
        expect(dup).not.toBe(f.manifestText);
        writeFileSync(f.manifestPath, dup);
        expect(() => prepare(f, true)).toThrow(/repeats a key/);

        // `siz\u0065` decodes to `size`, so the scan must compare keys after unescaping.
        const files = f.manifest.files as Record<string, unknown>[];
        const entryText = JSON.stringify(files[1], null, 2).replace(
            '"size":',
            '"siz\\u0065": 1,\n  "size":',
        );
        const manifestText = writeManifest(f, { ...f.manifest, files: [files[0], {}] }).replace(
            "{}",
            entryText,
        );
        writeFileSync(f.manifestPath, manifestText);
        expect(() => prepare(f, true)).toThrow(/repeats a key/);
    });

    test("umask-stripped source modes are accepted; extra permission bits are not", () => {
        const f = fixture();
        const launcherPath = join(f.packageDir, "payload", "bin", "eidnara-host");
        const modelPath = join(f.packageDir, "payload", "model", "model.onnx");
        // An `umask` of `077` strips group and other permission bits from installed payload files.
        chmodSync(launcherPath, 0o700);
        chmodSync(modelPath, 0o600);
        expect(prepare(f, true)?.kind).toBe("retained-fd");

        chmodSync(modelPath, 0o664);
        expect(() => prepare(f, true)).toThrow(/metadata does not match/);
    });

    test("a payload path deeper than the daemon's staging limit is invalid", () => {
        const f = fixture();
        const files = f.manifest.files as Record<string, unknown>[];
        const deep = `payload/${Array(127).fill("d").join("/")}/leaf`;
        expect(deep.split("/").length).toBe(129);
        writeManifest(f, {
            ...f.manifest,
            files: [...files, { ...files[1], path: deep }],
        });
        expect(() => prepare(f, true)).toThrow(/file entry is invalid/);
    });

    test("a relative declaring root yields an absolute payload directory", () => {
        const f = fixture();
        const cwd = process.cwd();
        process.chdir(f.root);
        try {
            const target = prepare(f, true, "parent");
            expect(target?.payloadDir).toBe(f.packageDir);
            expect(isAbsolute(target?.payloadDir ?? "")).toBe(true);
        } finally {
            process.chdir(cwd);
        }
    });

    test("a launcher entry without mode 755 fails closed without staging", () => {
        const f = fixture();
        const files = f.manifest.files as Record<string, unknown>[];
        const launcherPath = join(f.packageDir, "payload", "bin", "eidnara-host");
        chmodSync(launcherPath, 0o644);
        writeManifest(f, { ...f.manifest, files: [{ ...files[0], mode: "644" }, files[1]] });
        expect(() => prepare(f, true)).toThrow(/executable launcher/);
        expect(existsSync(f.dataRoot)).toBe(false);
    });

    test("manifest file order is the daemon's UTF-8 byte order, not UTF-16 code unit order", () => {
        // U+E000 encodes as EE 80 80 and U+10000 as F0 90 80 80, so UTF-8 orders them E000 first; UTF-16 encodes U+10000 as the surrogate pair D800 DC00, which sorts before E000.
        const f = fixture();
        const files = f.manifest.files as Record<string, unknown>[];
        const bmp = addPayloadFile(f, "payload/\uE000", "bmp private-use name\n");
        const astral = addPayloadFile(f, "payload/\u{10000}", "astral name\n");

        writeManifest(f, { ...f.manifest, files: [...files, bmp, astral] });
        expect(prepare(f, true)?.kind).toBe("retained-fd");

        writeManifest(f, { ...f.manifest, files: [...files, astral, bmp] });
        expect(() => prepare(f, true)).toThrow(/file entry is invalid/);
    });

    test("launcher digest drift fails closed without staging", () => {
        const f = fixture();
        const files = f.manifest.files as Record<string, unknown>[];
        writeManifest(f, {
            ...f.manifest,
            files: [{ ...files[0], sha256: "f".repeat(64) }, files[1]],
        });
        expect(() => prepare(f, true)).toThrow(/payload file bytes/);
    });

    test("non-launcher payload mutation and symlink substitution fail before staging", () => {
        const f = fixture();
        const modelPath = join(f.packageDir, "payload", "model", "model.onnx");
        writeFileSync(modelPath, "mutated model bytes\n", { mode: 0o644 });
        expect(() => prepare(f, true)).toThrow(/payload file/);

        rmSync(modelPath);
        symlinkSync(join(f.packageDir, "payload", "bin", "eidnara-host"), modelPath);
        expect(() => prepare(f, true)).toThrow(/without following links/);
    });

    test("resolveManagedPayloadDir returns the verified package directory", () => {
        const f = fixture();
        expect(
            resolveManagedPayloadDir({
                declaringParentRoot: f.parentRoot,
                target: "linux-x64-gnu",
            }),
        ).toBe(f.packageDir);
    });
});

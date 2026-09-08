import { afterEach, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
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
        mode: "production",
        package: {
            name: "@eidnara/host-linux-x64-gnu",
            version: "0.1.0",
            target: "linux-x64-gnu",
        },
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

    test("a development-mode payload stages but carries no manifest digest", () => {
        const f = fixture();
        writeManifest(f, { ...f.manifest, mode: "development" });

        const target = prepare(f, true);

        expect(target?.kind).toBe("retained-fd");
        expect(target?.retained.path).toContain(f.launcherDigest);
        // `crates/daemon/src/bin/eidnara-host.rs` accepts `--payload-manifest-digest` only for `mode: "production"`; a development payload takes the unqualified path, which reads no manifest.
        expect(target?.payloadManifestDigest).toBeUndefined();
    });

    test("a payload whose mode is neither production nor development is invalid", () => {
        const f = fixture();
        writeManifest(f, { ...f.manifest, mode: "staging" });
        expect(() => prepare(f, true)).toThrow(/mode/);
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

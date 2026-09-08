import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import {
    chmodSync,
    cpSync,
    mkdirSync,
    mkdtempSync,
    readFileSync,
    renameSync,
    rmSync,
    statSync,
    symlinkSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
    ADDON_PATH,
    buildDevPayload,
    canonicalJson,
    type DevPayloadResult,
    LAUNCHER_PATH,
    loadReleaseContext,
    MANIFEST_SCHEMA,
    PAYLOAD_TARGET,
    type PayloadManifest,
    payloadManifestDigest,
    validatePayloadManifest,
    validatePayloadPackageDir,
} from "./build-host-payload";

const rootDir = join(import.meta.dir, "..");
const RUST_PARSER = "crates/daemon/src/bin/eidnara-host.rs";

function sha256(bytes: Uint8Array | string): string {
    return createHash("sha256").update(bytes).digest("hex");
}

/** Returns struct field names, applying `#[serde(rename = "...")]` to the following field. */
function rustStructFields(source: string, name: string): string[] {
    const match = new RegExp(`struct ${name} \\{([^}]*)\\}`).exec(source);
    if (match === null || match[1] === undefined) throw new Error(`struct ${name} not found`);
    const fields: string[] = [];
    let rename: string | undefined;
    for (const raw of match[1].split("\n")) {
        const line = raw.trim();
        const renamed = /^#\[serde\(rename = "([^"]+)"\)\]$/.exec(line);
        if (renamed !== null) {
            rename = renamed[1];
            continue;
        }
        const field = /^([a-z0-9_]+): /.exec(line);
        if (field !== null && field[1] !== undefined) {
            fields.push(rename ?? field[1]);
            rename = undefined;
        }
    }
    return fields;
}

function cloneManifest(manifest: PayloadManifest): PayloadManifest {
    return JSON.parse(JSON.stringify(manifest)) as PayloadManifest;
}

describe("build-host-payload", () => {
    let tmp: string;
    let launcherPath: string;
    let releaseAddon: string;
    let debugAddon: string;
    let built: DevPayloadResult;

    beforeAll(() => {
        tmp = mkdtempSync(join(tmpdir(), "eidnara-payload-"));
        launcherPath = join(tmp, "eidnara-host");
        writeFileSync(launcherPath, "#!/bin/sh\nexit 0\n");
        releaseAddon = join(tmp, "release-addon.cjs");
        writeFileSync(
            releaseAddon,
            'module.exports = { buildProfile: () => "release", buildTarget: () => "linux-x86_64" };\n',
        );
        debugAddon = join(tmp, "debug-addon.cjs");
        writeFileSync(
            debugAddon,
            'module.exports = { buildProfile: () => "debug", buildTarget: () => "linux-x86_64" };\n',
        );
        built = buildDevPayload(rootDir, {
            outDir: join(tmp, "out"),
            launcherPath,
            addonPath: releaseAddon,
        });
    });

    afterAll(() => {
        rmSync(tmp, { recursive: true, force: true });
    });

    test("manifest key sets equal the daemon parser's struct fields", () => {
        const source = readFileSync(join(rootDir, RUST_PARSER), "utf8");
        const { manifest } = built;
        expect(new Set(Object.keys(manifest))).toEqual(
            new Set(rustStructFields(source, "TrustedPayloadManifest")),
        );
        expect(new Set(Object.keys(manifest.release))).toEqual(
            new Set(rustStructFields(source, "TrustedReleaseIdentity")),
        );
        expect(new Set(Object.keys(manifest.package))).toEqual(
            new Set(rustStructFields(source, "TrustedPackageIdentity")),
        );
        const fileFields = new Set(rustStructFields(source, "TrustedPayloadFile"));
        expect(fileFields.has("type")).toBe(true);
        for (const entry of manifest.files) {
            expect(new Set(Object.keys(entry))).toEqual(fileFields);
        }
        const schemaLiteral = /const PAYLOAD_MANIFEST_SCHEMA: &str = "([^"]+)";/.exec(source);
        expect(schemaLiteral?.[1]).toBe(MANIFEST_SCHEMA);
    });

    test("digests recompute from the committed release files", () => {
        const contractBytes = readFileSync(join(rootDir, "release/host-release.json"));
        const trimmed =
            contractBytes.at(-1) === 0x0a ? contractBytes.subarray(0, -1) : contractBytes;
        const lockBytes = readFileSync(join(rootDir, "release/production-inputs.lock.json"));
        const lock = JSON.parse(lockBytes.toString("utf8")) as { release_contract_sha256: string };
        const context = loadReleaseContext(rootDir);
        expect(context.contractSha256).toBe(sha256(trimmed));
        expect(context.contractSha256).toBe(lock.release_contract_sha256);
        expect(context.lockSha256).toBe(sha256(lockBytes));
        expect(built.manifest.release_contract_sha256).toBe(context.contractSha256);
        expect(built.manifest.production_inputs_lock_sha256).toBe(context.lockSha256);
    });

    test("dev manifest shape: sorted files, modes, literals", () => {
        const { manifest, outDir } = built;
        const paths = manifest.files.map((entry) => entry.path);
        for (let i = 1; i < paths.length; i += 1) {
            expect(paths[i - 1]! < paths[i]!).toBe(true);
        }
        const launcher = manifest.files.find((entry) => entry.path === LAUNCHER_PATH);
        const addon = manifest.files.find((entry) => entry.path === ADDON_PATH);
        expect(launcher?.mode).toBe("755");
        expect(addon?.mode).toBe("644");
        expect(statSync(join(outDir, LAUNCHER_PATH)).mode & 0o777).toBe(0o755);
        expect(statSync(join(outDir, ADDON_PATH)).mode & 0o777).toBe(0o644);
        expect(manifest.mode).toBe("development");
        expect(manifest.launcher).toBe(LAUNCHER_PATH);
        expect(manifest.schema).toBe("eidnara.payload-manifest/v1");
        expect(manifest.release.id).toBe("eidnara-host-release");
        expect(manifest.package.name).toBe(PAYLOAD_TARGET.package);
        expect(manifest.package.target).toBe(PAYLOAD_TARGET.target);
        expect(built.launcherSha256).toBe(sha256(readFileSync(launcherPath)));
        expect(built.addonSha256).toBe(sha256(readFileSync(releaseAddon)));
    });

    test("validator rejects malformed manifests", () => {
        const context = loadReleaseContext(rootDir);
        const valid = built.manifest;
        expect(() => validatePayloadManifest(valid, context)).not.toThrow();

        const unsorted = cloneManifest(valid);
        unsorted.files.reverse();
        expect(() => validatePayloadManifest(unsorted, context)).toThrow(/ascending/);

        const noLauncher = cloneManifest(valid);
        noLauncher.files = noLauncher.files.filter((entry) => entry.path !== LAUNCHER_PATH);
        expect(() => validatePayloadManifest(noLauncher, context)).toThrow(/launcher/);

        const launcher644 = cloneManifest(valid);
        for (const entry of launcher644.files) {
            if (entry.path === LAUNCHER_PATH) entry.mode = "644";
        }
        expect(() => validatePayloadManifest(launcher644, context)).toThrow(/mode must be 755/);

        const extraKey = { ...cloneManifest(valid), extra: true };
        expect(() => validatePayloadManifest(extraKey, context)).toThrow(/unknown key extra/);

        const { synapse: _dropped, ...missingKey } = cloneManifest(valid);
        expect(() => validatePayloadManifest(missingKey, context)).toThrow(/missing key synapse/);

        const upperSha = cloneManifest(valid);
        upperSha.files[0]!.sha256 = upperSha.files[0]!.sha256.toUpperCase();
        expect(() => validatePayloadManifest(upperSha, context)).toThrow(/sha256/);

        const traversal = cloneManifest(valid);
        traversal.files[0]!.path = "payload/../escape";
        expect(() => validatePayloadManifest(traversal, context)).toThrow(/unsafe payload path/);
    });

    test("debug-profile addon is refused", () => {
        expect(() =>
            buildDevPayload(rootDir, {
                outDir: join(tmp, "out-debug"),
                launcherPath,
                addonPath: debugAddon,
            }),
        ).toThrow(/release-profile addon/);
    });

    test("an addon built for another native target is refused", () => {
        const foreignAddon = join(tmp, "foreign-addon.cjs");
        writeFileSync(
            foreignAddon,
            'module.exports = { buildProfile: () => "release", buildTarget: () => "darwin-arm64" };\n',
        );
        expect(() =>
            buildDevPayload(rootDir, {
                outDir: join(tmp, "out-foreign"),
                launcherPath,
                addonPath: foreignAddon,
            }),
        ).toThrow(/linux-x86_64 addon; .* reports darwin-arm64/);
    });

    test("relative launcher, addon, and out paths resolve against the working directory", () => {
        const previousCwd = process.cwd();
        process.chdir(tmp);
        try {
            const result = buildDevPayload(rootDir, {
                outDir: "out-relative",
                launcherPath: "./eidnara-host",
                addonPath: "./release-addon.cjs",
            });
            expect(result.outDir).toBe(join(process.cwd(), "out-relative"));
            expect(result.addonSha256).toBe(built.addonSha256);
        } finally {
            process.chdir(previousCwd);
        }
    });

    test("validatePayloadPackageDir accepts the committed package and rejects extra files", () => {
        expect(() => validatePayloadPackageDir(rootDir)).not.toThrow();

        const shadow = join(tmp, "shadow-root");
        mkdirSync(join(shadow, "packages"), { recursive: true });
        cpSync(join(rootDir, "release"), join(shadow, "release"), { recursive: true });
        cpSync(join(rootDir, PAYLOAD_TARGET.dir), join(shadow, PAYLOAD_TARGET.dir), {
            recursive: true,
        });
        const packageJsonPath = join(shadow, PAYLOAD_TARGET.dir, "package.json");
        const pkg = JSON.parse(readFileSync(packageJsonPath, "utf8")) as { files: string[] };
        pkg.files.push("extra.txt");
        writeFileSync(packageJsonPath, `${JSON.stringify(pkg, null, 2)}\n`);
        expect(() => validatePayloadPackageDir(shadow)).toThrow(/files/);
    });

    test("a staged package with a stale manifest fails the package check", () => {
        const shadow = join(tmp, "shadow-staged");
        mkdirSync(join(shadow, "packages"), { recursive: true });
        cpSync(join(rootDir, "release"), join(shadow, "release"), { recursive: true });
        cpSync(join(rootDir, PAYLOAD_TARGET.dir), join(shadow, PAYLOAD_TARGET.dir), {
            recursive: true,
        });
        const packageDir = join(shadow, PAYLOAD_TARGET.dir);
        buildDevPayload(shadow, { outDir: packageDir, launcherPath, addonPath: releaseAddon });
        expect(() => validatePayloadPackageDir(shadow)).not.toThrow();
        chmodSync(join(packageDir, LAUNCHER_PATH), 0o644);
        expect(() => validatePayloadPackageDir(shadow)).toThrow(/mode drift/);
    });

    test("the default launcher prefers target/debug over target/release", () => {
        const shadow = join(tmp, "shadow-launcher");
        mkdirSync(join(shadow, "packages"), { recursive: true });
        cpSync(join(rootDir, "release"), join(shadow, "release"), { recursive: true });
        cpSync(join(rootDir, PAYLOAD_TARGET.dir), join(shadow, PAYLOAD_TARGET.dir), {
            recursive: true,
        });
        for (const profile of ["debug", "release"]) {
            mkdirSync(join(shadow, "target", profile), { recursive: true });
            writeFileSync(
                join(shadow, "target", profile, "eidnara-host"),
                `#!/bin/sh\n# ${profile}\n`,
            );
        }
        const result = buildDevPayload(shadow, {
            outDir: join(tmp, "out-launcher"),
            addonPath: releaseAddon,
        });
        expect(result.launcherSha256).toBe(
            sha256(readFileSync(join(shadow, "target", "debug", "eidnara-host"))),
        );
    });

    test("a staged payload tree without a manifest fails the package check", () => {
        const shadow = join(tmp, "shadow-orphan");
        mkdirSync(join(shadow, "packages"), { recursive: true });
        cpSync(join(rootDir, "release"), join(shadow, "release"), { recursive: true });
        cpSync(join(rootDir, PAYLOAD_TARGET.dir), join(shadow, PAYLOAD_TARGET.dir), {
            recursive: true,
        });
        const packageDir = join(shadow, PAYLOAD_TARGET.dir);
        buildDevPayload(shadow, { outDir: packageDir, launcherPath, addonPath: releaseAddon });
        rmSync(join(packageDir, "payload-manifest.json"));
        expect(() => validatePayloadPackageDir(shadow)).toThrow(/manifest.json is missing/);
    });

    test("a symlinked payload root fails the package check", () => {
        const shadow = join(tmp, "shadow-symlink");
        mkdirSync(join(shadow, "packages"), { recursive: true });
        cpSync(join(rootDir, "release"), join(shadow, "release"), { recursive: true });
        cpSync(join(rootDir, PAYLOAD_TARGET.dir), join(shadow, PAYLOAD_TARGET.dir), {
            recursive: true,
        });
        const packageDir = join(shadow, PAYLOAD_TARGET.dir);
        buildDevPayload(shadow, { outDir: packageDir, launcherPath, addonPath: releaseAddon });
        const outside = join(tmp, "outside-payload");
        renameSync(join(packageDir, "payload"), outside);
        symlinkSync(outside, join(packageDir, "payload"));
        expect(() => validatePayloadPackageDir(shadow)).toThrow(
            /payload root must not be a symlink/,
        );
    });

    test("payloadManifestDigest equals the digest of the written file minus its newline", () => {
        const bytes = readFileSync(built.manifestPath);
        expect(bytes.at(-1)).toBe(0x0a);
        expect(built.digest).toBe(sha256(bytes.subarray(0, -1)));
        expect(payloadManifestDigest(built.manifest)).toBe(built.digest);
        expect(bytes.subarray(0, -1).toString("utf8")).toBe(canonicalJson(built.manifest));
        expect(canonicalJson({ b: 1, a: { d: [3, { z: 1, y: 2 }], c: 2 } })).toBe(
            '{"a":{"c":2,"d":[3,{"y":2,"z":1}]},"b":1}',
        );
    });
});

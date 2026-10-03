import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import {
    chmodSync,
    cpSync,
    existsSync,
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
    canonicalJson,
    type PayloadResult,
    LAUNCHER_PATH,
    loadReleaseContext,
    MANIFEST_SCHEMA,
    buildPayload,
    PAYLOAD_TARGET,
    type PayloadManifest,
    payloadManifestDigest,
    validatePayloadManifest,
    validatePayloadPackageDir,
    verifyPayloadDir,
} from "./build-host-payload";
import { acquireInputs } from "./host-inputs";

const rootDir = join(import.meta.dir, "..");
const RUST_PARSER = "crates/daemon/src/bin/eidnara-host.rs";

function sha256(bytes: Uint8Array | string): string {
    return createHash("sha256").update(bytes).digest("hex");
}

/** Returns struct field names, applying `#[serde(rename = "...")]` to the following field. */
function rustStructFields(source: string, name: string): string[] {
    const match = new RegExp(`struct ${name} \\{([^}]*)\\}`).exec(source);
    if (match === null || match[1] === undefined)
        throw new Error(`struct ${name} not found`);
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

/** Shell stand-in for `eidnara-host`; `release-info` emits the contract file plus a newline, `input-lock-digest` emits the lock digest, and `build-profile` emits `profile`. */
function fakeLauncherScript(
    contractPath: string,
    lockSha256: string,
    profile: "debug" | "release" = "debug",
): string {
    return [
        "#!/bin/sh",
        'case "$1" in',
        `  release-info) cat ${JSON.stringify(contractPath)}; echo ;;`,
        `  input-lock-digest) echo ${lockSha256} ;;`,
        `  build-profile) echo ${profile} ;;`,
        "  *) exit 0 ;;",
        "esac",
        "",
    ].join("\n");
}

function writeExecutable(path: string, contents: string): void {
    writeFileSync(path, contents);
    chmodSync(path, 0o755);
}

/** CommonJS stand-in for the native addon: the two build probes plus a token counter that answers like the Rust binding does for a non-empty string. `estimateTokens` is omitted when `null` and replaced when a body is supplied. */
function fakeAddonModule(
    profile: string,
    target: string,
    estimateTokens: string | null = "(text) => text.split(/\\s+/).length",
): string {
    const counter =
        estimateTokens === null ? "" : `, estimateTokens: ${estimateTokens}`;
    return `module.exports = { buildProfile: () => ${JSON.stringify(profile)}, buildTarget: () => ${JSON.stringify(target)}${counter} };\n`;
}

/** A root with the committed release files, except that each lock input names a small local file, so payload builds run without the real model and runtime bytes. */
async function fixtureRoot(dir: string): Promise<string> {
    mkdirSync(join(dir, "packages"), { recursive: true });
    mkdirSync(join(dir, "fixture-inputs"), { recursive: true });
    cpSync(join(rootDir, "release"), join(dir, "release"), { recursive: true });
    cpSync(join(rootDir, PAYLOAD_TARGET.dir), join(dir, PAYLOAD_TARGET.dir), {
        recursive: true,
    });
    rmSync(join(dir, PAYLOAD_TARGET.dir, "payload"), { recursive: true, force: true });
    rmSync(join(dir, PAYLOAD_TARGET.dir, "payload-manifest.json"), { force: true });
    const lockPath = join(dir, "release/production-inputs.lock.json");
    const lock = JSON.parse(readFileSync(lockPath, "utf8")) as {
        inputs: Record<string, Record<string, unknown>>;
    };
    for (const [key, input] of Object.entries(lock.inputs)) {
        const bytes = Buffer.from(`fixture ${key}\n`);
        writeFileSync(join(dir, "fixture-inputs", key), bytes);
        input.sha256 = sha256(bytes);
        input.size_bytes = bytes.length;
        input.source = `repo:fixture-inputs/${key}`;
        delete input.archive;
    }
    writeFileSync(lockPath, `${JSON.stringify(lock, null, 2)}\n`);
    await acquireInputs(dir, { inputsDir: join(dir, "target", "host-inputs") });
    return dir;
}

describe("build-host-payload", () => {
    let tmp: string;
    let fixture: string;
    let lockSha256: string;
    let launcherPath: string;
    let releaseLauncherPath: string;
    let releaseAddon: string;
    let debugAddon: string;
    let built: PayloadResult;
    const contractPath = join(rootDir, "release/host-release.json");

    beforeAll(async () => {
        tmp = mkdtempSync(join(tmpdir(), "eidnara-payload-"));
        fixture = await fixtureRoot(join(tmp, "fixture"));
        lockSha256 = sha256(
            readFileSync(join(fixture, "release/production-inputs.lock.json")),
        );
        launcherPath = join(tmp, "eidnara-host");
        writeExecutable(
            launcherPath,
            fakeLauncherScript(contractPath, lockSha256),
        );
        releaseLauncherPath = join(tmp, "release-eidnara-host");
        writeExecutable(
            releaseLauncherPath,
            fakeLauncherScript(contractPath, lockSha256, "release"),
        );
        releaseAddon = join(tmp, "release-addon.cjs");
        writeFileSync(releaseAddon, fakeAddonModule("release", "linux-x86_64"));
        debugAddon = join(tmp, "debug-addon.cjs");
        writeFileSync(debugAddon, fakeAddonModule("debug", "linux-x86_64"));
        built = await buildPayload(fixture, "development", {
            outDir: join(tmp, "out"),
            launcherPath,
            addonPath: releaseAddon,
        });
    });

    afterAll(() => {
        rmSync(tmp, { recursive: true, force: true });
    });

    test("manifest key sets equal the daemon parser's struct fields", async () => {
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
        const fileFields = new Set(
            rustStructFields(source, "TrustedPayloadFile"),
        );
        expect(fileFields.has("type")).toBe(true);
        for (const entry of manifest.files) {
            expect(new Set(Object.keys(entry))).toEqual(fileFields);
        }
        const schemaLiteral =
            /const PAYLOAD_MANIFEST_SCHEMA: &str = "([^"]+)";/.exec(source);
        expect(schemaLiteral?.[1]).toBe(MANIFEST_SCHEMA);
    });

    test("digests recompute from the release files", async () => {
        const contractBytes = readFileSync(
            join(fixture, "release/host-release.json"),
        );
        const trimmed =
            contractBytes.at(-1) === 0x0a
                ? contractBytes.subarray(0, -1)
                : contractBytes;
        const lockBytes = readFileSync(
            join(fixture, "release/production-inputs.lock.json"),
        );
        const lock = JSON.parse(lockBytes.toString("utf8")) as {
            release_contract_sha256: string;
        };
        const context = loadReleaseContext(fixture);
        expect(context.contractSha256).toBe(sha256(trimmed));
        expect(context.contractSha256).toBe(lock.release_contract_sha256);
        expect(context.lockSha256).toBe(sha256(lockBytes));
        expect(built.manifest.release_contract_sha256).toBe(
            context.contractSha256,
        );
        expect(built.manifest.production_inputs_lock_sha256).toBe(
            context.lockSha256,
        );
    });

    test("dev manifest shape: sorted files, modes, literals", async () => {
        const { manifest, outDir } = built;
        const paths = manifest.files.map((entry) => entry.path);
        for (let i = 1; i < paths.length; i += 1) {
            expect(paths[i - 1]! < paths[i]!).toBe(true);
        }
        const launcher = manifest.files.find(
            (entry) => entry.path === LAUNCHER_PATH,
        );
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

    test("validator rejects malformed manifests", async () => {
        const context = loadReleaseContext(fixture);
        const valid = built.manifest;
        expect(() => validatePayloadManifest(valid, context)).not.toThrow();

        const unsorted = cloneManifest(valid);
        unsorted.files.reverse();
        expect(() => validatePayloadManifest(unsorted, context)).toThrow(
            /ascending/,
        );

        const noLauncher = cloneManifest(valid);
        noLauncher.files = noLauncher.files.filter(
            (entry) => entry.path !== LAUNCHER_PATH,
        );
        expect(() => validatePayloadManifest(noLauncher, context)).toThrow(
            /launcher/,
        );

        const launcher644 = cloneManifest(valid);
        for (const entry of launcher644.files) {
            if (entry.path === LAUNCHER_PATH) entry.mode = "644";
        }
        expect(() => validatePayloadManifest(launcher644, context)).toThrow(
            /mode must be 755/,
        );

        const extraKey = { ...cloneManifest(valid), extra: true };
        expect(() => validatePayloadManifest(extraKey, context)).toThrow(
            /unknown key extra/,
        );

        const { local_embeddings: _dropped, ...missingKey } =
            cloneManifest(valid);
        expect(() => validatePayloadManifest(missingKey, context)).toThrow(
            /missing key local_embeddings/,
        );

        const upperSha = cloneManifest(valid);
        upperSha.files[0]!.sha256 = upperSha.files[0]!.sha256.toUpperCase();
        expect(() => validatePayloadManifest(upperSha, context)).toThrow(
            /sha256/,
        );

        const traversal = cloneManifest(valid);
        traversal.files[0]!.path = "payload/../escape";
        expect(() => validatePayloadManifest(traversal, context)).toThrow(
            /unsafe payload path/,
        );
    });

    test("development and production builds stage every locked input at its locked bytes", async () => {
        const context = loadReleaseContext(fixture);
        const production = await buildPayload(fixture, "production", {
            outDir: join(tmp, "out-production"),
            launcherPath: releaseLauncherPath,
            addonPath: releaseAddon,
        });
        expect(production.manifest.mode).toBe("production");
        for (const result of [built, production]) {
            for (const input of context.inputs) {
                const entry = result.manifest.files.find((file) => file.path === input.payload_path);
                expect(entry).toEqual({
                    path: input.payload_path,
                    type: "file",
                    size: input.size_bytes,
                    mode: "644",
                    sha256: input.sha256,
                });
                expect(sha256(readFileSync(join(result.outDir, input.payload_path)))).toBe(input.sha256);
            }
        }
        const stripped = (result: PayloadResult) =>
            result.manifest.files.filter((file) => file.path !== LAUNCHER_PATH);
        expect(stripped(production)).toEqual(stripped(built));
    });

    test("a launcher whose build profile does not match the payload mode is refused", async () => {
        await expect(
            buildPayload(fixture, "production", {
                outDir: join(tmp, "out-production-debug"),
                launcherPath,
                addonPath: releaseAddon,
            }),
        ).rejects.toThrow(/production payload requires a release eidnara-host; .* reports debug/);
        await expect(
            buildPayload(fixture, "development", {
                outDir: join(tmp, "out-development-release"),
                launcherPath: releaseLauncherPath,
                addonPath: releaseAddon,
            }),
        ).rejects.toThrow(/development payload requires a debug eidnara-host; .* reports release/);
        expect(existsSync(join(tmp, "out-production-debug", "payload"))).toBe(false);
        expect(existsSync(join(tmp, "out-development-release", "payload"))).toBe(false);
    });

    test("a manifest that omits or alters a locked input is refused", async () => {
        const context = loadReleaseContext(fixture);
        const input = context.inputs[0]!;
        const omitted = cloneManifest(built.manifest);
        omitted.files = omitted.files.filter((file) => file.path !== input.payload_path);
        expect(() => validatePayloadManifest(omitted, context)).toThrow(/locked input/);
        const altered = cloneManifest(built.manifest);
        for (const file of altered.files) {
            if (file.path === input.payload_path) file.sha256 = "0".repeat(64);
        }
        expect(() => validatePayloadManifest(altered, context)).toThrow(/locked input/);
    });

    test("a missing or wrong cache entry refuses the build before the output tree changes", async () => {
        const context = loadReleaseContext(fixture);
        const input = context.inputs[0]!;
        const cache = join(tmp, "cache-wrong");
        cpSync(join(fixture, "target", "host-inputs"), cache, { recursive: true });
        writeFileSync(join(cache, input.sha256), "wrong same-name bytes");
        await expect(
            buildPayload(fixture, "development", { outDir: built.outDir, launcherPath, addonPath: releaseAddon, inputsDir: cache }),
        ).rejects.toThrow(/cache entry/);
        rmSync(join(cache, input.sha256));
        await expect(
            buildPayload(fixture, "development", { outDir: built.outDir, launcherPath, addonPath: releaseAddon, inputsDir: cache }),
        ).rejects.toThrow(/is missing/);
        expect(() => verifyPayloadDir(built.outDir, built.manifest)).not.toThrow();
    });

    test("an unsafe locked payload path refuses the build before the output tree changes", async () => {
        const { shadow } = shadowRoot("shadow-unsafe-path");
        const lockPath = join(shadow, "release/production-inputs.lock.json");
        const lock = JSON.parse(readFileSync(lockPath, "utf8")) as {
            inputs: Record<string, { payload_path: string }>;
        };
        const first = Object.values(lock.inputs)[0]!;
        first.payload_path = "payload/../escape";
        writeFileSync(lockPath, `${JSON.stringify(lock, null, 2)}\n`);
        const unsafeLauncher = join(tmp, "unsafe-eidnara-host");
        writeExecutable(unsafeLauncher, fakeLauncherScript(contractPath, sha256(readFileSync(lockPath))));
        await expect(
            buildPayload(shadow, "development", {
                outDir: built.outDir,
                launcherPath: unsafeLauncher,
                addonPath: releaseAddon,
            }),
        ).rejects.toThrow(/unsafe payload path/);
        expect(() => verifyPayloadDir(built.outDir, built.manifest)).not.toThrow();
    });

    test("debug-profile addon is refused", async () => {
        await expect(
            buildPayload(fixture, "development", {
                outDir: join(tmp, "out-debug"),
                launcherPath,
                addonPath: debugAddon,
            }),
        ).rejects.toThrow(/release-profile addon/);
    });

    test("an addon built for another native target is refused", async () => {
        const foreignAddon = join(tmp, "foreign-addon.cjs");
        writeFileSync(foreignAddon, fakeAddonModule("release", "darwin-arm64"));
        await expect(
            buildPayload(fixture, "development", {
                outDir: join(tmp, "out-foreign"),
                launcherPath,
                addonPath: foreignAddon,
            }),
        ).rejects.toThrow(/linux-x86_64 addon; .* reports darwin-arm64/);
    });

    test("an addon without the estimateTokens export is refused", async () => {
        const stale = join(tmp, "stale-addon.cjs");
        writeFileSync(stale, fakeAddonModule("release", "linux-x86_64", null));
        await expect(
            buildPayload(fixture, "development", {
                outDir: join(tmp, "out-stale"),
                launcherPath,
                addonPath: stale,
            }),
        ).rejects.toThrow(/estimateTokens/);
        expect(existsSync(join(tmp, "out-stale", "payload"))).toBe(false);
    });

    test("an estimateTokens export that cannot count a probe string is refused", async () => {
        for (const [name, body] of [
            ["zero", "() => 0"],
            ["fraction", "() => 1.5"],
            ["string", '() => "3"'],
            ["throws", '() => { throw new Error("no vocabulary"); }'],
        ] as const) {
            const broken = join(tmp, `broken-${name}-addon.cjs`);
            writeFileSync(broken, fakeAddonModule("release", "linux-x86_64", body));
            await expect(
                buildPayload(fixture, "development", {
                    outDir: join(tmp, `out-broken-${name}`),
                    launcherPath,
                    addonPath: broken,
                }),
                name,
            ).rejects.toThrow(/estimateTokens/);
        }
    });

    test("relative launcher, addon, and out paths resolve against the working directory", async () => {
        const previousCwd = process.cwd();
        process.chdir(tmp);
        try {
            const result = await buildPayload(fixture, "development", {
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

    /** Copies the fixture root's release files, inputs, and payload package into a fresh root under `tmp`. */
    function shadowRoot(name: string): { shadow: string; packageDir: string } {
        const shadow = join(tmp, name);
        mkdirSync(join(shadow, "packages"), { recursive: true });
        for (const dir of ["release", "fixture-inputs", "target", PAYLOAD_TARGET.dir]) {
            cpSync(join(fixture, dir), join(shadow, dir), { recursive: true });
        }
        return { shadow, packageDir: join(shadow, PAYLOAD_TARGET.dir) };
    }

    /** A shadow root whose payload package has a dev payload staged into it. */
    async function stagedShadow(name: string): Promise<{
        shadow: string;
        packageDir: string;
    }> {
        const root = shadowRoot(name);
        await buildPayload(root.shadow, "development", {
            outDir: root.packageDir,
            launcherPath,
            addonPath: releaseAddon,
        });
        expect(() => validatePayloadPackageDir(root.shadow)).not.toThrow();
        return root;
    }

    test("validatePayloadPackageDir accepts the committed package and rejects extra files", async () => {
        expect(() => validatePayloadPackageDir(rootDir)).not.toThrow();

        const { shadow, packageDir } = shadowRoot("shadow-root");
        const packageJsonPath = join(packageDir, "package.json");
        const pkg = JSON.parse(readFileSync(packageJsonPath, "utf8")) as {
            files: string[];
        };
        pkg.files.push("extra.txt");
        writeFileSync(packageJsonPath, `${JSON.stringify(pkg, null, 2)}\n`);
        expect(() => validatePayloadPackageDir(shadow)).toThrow(/files/);
    }, 30_000);

    test("a staged package with a stale manifest fails the package check", async () => {
        const { shadow, packageDir } = await stagedShadow("shadow-staged");
        chmodSync(join(packageDir, LAUNCHER_PATH), 0o644);
        expect(() => validatePayloadPackageDir(shadow)).toThrow(/mode drift/);
    });

    test("the default launcher is target/debug only; a release-only tree is refused", async () => {
        const { shadow } = shadowRoot("shadow-launcher");
        const releaseOnly = join(shadow, "target", "release", "eidnara-host");
        mkdirSync(join(shadow, "target", "release"), { recursive: true });
        writeExecutable(
            releaseOnly,
            fakeLauncherScript(contractPath, lockSha256),
        );
        await expect(
            buildPayload(shadow, "development", {
                outDir: join(tmp, "out-launcher"),
                addonPath: releaseAddon,
            }),
        ).rejects.toThrow(/no locally compiled debug eidnara-host/);

        const debug = join(shadow, "target", "debug", "eidnara-host");
        mkdirSync(join(shadow, "target", "debug"), { recursive: true });
        writeExecutable(
            debug,
            `${fakeLauncherScript(contractPath, lockSha256)}# debug\n`,
        );
        const result = await buildPayload(shadow, "development", {
            outDir: join(tmp, "out-launcher"),
            addonPath: releaseAddon,
        });
        expect(result.launcherSha256).toBe(sha256(readFileSync(debug)));
    });

    test("a launcher built from a different release contract or lock is refused", async () => {
        const staleContract = join(tmp, "stale-host-release.json");
        writeFileSync(staleContract, '{"stale":true}\n');
        const staleLauncher = join(tmp, "stale-eidnara-host");
        writeExecutable(
            staleLauncher,
            fakeLauncherScript(staleContract, lockSha256),
        );
        await expect(
            buildPayload(fixture, "development", {
                outDir: join(tmp, "out-stale"),
                launcherPath: staleLauncher,
                addonPath: releaseAddon,
            }),
        ).rejects.toThrow(/different release\/host-release.json/);

        const staleLock = join(tmp, "stale-lock-eidnara-host");
        writeExecutable(
            staleLock,
            fakeLauncherScript(contractPath, "0".repeat(64)),
        );
        await expect(
            buildPayload(fixture, "development", {
                outDir: join(tmp, "out-stale-lock"),
                launcherPath: staleLock,
                addonPath: releaseAddon,
            }),
        ).rejects.toThrow(/different release\/production-inputs.lock.json/);
    });

    test("a launcher that cannot answer release-info is refused", async () => {
        const foreign = join(tmp, "foreign-eidnara-host");
        writeExecutable(
            foreign,
            "#!/bin/sh\necho not eidnara-host >&2\nexit 1\n",
        );
        await expect(
            buildPayload(fixture, "development", {
                outDir: join(tmp, "out-foreign-launcher"),
                launcherPath: foreign,
                addonPath: releaseAddon,
            }),
        ).rejects.toThrow(/failed `release-info`: not eidnara-host/);
    });

    test("a staged payload tree without a manifest fails the package check", async () => {
        const { shadow, packageDir } = await stagedShadow("shadow-orphan");
        rmSync(join(packageDir, "payload-manifest.json"));
        expect(() => validatePayloadPackageDir(shadow)).toThrow(
            /manifest.json is missing/,
        );
    });

    test("a symlinked payload root fails the package check", async () => {
        const { shadow, packageDir } = await stagedShadow("shadow-symlink");
        const outside = join(tmp, "outside-payload");
        renameSync(join(packageDir, "payload"), outside);
        symlinkSync(outside, join(packageDir, "payload"));
        expect(() => validatePayloadPackageDir(shadow)).toThrow(
            /payload root must not be a symlink/,
        );
    });

    test("a symlinked manifest fails the package check", async () => {
        const { shadow, packageDir } = await stagedShadow("shadow-manifest-symlink");
        const outside = join(tmp, "outside-manifest.json");
        renameSync(join(packageDir, "payload-manifest.json"), outside);
        symlinkSync(outside, join(packageDir, "payload-manifest.json"));
        expect(() => validatePayloadPackageDir(shadow)).toThrow(
            /must be a regular file/,
        );
    });

    test("a symlinked package.json fails the package check", async () => {
        const { shadow, packageDir } = shadowRoot("shadow-package-symlink");
        const outside = join(tmp, "outside-package.json");
        renameSync(join(packageDir, "package.json"), outside);
        symlinkSync(outside, join(packageDir, "package.json"));
        expect(() => validatePayloadPackageDir(shadow)).toThrow(
            /package.json must be a regular file/,
        );
    });

    test("a source that is not a regular file is refused before it is read", async () => {
        const fifo = join(tmp, "addon.fifo");
        expect(Bun.spawnSync(["mkfifo", fifo]).exitCode).toBe(0);
        await expect(
            buildPayload(fixture, "development", {
                outDir: join(tmp, "out-fifo"),
                launcherPath,
                addonPath: fifo,
            }),
        ).rejects.toThrow(/addon source must be a regular file/);
    });

    test("the CLI refuses a flag where a path value is expected", async () => {
        const script = join(rootDir, "scripts", "build-host-payload.ts");
        const run = Bun.spawnSync(
            ["bun", script, "--dev", "--out", "--check"],
            {
                cwd: tmp,
                stdout: "pipe",
                stderr: "pipe",
            },
        );
        expect(run.exitCode).toBe(2);
        expect(run.stderr.toString()).toContain("--out requires a path");
        expect(existsSync(join(tmp, "--check"))).toBe(false);
    });

    test("the CLI accepts only native addon files for --addon", async () => {
        const script = join(rootDir, "scripts", "build-host-payload.ts");
        const run = Bun.spawnSync(
            [
                "bun",
                script,
                "--dev",
                "--out",
                "out-cjs",
                "--addon",
                releaseAddon,
            ],
            { cwd: tmp, stdout: "pipe", stderr: "pipe" },
        );
        expect(run.exitCode).toBe(2);
        expect(run.stderr.toString()).toContain(
            "--addon must name a .so or .node file",
        );
        expect(existsSync(join(tmp, "out-cjs"))).toBe(false);
    });

    test("payloadManifestDigest equals the digest of the written file minus its newline", async () => {
        const bytes = readFileSync(built.manifestPath);
        expect(bytes.at(-1)).toBe(0x0a);
        expect(built.digest).toBe(sha256(bytes.subarray(0, -1)));
        expect(payloadManifestDigest(built.manifest)).toBe(built.digest);
        expect(bytes.subarray(0, -1).toString("utf8")).toBe(
            canonicalJson(built.manifest),
        );
        expect(
            canonicalJson({ b: 1, a: { d: [3, { z: 1, y: 2 }], c: 2 } }),
        ).toBe('{"a":{"c":2,"d":[3,{"y":2,"z":1}]},"b":1}');
    });
});

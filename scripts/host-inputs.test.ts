import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gzipSync } from "node:zlib";

import {
    acquireInputs,
    cachePath,
    extractArchiveMember,
    type FetchLike,
    type LockedInput,
    lockedInputs,
    verifiedCacheEntry,
} from "./host-inputs";

const rootDir = join(import.meta.dir, "..");

function sha256(bytes: Uint8Array): string {
    return createHash("sha256").update(bytes).digest("hex");
}

/** `type` is the ustar typeflag byte. */
function tarEntry(name: string, body: Buffer, type = "0"): Buffer {
    const header = Buffer.alloc(512);
    header.write(name, 0, "utf8");
    header.write("0000644\0", 100);
    header.write("0000000\0", 108);
    header.write("0000000\0", 116);
    header.write(`${body.length.toString(8).padStart(11, "0")}\0`, 124);
    header.write("00000000000\0", 136);
    header.write(type, 156);
    header.write("ustar\0", 257);
    header.write("00", 263);
    header.fill(" ", 148, 156);
    let sum = 0;
    for (const byte of header) sum += byte;
    header.write(`${sum.toString(8).padStart(6, "0")}\0 `, 148);
    const padded = Buffer.alloc(Math.ceil(body.length / 512) * 512);
    body.copy(padded);
    return Buffer.concat([header, padded]);
}

function archive(entries: Buffer[]): Buffer {
    return gzipSync(Buffer.concat([...entries, Buffer.alloc(1024)]));
}

const MEMBER = "ort-1.0/lib/libort.so.1";

function respond(bytes: Buffer): FetchLike {
    return async () => new Response(new Uint8Array(bytes));
}

describe("host inputs", () => {
    let tmp: string;
    let inputsDir: string;

    beforeEach(() => {
        tmp = mkdtempSync(join(tmpdir(), "eidnara-inputs-"));
        inputsDir = join(tmp, "cache");
    });

    afterEach(() => {
        rmSync(tmp, { recursive: true, force: true });
    });

    function downloaded(bytes: Buffer): LockedInput {
        return {
            key: "model_onnx",
            sha256: sha256(bytes),
            size_bytes: bytes.length,
            source: "https://example.invalid/model.onnx",
            payload_path: "payload/model/x/model.onnx",
        };
    }

    test("the committed lock names every staged input with an https or repo source and a unique payload path", () => {
        const inputs = lockedInputs(rootDir);
        expect(inputs.map((input) => input.key)).toEqual([
            "bundle_manifest",
            "config",
            "corpus",
            "model_onnx",
            "ort_runtime",
            "special_tokens_map",
            "tokenizer",
            "tokenizer_config",
        ]);
        const corpus = inputs.find((input) => input.key === "corpus");
        expect(corpus?.sha256).toBe("df864f8a8ab3c914b9be5dbd340f9a1f83061b75a0a735845babdeb9db5e266d");
        expect(corpus?.size_bytes).toBe(49056);
        const model = inputs.find((input) => input.key === "model_onnx");
        expect(model?.sha256).toBe("947f31df7effaeec4edb57c50e4ed7e0f2034d9336063f92615b92e3e0d24d78");
        expect(model?.size_bytes).toBe(596392315);
        const ort = inputs.find((input) => input.key === "ort_runtime");
        expect(ort?.archive).toEqual({
            member: "onnxruntime-linux-x64-1.24.2/lib/libonnxruntime.so.1.24.2",
            sha256: "43725474ba5663642e17684717946693850e2005efbd724ac72da278fead25e6",
            size_bytes: 8123282,
        });
        expect(ort?.sha256).toBe("ffc84d48e845cf0b562ba4ea5ca32aaafc0d4069019fef4f63095b307d0270ad");
        for (const input of inputs.filter((candidate) => candidate.source.startsWith("repo:"))) {
            const bytes = readFileSync(join(rootDir, input.source.slice("repo:".length)));
            expect({ key: input.key, size: bytes.length, sha256: sha256(bytes) }).toEqual({
                key: input.key,
                size: input.size_bytes,
                sha256: input.sha256,
            });
        }
    });

    test("a verified download is published under its digest", async () => {
        const bytes = Buffer.from("model bytes");
        await acquireInputs(rootDir, { inputsDir, inputs: [downloaded(bytes)], fetchImpl: respond(bytes) });
        expect(readFileSync(cachePath(inputsDir, sha256(bytes)))).toEqual(bytes);
        expect(readdirSync(inputsDir)).toEqual([sha256(bytes)]);
    });

    test("a truncated, oversized, or wrong-digest download publishes nothing", async () => {
        const bytes = Buffer.from("model bytes");
        const input = downloaded(bytes);
        for (const [name, sent] of [
            ["truncated", bytes.subarray(0, 5)],
            ["oversized", Buffer.concat([bytes, Buffer.from("!")])],
            ["wrong digest", Buffer.from("MODEL BYTES")],
        ] as const) {
            await expect(
                acquireInputs(rootDir, { inputsDir, inputs: [input], fetchImpl: respond(Buffer.from(sent)) }),
                name,
            ).rejects.toThrow(/host inputs/);
            expect(existsSync(inputsDir) ? readdirSync(inputsDir) : [], name).toEqual([]);
        }
    });

    test("a failed HTTP response publishes nothing", async () => {
        const input = downloaded(Buffer.from("model bytes"));
        await expect(
            acquireInputs(rootDir, {
                inputsDir,
                inputs: [input],
                fetchImpl: async () => new Response("missing", { status: 404 }),
            }),
        ).rejects.toThrow(/HTTP 404/);
        expect(readdirSync(inputsDir)).toEqual([]);
    });

    test("wrong same-name cache content is refused and replaced by verified bytes", async () => {
        const bytes = Buffer.from("model bytes");
        const input = downloaded(bytes);
        mkdirSync(inputsDir, { recursive: true });
        writeFileSync(cachePath(inputsDir, input.sha256), "MODEL BYTES");
        await expect(verifiedCacheEntry(inputsDir, input, "model")).rejects.toThrow(/does not match its locked sha256/);
        writeFileSync(cachePath(inputsDir, input.sha256), "short");
        await expect(verifiedCacheEntry(inputsDir, input, "model")).rejects.toThrow(/has 5 bytes/);
        await acquireInputs(rootDir, { inputsDir, inputs: [input], fetchImpl: respond(bytes) });
        expect(await verifiedCacheEntry(inputsDir, input, "model")).toBe(cachePath(inputsDir, input.sha256));
    });

    test("an archive input stages only its verified member", async () => {
        const member = Buffer.from("runtime library");
        const tgz = archive([
            tarEntry("ort-1.0/", Buffer.alloc(0), "5"),
            tarEntry("ort-1.0/lib/libort.so", Buffer.alloc(0), "2"),
            tarEntry(MEMBER, member),
            tarEntry("ort-1.0/LICENSE", Buffer.from("MIT")),
        ]);
        const input: LockedInput = {
            key: "ort_runtime",
            sha256: sha256(member),
            size_bytes: member.length,
            source: "https://example.invalid/ort.tgz",
            payload_path: "payload/ort/libort.so",
            archive: { member: MEMBER, sha256: sha256(tgz), size_bytes: tgz.length },
        };
        await acquireInputs(rootDir, { inputsDir, inputs: [input], fetchImpl: respond(tgz) });
        expect(readFileSync(cachePath(inputsDir, input.sha256))).toEqual(member);
        expect(readdirSync(inputsDir).sort()).toEqual([sha256(member), sha256(tgz)].sort());

        const forged = archive([tarEntry(MEMBER, Buffer.from("runtime LIBRARY"))]);
        const forgedInput = { ...input, archive: { member: MEMBER, sha256: sha256(forged), size_bytes: forged.length } };
        rmSync(inputsDir, { recursive: true, force: true });
        await expect(
            acquireInputs(rootDir, { inputsDir, inputs: [forgedInput], fetchImpl: respond(forged) }),
        ).rejects.toThrow(/do not match the locked sha256/);
        expect(existsSync(cachePath(inputsDir, input.sha256))).toBe(false);
    });

    test("archives with unexpected paths, types, or members are refused", () => {
        const member = Buffer.from("runtime library");
        const cases: [string, Buffer[], RegExp][] = [
            ["traversal", [tarEntry("ort-1.0/../evil", member), tarEntry(MEMBER, member)], /outside ort-1.0/],
            ["absolute", [tarEntry("/etc/passwd", member)], /outside ort-1.0/],
            ["foreign root", [tarEntry("other/lib/libort.so.1", member)], /outside ort-1.0/],
            ["hard link", [tarEntry("ort-1.0/link", Buffer.alloc(0), "1"), tarEntry(MEMBER, member)], /unexpected type/],
            ["long name", [tarEntry("././@LongLink", member, "L")], /outside ort-1.0|unexpected type/],
            ["symlink member", [tarEntry(MEMBER, Buffer.alloc(0), "2")], /not a regular file/],
            ["duplicate member", [tarEntry(MEMBER, member), tarEntry(MEMBER, member)], /appears twice/],
            ["missing member", [tarEntry("ort-1.0/LICENSE", member)], /has no member/],
        ];
        for (const [name, entries, error] of cases) {
            expect(() => extractArchiveMember(archive(entries), MEMBER), name).toThrow(error);
        }
        expect(() => extractArchiveMember(Buffer.from("not gzip"), MEMBER)).toThrow(/decompress/);
    });
});

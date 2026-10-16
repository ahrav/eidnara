import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

/**
 * Recomputes the digest `build.rs` stamps into the addon: FNV-1a (32-bit) over every listed
 * input, walked in sorted workspace-relative path order, hashing each path, a NUL, its bytes,
 * and a NUL. The input list is read from `build.rs` itself, so the two cannot drift.
 */
function sourceInputs(root: string): string[] {
    const script = readFileSync(join(root, "packages/shm-native/build.rs"), "utf8");
    const list = /const SOURCE_INPUTS: &\[&str\] = &\[([^\]]*)\];/.exec(script)?.[1];
    if (list === undefined) throw new Error("build.rs declares no SOURCE_INPUTS list");
    return [...list.matchAll(/"([^"]+)"/g)].map((match) => match[1] as string);
}

function collect(root: string, relative: string, files: string[]): void {
    const path = join(root, relative);
    const stat = statSync(path, { throwIfNoEntry: false });
    if (!stat) return;
    if (stat.isDirectory()) {
        for (const entry of readdirSync(path).sort()) collect(root, join(relative, entry), files);
    } else if (stat.isFile()) {
        files.push(relative);
    }
}

function fnv1a(hash: number, bytes: Uint8Array): number {
    let value = hash;
    for (const byte of bytes) {
        value ^= byte;
        value = Math.imul(value, 0x0100_0193) >>> 0;
    }
    return value;
}

export function checkoutSourceDigest(root: string): string {
    const files: string[] = [];
    for (const input of sourceInputs(root)) collect(root, input, files);
    // Matches Rust's component-wise `Path` ordering.
    files.sort(compareComponents);
    const nul = new Uint8Array([0]);
    let hash = 0x811c_9dc5;
    for (const file of files) {
        hash = fnv1a(hash, new TextEncoder().encode(file));
        hash = fnv1a(hash, nul);
        hash = fnv1a(hash, readFileSync(join(root, file)));
        hash = fnv1a(hash, nul);
    }
    return hash.toString(16).padStart(8, "0");
}

function compareComponents(left: string, right: string): number {
    const a = left.split("/");
    const b = right.split("/");
    for (let index = 0; index < Math.min(a.length, b.length); index++) {
        const x = a[index] as string;
        const y = b[index] as string;
        if (x !== y) return x < y ? -1 : 1;
    }
    return a.length - b.length;
}

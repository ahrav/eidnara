import { afterEach, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { canonicalManifest, completeRuntimeClosure } from "./complete-harness-runtime";

const temporary: string[] = [];
afterEach(() => { for (const root of temporary.splice(0)) rmSync(root, { recursive:true, force:true }); });

function fixture(source = "exports.answer = 42; console.log(exports.answer);\n") {
    const root = mkdtempSync(join(tmpdir(), "closure-runtime-"));
    temporary.push(root);
    const entry = "node_modules/@sample/client/dist/cjs/deep/main.js";
    const outer = "node_modules/@sample/client/package.json";
    const boundary = "node_modules/@sample/client/dist/cjs/package.json";
    const write = (path: string, text: string) => {
        mkdirSync(dirname(join(root,path)), { recursive:true });
        writeFileSync(join(root,path), text);
    };
    write(entry,source);
    write(outer,'{"type":"module"}\n');
    write(boundary,'{"type":"commonjs"}\n');
    const node = (path: string, kind: string) => {
        const bytes = readFileSync(join(root,path));
        return { path, source_path:path, source_root:"install", kind, mode:0o600, size_bytes:bytes.length, sha256:createHash("sha256").update(bytes).digest("hex"), dependencies:[] as Array<{ path:string; kind:string }> };
    };
    const entryNode = node(entry,"module");
    return { root, entry, entryNode, boundary, write, manifest:{ entrypoint:entry, nodes:[entryNode,node(outer,"data")] } };
}

function copyRuntime(root: string, nodes: Array<{ path:string }>): string {
    const copy = join(root,"copy");
    for (const node of nodes) {
        mkdirSync(dirname(join(copy,node.path)), { recursive:true });
        copyFileSync(join(root,node.path),join(copy,node.path));
    }
    return copy;
}

test("preserves nested package format in a copied runtime, not only the package root", () => {
    const f = fixture();
    const original = structuredClone(f.manifest);
    const result = completeRuntimeClosure(f.manifest,{ install:f.root });
    expect(result.added).toEqual([f.boundary]);
    expect(f.manifest).toEqual(original);
    const copy = copyRuntime(f.root,original.nodes);
    const broken = spawnSync("node",[join(copy,f.entry)],{ encoding:"utf8" });
    expect(broken.error).toBeUndefined();
    expect(broken.status).not.toBe(0);
    copyFileSync(join(f.root,f.boundary),join(copy,f.boundary));
    const fixed = spawnSync("node",[join(copy,f.entry)],{ encoding:"utf8" });
    expect(fixed.status).toBe(0);
    expect(fixed.stdout.trim()).toBe("42");
    const again = completeRuntimeClosure(result.manifest,{ install:f.root });
    expect(again.added).toEqual([]);
    expect(canonicalManifest(again.manifest)).toBe(canonicalManifest(result.manifest));
});

test("includes runtime data without copying unrelated examples", () => {
    const f = fixture("console.log(JSON.parse(require('node:fs').readFileSync(require('node:path').join(__dirname,'theme.json'),'utf8')).answer);\n");
    f.write("node_modules/@sample/client/dist/cjs/deep/theme.json",'{"answer":42}');
    f.write("node_modules/@sample/client/examples/unrelated.json","{}");
    const result = completeRuntimeClosure(f.manifest,{ install:f.root });
    expect(result.added).toContain("node_modules/@sample/client/dist/cjs/deep/theme.json");
    expect(result.added.some((path) => path.includes("examples/"))).toBe(false);
    const copy = copyRuntime(f.root,result.manifest.nodes);
    const run = spawnSync("node",[join(copy,f.entry)],{ encoding:"utf8" });
    expect(run.status).toBe(0);
    expect(run.stdout.trim()).toBe("42");
});

test("includes variable-specifier modules and their installed production dependencies", () => {
    const f = fixture("const load = name => import('./'+name+'.js'); load('later').then(module => console.log(module.answer));\n");
    f.write("node_modules/@sample/client/dist/cjs/deep/later.js","exports.answer = require('helper') + 1;\n");
    f.write("node_modules/@sample/client/node_modules/helper/package.json",'{"main":"index.js"}');
    f.write("node_modules/@sample/client/node_modules/helper/index.js","module.exports = 41;\n");
    const result = completeRuntimeClosure(f.manifest,{ install:f.root });
    const copy = copyRuntime(f.root,result.manifest.nodes);
    const run = spawnSync("node",[join(copy,f.entry)],{ encoding:"utf8" });
    expect(run.status).toBe(0);
    expect(run.stdout.trim()).toBe("42");
});

test("refuses changed existing pins and escaping module paths", () => {
    const f = fixture();
    const completed = completeRuntimeClosure(f.manifest,{ install:f.root });
    f.write(f.boundary,'{"type":"module"}\n');
    expect(() => completeRuntimeClosure(completed.manifest,{ install:f.root })).toThrow("Existing runtime pin differs");
    f.entryNode.source_path = "../outside.js";
    expect(() => completeRuntimeClosure(f.manifest,{ install:f.root })).toThrow("unchanged relative entrypoint path");
});

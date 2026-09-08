import { describe, expect, test } from "bun:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import * as https from "node:https";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { validateSmartNoteHttpUrl } from "./ssrf-guard";
import { selfSignedCertificate } from "./test-support/self-signed-certificate";

const here = path.dirname(fileURLToPath(import.meta.url));
const packageRoot = path.resolve(here, "../../../..");

/** The Node subprocess must satisfy `engines.node` so these tests exercise a supported runtime. */
async function supportedNodeVersion(): Promise<string> {
    const manifest = JSON.parse(await readFile(path.join(packageRoot, "package.json"), "utf8"));
    const range = manifest.engines?.node;
    expect(typeof range).toBe("string");
    const probe = Bun.spawn(["node", "-p", "process.versions.node"], {
        stdout: "pipe",
        stderr: "pipe",
    });
    const [version, stderr, exitCode] = await Promise.all([
        new Response(probe.stdout).text(),
        new Response(probe.stderr).text(),
        probe.exited,
    ]);
    expect(stderr).toBe("");
    expect(exitCode).toBe(0);
    const trimmed = version.trim();
    if (!Bun.semver.satisfies(trimmed, range)) {
        throw new Error(`ambient node ${trimmed} does not satisfy engines.node ${range}`);
    }
    return trimmed;
}

async function bundleGuard(dir: string): Promise<string> {
    const result = await Bun.build({
        entrypoints: [path.join(here, "ssrf-guard.ts")],
        outdir: dir,
        target: "node",
        format: "esm",
    });
    expect(result.success).toBe(true);
    const bundled = result.outputs[0]?.path;
    expect(typeof bundled).toBe("string");
    return bundled as string;
}

async function runScript(
    runtime: string[],
    script: string,
    env: Record<string, string> = {},
): Promise<unknown> {
    const proc = Bun.spawn([...runtime, script], {
        stdout: "pipe",
        stderr: "pipe",
        env: { ...process.env, ...env },
    });
    const [stdout, stderr, exitCode] = await Promise.all([
        new Response(proc.stdout).text(),
        new Response(proc.stderr).text(),
        proc.exited,
    ]);
    expect(stderr).toBe("");
    expect(exitCode).toBe(0);
    return JSON.parse(stdout);
}

async function listenHttps(dnsName: string): Promise<{
    cert: string;
    port: number;
    close: () => void;
}> {
    const { key, cert } = selfSignedCertificate(dnsName);
    const server = https.createServer({ key, cert }, (_request, response) => {
        response.writeHead(200, { "content-type": "text/plain" });
        response.end("ok");
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
    const { port } = server.address() as AddressInfo;
    return { cert, port, close: () => server.close() };
}

describe("smart-note SSRF guard runtime parity", () => {
    test("Bun and Node classify public/private DNS the same way", async () => {
        await supportedNodeVersion();
        const bunAllowed = await validateSmartNoteHttpUrl("https://example.test/", {
            signal: new AbortController().signal,
            resolver: { lookup: async () => [{ address: "93.184.216.34", family: 4 }] },
        }).then(
            () => true,
            () => false,
        );
        const bunBlocked = await validateSmartNoteHttpUrl("https://example.test/", {
            signal: new AbortController().signal,
            resolver: {
                lookup: async () => [
                    { address: "93.184.216.34", family: 4 },
                    { address: "10.0.0.1", family: 4 },
                ],
            },
        }).then(
            () => false,
            () => true,
        );

        const dir = await mkdtemp(path.join(tmpdir(), "eidnara-ssrf-parity-"));
        try {
            const bundledPath = await bundleGuard(dir);
            const script = path.join(dir, "check.mjs");
            await writeFile(
                script,
                `import { validateSmartNoteHttpUrl } from ${JSON.stringify(`file://${bundledPath}`)};
const signal = new AbortController().signal;
const allowed = await validateSmartNoteHttpUrl('https://example.test/', { signal, resolver: { lookup: async () => [{ address: '93.184.216.34', family: 4 }] } }).then(() => true, () => false);
const blocked = await validateSmartNoteHttpUrl('https://example.test/', { signal, resolver: { lookup: async () => [{ address: '93.184.216.34', family: 4 }, { address: '10.0.0.1', family: 4 }] } }).then(() => false, () => true);
console.log(JSON.stringify({ allowed, blocked }));
`,
                "utf8",
            );
            expect(await runScript(["node"], script)).toEqual({
                allowed: bunAllowed,
                blocked: bunBlocked,
            });
        } finally {
            await rm(dir, { recursive: true, force: true });
        }
    });

    // The client connects to the pinned loopback address while TLS verifies the URL hostname.
    test("Bun and Node verify TLS against the URL hostname, not the pinned address", async () => {
        await supportedNodeVersion();
        const matching = await listenHttps("example.test");
        const mismatched = await listenHttps("other.test");
        const dir = await mkdtemp(path.join(tmpdir(), "eidnara-ssrf-tls-"));
        try {
            const bundledPath = await bundleGuard(dir);
            const trust = path.join(dir, "trust.pem");
            await writeFile(trust, `${matching.cert}${mismatched.cert}`, "utf8");
            const script = path.join(dir, "tls.mjs");
            await writeFile(
                script,
                `import { requestValidatedAddress } from ${JSON.stringify(`file://${bundledPath}`)};
const attempt = (port) => requestValidatedAddress(
    { url: new URL('https://example.test:' + port + '/'), hostname: 'example.test', addresses: [] },
    { address: '127.0.0.1', family: 4, classification: 'global' },
    { signal: new AbortController().signal, timeoutMs: 5000, bodyLimitBytes: 4096 },
).then((result) => ({ ok: true, status: result.status, body: result.body }), () => ({ ok: false }));
console.log(JSON.stringify({ matching: await attempt(${matching.port}), mismatched: await attempt(${mismatched.port}) }));
`,
                "utf8",
            );
            const expected = {
                matching: { ok: true, status: 200, body: "ok" },
                mismatched: { ok: false },
            };
            const env = { NODE_EXTRA_CA_CERTS: trust };
            expect(await runScript([process.execPath], script, env)).toEqual(expected);
            expect(await runScript(["node"], script, env)).toEqual(expected);
        } finally {
            matching.close();
            mismatched.close();
            await rm(dir, { recursive: true, force: true });
        }
    }, 30_000);
});

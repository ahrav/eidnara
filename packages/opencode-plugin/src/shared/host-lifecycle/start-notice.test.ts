import { afterEach, describe, expect, it } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { createLazyManagedDemandStart } from "../../hooks/context/module-transport";
import { _resetProcessAwsSourceForTesting } from "../host-client/aws-source";
import {
    createStartRefusalAnnouncer,
    credentialProcessProfile,
    isPersistentStartRefusal,
    managedStartNotice,
} from "./start-notice";

const roots: string[] = [];
const root = (): string => {
    const dir = mkdtempSync(join(tmpdir(), "eidnara-start-notice-"));
    roots.push(dir);
    return dir;
};

afterEach(() => {
    for (const dir of roots.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function awsEnv(config: string, profile: string, credentials?: string): Record<string, string> {
    const dir = root();
    const file = join(dir, "config");
    writeFileSync(file, config);
    const env = { HOME: dir, AWS_PROFILE: profile, AWS_REGION: "us-west-2", AWS_CONFIG_FILE: file };
    if (credentials === undefined) return env;
    const shared = join(dir, "credentials");
    writeFileSync(shared, credentials);
    return { ...env, AWS_SHARED_CREDENTIALS_FILE: shared };
}

async function withProcessEnv<T>(
    overrides: Record<string, string | undefined>,
    body: () => Promise<T>,
): Promise<T> {
    const saved = Object.fromEntries(
        Object.keys(overrides).map((name) => [name, process.env[name]]),
    );
    const apply = (values: Record<string, string | undefined>): void => {
        for (const [name, value] of Object.entries(values)) {
            if (value === undefined) delete process.env[name];
            else process.env[name] = value;
        }
    };
    apply(overrides);
    _resetProcessAwsSourceForTesting();
    try {
        return await body();
    } finally {
        apply(saved);
        _resetProcessAwsSourceForTesting();
    }
}

describe("managed start notice", () => {
    it("names a selected profile whose section runs credential_process", () => {
        const config = [
            "[profile midway]",
            "credential_process = ada credentials print --profile=midway",
            "[profile sso]",
            "sso_session = corp",
        ].join("\n");
        expect(credentialProcessProfile(awsEnv(config, "midway"))).toBe("midway");
        expect(credentialProcessProfile(awsEnv(config, "sso"))).toBeUndefined();
        expect(
            credentialProcessProfile(awsEnv("[default]\ncredential_process=x\n", "default")),
        ).toBe("default");
        expect(credentialProcessProfile({ AWS_REGION: "us-west-2" })).toBeUndefined();
    });

    it("reads section headers with trailing comments as the AWS profile parser does", () => {
        const following = [
            "[profile dev]",
            "sso_session = corp",
            "[profile midway] # ada",
            "credential_process = ada credentials print",
        ].join("\n");
        expect(credentialProcessProfile(awsEnv(following, "dev"))).toBeUndefined();
        expect(credentialProcessProfile(awsEnv(following, "midway"))).toBe("midway");
        const selected = "[profile midway] ; corp\ncredential_process = ada\n";
        expect(credentialProcessProfile(awsEnv(selected, "midway"))).toBe("midway");
    });

    it("finds credential_process in the shared credentials file as native admission merges it", () => {
        const config = "[profile midway]\nregion = us-west-2\n";
        const credentials = "[midway]\ncredential_process = ada credentials print\n";
        expect(credentialProcessProfile(awsEnv(config, "midway", credentials))).toBe("midway");
        expect(
            credentialProcessProfile(
                awsEnv(config, "midway", "[default]\naws_access_key_id = k\n"),
            ),
        ).toBeUndefined();
        // A `[profile x]` header is a plain section named "profile x" in the credentials file.
        expect(
            credentialProcessProfile(
                awsEnv(config, "midway", "[profile midway]\ncredential_process = x\n"),
            ),
        ).toBeUndefined();
    });

    it("follows a role chain to the source profile that runs credential_process", () => {
        const role = "role_arn = arn:aws:iam::123456789012:role/dev";
        const chain = [
            "[profile dev]",
            role,
            "source_profile = midway",
            "[profile midway]",
            "credential_process = ada credentials print",
        ].join("\n");
        expect(credentialProcessProfile(awsEnv(chain, "dev"))).toBe("midway");
        const notice = managedStartNotice("harness_unavailable", null, awsEnv(chain, "dev"));
        expect(notice).toContain('AWS profile "midway", the source profile of "dev", supplies');
        const sourceInCredentials = awsEnv(
            `[profile dev]\n${role}\nsource_profile = midway\n`,
            "dev",
            "[midway]\ncredential_process = x\n",
        );
        expect(credentialProcessProfile(sourceInCredentials)).toBe("midway");
        // Traversal requires role_arn and an existing source section.
        const roleless =
            "[profile dev]\nsource_profile = midway\n[profile midway]\ncredential_process = x\n";
        expect(credentialProcessProfile(awsEnv(roleless, "dev"))).toBeUndefined();
        const missing = `[profile dev]\n${role}\nsource_profile = gone\n`;
        expect(credentialProcessProfile(awsEnv(missing, "dev"))).toBeUndefined();
        const cycle = `[profile a]\n${role}\nsource_profile = b\n[profile b]\n${role}\nsource_profile = a\n`;
        expect(credentialProcessProfile(awsEnv(cycle, "a"))).toBeUndefined();
        const self = `[profile a]\n${role}\nsource_profile = a\n`;
        expect(credentialProcessProfile(awsEnv(self, "a"))).toBeUndefined();
    });

    it("ignores a credential_process continuation line nested under another property", () => {
        const nested = "[profile dev]\nsso_session = corp\ns3 =\n  credential_process = x\n";
        expect(credentialProcessProfile(awsEnv(nested, "dev"))).toBeUndefined();
    });

    it("skips a config path that is not a bounded regular file without blocking", () => {
        const dir = root();
        const fifo = join(dir, "config");
        execFileSync("mkfifo", [fifo]);
        const env = {
            HOME: dir,
            AWS_PROFILE: "dev",
            AWS_REGION: "us-west-2",
            AWS_CONFIG_FILE: fifo,
        };
        expect(credentialProcessProfile(env)).toBeUndefined();
        const large = awsEnv(
            `[profile dev]\ncredential_process = x\n#${"x".repeat(256 * 1024)}\n`,
            "dev",
        );
        expect(credentialProcessProfile(large)).toBeUndefined();
    });

    it("explains a credential_process refusal and an install-layout refusal", () => {
        const env = awsEnv("[profile midway]\ncredential_process = ada\n", "midway");
        const harness = managedStartNotice("harness_unavailable", null, env);
        expect(harness).toContain("Eidnara is off");
        expect(harness).toContain(
            'AWS profile "midway" supplies credentials through credential_process',
        );
        expect(harness).not.toContain("static-key profiles");
        expect(harness).toContain(
            "role profiles, including a role whose source profile holds static keys",
        );
        expect(managedStartNotice("unsupported_install_layout", null, {})).toContain("npm install");
        expect(managedStartNotice("native_payload_missing", "reinstall_eidnara", {})).toContain(
            "Suggested fix: reinstall eidnara.",
        );
    });

    it("prefers the native harness remediation over the AWS profile hint", () => {
        const env = awsEnv("[profile midway]\ncredential_process = ada\n", "midway");
        const notice = managedStartNotice(
            "harness_unavailable",
            "restart_with_supported_harness",
            env,
        );
        expect(notice).toContain("Suggested fix: restart with supported harness.");
        expect(notice).not.toContain("credential_process");
        expect(managedStartNotice("harness_unavailable", null, env)).toContain(
            'AWS profile "midway" supplies credentials through credential_process',
        );
    });

    it("explains an inadmissible AWS source selection", () => {
        const notice = managedStartNotice("harness_unavailable", null, {
            HOME: "/home/u",
            AWS_PROFILE: "corp",
        });
        expect(notice).toContain(
            "The AWS source selection is not admissible (AWS_REGION: missing)",
        );
    });

    it("stays silent for a compatibility probe that missed its budget after a start", () => {
        expect(isPersistentStartRefusal("native_probe_unavailable")).toBe(false);
        expect(isPersistentStartRefusal("startup_timeout")).toBe(false);
        expect(isPersistentStartRefusal("lifecycle_busy")).toBe(false);
        expect(isPersistentStartRefusal("unsupported_install_layout")).toBe(true);
        expect(isPersistentStartRefusal("harness_unavailable")).toBe(true);
        const announcer = createStartRefusalAnnouncer(() => ({}));
        const seen: string[] = [];
        announcer.listen((notice) => seen.push(notice));
        announcer.refused("native_probe_unavailable", "run_daemon_restart");
        expect(seen).toEqual([]);
    });

    it("announces a timeout reason once it repeats", () => {
        const announcer = createStartRefusalAnnouncer(() => ({}));
        const seen: string[] = [];
        announcer.listen((notice) => seen.push(notice));
        announcer.refused("native_probe_unavailable", "run_daemon_restart");
        announcer.refused("native_probe_unavailable", "run_daemon_restart");
        expect(seen).toEqual([]);
        announcer.refused("native_probe_unavailable", "run_daemon_restart");
        expect(seen).toHaveLength(1);
        expect(seen[0]).toContain("did not start in 3 attempts (native_probe_unavailable)");
        expect(seen[0]).toContain("Suggested fix: run daemon restart.");
        announcer.refused("native_probe_unavailable", "run_daemon_restart");
        announcer.refused("startup_timeout", "inspect_daemon_process");
        announcer.refused("startup_timeout", "inspect_daemon_process");
        expect(seen).toHaveLength(1);
        announcer.refused("startup_timeout", "inspect_daemon_process");
        expect(seen).toHaveLength(2);
        expect(seen[1]).toContain("startup_timeout");
    });

    it("announces each persistent reason once and holds notices until a listener exists", () => {
        const announcer = createStartRefusalAnnouncer(() => ({}));
        announcer.refused("unsupported_install_layout", null);
        announcer.refused("starting", "wait_and_retry");
        const seen: string[] = [];
        announcer.listen((notice) => seen.push(notice));
        announcer.refused("unsupported_install_layout", null);
        announcer.refused("harness_unavailable", null);
        expect(seen).toHaveLength(2);
        expect(seen[0]).toContain("unsupported_install_layout");
        expect(seen[1]).toContain("harness_unavailable");
    });

    it("reports a refused managed start through onRefusal", async () => {
        const dir = root();
        const reasons: string[] = [];
        const saved = process.env.XDG_DATA_HOME;
        process.env.XDG_DATA_HOME = join(dir, "data");
        try {
            const start = createLazyManagedDemandStart({
                declaringModuleUrl: pathToFileURL(join(dir, "dist", "index.js")).href,
                parentPackageName: "@eidnara/opencode",
                onRefusal: (reason) => reasons.push(reason),
            });
            const outcome = await start({ origin: "managed-default", capability: "context" });
            expect(outcome.ok).toBe(false);
            expect(reasons).toEqual([outcome.reason]);
        } finally {
            if (saved === undefined) delete process.env.XDG_DATA_HOME;
            else process.env.XDG_DATA_HOME = saved;
        }
    });

    it("reports an inadmissible selector that refuses the startup envelope", async () => {
        const dir = root();
        const reasons: string[] = [];
        await withProcessEnv(
            {
                HOME: dir,
                AWS_PROFILE: "corp",
                AWS_REGION: undefined,
                XDG_DATA_HOME: join(dir, "data"),
            },
            async () => {
                const start = createLazyManagedDemandStart({
                    declaringModuleUrl: pathToFileURL(join(dir, "dist", "index.js")).href,
                    parentPackageName: "@eidnara/opencode",
                    onRefusal: (reason) => reasons.push(reason),
                });
                await expect(
                    start({ origin: "managed-default", capability: "context" }),
                ).rejects.toMatchObject({ code: "harness_unavailable" });
            },
        );
        expect(reasons).toEqual(["harness_unavailable"]);
    });
});

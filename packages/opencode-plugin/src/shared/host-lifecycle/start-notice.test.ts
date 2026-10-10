import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { createLazyManagedDemandStart } from "../../hooks/context/module-transport";
import {
    createStartRefusalAnnouncer,
    credentialProcessProfile,
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

function awsEnv(config: string, profile: string): Record<string, string> {
    const dir = root();
    const file = join(dir, "config");
    writeFileSync(file, config);
    return { HOME: dir, AWS_PROFILE: profile, AWS_REGION: "us-west-2", AWS_CONFIG_FILE: file };
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

    it("explains a credential_process refusal and an install-layout refusal", () => {
        const env = awsEnv("[profile midway]\ncredential_process = ada\n", "midway");
        const harness = managedStartNotice("harness_unavailable", null, env);
        expect(harness).toContain("Eidnara is off");
        expect(harness).toContain(
            'AWS profile "midway" supplies credentials through credential_process',
        );
        expect(managedStartNotice("unsupported_install_layout", null, {})).toContain("npm install");
        expect(managedStartNotice("native_payload_missing", "reinstall_eidnara", {})).toContain(
            "Suggested fix: reinstall eidnara.",
        );
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
});

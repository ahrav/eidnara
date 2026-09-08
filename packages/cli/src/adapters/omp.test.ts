import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { OmpAdapter } from "./omp";

const original = {
    HOME: process.env.HOME,
    PATH: process.env.PATH,
    PI_CODING_AGENT_DIR: process.env.PI_CODING_AGENT_DIR,
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
};
const roots: string[] = [];

afterEach(() => {
    for (const [key, value] of Object.entries(original)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe("OmpAdapter", () => {
    it("detects an enabled Eidnara plugin from omp plugin list", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-adapter-"));
        roots.push(root);
        const bin = join(root, "bin");
        mkdirSync(bin, { recursive: true });
        const omp = join(bin, "omp");
        writeFileSync(
            omp,
            `#!/bin/sh
if [ "$1 $2 $3" = "plugin list --json" ]; then
  printf '%s' '{"npm":[{"name":"@eidnara/pi","version":"0.33.0","enabled":true}],"marketplace":[]}'
fi
`,
            { mode: 0o755 },
        );
        process.env.PATH = bin;
        process.env.HOME = root;
        delete process.env.XDG_DATA_HOME;

        const adapter = new OmpAdapter();
        expect(adapter.isInstalled()).toBe(true);
        expect(adapter.hasPluginEntry()).toBe(true);
    }, 30_000);

    /**
     * The fake reports the plugin installed but disabled, accepts `plugin enable`,
     * then fails the verification `plugin list` and records every plugin command.
     */
    function makeUncertainEnableFake(options: { failDisable: boolean }): {
        root: string;
        commandLog: string;
    } {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-adapter-"));
        roots.push(root);
        const bin = join(root, "bin");
        mkdirSync(bin, { recursive: true });
        const commandLog = join(root, "commands.log");
        const listCount = join(root, "list-count");
        writeFileSync(
            join(bin, "omp"),
            `#!/bin/sh
if [ "$1 $2 $3" = "plugin list --json" ]; then
  if [ -f "${listCount}" ]; then
    echo "list failed" >&2
    exit 1
  fi
  : > "${listCount}"
  printf '%s' '{"npm":[{"name":"@eidnara/pi","version":"0.33.0","enabled":false}],"marketplace":[]}'
elif [ "$1" = "plugin" ]; then
  echo "$1 $2 $3" >> "${commandLog}"
  if [ "$2" = "disable" ] && ${options.failDisable ? "true" : "false"}; then
    echo "disable refused" >&2
    exit 1
  fi
fi
`,
            { mode: 0o755 },
        );
        process.env.PATH = bin;
        process.env.HOME = root;
        delete process.env.XDG_DATA_HOME;
        return { root, commandLog };
    }

    /** The fake reports no Eidnara plugin until `plugin install` runs, then reports it enabled. */
    function makeInstallFake(options: { installFails: boolean }): { commandLog: string } {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-adapter-"));
        roots.push(root);
        const bin = join(root, "bin");
        mkdirSync(bin, { recursive: true });
        const commandLog = join(root, "commands.log");
        const installed = join(root, "installed");
        writeFileSync(
            join(bin, "omp"),
            `#!/bin/sh
if [ "$1 $2 $3" = "plugin list --json" ]; then
  if [ -f "${installed}" ]; then
    printf '%s' '{"npm":[{"name":"@eidnara/pi","version":"0.33.0","enabled":true}],"marketplace":[]}'
  else
    printf '%s' '{"npm":[],"marketplace":[]}'
  fi
elif [ "$1" = "plugin" ]; then
  echo "$1 $2 $3" >> "${commandLog}"
  if [ "$2" = "install" ]; then
    if ${options.installFails ? "true" : "false"}; then echo "registry unreachable" >&2; exit 1; fi
    : > "${installed}"
  fi
fi
`,
            { mode: 0o755 },
        );
        process.env.PATH = bin;
        process.env.HOME = root;
        delete process.env.XDG_DATA_HOME;
        return { commandLog };
    }

    it("installs a missing plugin and reports it as added", async () => {
        const { commandLog } = makeInstallFake({ installFails: false });

        const result = await new OmpAdapter().ensurePluginEntry();

        expect(result.ok).toBe(true);
        expect(result.action).toBe("added");
        expect(readFileSync(commandLog, "utf-8").trim()).toBe("plugin install @eidnara/pi");
    });

    it("reports a failed install without running enable", async () => {
        const { commandLog } = makeInstallFake({ installFails: true });

        const result = await new OmpAdapter().ensurePluginEntry();

        expect(result.ok).toBe(false);
        expect(result.message).toContain("registry unreachable");
        expect(readFileSync(commandLog, "utf-8").trim()).toBe("plugin install @eidnara/pi");
    });

    it("restores the recorded prior state when enablement cannot be verified", async () => {
        const { root, commandLog } = makeUncertainEnableFake({ failDisable: false });
        const lockPath = new OmpAdapter().getConfigPaths().pluginConfigPath;
        mkdirSync(join(lockPath, ".."), { recursive: true });
        writeFileSync(lockPath, JSON.stringify({ plugins: { "@eidnara/pi": { enabled: false } } }));
        expect(lockPath.startsWith(root)).toBe(true);

        const result = await new OmpAdapter().ensurePluginEntry();

        expect(result.ok).toBe(false);
        expect(result.message).toContain("could not verify the plugin state");
        expect(result.message).toContain("restored the prior plugin state");
        expect(readFileSync(commandLog, "utf-8").trim().split("\n")).toEqual([
            "plugin enable @eidnara/pi",
            "plugin disable @eidnara/pi",
        ]);
    }, 30_000);

    it("reports a failed restore with the manual command", async () => {
        const { commandLog } = makeUncertainEnableFake({ failDisable: true });
        const lockPath = new OmpAdapter().getConfigPaths().pluginConfigPath;
        mkdirSync(join(lockPath, ".."), { recursive: true });
        writeFileSync(lockPath, JSON.stringify({ plugins: { "@eidnara/pi": { enabled: false } } }));

        const result = await new OmpAdapter().ensurePluginEntry();

        expect(result.ok).toBe(false);
        expect(result.message).toContain(
            "restoring the prior plugin state failed (disable refused)",
        );
        expect(result.message).toContain("Run `omp plugin disable @eidnara/pi` by hand");
        expect(readFileSync(commandLog, "utf-8")).toContain("plugin disable @eidnara/pi");
    }, 30_000);

    it("names the manual check when the prior state is unknown", async () => {
        const { commandLog } = makeUncertainEnableFake({ failDisable: false });

        const result = await new OmpAdapter().ensurePluginEntry();

        expect(result.ok).toBe(false);
        expect(result.message).toContain("prior enable state could not be read");
        expect(result.message).toContain(
            "run `omp plugin disable @eidnara/pi` if Eidnara must stay off",
        );
        expect(readFileSync(commandLog, "utf-8").trim()).toBe("plugin enable @eidnara/pi");
    }, 30_000);
});

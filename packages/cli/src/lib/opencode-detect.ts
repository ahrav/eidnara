import { existsSync, realpathSync } from "node:fs";
import os from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { findOnPath, isExecutableFile } from "./find-on-path";
export type OpenCodeInstallSource = "PATH" | "home-bin" | "desktop" | "app";
export interface OpenCodeInstallation {
    /** CLI installs execute `path`; all installations display `path`. */
    path: string;
    source: OpenCodeInstallSource;
    kind: "cli" | "desktop";
}

/**
 * `cli` requires a runnable `opencode` binary, including a stock installation, PATH entry, or version-manager/package-manager shim.
 * `desktop` means OpenCode Desktop is installed without a runnable CLI binary.
 *
 * `detectOpenCode` returns the highest-priority detected installation.
 */
export type OpenCodeDetection =
    | { kind: "cli"; binary: string }
    | { kind: "desktop"; marker: string }
    | { kind: "none" };

// `OPENCODE_DESKTOP_APP_IDS` lists Electron userData directory names for Desktop release channels.
// The settings file under these directories indicates that Desktop has run.
// GUI app paths indicate installation but not prior execution.
export const OPENCODE_DESKTOP_APP_IDS = [
    "ai.opencode.desktop",
    "ai.opencode.desktop.beta",
    "ai.opencode.desktop.dev",
] as const;

// Desktop writes `opencode.settings` in its electron-store userData directory.
const OPENCODE_DESKTOP_SETTINGS_FILE = "opencode.settings";

/**
 * Injectable dependencies let tests avoid the host filesystem and real `$HOME`.
 */
export interface DetectDeps {
    exists: (path: string) => boolean;
    isExecutable: (path: string) => boolean;
    home: string;
    platform: NodeJS.Platform;
    env: NodeJS.ProcessEnv;
    /** `onPath` searches PATH for a bare `opencode` binary. */
    onPath: (binary: string) => string | null;
    /** `addCandidate` uses `realpath` to collapse symlink aliases. */
    realpath?: (path: string) => string;
}

// `homedir()` throws for a UID without a passwd entry when `HOME` is unset; detection then
// simply finds no home-relative installation.
function safeHomeDir(): string {
    try {
        return os.homedir();
    } catch {
        return "";
    }
}

function defaultDeps(): DetectDeps {
    return {
        exists: existsSync,
        isExecutable: isExecutableFile,
        home: process.env.HOME?.trim() || safeHomeDir(),
        platform: process.platform,
        env: process.env,
        onPath: findOnPath,
    };
}
function stockCliBinary(d: DetectDeps): string {
    return d.platform === "win32"
        ? join(d.home, ".opencode", "bin", "opencode.exe")
        : join(d.home, ".opencode", "bin", "opencode");
}
function extraCliCandidates(d: DetectDeps): string[] {
    if (d.platform === "win32") {
        const appdata = d.env.APPDATA ?? "";
        const localappdata = d.env.LOCALAPPDATA ?? "";
        const userprofile = d.env.USERPROFILE ?? d.home;
        const out: string[] = [];
        if (appdata) {
            out.push(join(appdata, "npm", "opencode.cmd"));
            out.push(join(appdata, "npm", "opencode.exe"));
        }
        if (localappdata) {
            out.push(join(localappdata, "Microsoft", "WinGet", "Links", "opencode.exe"));
            out.push(join(localappdata, "opencode", "bin", "opencode.exe"));
        }
        if (userprofile) {
            out.push(join(userprofile, "scoop", "shims", "opencode.exe"));
        }
        return out;
    }
    return [
        "/usr/local/bin/opencode",
        "/opt/homebrew/bin/opencode",
        join(d.home, ".local", "bin", "opencode"),
        join(d.home, ".local", "share", "mise", "shims", "opencode"),
        join(d.home, ".asdf", "shims", "opencode"),
        join(d.home, ".volta", "bin", "opencode"),
    ];
}

function canonicalPath(d: DetectDeps, path: string): string {
    try {
        return d.realpath ? d.realpath(path) : realpathSync(path);
    } catch {
        return path;
    }
}

/** `addCandidate` uses each candidate's real path to collapse symlink aliases. */
function addCandidate(
    installations: OpenCodeInstallation[],
    seenRealpaths: Set<string>,
    d: DetectDeps,
    candidate: string,
    source: OpenCodeInstallSource,
    kind: OpenCodeInstallation["kind"],
): void {
    const path = canonicalPath(d, candidate);
    if (seenRealpaths.has(path)) return;
    seenRealpaths.add(path);
    installations.push({ path, source, kind });
}

/** Linux Desktop userData uses the XDG config base. */
function xdgConfigHome(d: DetectDeps): string {
    const xdg = d.env.XDG_CONFIG_HOME;
    if (xdg && xdg.length > 0) return xdg;
    return join(d.home, ".config");
}
function desktopUserDataDir(d: DetectDeps, appId: string): string {
    switch (d.platform) {
        case "darwin":
            return join(d.home, "Library", "Application Support", appId);
        case "win32":
            return join(d.env.APPDATA ?? join(d.home, "AppData", "Roaming"), appId);
        default:
            return join(xdgConfigHome(d), appId);
    }
}
function desktopAppPaths(d: DetectDeps): string[] {
    switch (d.platform) {
        case "darwin":
            return ["/Applications/OpenCode.app", join(d.home, "Applications", "OpenCode.app")];
        case "win32": {
            const localappdata = d.env.LOCALAPPDATA ?? join(d.home, "AppData", "Local");
            return [join(localappdata, "Programs", "OpenCode", "OpenCode.exe")];
        }
        default: {
            const dataHome =
                d.env.XDG_DATA_HOME && d.env.XDG_DATA_HOME.length > 0
                    ? d.env.XDG_DATA_HOME
                    : join(d.home, ".local", "share");
            return OPENCODE_DESKTOP_APP_IDS.map((appId) =>
                join(dataHome, "applications", `${appId}.desktop`),
            );
        }
    }
}
export function openCodeDesktopSettingsMarkers(deps?: Partial<DetectDeps>): string[] {
    const d = { ...defaultDeps(), ...deps };
    return OPENCODE_DESKTOP_APP_IDS.map((appId) =>
        join(desktopUserDataDir(d, appId), OPENCODE_DESKTOP_SETTINGS_FILE),
    );
}

export function detectOpenCodeInstallations(deps?: Partial<DetectDeps>): OpenCodeInstallation[] {
    const d = { ...defaultDeps(), ...deps };
    const installations: OpenCodeInstallation[] = [];
    const seenRealpaths = new Set<string>();
    // With no home directory the home-derived candidates collapse to cwd-relative paths such as
    // `.opencode/bin/opencode`; probing (and later executing) those would trust the project checkout.
    const probeExecutable = (path: string) => isAbsolute(path) && d.isExecutable(path);
    const probeExists = (path: string) => isAbsolute(path) && d.exists(path);

    // PATH is first because shells resolve it when multiple installations exist. A relative PATH
    // entry yields a relative hit that the shell would run from the current directory, so it is
    // resolved before the absolute-path gate.
    const onPath = d.onPath("opencode");
    const onPathResolved = onPath ? resolve(onPath) : null;
    if (onPathResolved && probeExecutable(onPathResolved)) {
        addCandidate(installations, seenRealpaths, d, onPathResolved, "PATH", "cli");
    }

    const stockBin = stockCliBinary(d);
    if (probeExecutable(stockBin)) {
        addCandidate(installations, seenRealpaths, d, stockBin, "home-bin", "cli");
    }

    for (const candidate of extraCliCandidates(d)) {
        if (probeExecutable(candidate)) {
            addCandidate(installations, seenRealpaths, d, candidate, "PATH", "cli");
        }
    }

    for (const appId of OPENCODE_DESKTOP_APP_IDS) {
        const marker = join(desktopUserDataDir(d, appId), OPENCODE_DESKTOP_SETTINGS_FILE);
        if (probeExists(marker)) {
            addCandidate(installations, seenRealpaths, d, marker, "desktop", "desktop");
        }
    }
    for (const appPath of desktopAppPaths(d)) {
        if (probeExists(appPath)) {
            addCandidate(installations, seenRealpaths, d, appPath, "app", "desktop");
        }
    }

    return installations;
}

/**
 */
export function detectOpenCode(deps?: Partial<DetectDeps>): OpenCodeDetection {
    const active = detectOpenCodeInstallations(deps)[0];
    if (!active) return { kind: "none" };
    return active.kind === "cli"
        ? { kind: "cli", binary: active.path }
        : { kind: "desktop", marker: active.path };
}

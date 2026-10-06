import { STATIC_AWS_CREDENTIAL_NAMES } from "./credential-fingerprint";

const AWS_PROFILE_MAX_BYTES = 256;
const AWS_REGION_MAX_BYTES = 64;
const AWS_PATH_MAX_BYTES = 4096;
const AWS_SSO_CACHE_SUFFIX = "/.aws/sso/cache";

const PROFILE_SOURCE_FIELDS = [
    "kind",
    "profile",
    "region",
    "config_file",
    "credentials_file",
    "sso_cache_root",
] as const;

export interface AwsProfileSource {
    readonly kind: "profile";
    readonly profile: string;
    readonly region: string;
    readonly config_file: string;
    readonly credentials_file: string;
    readonly sso_cache_root: string;
}

export type AwsSourceSelection =
    | { readonly mode: "environment" }
    | { readonly mode: "profile"; readonly source: AwsProfileSource };

export type AwsSourceErrorCode =
    | "missing"
    | "not_an_object"
    | "unknown_field"
    | "invalid_type"
    | "empty"
    | "too_long"
    | "contains_nul"
    | "not_utf8"
    | "path_not_normal"
    | "sso_cache_root"
    | "home_not_absolute";

export class AwsSourceError extends Error {
    constructor(
        readonly code: AwsSourceErrorCode,
        readonly field: string,
    ) {
        super(`aws source ${field}: ${code}`);
        this.name = "AwsSourceError";
    }
}

function lexicallyNormal(path: string): string | null {
    if (!path.startsWith("/")) return null;
    const parts: string[] = [];
    for (const part of path.slice(1).split("/")) {
        if (part === "" || part === ".") continue;
        if (part === "..") parts.pop();
        else parts.push(part);
    }
    return `/${parts.join("/")}`;
}

function bounded(field: string, value: unknown, maxBytes: number): string {
    if (value === undefined) throw new AwsSourceError("missing", field);
    if (typeof value !== "string") throw new AwsSourceError("invalid_type", field);
    if (value.length === 0) throw new AwsSourceError("empty", field);
    if (!(value as string & { isWellFormed(): boolean }).isWellFormed()) {
        throw new AwsSourceError("not_utf8", field);
    }
    if (Buffer.byteLength(value) > maxBytes) throw new AwsSourceError("too_long", field);
    if (value.includes("\0")) throw new AwsSourceError("contains_nul", field);
    return value;
}

function normalPath(field: string, value: unknown): string {
    const path = bounded(field, value, AWS_PATH_MAX_BYTES);
    if (lexicallyNormal(path) !== path) throw new AwsSourceError("path_not_normal", field);
    return path;
}

export function parseAwsProfileSource(value: unknown): AwsProfileSource {
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
        throw new AwsSourceError("not_an_object", "aws_source");
    }
    const record = value as Record<string, unknown>;
    for (const key of Object.keys(record)) {
        if (!(PROFILE_SOURCE_FIELDS as readonly string[]).includes(key)) {
            throw new AwsSourceError("unknown_field", key);
        }
    }
    if (record.kind === undefined) throw new AwsSourceError("missing", "kind");
    if (record.kind !== "profile") throw new AwsSourceError("invalid_type", "kind");
    const source: AwsProfileSource = {
        kind: "profile",
        profile: bounded("profile", record.profile, AWS_PROFILE_MAX_BYTES),
        region: bounded("region", record.region, AWS_REGION_MAX_BYTES),
        config_file: normalPath("config_file", record.config_file),
        credentials_file: normalPath("credentials_file", record.credentials_file),
        sso_cache_root: normalPath("sso_cache_root", record.sso_cache_root),
    };
    if (!source.sso_cache_root.endsWith(AWS_SSO_CACHE_SUFFIX)) {
        throw new AwsSourceError("sso_cache_root", "sso_cache_root");
    }
    return Object.freeze(source);
}

function overrideOrDefault(
    env: Readonly<Record<string, string | undefined>>,
    name: string,
    fallback: string,
): string {
    const value = env[name];
    if (value === undefined) return fallback;
    const path = bounded(name, value, AWS_PATH_MAX_BYTES);
    const normal = lexicallyNormal(path);
    if (normal === null) throw new AwsSourceError("path_not_normal", name);
    return normal;
}

export function selectAwsSource(
    env: Readonly<Record<string, string | undefined>>,
): AwsSourceSelection {
    const profile = env.AWS_PROFILE;
    if (profile === undefined) return ENVIRONMENT;
    const homeValue = bounded("HOME", env.HOME, AWS_PATH_MAX_BYTES);
    const home = lexicallyNormal(homeValue);
    if (home === null) throw new AwsSourceError("home_not_absolute", "HOME");
    const base = home === "/" ? "" : home;
    return {
        mode: "profile",
        source: parseAwsProfileSource({
            kind: "profile",
            profile: bounded("AWS_PROFILE", profile, AWS_PROFILE_MAX_BYTES),
            region: bounded("AWS_REGION", env.AWS_REGION, AWS_REGION_MAX_BYTES),
            config_file: overrideOrDefault(env, "AWS_CONFIG_FILE", `${base}/.aws/config`),
            credentials_file: overrideOrDefault(
                env,
                "AWS_SHARED_CREDENTIALS_FILE",
                `${base}/.aws/credentials`,
            ),
            sso_cache_root: `${base}${AWS_SSO_CACHE_SUFFIX}`,
        }),
    };
}

const ENVIRONMENT: AwsSourceSelection = Object.freeze({ mode: "environment" });
const captured = new Map<string, AwsSourceSelection>();

export function captureAwsSource(
    env: Readonly<Record<string, string | undefined>>,
): AwsSourceSelection {
    const selection = selectAwsSource(env);
    if (selection.mode === "environment") return ENVIRONMENT;
    const key = JSON.stringify(PROFILE_SOURCE_FIELDS.map((field) => selection.source[field]));
    const existing = captured.get(key);
    if (existing !== undefined) return existing;
    const frozen = Object.freeze(selection);
    captured.set(key, frozen);
    return frozen;
}

let processCapture: { selection: AwsSourceSelection } | { error: unknown } | undefined;

export function processAwsSource(): AwsSourceSelection {
    if (processCapture === undefined) {
        try {
            processCapture = { selection: captureAwsSource(process.env) };
        } catch (error) {
            processCapture = { error };
        }
    }
    if ("error" in processCapture) throw processCapture.error;
    return processCapture.selection;
}

export function sourceBinding(
    selection: AwsSourceSelection,
    credentials: Readonly<Record<string, string>>,
): { credentials?: Record<string, string>; aws_source?: AwsProfileSource } {
    const kept = Object.fromEntries(
        Object.entries(credentials).filter(
            ([name]) =>
                selection.mode === "environment" || !STATIC_AWS_CREDENTIAL_NAMES.includes(name),
        ),
    );
    return {
        ...(Object.keys(kept).length === 0 ? {} : { credentials: kept }),
        ...(selection.mode === "profile" ? { aws_source: selection.source } : {}),
    };
}

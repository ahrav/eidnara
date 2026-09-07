export function getErrorMessage(error: unknown): string {
    if (!(error instanceof Error)) return safeString(error);
    const message = readField(error, "message");
    return typeof message === "string" ? message : safeString(message ?? error);
}

/**
 *
 * `getErrorMessage` omits `Error.name` when `Error.message` is empty.
 *
 * Captures:
 *
 */
export interface ErrorDescription {
    name: string;
    message: string;
    status?: string;
    code?: string;
    causeName?: string;
    stackHead?: string;
    stringForm: string;
    /* */
    brief: string;
}

/** A throwing getter on a proxied or third-party error yields `undefined` instead of propagating. */
function readField(target: object, key: string): unknown {
    try {
        return (target as Record<string, unknown>)[key];
    } catch {
        return undefined;
    }
}

function readConstructorName(target: object): string | undefined {
    const ctor = readField(target, "constructor");
    if (typeof ctor !== "function" && (typeof ctor !== "object" || ctor === null)) return undefined;
    return readString(readField(ctor, "name"));
}

function readString(value: unknown): string | undefined {
    if (typeof value === "string" && value.length > 0) return value;
    if (typeof value === "number") return String(value);
    return undefined;
}

function clip(value: string, max: number): string {
    if (value.length <= max) return value;
    return `${value.slice(0, max)}…`;
}

export function describeError(error: unknown): ErrorDescription {
    const stringForm = clip(safeString(error), 400);

    if (!(error instanceof Error) && !(error && typeof error === "object")) {
        return {
            name: typeof error,
            message: "",
            stringForm,
            brief: stringForm || "<empty>",
        };
    }

    const obj = error as object;
    const name = readString(readField(obj, "name")) ?? readConstructorName(obj) ?? "Error";

    const message = readString(readField(obj, "message")) ?? "";
    const status = readString(readField(obj, "status")) ?? readString(readField(obj, "statusCode"));
    const code = readString(readField(obj, "code"));

    let causeName: string | undefined;
    const cause = readField(obj, "cause");
    if (cause && typeof cause === "object") {
        causeName = readString(readField(cause, "name")) ?? readConstructorName(cause);
    }

    const stack = readString(readField(obj, "stack"));
    const stackHead = stack
        ? stack
              .split("\n")
              .slice(0, 4)
              .map((l) => l.trim())
              .filter((l) => l.length > 0)
              .join(" | ")
        : undefined;

    const briefParts: string[] = [];
    if (name) briefParts.push(name);
    if (message) briefParts.push(`message="${clip(message, 200)}"`);
    if (status) briefParts.push(`status=${status}`);
    if (code) briefParts.push(`code=${code}`);
    if (causeName) briefParts.push(`cause=${causeName}`);
    if (!message && stringForm && stringForm !== name) {
        briefParts.push(`str="${clip(stringForm, 200)}"`);
    }
    const brief = briefParts.join(" ") || stringForm || name;

    return {
        name,
        message,
        ...(status ? { status } : {}),
        ...(code ? { code } : {}),
        ...(causeName ? { causeName } : {}),
        ...(stackHead ? { stackHead } : {}),
        stringForm,
        brief,
    };
}

function safeString(value: unknown): string {
    try {
        return String(value);
    } catch {
        return "<unstringifiable>";
    }
}

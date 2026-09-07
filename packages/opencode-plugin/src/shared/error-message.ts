/** If the brand check or a field read throws, fall back to the value's string form. */
export function getErrorMessage(error: unknown): string {
    try {
        if (!(error instanceof Error)) return safeString(error);
        const message = readField(error, "message");
        return typeof message === "string" ? message : safeString(message ?? error);
    } catch {
        return safeString(error);
    }
}

export interface ErrorDescription {
    /** `error.name`, else `error.constructor.name`, else `"Error"`. */
    name: string;
    /** `error.message` when it is a string, else `""`. */
    message: string;
    /** `error.status`, else `error.statusCode`, as a string. */
    status?: string;
    code?: string;
    /** `error.cause.name`, else `error.cause.constructor.name`. */
    causeName?: string;
    /** Contains the first four non-empty lines of `error.stack`, joined by `" | "`. */
    stackHead?: string;
    /** `String(error)` clipped to 400 characters, or `"<unstringifiable>"`. */
    stringForm: string;
    /**
     * One-line summary: `name`, then the non-empty `message`, `status`, `code`, and `cause` fields.
     * An empty `message` is replaced by `str="…"` holding `stringForm` unless it equals `name`.
     * Text components are clipped to 200 characters.
     */
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

/** If error classification throws, return a description from `stringForm` alone. */
export function describeError(error: unknown): ErrorDescription {
    const stringForm = clip(safeString(error), 400);
    try {
        return describeErrorValue(error, stringForm);
    } catch {
        return { name: "Error", message: "", stringForm, brief: stringForm || "<empty>" };
    }
}

function describeErrorValue(error: unknown, stringForm: string): ErrorDescription {
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

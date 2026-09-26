import { createHash } from "node:crypto";
import { types } from "node:util";

const TRANSFORM_CAPTURE_MAX_PASSES = 64;
const TRANSFORM_CAPTURE_MAX_BYTES = 64 * 1024 * 1024;
const MAX_REFERENCEABLE_DEPTH = 256;
/** Root arrays, the root tape, and the capture record itself. */
const ROOT_CAPTURE_BYTES = 256;
/** One retained tape slot; a string field also charges its UTF-16 units in case the host drops it. */
const TAPE_SLOT_BYTES = 8;
/** Members and snapshots arrays hold one slot each per root message. */
const ROOT_MEMBER_BYTES = 16;
/** Widest JSON scalar token: an f64 rendered with full precision and exponent. */
const SCALAR_WIRE_BYTES = 24;

type SnapshotField = string | number | boolean | symbol | null;
const ARRAY = Symbol("array");
const END_ARRAY = Symbol("end_array");
const OBJECT = Symbol("object");
const END_OBJECT = Symbol("end_object");
const EXTRA_KEY = Symbol("extra_key");
const ARRAY_METHODS = new Set(Object.getOwnPropertyNames(Array.prototype));
ARRAY_METHODS.delete("length");
ARRAY_METHODS.add("then");
const ARRAY_SYMBOLS = Object.getOwnPropertySymbols(Array.prototype).concat(
    Symbol.isConcatSpreadable,
);

export interface MessageContentSnapshot {
    fields: SnapshotField[];
}

/**
 * A SHA-256 chain over the tapes of a run of leading messages, so a later pass can recognize the
 * run unchanged without retaining its strings. Symbols other than the tape markers are kept for
 * identity.
 */
export interface HistoryDigest {
    readonly count: number;
    readonly digest: string;
    readonly symbols: readonly symbol[];
    /** The walks' total spend on the run; walks over identical content spend exactly this. */
    readonly bytes: number;
}

export interface ReferenceableRejection {
    reason:
        | "proxy"
        | "accessor"
        | "prototype"
        | "prototype_accessor"
        | "boxed_primitive"
        | "cycle"
        | "depth"
        | "sparse_array"
        | "undefined_element"
        | "nonfinite_number"
        | "bigint"
        | "function"
        | "symbol"
        | "undefined"
        | "not_array"
        | "to_json"
        | "extra_property";
    path: string;
}

/** A bounded walk or reservation failed; the transform owner treats this as a local decline. */
export class CaptureBudgetExceeded extends Error {
    constructor(detail: string) {
        super(`transform capture byte budget exceeded: ${detail}`);
    }
}

// Object.defineProperty bypasses inherited numeric setters on membership and output arrays.
function defineSlot<T>(array: T[], index: number, value: T): void {
    Object.defineProperty(array, index, {
        value,
        writable: true,
        enumerable: true,
        configurable: true,
    });
}

/** Own data property read that cannot run an accessor or proxy trap. */
export function readOwnDataProperty(value: unknown, key: string): unknown {
    if (value === null || typeof value !== "object" || types.isProxy(value)) return undefined;
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    return descriptor && Object.hasOwn(descriptor, "value") ? descriptor.value : undefined;
}

// Source values are arrays, plain objects, strings, numbers and booleans; a read that misses an
// own property, or any method call on a primitive, resolves through one of these prototypes.
const BUILTIN_PROTOTYPES: readonly (readonly [string, object])[] = [
    ["Array", Array.prototype],
    ["Object", Object.prototype],
    ["String", String.prototype],
    ["Number", Number.prototype],
    ["Boolean", Boolean.prototype],
];

/**
 * The root array is the one value that crosses back to the host as a return value, so a `then`
 * anywhere on its chain would be assimilated by the host's promise machinery; `in` observes an
 * inherited accessor without invoking it. Reads of absent optional fields and out-of-range indexes
 * fall through to the built-in prototypes, so an accessor installed there is refused the same way.
 */
export function rootArrayRejection(value: unknown): ReferenceableRejection | undefined {
    if (types.isProxy(value)) return { reason: "proxy", path: "" };
    if (!Array.isArray(value)) return { reason: "not_array", path: "" };
    if (
        Object.getPrototypeOf(value) !== Array.prototype ||
        Object.getPrototypeOf(Array.prototype) !== Object.prototype ||
        Object.getPrototypeOf(Object.prototype) !== null
    )
        return { reason: "prototype", path: "" };
    if ("then" in value) return { reason: "extra_property", path: "/then" };
    // Indexed loops: `for...of` would read `Array.prototype[Symbol.iterator]` before it is inspected.
    for (let p = 0; p < BUILTIN_PROTOTYPES.length; p += 1) {
        const name = BUILTIN_PROTOTYPES[p]![0];
        const prototype = BUILTIN_PROTOTYPES[p]![1];
        const keys = Reflect.ownKeys(prototype);
        for (let k = 0; k < keys.length; k += 1) {
            const key = keys[k]!;
            const slot = Object.getOwnPropertyDescriptor(prototype, key);
            if (key !== "__proto__" && slot && !Object.hasOwn(slot, "value"))
                return { reason: "prototype_accessor", path: `${name}.prototype/${String(key)}` };
        }
    }
    return undefined;
}

class SourceRejected extends Error {
    constructor(
        readonly reason: ReferenceableRejection["reason"],
        readonly path: string,
    ) {
        super(`unsupported source: ${reason} at ${path}`);
    }
}
const CONTENT_CHANGED = Symbol("content_changed");

/**
 * The charge covers retained tape slots and the strings they can keep alive, not engine
 * enumeration, transient descriptors, or RSS. Own-name inspection includes hidden accessors; a
 * field-name allowlist would drift from consumers.
 */
class ReferenceableWalk {
    bytes = 0;
    /** Upper bound on the canonical JSON length of the walked values, in two-byte units for strings. */
    wireBytes = 0;
    private field?: (value: SnapshotField) => void;
    private readonly ancestors = new Set<object>();

    /** Only inspection reads `wireBytes`; capture and recheck walks skip the per-unit escape scan. */
    constructor(
        private readonly maxBytes = TRANSFORM_CAPTURE_MAX_BYTES,
        private readonly estimateWire = true,
    ) {}

    spend(bytes: number): void {
        if (!Number.isSafeInteger(bytes) || bytes < 0 || bytes > this.maxBytes - this.bytes) {
            throw new CaptureBudgetExceeded("source and snapshot walk");
        }
        this.bytes += bytes;
    }

    /** `wireBytes` counts only tokens JSON serialization emits; tape-only markers pass zero. */
    private emit(value: SnapshotField, wireBytes: number): void {
        // A tape slot keeps its string, or a symbol's description, alive after the host drops it.
        const retained =
            typeof value === "string"
                ? value.length * 2
                : typeof value === "symbol"
                  ? (value.description?.length ?? 0) * 2
                  : 0;
        this.spend(TAPE_SLOT_BYTES + retained);
        if (typeof value === "string" && wireBytes > 0 && this.estimateWire) {
            for (let index = 0; index < value.length; index += 1) {
                const unit = value.charCodeAt(index);
                if (
                    unit === 0x22 ||
                    unit === 0x5c ||
                    (unit >= 0x08 && unit <= 0x0d && unit !== 0x0b)
                ) {
                    wireBytes += 2;
                } else if (unit < 0x20) {
                    wireBytes += 10;
                } else if (unit >= 0xd800 && unit <= 0xdbff) {
                    const next = value.charCodeAt(index + 1);
                    if (next >= 0xdc00 && next <= 0xdfff) index += 1;
                    else wireBytes += 10;
                } else if (unit >= 0xdc00 && unit <= 0xdfff) {
                    wireBytes += 10;
                }
            }
        }
        this.wire(wireBytes);
        this.field?.(value);
    }

    private wire(bytes: number): void {
        const total = this.wireBytes + bytes;
        if (!Number.isSafeInteger(total)) throw new CaptureBudgetExceeded("wire estimate");
        this.wireBytes = total;
    }

    walk(value: unknown, path = ""): void {
        const type = typeof value;
        if (value === null || type !== "object") {
            if (type === "number" && !Number.isFinite(value))
                throw new SourceRejected("nonfinite_number", path);
            if (value !== null && type !== "string" && type !== "number" && type !== "boolean")
                throw new SourceRejected(type as ReferenceableRejection["reason"], path);
            this.emit(
                value as SnapshotField,
                type === "string" ? (value as string).length * 2 + 4 : SCALAR_WIRE_BYTES,
            );
            return;
        }
        this.entries(value as object, path, (key, slot) => this.walk(slot.value, `${path}/${key}`));
    }

    members(messages: unknown, visit: (slot: PropertyDescriptor, index: number) => void): number {
        const rejection = rootArrayRejection(messages);
        if (rejection) throw new SourceRejected(rejection.reason, rejection.path);
        this.spend(ROOT_CAPTURE_BYTES);
        return this.entries(messages as unknown[], "", (key, slot) => {
            this.spend(ROOT_MEMBER_BYTES);
            visit(slot, Number(key));
        });
    }

    /** Hands each tape field to `sink` instead of recording it. */
    stream(operation: () => void, sink: (value: SnapshotField) => void): void {
        const outer = this.field;
        this.field = sink;
        try {
            operation();
        } finally {
            this.field = outer;
        }
    }

    recordOrCompare(
        operation: () => void,
        expected?: MessageContentSnapshot,
    ): MessageContentSnapshot {
        // Private tapes have no inherited setters. Returned tapes regain the ordinary array API.
        const fields: SnapshotField[] = expected?.fields ?? Object.setPrototypeOf([], null);
        let index = 0;
        const outer = this.field;
        this.field = (value) => {
            if (expected) {
                if (index >= fields.length || !Object.is(value, fields[index]))
                    throw CONTENT_CHANGED;
            } else fields[index] = value;
            index += 1;
        };
        try {
            operation();
            if (expected && index !== fields.length) throw CONTENT_CHANGED;
            return expected ?? { fields: Object.setPrototypeOf(fields, Array.prototype) };
        } finally {
            this.field = outer;
        }
    }

    private attributes(slot: PropertyDescriptor): void {
        this.emit(slot.enumerable === true, 0);
        this.emit(slot.writable === true, 0);
        this.emit(slot.configurable === true, 0);
    }

    /** Every descriptor read spends one slot so the byte budget also bounds traversal work. */
    private data(value: object, key: PropertyKey, path: string): PropertyDescriptor {
        this.spend(TAPE_SLOT_BYTES);
        const slot = Object.getOwnPropertyDescriptor(value, key);
        if (!slot) throw new SourceRejected("sparse_array", path);
        if (!Object.hasOwn(slot, "value")) throw new SourceRejected("accessor", path);
        return slot;
    }

    /** Proxy rejection precedes prototype and descriptor reads; source iterators are never used. */
    private entries(
        value: object,
        path: string,
        visit: (key: string, slot: PropertyDescriptor) => void,
    ): number {
        if (types.isProxy(value)) throw new SourceRejected("proxy", path);
        // JSON serializes a boxed primitive by its internal slot, which no own property records.
        if (types.isBoxedPrimitive(value)) throw new SourceRejected("boxed_primitive", path);
        const array = Array.isArray(value);
        const prototype = Object.getPrototypeOf(value);
        if (
            (array
                ? prototype !== Array.prototype
                : prototype !== Object.prototype && prototype !== null) ||
            Object.getPrototypeOf(Array.prototype) !== Object.prototype
        )
            throw new SourceRejected("prototype", path);
        if (this.ancestors.size > MAX_REFERENCEABLE_DEPTH) throw new SourceRejected("depth", path);
        if (this.ancestors.has(value)) throw new SourceRejected("cycle", path);
        // JSON looks up toJSON even when it is hidden or inherited.
        for (
            let holder: object | null = value;
            holder !== null;
            holder = Object.getPrototypeOf(holder)
        ) {
            const hook = Object.getOwnPropertyDescriptor(holder, "toJSON");
            if (hook && (!Object.hasOwn(hook, "value") || hook.value !== undefined)) {
                const reason = !Object.hasOwn(hook, "value")
                    ? "accessor"
                    : types.isProxy(hook.value)
                      ? "proxy"
                      : typeof hook.value === "function"
                        ? "function"
                        : "to_json";
                throw new SourceRejected(reason, `${path}/toJSON`);
            }
        }
        const lengthSlot = array ? this.data(value, "length", path) : undefined;
        const length: number = lengthSlot?.value ?? 0;
        if (array) {
            // The declared length is charged before any element read so a huge sparse length fails early.
            this.spend(length);
            if (ARRAY_SYMBOLS.some((key) => Object.getOwnPropertyDescriptor(value, key)))
                throw new SourceRejected("extra_property", path);
        }
        // Opening and closing brackets; each element or key pays its own separator.
        this.emit(array ? ARRAY : OBJECT, 1);
        // A null prototype changes what an absent optional field reads as, so the tape records it.
        if (!array) this.emit(prototype === null, 0);
        if (lengthSlot) {
            this.emit(length, 0);
            this.attributes(lengthSlot);
        }
        this.ancestors.add(value);
        let count = 0;
        try {
            for (const key of Object.getOwnPropertySymbols(value)) {
                const slot = this.data(value, key, path);
                this.emit(EXTRA_KEY, 0);
                this.emit(key, 0);
                this.attributes(slot);
                this.walk(slot.value, path);
            }
            for (const key of Object.getOwnPropertyNames(value)) {
                if (array && key === "length") continue;
                if (array && ARRAY_METHODS.has(key))
                    throw new SourceRejected("extra_property", path);
                const slot = this.data(value, key, `${path}/${key}`);
                // Own names list integer indexes ascending, so an index key that is not `count` leaves a hole at `count`.
                if (array && key !== String(count)) {
                    const index = Number(key);
                    if (Number.isInteger(index) && index >= 0 && String(index) === key)
                        this.data(value, String(count), `${path}/${count}`);
                    this.emit(EXTRA_KEY, 0);
                    this.emit(key, 0);
                    this.attributes(slot);
                    this.walk(slot.value, `${path}/${key}`);
                    continue;
                }
                if (slot.value === undefined) {
                    if (array) throw new SourceRejected("undefined_element", `${path}/${key}`);
                    continue;
                }
                // Object keys render quoted with a colon; array elements pay only their comma.
                if (array) this.wire(1);
                else this.emit(key, key.length * 2 + 6);
                this.attributes(slot);
                visit(key, slot);
                count += 1;
            }
            if (array && count !== length) this.data(value, String(count), `${path}/${count}`);
            this.emit(array ? END_ARRAY : END_OBJECT, 1);
            this.emit(count, 0);
            return count;
        } finally {
            this.ancestors.delete(value);
        }
    }
}

export interface ReferenceableInspection {
    ok: true;
    /** Per-message upper bounds for JSON escaping, not retained heap charges. */
    messageWireBytes: number[];
    estimatedBytes: number;
}

/**
 * Byte-limit exhaustion throws CaptureBudgetExceeded. The first `skip` members are left to a
 * digest-verified capture: only their root slots are inspected, and their wire bounds read zero.
 */
export function inspectReferenceableMessages(
    messages: unknown,
    maxBytes = TRANSFORM_CAPTURE_MAX_BYTES,
    skip = 0,
): ReferenceableInspection | { ok: false; rejection: ReferenceableRejection } {
    const walker = new ReferenceableWalk(maxBytes);
    const messageWireBytes: number[] = [];
    try {
        walker.members(messages, (slot, index) => {
            if (index < skip) {
                defineSlot(messageWireBytes, index, 0);
                return;
            }
            const wireBefore = walker.wireBytes;
            walker.walk(slot.value, `/${index}`);
            defineSlot(messageWireBytes, index, walker.wireBytes - wireBefore);
        });
    } catch (error) {
        if (error instanceof SourceRejected)
            return { ok: false, rejection: { reason: error.reason, path: error.path } };
        throw error;
    }
    return { ok: true, estimatedBytes: walker.bytes, messageWireBytes };
}

export function snapshotFieldsEqual(
    left: MessageContentSnapshot,
    right: MessageContentSnapshot,
): boolean {
    if (left.fields.length !== right.fields.length) return false;
    for (let index = 0; index < left.fields.length; index += 1) {
        if (!Object.is(left.fields[index], right.fields[index])) return false;
    }
    return true;
}

/**
 * Pending text is flushed once it reaches this many UTF-16 units, and a longer string is hashed
 * in slices of this size, so one large member never builds a whole-member hash input.
 */
const HASH_CHUNK_UNITS = 1 << 16;

/**
 * Streams member tapes into one SHA-256 chain. Every token is self-delimiting: a string carries
 * its UTF-16 length, a number ends at `;`, and a member ends at `|`. The text is hashed as
 * UTF-16 code units, which keeps lone surrogates and makes the digest independent of chunking.
 * Symbols other than the tape markers cannot be hashed by identity, so they are kept in order.
 */
class TapeHasher {
    private readonly hash = createHash("sha256");
    private text = "";
    private readonly symbols: symbol[] = Object.setPrototypeOf([], null);
    private count = 0;
    private bytes = 0;

    readonly push = (value: SnapshotField): void => {
        if (typeof value === "string") {
            if (value.length < HASH_CHUNK_UNITS) {
                this.text += `s${value.length}:${value}`;
                if (this.text.length >= HASH_CHUNK_UNITS) this.flush();
            } else {
                this.text += `s${value.length}:`;
                this.flush();
                for (let start = 0; start < value.length; start += HASH_CHUNK_UNITS)
                    this.hash.update(value.slice(start, start + HASH_CHUNK_UNITS), "utf16le");
            }
        } else if (typeof value === "number")
            this.text += Object.is(value, -0) ? "-" : `n${value};`;
        else if (typeof value === "boolean") this.text += value ? "t" : "f";
        else if (value === null) this.text += "z";
        else if (value === ARRAY) this.text += "[";
        else if (value === END_ARRAY) this.text += "]";
        else if (value === OBJECT) this.text += "{";
        else if (value === END_OBJECT) this.text += "}";
        else if (value === EXTRA_KEY) this.text += "+";
        else {
            this.text += "y";
            this.symbols[this.symbols.length] = value;
        }
    };

    /** Closes one member whose walk spent `bytes`. */
    end(bytes: number): void {
        this.text += "|";
        this.count += 1;
        this.bytes += bytes;
        if (this.text.length >= HASH_CHUNK_UNITS) this.flush();
    }

    private flush(): void {
        if (this.text.length === 0) return;
        this.hash.update(this.text, "utf16le");
        this.text = "";
    }

    add(snapshot: TapedMember): void {
        for (let index = 0; index < snapshot.fields.length; index += 1)
            this.push(snapshot.fields[index] as SnapshotField);
        this.end(snapshot.bytes);
    }

    /** The chain so far; hashing may continue afterwards. */
    digest(): HistoryDigest {
        this.flush();
        const symbols: symbol[] = [];
        for (let index = 0; index < this.symbols.length; index += 1)
            defineSlot(symbols, index, this.symbols[index] as symbol);
        return {
            count: this.count,
            digest: this.hash.copy().digest("base64"),
            symbols,
            bytes: this.bytes,
        };
    }
}

interface TapedMember {
    fields: readonly SnapshotField[];
    bytes: number;
}

export function historyDigestsEqual(left: HistoryDigest, right: HistoryDigest): boolean {
    if (left === right) return true;
    if (
        left.count !== right.count ||
        left.digest !== right.digest ||
        left.bytes !== right.bytes ||
        left.symbols.length !== right.symbols.length
    )
        return false;
    for (let index = 0; index < left.symbols.length; index += 1) {
        if (left.symbols[index] !== right.symbols[index]) return false;
    }
    return true;
}

/**
 * Walks the members of a digested prefix under the prefix's recorded spend, so a changed prefix
 * costs no more than the original. Each walk is standalone; only its spend and hash are kept.
 */
class PrefixVerifier {
    private readonly walker: ReferenceableWalk;
    private readonly hasher = new TapeHasher();

    constructor(readonly expected: HistoryDigest) {
        this.walker = new ReferenceableWalk(expected.bytes, false);
    }

    /** Throws SourceRejected or CaptureBudgetExceeded when the member cannot match. */
    walk(value: unknown, index: number): void {
        const before = this.walker.bytes;
        this.walker.stream(() => this.walker.walk(value, `/${index}`), this.hasher.push);
        this.hasher.end(this.walker.bytes - before);
    }

    matches(): boolean {
        return historyDigestsEqual(this.hasher.digest(), this.expected);
    }

    continueHashing(): TapeHasher {
        return this.hasher;
    }
}

export interface CapturedMessages {
    members: readonly unknown[];
    /** Members covered by `verified` have no tape; their recheck walks the digest. */
    snapshots: readonly (MessageContentSnapshot | undefined)[];
    rootSnapshot: MessageContentSnapshot;
    /** The prior digest the leading members matched, when the capture was given one. */
    verified?: HistoryDigest;
}

export interface CapturedHistory extends CapturedMessages {
    /** `history` excludes the final member; `terminal` hashes it alone. */
    history: HistoryDigest;
    terminal?: HistoryDigest;
    /** The first taped member alone, compared against a prior pass's terminal. */
    boundary?: HistoryDigest;
}

const PREFIX_CHANGED = Symbol("prefix_changed");

/** The caller reserves the inspection charge before allocating retained capture state. */
export function captureMessages(messages: unknown, lease: CaptureLease): CapturedMessages {
    return walkCapture(messages, lease);
}

/**
 * With a `prefix`, capture charges only the root and members after the prefix; it verifies
 * leading members instead of retaining them. A mismatch, or no member after the prefix, returns
 * `undefined`.
 */
export function captureHistory(messages: unknown, lease: CaptureLease): CapturedHistory;
export function captureHistory(
    messages: unknown,
    lease: CaptureLease,
    prefix: HistoryDigest,
): CapturedHistory | undefined;
export function captureHistory(
    messages: unknown,
    lease: CaptureLease,
    prefix?: HistoryDigest,
): CapturedHistory | undefined {
    const verifier = prefix && new PrefixVerifier(prefix);
    const taped: TapedMember[] = [];
    let captured: CapturedMessages;
    try {
        captured = walkCapture(messages, lease, { taped, verifier });
    } catch (error) {
        if (error === PREFIX_CHANGED) return undefined;
        throw error;
    }
    const hasher = verifier?.continueHashing() ?? new TapeHasher();
    for (let index = 0; index < taped.length - 1; index += 1)
        hasher.add(taped[index] as TapedMember);
    const single = (member: TapedMember | undefined): HistoryDigest | undefined => {
        if (!member) return undefined;
        const alone = new TapeHasher();
        alone.add(member);
        return alone.digest();
    };
    const boundary = single(taped[0]);
    return {
        ...captured,
        verified: prefix,
        history: hasher.digest(),
        terminal: taped.length > 1 ? single(taped[taped.length - 1]) : boundary,
        boundary,
    };
}

/** `digest.verifier` validates leading members; a mismatch throws `PREFIX_CHANGED`. */
function walkCapture(
    messages: unknown,
    lease: CaptureLease,
    digest?: { taped: TapedMember[]; verifier?: PrefixVerifier },
): CapturedMessages {
    if (lease.signal.aborted || lease.chargedBytes < ROOT_CAPTURE_BYTES)
        throw new CaptureBudgetExceeded("capture requires a live reservation");
    const walker = new ReferenceableWalk(
        Math.min(lease.chargedBytes, TRANSFORM_CAPTURE_MAX_BYTES),
        false,
    );
    const verifier = digest?.verifier;
    const verifiedCount = verifier?.expected.count ?? 0;
    const members: unknown[] = [];
    const snapshots: (MessageContentSnapshot | undefined)[] = [];
    const rootSnapshot = walker.recordOrCompare(() => {
        const count = walker.members(messages, (slot, index) => {
            if (verifier && index < verifiedCount) {
                try {
                    verifier.walk(slot.value, index);
                } catch (error) {
                    if (error instanceof SourceRejected || error instanceof CaptureBudgetExceeded)
                        throw PREFIX_CHANGED;
                    throw error;
                }
                defineSlot(snapshots, index, undefined);
            } else {
                if (index === verifiedCount && verifier && !verifier.matches())
                    throw PREFIX_CHANGED;
                const before = walker.bytes;
                const snapshot = walker.recordOrCompare(() => walker.walk(slot.value, `/${index}`));
                defineSlot(snapshots, index, snapshot);
                if (digest)
                    defineSlot(digest.taped, digest.taped.length, {
                        fields: snapshot.fields,
                        bytes: walker.bytes - before,
                    });
            }
            defineSlot(members, index, slot.value);
        });
        if (verifier && count <= verifiedCount) throw PREFIX_CHANGED;
    });
    return { members, snapshots, rootSnapshot };
}

/** Membership is checked through own descriptors; an accessor or inherited slot cannot match. */
export function capturedMessagesUnchanged(live: unknown, captured: CapturedMessages): boolean {
    try {
        const walker = new ReferenceableWalk(TRANSFORM_CAPTURE_MAX_BYTES, false);
        const verifier = captured.verified && new PrefixVerifier(captured.verified);
        walker.recordOrCompare(() => {
            const count = walker.members(live, (slot, index) => {
                if (
                    index >= captured.members.length ||
                    !Object.is(slot.value, captured.members[index])
                )
                    throw CONTENT_CHANGED;
                if (verifier && index < verifier.expected.count) {
                    verifier.walk(slot.value, index);
                    return;
                }
                // Bounds precede indexing so a short record cannot read an inherited slot.
                const snapshot =
                    index < captured.snapshots.length ? captured.snapshots[index] : undefined;
                if (!snapshot) throw CONTENT_CHANGED;
                walker.recordOrCompare(() => walker.walk(slot.value, `/${index}`), snapshot);
            });
            if (count !== captured.members.length) throw CONTENT_CHANGED;
            if (verifier && !verifier.matches()) throw CONTENT_CHANGED;
        }, captured.rootSnapshot);
        return true;
    } catch (error) {
        if (
            error === CONTENT_CHANGED ||
            error instanceof SourceRejected ||
            error instanceof CaptureBudgetExceeded
        )
            return false;
        throw error;
    }
}

type HostArrayRejectionReason =
    | ReferenceableRejection["reason"]
    | "not_extensible"
    | "length_not_writable"
    | "element_not_writable"
    | "budget";

export function hostArrayReplacementRejection(target: unknown): HostArrayRejectionReason | null {
    if (types.isProxy(target)) return "proxy";
    if (!Array.isArray(target)) return "not_array";
    if (!Object.isExtensible(target)) return "not_extensible";
    if (!Object.getOwnPropertyDescriptor(target, "length")?.writable) return "length_not_writable";
    try {
        let writable = true;
        new ReferenceableWalk().members(target, (slot) => {
            writable = writable && slot.writable === true && slot.configurable === true;
        });
        return writable ? null : "element_not_writable";
    } catch (error) {
        if (error instanceof SourceRejected) return error.reason;
        if (error instanceof CaptureBudgetExceeded) return "budget";
        throw error;
    }
}

/**
 * Precondition: `hostArrayReplacementRejection(target)` returns `null`. Own-slot definitions
 * bypass inherited setters; a writable length and configurable slots allow shrinking.
 */
export function replaceHostArrayContents(target: unknown[], next: readonly unknown[]): void {
    for (let index = 0; index < next.length; index += 1) {
        defineSlot(target, index, next[index]);
    }
    Object.defineProperty(target, "length", { value: next.length });
}

export interface CaptureLease {
    readonly signal: AbortSignal;
    readonly chargedBytes: number;
    /** Owner-wide headroom shared by every lease, not this lease's own remainder; zero once released. */
    readonly remainingBytes: number;
    reserve(bytes: number): boolean;
    requestCancel(reason: string): void;
    release(): void;
}

/** TransformCaptureAdmission permits one live lease per session. */
export class TransformCaptureAdmission {
    private readonly leases = new Map<string, CaptureLease>();
    private chargedBytesTotal = 0;
    constructor(
        private readonly limits = {
            maxPasses: TRANSFORM_CAPTURE_MAX_PASSES,
            maxBytes: TRANSFORM_CAPTURE_MAX_BYTES,
        },
    ) {}

    get activePasses(): number {
        return this.leases.size;
    }
    get chargedBytes(): number {
        return this.chargedBytesTotal;
    }
    get remainingBytes(): number {
        return this.limits.maxBytes - this.chargedBytesTotal;
    }

    admit(
        sessionId: string,
    ): { lease: CaptureLease } | { declined: "session_busy" | "pass_count" } {
        const held = this.leases.get(sessionId);
        if (held) {
            held.requestCancel(
                `transform pass for session ${sessionId} superseded by a newer call`,
            );
            return { declined: "session_busy" };
        }
        if (this.leases.size >= this.limits.maxPasses) return { declined: "pass_count" };
        const controller = new AbortController();
        const owner = this;
        let bytes = 0;
        let released = false;
        const lease: CaptureLease = {
            signal: controller.signal,
            get chargedBytes() {
                return bytes;
            },
            get remainingBytes() {
                return released ? 0 : owner.remainingBytes;
            },
            reserve(additional) {
                if (
                    released ||
                    !Number.isSafeInteger(additional) ||
                    additional < 0 ||
                    additional > owner.remainingBytes
                )
                    return false;
                owner.chargedBytesTotal += additional;
                bytes += additional;
                return true;
            },
            requestCancel(reason) {
                if (!controller.signal.aborted) controller.abort(new Error(reason));
            },
            release() {
                if (released) return;
                released = true;
                owner.chargedBytesTotal -= bytes;
                bytes = 0;
                owner.leases.delete(sessionId);
            },
        };
        this.leases.set(sessionId, lease);
        return { lease };
    }

    requestCancel(sessionId: string, reason: string): void {
        this.leases.get(sessionId)?.requestCancel(reason);
    }
}

/** Module-wide default owner; tests can inject a separate instance. */
export const defaultTransformCaptureAdmission = new TransformCaptureAdmission();

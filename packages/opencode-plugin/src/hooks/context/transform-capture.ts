import { types } from "node:util";

const MAX_SOURCE_WALK_BYTES = 256 * 1024 * 1024;
const MAX_REFERENCEABLE_DEPTH = 256;
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

export class SourceRejected extends Error {
    override name = "SourceRejected";
    readonly logLevel: "debug" | "warn" = "debug";
}
export class SourceWalkLimitExceeded extends SourceRejected {
    override name = "SourceWalkLimitExceeded";
    override readonly logLevel = "warn";
}

// Own-slot definitions bypass inherited numeric setters on private arrays.
function defineSlot<T>(array: T[], index: number, value: T): void {
    Object.defineProperty(array, index, {
        value,
        writable: true,
        enumerable: true,
        configurable: true,
    });
}

export function readOwnDataProperty(value: unknown, key: string): unknown {
    if (value === null || typeof value !== "object" || types.isProxy(value)) return undefined;
    const descriptor = Object.getOwnPropertyDescriptor(value, key);
    return descriptor && "value" in descriptor ? descriptor.value : undefined;
}

/**
 * The root array is the one value that crosses back to the host as a return value, so a `then`
 * anywhere on its chain would be assimilated by the host's promise machinery; `in` observes an
 * inherited accessor without invoking it. Returns the rejection reason, or `undefined` when the
 * root is a plain non-proxy array over an untouched prototype chain.
 */
export function rootArrayRejection(value: unknown): string | undefined {
    if (types.isProxy(value)) return "proxy root array";
    if (!Array.isArray(value)) return "root is not an array";
    if (
        Object.getPrototypeOf(value) !== Array.prototype ||
        Object.getPrototypeOf(Array.prototype) !== Object.prototype ||
        Object.getPrototypeOf(Object.prototype) !== null
    )
        return "unsupported prototype on root array";
    if ("then" in value) return "then property on root array";
    return undefined;
}

const CONTENT_CHANGED = Symbol("content_changed");

class ReferenceableWalk {
    private bytes = 0;
    private field?: (value: SnapshotField) => void;
    private readonly ancestors = new Set<object>();

    private spend(bytes: number): void {
        if (bytes > MAX_SOURCE_WALK_BYTES - this.bytes)
            throw new SourceWalkLimitExceeded("source and snapshot walk limit exceeded");
        this.bytes += bytes;
    }

    private emit(value: SnapshotField): void {
        this.spend(16 + (typeof value === "string" ? 32 + value.length * 2 : 0));
        this.field?.(value);
    }

    walk(value: unknown, path = ""): void {
        this.spend(16);
        const type = typeof value;
        if (value === null || type !== "object") {
            if (type === "number" && !Number.isFinite(value))
                throw new SourceRejected(`nonfinite number at ${path}`);
            if (value !== null && type !== "string" && type !== "number" && type !== "boolean")
                throw new SourceRejected(`unsupported ${type} at ${path}`);
            this.emit(value as SnapshotField);
            return;
        }
        this.entries(value as object, path, (key, slot) => this.walk(slot.value, `${path}/${key}`));
    }

    members(messages: unknown, visit: (slot: PropertyDescriptor, index: number) => void): number {
        const rejection = rootArrayRejection(messages);
        if (rejection !== undefined) throw new SourceRejected(rejection);
        this.spend(16);
        return this.entries(messages as unknown[], "", (key, slot) => visit(slot, Number(key)));
    }

    recordOrCompare(
        operation: () => void,
        expected?: MessageContentSnapshot,
    ): MessageContentSnapshot {
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
        this.emit(slot.enumerable === true);
        this.emit(slot.writable === true);
        this.emit(slot.configurable === true);
    }

    private data(value: object, key: PropertyKey, path: string): PropertyDescriptor {
        this.spend(48 + (typeof key === "string" ? key.length * 4 : 0));
        const slot = Object.getOwnPropertyDescriptor(value, key);
        if (!slot) throw new SourceRejected(`sparse array at ${path}`);
        if (!("value" in slot)) throw new SourceRejected(`accessor at ${path}`);
        return slot;
    }

    private entries(
        value: object,
        path: string,
        visit: (key: string, slot: PropertyDescriptor) => void,
    ): number {
        if (types.isProxy(value)) throw new SourceRejected(`proxy at ${path}`);
        const array = Array.isArray(value);
        const prototype = Object.getPrototypeOf(value);
        if (
            (array
                ? prototype !== Array.prototype
                : prototype !== Object.prototype && prototype !== null) ||
            Object.getPrototypeOf(Array.prototype) !== Object.prototype
        )
            throw new SourceRejected(`unsupported prototype at ${path}`);
        if (this.ancestors.size > MAX_REFERENCEABLE_DEPTH)
            throw new SourceRejected(`excessive depth at ${path}`);
        if (this.ancestors.has(value)) throw new SourceRejected(`cycle at ${path}`);
        this.spend(64);
        // JSON looks up toJSON even when it is hidden or inherited.
        for (
            let holder: object | null = value;
            holder !== null;
            holder = Object.getPrototypeOf(holder)
        ) {
            const hook = Object.getOwnPropertyDescriptor(holder, "toJSON");
            if (hook && (!("value" in hook) || hook.value !== undefined))
                throw new SourceRejected(`toJSON at ${path}`);
        }
        const lengthSlot = array ? this.data(value, "length", path) : undefined;
        const length: number = lengthSlot?.value ?? 0;
        if (array) {
            this.spend(length * 64);
            for (const key of ARRAY_SYMBOLS) {
                if (Object.getOwnPropertyDescriptor(value, key))
                    throw new SourceRejected(`array operation override at ${path}`);
            }
        }
        this.emit(array ? ARRAY : OBJECT);
        if (lengthSlot) {
            this.emit(length);
            this.attributes(lengthSlot);
        }
        this.ancestors.add(value);
        let count = 0;
        try {
            for (const key of Object.getOwnPropertySymbols(value)) {
                const slot = this.data(value, key, path);
                this.emit(EXTRA_KEY);
                this.emit(key);
                this.attributes(slot);
                this.walk(slot.value, path);
            }
            for (const key of Object.getOwnPropertyNames(value)) {
                this.spend(16);
                if (array && key === "length") continue;
                if (array && ARRAY_METHODS.has(key))
                    throw new SourceRejected(`array operation override at ${path}/${key}`);
                const slot = this.data(value, key, `${path}/${key}`);
                if (array && key !== String(count)) {
                    const index = Number(key);
                    if (
                        Number.isInteger(index) &&
                        index >= 0 &&
                        index < length &&
                        String(index) === key
                    )
                        this.data(value, String(count), `${path}/${count}`);
                    this.emit(EXTRA_KEY);
                    this.emit(key);
                    this.attributes(slot);
                    this.walk(slot.value, `${path}/${key}`);
                    continue;
                }
                if (slot.value === undefined) {
                    if (array) throw new SourceRejected(`undefined element at ${path}/${key}`);
                    continue;
                }
                if (!array) this.emit(key);
                this.attributes(slot);
                visit(key, slot);
                count += 1;
            }
            if (array && count !== length) this.data(value, String(count), `${path}/${count}`);
            this.emit(array ? END_ARRAY : END_OBJECT);
            this.emit(count);
            return count;
        } finally {
            this.ancestors.delete(value);
        }
    }
}

export function assertReferenceableMessages(messages: unknown): void {
    const walker = new ReferenceableWalk();
    walker.members(messages, (slot, index) => walker.walk(slot.value, `/${index}`));
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

export interface CapturedMessages {
    members: unknown[];
    snapshots: MessageContentSnapshot[];
    rootSnapshot: MessageContentSnapshot;
}

export function captureMessages(messages: unknown): CapturedMessages {
    const walker = new ReferenceableWalk();
    const members: unknown[] = [];
    const snapshots: MessageContentSnapshot[] = [];
    const rootSnapshot = walker.recordOrCompare(() => {
        walker.members(messages, (slot, index) => {
            defineSlot(
                snapshots,
                index,
                walker.recordOrCompare(() => walker.walk(slot.value, `/${index}`)),
            );
            defineSlot(members, index, slot.value);
        });
    });
    return { members, snapshots, rootSnapshot };
}

export function assertCapturedMessagesUnchanged(live: unknown, captured: CapturedMessages): void {
    try {
        const walker = new ReferenceableWalk();
        walker.recordOrCompare(() => {
            const count = walker.members(live, (slot, index) => {
                if (
                    index >= captured.members.length ||
                    !Object.is(slot.value, captured.members[index])
                )
                    throw CONTENT_CHANGED;
                walker.recordOrCompare(
                    () => walker.walk(slot.value, `/${index}`),
                    captured.snapshots[index],
                );
            });
            if (count !== captured.members.length) throw CONTENT_CHANGED;
        }, captured.rootSnapshot);
    } catch (error) {
        if (error === CONTENT_CHANGED) throw new SourceRejected("source changed during transform");
        throw error;
    }
}

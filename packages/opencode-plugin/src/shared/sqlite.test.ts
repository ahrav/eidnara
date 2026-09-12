import { describe, expect, it } from "bun:test";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    buildNodeSqliteDatabaseClass,
    collectSqliteRuntimeGateInput,
    Database,
    detectSqliteRuntime,
    isInTransaction,
    loadSqliteModule,
    runImmediate,
    SqliteRuntimeUnavailableError,
    withPrivilegedWriter,
} from "./sqlite";

// The wrappers reject promise-returning callbacks at the type level; these
// tests cast past that to reach the runtime guards the types back up.
type UncheckedBody = () => void;

function withTempDir<T>(body: (dir: string) => T): T {
    const dir = mkdtempSync(join(tmpdir(), "eidnara-sqlite-test-"));
    try {
        return body(dir);
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
}

describe("SQLite runtime selector", () => {
    it("wraps a missing node:sqlite module with the detected runtime and cause", async () => {
        const cause = Object.assign(new Error("No such built-in module: node:sqlite"), {
            code: "ERR_UNKNOWN_BUILTIN_MODULE",
            name: "ResolveMessage",
        });
        let requestedSpecifier = "";
        let thrown: unknown;

        try {
            await loadSqliteModule("Node.js", async (specifier) => {
                requestedSpecifier = specifier;
                throw cause;
            });
        } catch (error) {
            thrown = error;
        }

        expect(requestedSpecifier).toBe("node:sqlite");
        expect(thrown).toBeInstanceOf(SqliteRuntimeUnavailableError);
        const compatibilityError = thrown as SqliteRuntimeUnavailableError;
        expect(compatibilityError.runtime).toBe("Node.js");
        expect(compatibilityError.specifier).toBe("node:sqlite");
        expect(compatibilityError.message).toContain("this Node.js build lacks node:sqlite");
        expect(compatibilityError.message).not.toContain("Bun build lacks node:sqlite");
        expect(compatibilityError.message).toContain("Node.js >= 24");
        expect(compatibilityError.message).toContain("Bun with bun:sqlite");
        expect(compatibilityError.message).toContain("node:sqlite");
        expect(compatibilityError.cause).toBe(cause);
    });

    it("blames Bun when bun:sqlite is missing under Bun", async () => {
        const cause = Object.assign(new Error("Cannot find module bun:sqlite"), {
            code: "ERR_MODULE_NOT_FOUND",
        });
        let thrown: unknown;
        try {
            await loadSqliteModule("Bun", async () => {
                throw cause;
            });
        } catch (error) {
            thrown = error;
        }
        expect((thrown as Error).message).toContain("this Bun build lacks bun:sqlite");
    });
});

describe("withPrivilegedWriter", () => {
    function openWithPrivilegeTable(): Database {
        const db = new Database(":memory:");
        db.exec(
            "CREATE TABLE context_privilege_state(id INTEGER PRIMARY KEY, enabled INTEGER NOT NULL);" +
                "CREATE TABLE t(a);" +
                "CREATE TRIGGER trg BEFORE INSERT ON t BEGIN SELECT RAISE(ROLLBACK, 'trigger-rollback'); END;",
        );
        return db;
    }

    it("propagates the operation error when RAISE(ROLLBACK) already ended the transaction", () => {
        const db = openWithPrivilegeTable();
        try {
            let thrown: unknown;
            try {
                withPrivilegedWriter(db, () => {
                    db.prepare("INSERT INTO t VALUES (1)").run();
                });
            } catch (error) {
                thrown = error;
            }
            expect((thrown as Error).message).toContain("trigger-rollback");
            expect(isInTransaction(db)).toBe(false);
        } finally {
            db.close();
        }
    });

    it("propagates the operation error from a nested scope whose savepoint was destroyed", () => {
        const db = openWithPrivilegeTable();
        try {
            let thrown: unknown;
            try {
                runImmediate(db, () => {
                    withPrivilegedWriter(db, () => {
                        db.prepare("INSERT INTO t VALUES (1)").run();
                    });
                });
            } catch (error) {
                thrown = error;
            }
            expect((thrown as Error).message).toContain("trigger-rollback");
            expect(isInTransaction(db)).toBe(false);
        } finally {
            db.close();
        }
    });

    it("rolls back the operation and clears the privilege flag on an ordinary failure", () => {
        const db = new Database(":memory:");
        try {
            db.exec(
                "CREATE TABLE context_privilege_state(id INTEGER PRIMARY KEY, enabled INTEGER NOT NULL);" +
                    "CREATE TABLE plain(a);",
            );
            expect(() =>
                withPrivilegedWriter(db, () => {
                    db.prepare("INSERT INTO plain VALUES (1)").run();
                    throw new Error("boom");
                }),
            ).toThrow("boom");
            expect(isInTransaction(db)).toBe(false);
            expect(db.prepare("SELECT COUNT(*) AS n FROM plain").get()).toEqual({ n: 0 });
        } finally {
            db.close();
        }
    });

    it("rejects an async operation before it runs so no write escapes the transaction", async () => {
        const db = new Database(":memory:");
        try {
            db.exec(
                "CREATE TABLE context_privilege_state(id INTEGER PRIMARY KEY, enabled INTEGER NOT NULL);" +
                    "CREATE TABLE plain(a);",
            );
            expect(() =>
                withPrivilegedWriter(db, (async () => {
                    db.prepare("INSERT INTO plain VALUES ('pre-await')").run();
                    await Promise.resolve();
                    db.prepare("INSERT INTO plain VALUES ('post-await')").run();
                }) as unknown as UncheckedBody),
            ).toThrow(TypeError);
            await new Promise((resolve) => setTimeout(resolve, 10));
            expect(isInTransaction(db)).toBe(false);
            expect(db.prepare("SELECT COUNT(*) AS n FROM plain").get()).toEqual({ n: 0 });
            expect(db.prepare("SELECT COUNT(*) AS n FROM context_privilege_state").get()).toEqual({
                n: 0,
            });
        } finally {
            db.close();
        }
    });
});

describe("runImmediate", () => {
    it("rejects an async body before it runs so no write escapes the transaction", async () => {
        const db = new Database(":memory:");
        try {
            db.exec("CREATE TABLE plain(a)");
            expect(() =>
                runImmediate(db, (async () => {
                    db.prepare("INSERT INTO plain VALUES ('pre-await')").run();
                    await Promise.resolve();
                    db.prepare("INSERT INTO plain VALUES ('post-await')").run();
                }) as unknown as UncheckedBody),
            ).toThrow(/cannot be an async function/);
            await new Promise((resolve) => setTimeout(resolve, 10));
            expect(isInTransaction(db)).toBe(false);
            expect(db.prepare("SELECT COUNT(*) AS n FROM plain").get()).toEqual({ n: 0 });
        } finally {
            db.close();
        }
    });

    it("rejects a plain body that returns a promise and rolls back its synchronous prefix", () => {
        const db = new Database(":memory:");
        try {
            db.exec("CREATE TABLE plain(a)");
            expect(() =>
                runImmediate(db, (() => {
                    db.prepare("INSERT INTO plain VALUES (1)").run();
                    return Promise.resolve();
                }) as unknown as UncheckedBody),
            ).toThrow(/cannot return a promise/);
            expect(isInTransaction(db)).toBe(false);
            expect(db.prepare("SELECT COUNT(*) AS n FROM plain").get()).toEqual({ n: 0 });
        } finally {
            db.close();
        }
    });

    it("commits a synchronous body", () => {
        const db = new Database(":memory:");
        try {
            db.exec("CREATE TABLE plain(a)");
            const result = runImmediate(db, () => {
                db.prepare("INSERT INTO plain VALUES (1)").run();
                return "done";
            });
            expect(result).toBe("done");
            expect(db.prepare("SELECT COUNT(*) AS n FROM plain").get()).toEqual({ n: 1 });
        } finally {
            db.close();
        }
    });
});

describe("collectSqliteRuntimeGateInput", () => {
    it.if(detectSqliteRuntime() === "Bun")("reports the running Bun version", () => {
        const input = collectSqliteRuntimeGateInput();
        expect(input.runtime).toBe("Bun");
        expect(input.runtimeVersion).toBe(Bun.version);
        expect(input.runtimeVersion).not.toBe("0.0.0");
    });
});

type ExecLog = string[];

/** The transaction flag flips on BEGIN/COMMIT/ROLLBACK so tests can drive the nested-savepoint branch. */
function makeFakeDatabaseSync() {
    const constructed: Array<{ location: unknown; options: unknown }> = [];
    const execLog: ExecLog = [];
    class FakeDatabaseSync {
        inTx = false;
        constructor(location: unknown, options: unknown) {
            constructed.push({ location, options });
        }
        get isTransaction(): boolean {
            return this.inTx;
        }
        exec(sql: string): void {
            execLog.push(sql);
            if (/^BEGIN/.test(sql)) this.inTx = true;
            if (sql === "COMMIT" || sql === "ROLLBACK") this.inTx = false;
        }
        prepare(_sql: string) {
            return {
                run: (...args: unknown[]) => ({ args }),
                get: (...args: unknown[]) => ({ args }),
                all: (...args: unknown[]) => ({ args }),
            };
        }
    }
    return { FakeDatabaseSync, constructed, execLog };
}

describe("node:sqlite adapter constructor", () => {
    it("maps readonly to readOnly and forwards nothing else", () => {
        const { FakeDatabaseSync, constructed } = makeFakeDatabaseSync();
        const Impl = buildNodeSqliteDatabaseClass(FakeDatabaseSync);
        new Impl(":memory:", { readonly: true });
        expect(constructed[0]?.location).toBe(":memory:");
        expect(constructed[0]?.options).toEqual({ readOnly: true });
    });

    it("opens an anonymous in-memory database when the location is omitted", () => {
        const { FakeDatabaseSync, constructed } = makeFakeDatabaseSync();
        const Impl = buildNodeSqliteDatabaseClass(FakeDatabaseSync);
        new Impl();
        expect(constructed[0]?.location).toBe(":memory:");
    });

    it("rejects a non-string location and unsupported better-sqlite3 options before constructing", () => {
        const { FakeDatabaseSync, constructed } = makeFakeDatabaseSync();
        const Impl = buildNodeSqliteDatabaseClass(FakeDatabaseSync);
        expect(() => new Impl(Buffer.from("/tmp/x.db"))).toThrow(TypeError);
        expect(() => new Impl(":memory:", { timeout: 5000 })).toThrow(/timeout/);
        expect(constructed).toHaveLength(0);
    });

    it("honors fileMustExist by refusing a missing path before opening", () => {
        withTempDir((dir) => {
            const { FakeDatabaseSync, constructed } = makeFakeDatabaseSync();
            const Impl = buildNodeSqliteDatabaseClass(FakeDatabaseSync);
            const missing = join(dir, "missing.db");
            expect(() => new Impl(missing, { fileMustExist: true })).toThrow(missing);
            expect(constructed).toHaveLength(0);

            const present = join(dir, "present.db");
            writeFileSync(present, "");
            new Impl(present, { fileMustExist: true });
            expect(constructed[0]?.location).toBe(present);
            expect(constructed[0]?.options).toEqual({});
        });
    });
});

describe("node:sqlite adapter transaction shim", () => {
    it("rolls back a nested savepoint under the same name it created", () => {
        const { FakeDatabaseSync, execLog } = makeFakeDatabaseSync();
        const Impl = buildNodeSqliteDatabaseClass(FakeDatabaseSync);
        const db = new Impl(":memory:") as unknown as {
            exec(sql: string): void;
            transaction<F extends (...args: unknown[]) => unknown>(fn: F): F;
        };
        db.exec("BEGIN IMMEDIATE");
        const failing = db.transaction(() => {
            throw new Error("inner");
        });
        expect(() => failing()).toThrow("inner");

        const savepoint = execLog.find((sql) => sql.startsWith("SAVEPOINT "));
        expect(savepoint).toBeDefined();
        const name = (savepoint as string).slice("SAVEPOINT ".length);
        expect(execLog).toContain(`ROLLBACK TO ${name}`);
        expect(execLog).toContain(`RELEASE ${name}`);
    });

    it("rejects an async callback at wrap time so no transaction is opened", () => {
        const { FakeDatabaseSync, execLog } = makeFakeDatabaseSync();
        const Impl = buildNodeSqliteDatabaseClass(FakeDatabaseSync);
        const db = new Impl(":memory:") as unknown as {
            transaction<F extends (...args: unknown[]) => unknown>(fn: F): F;
        };
        expect(() =>
            db.transaction(async () => {
                await Promise.resolve();
            }),
        ).toThrow(/cannot be an async function/);
        expect(execLog).toEqual([]);
    });

    it("rejects a plain callback that returns a promise and rolls back", () => {
        const { FakeDatabaseSync, execLog } = makeFakeDatabaseSync();
        const Impl = buildNodeSqliteDatabaseClass(FakeDatabaseSync);
        const db = new Impl(":memory:") as unknown as {
            transaction<F extends (...args: unknown[]) => unknown>(fn: F): F;
        };
        const thenableTx = db.transaction(() => Promise.resolve());
        expect(() => thenableTx()).toThrow(/cannot return a promise/);
        expect(execLog).toEqual(["BEGIN", "ROLLBACK"]);
    });
});

describe("bun:sqlite adapter constructor", () => {
    const onBun = detectSqliteRuntime() === "Bun";

    it.if(onBun)("accepts better-sqlite3-style options that leave the file writable", () => {
        withTempDir((dir) => {
            for (const options of [{}, { readonly: false }]) {
                const path = join(dir, `w-${Object.keys(options).length}.db`);
                const db = new Database(path, options);
                try {
                    db.exec("CREATE TABLE t(a)");
                    db.prepare("INSERT INTO t VALUES (?)").run(1);
                    expect(db.prepare("SELECT COUNT(*) AS n FROM t").get()).toEqual({ n: 1 });
                } finally {
                    db.close();
                }
            }
        });
    });

    it.if(onBun)("rejects writes on a readonly handle", () => {
        withTempDir((dir) => {
            const path = join(dir, "ro.db");
            const writer = new Database(path);
            writer.exec("CREATE TABLE t(a)");
            writer.close();
            const reader = new Database(path, { readonly: true });
            try {
                expect(() => reader.exec("INSERT INTO t VALUES (1)")).toThrow(/readonly/);
            } finally {
                reader.close();
            }
        });
    });

    it.if(onBun)("honors fileMustExist and does not create the file", () => {
        withTempDir((dir) => {
            const missing = join(dir, "missing.db");
            expect(() => new Database(missing, { fileMustExist: true })).toThrow(missing);
            expect(existsSync(missing)).toBe(false);
        });
    });

    it.if(onBun)("rejects a non-string location and unsupported better-sqlite3 options", () => {
        expect(() => new Database(Buffer.from("/tmp/x.db"))).toThrow(TypeError);
        expect(() => new Database(":memory:", { timeout: 5000 })).toThrow(/timeout/);
    });
});

describe("bun:sqlite adapter transaction", () => {
    const onBun = detectSqliteRuntime() === "Bun";

    it.if(onBun)(
        "rejects an async callback before it runs so no write escapes the transaction",
        async () => {
            const db = new Database(":memory:");
            try {
                db.exec("CREATE TABLE plain(a)");
                expect(() =>
                    db.transaction(async () => {
                        db.prepare("INSERT INTO plain VALUES ('pre-await')").run();
                        await Promise.resolve();
                        db.prepare("INSERT INTO plain VALUES ('post-await')").run();
                    }),
                ).toThrow(/cannot be an async function/);
                // A continuation that had started would land here as an autocommit write.
                await new Promise((resolve) => setTimeout(resolve, 10));
                expect(isInTransaction(db)).toBe(false);
                expect(db.prepare("SELECT COUNT(*) AS n FROM plain").get()).toEqual({ n: 0 });
            } finally {
                db.close();
            }
        },
    );

    it.if(onBun)("keeps the mode variants and commits a synchronous callback", () => {
        const db = new Database(":memory:");
        try {
            db.exec("CREATE TABLE plain(a)");
            const insert = db.transaction((value: number) => {
                db.prepare("INSERT INTO plain VALUES (?)").run(value);
                return value * 2;
            });
            expect(insert(1)).toBe(2);
            expect(insert.immediate(2)).toBe(4);
            expect(insert.deferred(3)).toBe(6);
            expect(insert.exclusive(4)).toBe(8);
            expect(db.prepare("SELECT COUNT(*) AS n FROM plain").get()).toEqual({ n: 4 });
        } finally {
            db.close();
        }
    });
});

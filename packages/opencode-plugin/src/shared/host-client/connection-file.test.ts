import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { execFile } from "node:child_process";
import { chmod, mkdir, mkdtemp, realpath, rename, rm, symlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import {
    ConnectionFileError,
    MAX_CONNECTION_FILE_LEN,
    type ReadConnectionFileOptions,
    readConnectionFile,
    toExactByteArray,
} from "./connection-file";
import { Deadline } from "./deadline";

const execFileAsync = promisify(execFile);

let tmpDir = "";
let fileCounter = 0;

beforeAll(async () => {
    // `os.tmpdir()` is a symlink on macOS (`/var`), and the reader rejects symlinked ancestors.
    tmpDir = await realpath(await mkdtemp(path.join(os.tmpdir(), "eidnara-host-conn-file-")));
});

afterAll(async () => {
    if (tmpDir) await rm(tmpDir, { recursive: true, force: true });
});

function freshPath(name: string): string {
    fileCounter += 1;
    return path.join(tmpDir, `${fileCounter}-${name}`);
}

const KEY = Array.from({ length: 32 }, (_, i) => i);
const DAEMON_ID = Array.from({ length: 16 }, (_, i) => 0x60 + i);

function validJson(overrides: Record<string, unknown> = {}): Record<string, unknown> {
    return {
        schema: 2,
        wire_version: 2,
        setup_socket: "/tmp/eidnara-host.sock",
        key: KEY,
        daemon_id: DAEMON_ID,
        pid: 4_242,
        daemon_ver: "eidnara-host/0.1.0",
        ...overrides,
    };
}

async function writePrivateFile(filePath: string, content: string | Uint8Array): Promise<void> {
    await writeFile(filePath, content, { mode: 0o600 });
}

function options(overrides: Partial<ReadConnectionFileOptions> = {}): ReadConnectionFileOptions {
    return { deadline: Deadline.start(60_000), ...overrides };
}

async function expectFailure(
    filePath: string,
    code: ConnectionFileError["code"],
    overrides: Partial<ReadConnectionFileOptions> = {},
): Promise<void> {
    const attempt = readConnectionFile(filePath, options(overrides));
    const error = await attempt.then(
        () => {
            throw new Error("readConnectionFile unexpectedly succeeded");
        },
        (thrown: unknown) => thrown,
    );
    expect(error).toBeInstanceOf(ConnectionFileError);
    expect((error as ConnectionFileError).code).toBe(code);
}

async function writeInvalid(
    json: Record<string, unknown>,
    code: ConnectionFileError["code"],
): Promise<void> {
    const filePath = freshPath("invalid.json");
    await writePrivateFile(filePath, JSON.stringify(json));
    await expectFailure(filePath, code);
}

describe("direct-file snapshot", () => {
    test("accepts a valid owner-only file and returns an immutable snapshot", async () => {
        const filePath = freshPath("valid.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        const snapshot = await readConnectionFile(filePath, options());
        expect(snapshot.setupSocket).toBe("/tmp/eidnara-host.sock");
        expect(Array.from(snapshot.key)).toEqual(KEY);
        expect(Array.from(snapshot.daemonId)).toEqual(DAEMON_ID);
        expect(snapshot.pid).toBe(4_242);
        expect(snapshot.daemonVer).toBe("eidnara-host/0.1.0");
        expect(Object.isFrozen(snapshot)).toBe(true);
    });

    test("accepts a whitespace-padded file of exactly 65,536 bytes", async () => {
        const filePath = freshPath("padded.json");
        const body = JSON.stringify(validJson()).padEnd(MAX_CONNECTION_FILE_LEN, " ");
        expect(body.length).toBe(65_536);
        await writePrivateFile(filePath, body);
        const snapshot = await readConnectionFile(filePath, options());
        expect(snapshot.setupSocket).toBe("/tmp/eidnara-host.sock");
    });

    test("rejects 65,537 bytes as oversize, not as a JSON failure", async () => {
        const filePath = freshPath("oversize.json");
        await writePrivateFile(
            filePath,
            JSON.stringify(validJson()).padEnd(MAX_CONNECTION_FILE_LEN + 1, " "),
        );
        await expectFailure(filePath, "oversize");
    });

    test("rejects a directory", async () => {
        const dirPath = freshPath("a-directory");
        await mkdir(dirPath, { mode: 0o700 });
        await expectFailure(dirPath, "not_regular_file");
    });

    test("rejects a FIFO without hanging", async () => {
        const fifoPath = freshPath("a-fifo");
        const created = await execFileAsync("mkfifo", ["-m", "600", fifoPath]).then(
            () => true,
            () => false,
        );
        if (!created) return;
        await expectFailure(fifoPath, "not_regular_file");
    });

    test("rejects a symlink at the connection-file path", async () => {
        const target = freshPath("link-target.json");
        await writePrivateFile(target, JSON.stringify(validJson()));
        const linkPath = freshPath("untrusted-link.json");
        await symlink(target, linkPath);
        await expectFailure(linkPath, "not_regular_file");
    });

    test("rejects any group or other permission bit", async () => {
        for (const mode of [0o644, 0o640, 0o604, 0o601]) {
            const filePath = freshPath(`mode-${mode.toString(8)}.json`);
            await writeFile(filePath, JSON.stringify(validJson()), { mode });
            await expectFailure(filePath, "insecure_permissions");
        }
    });

    test("rejects a file owned by another user", async () => {
        const filePath = freshPath("foreign.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        const uid = (process.getuid?.() ?? 0) + 1;
        // The injected uid trips the ancestor owner check on the user-owned temp dir before the descriptor check;
        // both return `foreign_owner`.
        await expectFailure(filePath, "foreign_owner", { uid });
    });

    test("rejects a parent directory with any group or other permission bit", async () => {
        // A directory another user can write lets that user rename over the canonical name.
        for (const mode of [0o770, 0o750, 0o705, 0o701]) {
            const dirPath = freshPath(`dir-mode-${mode.toString(8)}`);
            await mkdir(dirPath, { mode: 0o700 });
            await chmod(dirPath, mode);
            const filePath = path.join(dirPath, "connection.json");
            await writePrivateFile(filePath, JSON.stringify(validJson()));
            await expectFailure(filePath, "insecure_permissions");
        }
    });

    test("rejects a symlink as the parent directory", async () => {
        const realDir = freshPath("real-run-dir");
        await mkdir(realDir, { mode: 0o700 });
        await writePrivateFile(path.join(realDir, "connection.json"), JSON.stringify(validJson()));
        const linkDir = freshPath("linked-run-dir");
        await symlink(realDir, linkDir);
        await expectFailure(path.join(linkDir, "connection.json"), "not_directory");
    });

    test("accepts an owner-only parent directory", async () => {
        const dirPath = freshPath("private-run-dir");
        await mkdir(dirPath, { mode: 0o700 });
        const filePath = path.join(dirPath, "connection.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        const snapshot = await readConnectionFile(filePath, options());
        expect(snapshot.pid).toBe(4_242);
    });

    test("rejects a group- or other-writable ancestor above the parent", async () => {
        // An ancestor writable by another user lets that user rename a whole subtree into place.
        for (const mode of [0o777, 0o775, 0o757, 0o722, 0o702]) {
            const grandparent = freshPath(`loose-ancestor-${mode.toString(8)}`);
            const dirPath = path.join(grandparent, "run");
            await mkdir(dirPath, { recursive: true, mode: 0o700 });
            const filePath = path.join(dirPath, "connection.json");
            await writePrivateFile(filePath, JSON.stringify(validJson()));
            await chmod(grandparent, mode);
            await expectFailure(filePath, "insecure_permissions");
        }
    });

    test("accepts a world-writable sticky ancestor and a group-readable ancestor", async () => {
        // A sticky directory restricts renames to the entry owner, directory owner, or root, so it cannot be used to swap the subtree.
        // Read or search bits for others on an ancestor grant no rename ability.
        for (const mode of ["1777", "755", "750", "705"]) {
            const grandparent = freshPath(`safe-ancestor-${mode}`);
            const dirPath = path.join(grandparent, "run");
            await mkdir(dirPath, { recursive: true, mode: 0o700 });
            const filePath = path.join(dirPath, "connection.json");
            await writePrivateFile(filePath, JSON.stringify(validJson()));
            // Bun's `fs.chmod` masks the mode to `0o777` and drops the sticky bit; chmod(1) sets it.
            await execFileAsync("chmod", [mode, grandparent]);
            const snapshot = await readConnectionFile(filePath, options());
            expect(snapshot.pid).toBe(4_242);
        }
    });

    test("rejects a symlinked ancestor above the parent", async () => {
        const realGrandparent = freshPath("real-ancestor");
        const realDir = path.join(realGrandparent, "run");
        await mkdir(realDir, { recursive: true, mode: 0o700 });
        await writePrivateFile(path.join(realDir, "connection.json"), JSON.stringify(validJson()));
        const linkGrandparent = freshPath("linked-ancestor");
        await symlink(realGrandparent, linkGrandparent);
        await expectFailure(path.join(linkGrandparent, "run", "connection.json"), "not_directory");
    });

    test("rejects an ancestor owned by neither the current user nor root", async () => {
        // Only the temp-dir chain is user-owned; root-owned ancestors such as `/` remain acceptable under the injected uid.
        const dirPath = freshPath("foreign-ancestor");
        await mkdir(path.join(dirPath, "run"), { recursive: true, mode: 0o700 });
        const filePath = path.join(dirPath, "run", "connection.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        const uid = (process.getuid?.() ?? 0) + 1;
        await expectFailure(filePath, "foreign_owner", { uid });
    });

    test("resolves a relative path against the working directory before the ancestor walk", async () => {
        const dirPath = freshPath("relative-run-dir");
        await mkdir(dirPath, { mode: 0o700 });
        const filePath = path.join(dirPath, "connection.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        const snapshot = await readConnectionFile(
            path.relative(process.cwd(), filePath),
            options(),
        );
        expect(snapshot.pid).toBe(4_242);
    });

    test("fails closed without a restart when the mode is relaxed after the read", async () => {
        // A mode of 0644 after the read means the key bytes were exposed while the snapshot held them.
        const filePath = freshPath("relaxed-after-read.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        let attempts = 0;
        const afterRead = async (): Promise<void> => {
            attempts += 1;
            await chmod(filePath, 0o644);
        };
        await expectFailure(filePath, "insecure_permissions", { afterRead });
        expect(attempts).toBe(1);
    });

    test("restarts once after an in-place rewrite during the read and returns the rewritten content", async () => {
        // An in-place rewrite keeps the inode, so identity alone would accept a torn or superseded snapshot.
        // The post-read descriptor stat must see the same size, mtime, and ctime as the pre-read stat.
        const filePath = freshPath("rewritten-in-place.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        let attempts = 0;
        const afterRead = async (): Promise<void> => {
            attempts += 1;
            if (attempts > 1) return;
            await writePrivateFile(
                filePath,
                JSON.stringify(validJson({ setup_socket: "/tmp/eidnara-host-rewritten.sock" })),
            );
        };
        const snapshot = await readConnectionFile(filePath, options({ afterRead }));
        expect(attempts).toBe(2);
        expect(snapshot.setupSocket).toBe("/tmp/eidnara-host-rewritten.sock");
    });

    test("fails closed on a second in-place rewrite", async () => {
        const filePath = freshPath("rewritten-twice.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        let attempts = 0;
        const afterRead = async (): Promise<void> => {
            attempts += 1;
            // Each rewrite changes the byte length so the size comparison detects it even within one timestamp tick.
            await writePrivateFile(
                filePath,
                JSON.stringify(
                    validJson({ setup_socket: `/tmp/eidnara-host-${"x".repeat(attempts)}.sock` }),
                ),
            );
        };
        await expectFailure(filePath, "replaced_during_read", { afterRead });
        expect(attempts).toBe(2);
    });

    test("fails closed when the directory entry is renamed over after the read", async () => {
        // The descriptor still names the original inode, so only the entry `lstat` can detect the swap.
        const filePath = freshPath("swapped-after-read.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        const afterRead = async (): Promise<void> => {
            const replacement = freshPath("replacement-after-read.json");
            await writePrivateFile(replacement, JSON.stringify(validJson()));
            await rename(replacement, filePath);
        };
        await expectFailure(filePath, "replaced_during_read", { afterRead });
    });

    test("permits exactly one restart after an atomic replacement", async () => {
        const filePath = freshPath("replaced-once.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        let attempts = 0;
        const afterOpen = async (): Promise<void> => {
            attempts += 1;
            if (attempts > 1) return;
            const replacement = freshPath("replacement.json");
            await writePrivateFile(
                replacement,
                JSON.stringify(validJson({ setup_socket: "/tmp/eidnara-host-replacement.sock" })),
            );
            await rename(replacement, filePath);
        };
        const snapshot = await readConnectionFile(filePath, options({ afterOpen }));
        expect(attempts).toBe(2);
        expect(snapshot.setupSocket).toBe("/tmp/eidnara-host-replacement.sock");
    });

    test("fails closed on a second replacement", async () => {
        const filePath = freshPath("replaced-twice.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        const afterOpen = async (): Promise<void> => {
            const replacement = freshPath("replacement.json");
            await writePrivateFile(replacement, JSON.stringify(validJson()));
            await rename(replacement, filePath);
        };
        await expectFailure(filePath, "replaced_during_read", { afterOpen });
    });

    test("classifies a missing file as not_found discovery churn", async () => {
        // ENOENT must map to a retryable `ConnectionFileError` so recovery continues during republication.
        // The retry allowlist must include ENOENT so recovery does not stop during republication.
        // The daemon can unlink its published file before the replacement lands.
        await expectFailure(freshPath("absent.json"), "not_found");
    });

    test("classifies a regular file above the parent as not_directory, not churn", async () => {
        const filePath = freshPath("not-a-dir.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        // `not_directory` is permanent and stops recovery instead of retrying until the deadline.
        await expectFailure(path.join(filePath, "sub", "child.json"), "not_directory");
    });

    test("classifies a permanent stat failure as stat_failed, not churn", async () => {
        // When `process.getuid?.() === 0`, root bypasses directory search permission, so this test cannot exercise EACCES.
        if (process.getuid?.() === 0) return;
        const dirPath = freshPath("unsearchable-run-dir");
        await mkdir(dirPath, { mode: 0o700 });
        const filePath = path.join(dirPath, "connection.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        // The parent passes the owner-only check, then the file `lstat` fails with EACCES.
        // EACCES is permanent and stops recovery instead of retrying until the deadline.
        await chmod(dirPath, 0o600);
        try {
            await expectFailure(filePath, "stat_failed");
        } finally {
            await chmod(dirPath, 0o700);
        }
    });

    test("classifies a regular file as the immediate parent as not_directory", async () => {
        const filePath = freshPath("parent-is-a-file.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        await expectFailure(path.join(filePath, "child.json"), "not_directory");
    });

    test("classifies a permanent open failure as open_failed, not churn", async () => {
        // When `process.getuid?.() === 0`, root can open mode-000 files, so this test cannot exercise EACCES.
        if (process.getuid?.() === 0) return;
        const filePath = freshPath("unreadable.json");
        await writeFile(filePath, JSON.stringify(validJson()), { mode: 0o000 });
        // The pre-open `lstat` succeeds, then `open(2)` fails with EACCES.
        // EACCES is permanent evidence outside the retryable churn classes.
        await expectFailure(filePath, "open_failed");
    });

    test("classifies an unlink during the snapshot as discovery churn", async () => {
        const filePath = freshPath("unlinked-mid-read.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        const afterOpen = async (): Promise<void> => {
            await rm(filePath, { force: true });
        };
        // After removal, the post-read `stat` reports `replaced_during_read`.
        // The one-restart rule retries `replaced_during_read`.
        // The restart's initial `stat` reports `not_found` while the file remains absent.
        // `replaced_during_read` and `not_found` are retryable churn codes.
        await expectFailure(filePath, "not_found", { afterOpen });
    });

    test("fails closed on win32 before any filesystem work", async () => {
        await expectFailure(path.join(tmpDir, "never-touched.json"), "unsupported_platform", {
            platform: "win32",
        });
    });

    test("fails closed on an already-expired deadline", async () => {
        const filePath = freshPath("deadline.json");
        await writePrivateFile(filePath, JSON.stringify(validJson()));
        await expectFailure(filePath, "deadline_expired", {
            deadline: Deadline.start(0, () => 0),
        });
    });
});

describe("snapshot JSON validation", () => {
    test("rejects invalid UTF-8 bytes", async () => {
        const filePath = freshPath("bad-utf8.json");
        await writePrivateFile(filePath, Uint8Array.from([0x7b, 0xff, 0xfe, 0x7d]));
        await expectFailure(filePath, "invalid_utf8");
    });

    test("rejects malformed JSON and non-object roots", async () => {
        const badJson = freshPath("not-json.json");
        await writePrivateFile(badJson, "{nope");
        await expectFailure(badJson, "invalid_json");
        const arrayRoot = freshPath("array-root.json");
        await writePrivateFile(arrayRoot, "[1,2,3]");
        await expectFailure(arrayRoot, "invalid_json");
    });

    test("an invalid_json failure retains no parse error that could quote key bytes", async () => {
        // V8's SyntaxError message quotes the source text around the fault, so a malformed
        // publication with an intact key array would carry key bytes through `cause`.
        const filePath = freshPath("malformed-with-key.json");
        const malformed = JSON.stringify(validJson()).replace('],"daemon_id"', ',],"daemon_id"');
        await writePrivateFile(filePath, malformed);
        const error = await readConnectionFile(filePath, options()).then(
            () => {
                throw new Error("readConnectionFile unexpectedly succeeded");
            },
            (thrown: unknown) => thrown as ConnectionFileError,
        );
        expect(error).toBeInstanceOf(ConnectionFileError);
        expect(error.code).toBe("invalid_json");
        expect(error.cause).toBeUndefined();
        expect(error.message).not.toContain(String(KEY[KEY.length - 1]));
    });

    test("rejects a missing or wrong schema", async () => {
        await writeInvalid(validJson({ schema: 1 }), "invalid_schema");
        await writeInvalid(validJson({ schema: "2" }), "invalid_schema");
        const noSchema = validJson();
        delete noSchema.schema;
        await writeInvalid(noSchema, "invalid_schema");
    });

    test("requires wire_version to be exactly 2 and rejects every other value", async () => {
        const absent = validJson();
        delete absent.wire_version;
        await writeInvalid(absent, "invalid_wire_version");
        await writeInvalid(validJson({ wire_version: 3 }), "invalid_wire_version");
        await writeInvalid(validJson({ wire_version: null }), "invalid_wire_version");
        await writeInvalid(validJson({ wire_version: "2" }), "invalid_wire_version");
        await writeInvalid(validJson({ wire_version: 1 }), "invalid_wire_version");
    });

    test("rejects missing, relative, and non-string setup socket paths", async () => {
        const absent = validJson();
        delete absent.setup_socket;
        await writeInvalid(absent, "invalid_setup_socket");
        await writeInvalid(validJson({ setup_socket: "relative.sock" }), "invalid_setup_socket");
        await writeInvalid(validJson({ setup_socket: "" }), "invalid_setup_socket");
        await writeInvalid(validJson({ setup_socket: 43123 }), "invalid_setup_socket");
    });

    test("rejects key and daemon_id byte-array violations", async () => {
        await writeInvalid(validJson({ key: KEY.slice(0, 31) }), "invalid_key");
        await writeInvalid(validJson({ key: [...KEY, 0] }), "invalid_key");
        await writeInvalid(validJson({ key: [1.5, ...KEY.slice(1)] }), "invalid_key");
        await writeInvalid(validJson({ key: [-1, ...KEY.slice(1)] }), "invalid_key");
        await writeInvalid(validJson({ key: [256, ...KEY.slice(1)] }), "invalid_key");
        await writeInvalid(validJson({ key: [null, ...KEY.slice(1)] }), "invalid_key");
        await writeInvalid(validJson({ key: "not-an-array" }), "invalid_key");
        await writeInvalid(validJson({ daemon_id: DAEMON_ID.slice(0, 15) }), "invalid_daemon_id");
        await writeInvalid(
            validJson({ daemon_id: [256, ...DAEMON_ID.slice(1)] }),
            "invalid_daemon_id",
        );
    });

    test("rejects an unsafe or missing pid", async () => {
        await writeInvalid(validJson({ pid: 2 ** 53 }), "invalid_pid");
        await writeInvalid(validJson({ pid: "4242" }), "invalid_pid");
        await writeInvalid(validJson({ pid: 0 }), "invalid_pid");
        await writeInvalid(validJson({ pid: -1 }), "invalid_pid");
        const noPid = validJson();
        delete noPid.pid;
        await writeInvalid(noPid, "invalid_pid");
    });

    test("rejects an empty or missing daemon_ver", async () => {
        await writeInvalid(validJson({ daemon_ver: "" }), "invalid_daemon_ver");
        await writeInvalid(validJson({ daemon_ver: 7 }), "invalid_daemon_ver");
    });
});

describe("toExactByteArray", () => {
    test("accepts an exact-length dense integer array", () => {
        const bytes = toExactByteArray([0, 128, 255], 3);
        expect(bytes).not.toBeNull();
        expect(Array.from(bytes as Uint8Array)).toEqual([0, 128, 255]);
    });

    test("rejects sparse arrays even when length matches", () => {
        const sparse = new Array(3);
        sparse[0] = 1;
        sparse[2] = 2;
        expect(toExactByteArray(sparse, 3)).toBeNull();
        expect(toExactByteArray(new Array(32), 32)).toBeNull();
    });

    test("rejects wrong length, fractions, negatives, over-255, and non-numbers", () => {
        expect(toExactByteArray([1, 2], 3)).toBeNull();
        expect(toExactByteArray([1, 2, 3, 4], 3)).toBeNull();
        expect(toExactByteArray([1.5, 0, 0], 3)).toBeNull();
        expect(toExactByteArray([-1, 0, 0], 3)).toBeNull();
        expect(toExactByteArray([256, 0, 0], 3)).toBeNull();
        expect(toExactByteArray([null, 0, 0], 3)).toBeNull();
        expect(toExactByteArray(["1", 0, 0], 3)).toBeNull();
        expect(toExactByteArray("123", 3)).toBeNull();
    });
});

import { describe, expect, test } from "bun:test";

import {
    type CargoMetadata,
    forbiddenDependencyEdges,
    type MetadataDependency,
} from "./forbid-test-support-dependencies";

function dep(
    overrides: Partial<MetadataDependency> & { name: string },
): MetadataDependency {
    return { kind: null, features: [], target: null, ...overrides };
}

function workspace(dependencies: MetadataDependency[]): CargoMetadata {
    return {
        packages: [
            { name: "daemon", source: null, dependencies },
            {
                name: "serde",
                source: "registry+https://github.com/rust-lang/crates.io-index",
                dependencies: [
                    dep({ name: "serde_derive", features: ["test-support"] }),
                ],
            },
        ],
    };
}

describe("forbiddenDependencyEdges", () => {
    test("accepts test-support and eval-core under [dev-dependencies]", () => {
        const metadata = workspace([
            dep({ name: "kernel" }),
            dep({ name: "kernel", kind: "dev", features: ["test-support"] }),
            dep({ name: "eval-core", kind: "dev" }),
            dep({ name: "storage", features: ["sqlite"] }),
        ]);
        expect(forbiddenDependencyEdges(metadata)).toEqual([]);
    });

    test("rejects a test-support feature under [dependencies]", () => {
        const metadata = workspace([
            dep({ name: "kernel", features: ["test-support"] }),
        ]);
        expect(forbiddenDependencyEdges(metadata)).toEqual([
            "daemon [dependencies] kernel enables test-support",
        ]);
    });

    test("rejects a test-support feature under [build-dependencies] and target tables", () => {
        const metadata = workspace([
            dep({ name: "storage", kind: "build", features: ["test-support"] }),
            dep({
                name: "memory-store",
                target: "cfg(unix)",
                features: ["test-support"],
            }),
        ]);
        expect(forbiddenDependencyEdges(metadata)).toEqual([
            "daemon [build-dependencies] storage enables test-support",
            "daemon [target.cfg(unix).dependencies] memory-store enables test-support",
        ]);
    });

    test("rejects eval-core outside [dev-dependencies]", () => {
        const metadata = workspace([dep({ name: "eval-core" })]);
        expect(forbiddenDependencyEdges(metadata)).toEqual([
            "daemon [dependencies] depends on dev-only package eval-core",
        ]);
    });

    test("ignores registry packages", () => {
        expect(forbiddenDependencyEdges(workspace([]))).toEqual([]);
    });

    test("accepts a test-support feature that default does not reach", () => {
        const metadata = workspace([dep({ name: "host-runtime" })]);
        metadata.packages[0]!.features = {
            "direct-host-fixture": [],
            "test-support": ["host-runtime/test-support"],
        };
        expect(forbiddenDependencyEdges(metadata)).toEqual([]);
    });

    test("rejects a default feature that reaches test-support through the feature table", () => {
        const metadata = workspace([dep({ name: "host-runtime" })]);
        metadata.packages[0]!.features = {
            default: ["fixtures"],
            fixtures: ["dep:host-runtime", "test-support"],
            "test-support": ["host-runtime/test-support"],
        };
        expect(forbiddenDependencyEdges(metadata)).toEqual([
            "daemon [features] default reaches host-runtime/test-support through test-support",
        ]);
    });
});

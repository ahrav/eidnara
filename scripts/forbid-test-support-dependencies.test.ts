import { describe, expect, test } from "bun:test";

import {
    type CargoMetadata,
    forbiddenCoreSources,
    forbiddenDependencyEdges,
    productCrates,
    type MetadataDependency,
} from "./forbid-test-support-dependencies";

function dep(
    overrides: Partial<MetadataDependency> & { name: string },
): MetadataDependency {
    return { kind: null, features: [], target: null, ...overrides };
}

function workspace(dependencies: MetadataDependency[]): CargoMetadata {
    return {
        workspace_root: "/workspace",
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
            "daemon [features] default enables test-support",
            "daemon [features] default reaches host-runtime/test-support through test-support",
        ]);
    });

    test("rejects a default feature that enables the package's own leaf test-support", () => {
        const metadata = workspace([]);
        metadata.packages[0]!.features = {
            default: ["test-support"],
            "test-support": [],
        };
        expect(forbiddenDependencyEdges(metadata)).toEqual([
            "daemon [features] default enables test-support",
        ]);
    });

    test("rejects a forwarding feature enabled from another package's normal table", () => {
        const metadata = workspace([dep({ name: "host-runtime" })]);
        metadata.packages[0]!.features = {
            "direct-host-fixture": ["host-runtime/test-support"],
            bench: ["fixtures"],
            fixtures: ["test-support"],
            "test-support": [],
        };
        metadata.packages.push({
            name: "cli",
            source: null,
            dependencies: [
                dep({ name: "daemon", features: ["direct-host-fixture"] }),
                dep({ name: "daemon", kind: "build", features: ["bench"] }),
                dep({ name: "daemon", kind: "dev", features: ["bench"] }),
            ],
        });
        expect(forbiddenDependencyEdges(metadata)).toEqual([
            "cli [build-dependencies] daemon enables test-support through bench",
            "cli [dependencies] daemon reaches host-runtime/test-support through direct-host-fixture",
        ]);
    });

    test("follows dependency-feature forwarding through an intermediate local package", () => {
        const metadata = workspace([dep({ name: "host-runtime", features: ["fixtures"] })]);
        metadata.packages.push(
            {
                name: "host-runtime",
                source: null,
                dependencies: [dep({ name: "storage" })],
                features: { fixtures: ["storage/fixtures"], "test-support": [] },
            },
            {
                name: "storage",
                source: null,
                dependencies: [],
                features: { fixtures: ["test-support"], "test-support": [] },
            },
        );
        expect(forbiddenDependencyEdges(metadata)).toEqual([
            "daemon [dependencies] host-runtime enables storage/test-support through fixtures",
        ]);
    });

    test("resolves a forwarding entry through a renamed local dependency", () => {
        const metadata = workspace([dep({ name: "host-runtime", features: ["fixtures"] })]);
        metadata.packages.push(
            {
                name: "host-runtime",
                source: null,
                dependencies: [dep({ name: "storage", rename: "store" })],
                features: { fixtures: ["store/fixtures"], "test-support": [] },
            },
            {
                name: "storage",
                source: null,
                dependencies: [],
                features: { fixtures: ["test-support"], "test-support": [] },
            },
        );
        expect(forbiddenDependencyEdges(metadata)).toEqual([
            "daemon [dependencies] host-runtime enables storage/test-support through fixtures",
        ]);
    });

    test("does not read a dep: entry as the same-named explicit feature", () => {
        const metadata = workspace([dep({ name: "x" })]);
        metadata.packages[0]!.features = {
            default: ["activate"],
            activate: ["dep:x"],
            x: ["test-support"],
            "test-support": [],
        };
        expect(forbiddenDependencyEdges(metadata)).toEqual([]);
    });

    test("resolves edge features only against local packages", () => {
        const metadata = workspace([dep({ name: "serde", features: ["derive"] })]);
        metadata.packages[1]!.features = { derive: ["serde_derive/test-support"] };
        expect(forbiddenDependencyEdges(metadata)).toEqual([]);
    });
});

describe("eval-core fences", () => {
    const crates = ["kernel", "daemon", "storage"];
    const core = (dependencies: MetadataDependency[]): CargoMetadata => ({
        workspace_root: "/workspace",
        packages: [{ name: "eval-core", source: null, dependencies }],
    });
    const closed = [
        dep({ name: "context-core" }),
        dep({ name: "serde" }),
        dep({ name: "serde_json" }),
        dep({ name: "sha2" }),
    ];

    test("accepts exactly the closed dependency set plus dev-dependencies", () => {
        expect(forbiddenDependencyEdges(core([...closed, dep({ name: "proptest", kind: "dev" })]))).toEqual([]);
    });

    test("rejects a kernel edge and a missing member of the closed set", () => {
        expect(forbiddenDependencyEdges(core([...closed, dep({ name: "kernel" })]))).toEqual([
            "eval-core [dependencies] names kernel outside its closed set",
        ]);
        expect(forbiddenDependencyEdges(core(closed.slice(1)))).toEqual([
            "eval-core [dependencies] lacks context-core from its closed set",
        ]);
    });

    test("rejects a build script on eval-core", () => {
        const metadata = core(closed);
        metadata.packages[0]!.targets = [
            { name: "eval-core", kind: ["lib"] },
            { name: "build-script-build", kind: ["custom-build"] },
        ];
        expect(forbiddenDependencyEdges(metadata)).toEqual([
            "eval-core [package] has a build script",
        ]);
    });

    test("rejects product-crate paths and std effect modules in core source", () => {
        expect(
            forbiddenCoreSources({
                "a.rs": "use kernel::EligibilityVerdict;\nlet t = std::time::Instant::now();\nstd::os::unix::fs::symlink(a, b);\n",
                "b.rs": "use std::collections::BTreeMap;\nlet p = std::path::Path::new(\"x\");\n",
                "c.rs": "use serde::Serialize;\n",
            }, crates),
        ).toEqual([
            "a.rs:1: use kernel::EligibilityVerdict;",
            "a.rs:2: let t = std::time::Instant::now();",
            "a.rs:3: std::os::unix::fs::symlink(a, b);",
            "b.rs:2: let p = std::path::Path::new(\"x\");",
        ]);
    });

    test("rejects std effect modules named inside a brace-grouped use", () => {
        expect(
            forbiddenCoreSources({
                "grouped.rs": [
                    "use std::{fs, io};",
                    "use std::{collections::BTreeMap, time::Instant};",
                    "use std::{collections::{BTreeMap, BTreeSet}, fmt};",
                    "use std::collections::{BTreeMap, BTreeSet};",
                    "",
                ].join("\n"),
            }, crates),
        ).toEqual([
            "grouped.rs:1: use std::{fs, io};",
            "grouped.rs:2: use std::{collections::BTreeMap, time::Instant};",
        ]);
    });

    test("rejects std effect modules in a rustfmt-wrapped brace-grouped use", () => {
        expect(
            forbiddenCoreSources({
                "wrapped.rs": [
                    "use std::{",
                    "    collections::HashMap,",
                    "    fs,",
                    "};",
                    "use std::{",
                    "    collections::{BTreeMap, BTreeSet},",
                    "    fmt,",
                    "};",
                    "",
                ].join("\n"),
            }, crates),
        ).toEqual(["wrapped.rs:1: use std::{"]);
    });

    test("rejects renaming the std root, which would hide effect paths", () => {
        expect(
            forbiddenCoreSources({
                "alias.rs": [
                    "use std as standard;",
                    "use ::std as s;",
                    "use std::{self as st, fmt};",
                    "extern crate std as core_std;",
                    "use {serde::Serialize, std as s};",
                    "use std::*;",
                    "use std::{collections::BTreeMap, *};",
                    "let bytes = standard::fs::read(\"x\");",
                    "use std::collections::HashMap as Map;",
                    "use std::collections::*;",
                    "",
                ].join("\n"),
            }, crates),
        ).toEqual([
            "alias.rs:1: use std as standard;",
            "alias.rs:2: use ::std as s;",
            "alias.rs:3: use std::{self as st, fmt};",
            "alias.rs:4: extern crate std as core_std;",
            "alias.rs:5: use {serde::Serialize, std as s};",
            "alias.rs:6: use std::*;",
            "alias.rs:7: use std::{collections::BTreeMap, *};",
        ]);
    });

    test("rejects every workspace crate outside the closed set, not a fixed list", () => {
        expect(
            forbiddenCoreSources(
                { "a.rs": "#[cfg(test)]\nuse shm_transport::Frame;\nlet l = lease::Lease::new();\n" },
                ["kernel", "shm_transport", "lease"],
            ),
        ).toEqual(["a.rs:2: use shm_transport::Frame;", "a.rs:3: let l = lease::Lease::new();"]);
    });

    test("rejects renaming a product crate, by extern crate or a use group", () => {
        expect(
            forbiddenCoreSources(
                {
                    "a.rs": [
                        "#[cfg(test)]",
                        "extern crate kernel as k;",
                        "use {serde::Serialize, storage as st};",
                        "let s = k::Surface::AutoInject;",
                        "",
                    ].join("\n"),
                },
                crates,
            ),
        ).toEqual(["a.rs:2: extern crate kernel as k;", "a.rs:3: use {serde::Serialize, storage as st};"]);
    });

    test("fences every local package and eval-core's Cargo renames of them", () => {
        const metadata: CargoMetadata = {
            workspace_root: "/workspace",
            packages: [
                {
                    name: "eval-core",
                    source: null,
                    dependencies: [
                        ...closed,
                        dep({ name: "storage", rename: "engine", kind: "dev" }),
                        dep({ name: "tokio", rename: "rt", kind: "dev" }),
                        dep({ name: "serde_json", rename: "sj", kind: "dev" }),
                        dep({ name: "proptest", rename: "pt", kind: "dev" }),
                    ],
                },
                { name: "context-core", source: null, dependencies: [] },
                { name: "shm-transport", source: null, dependencies: [] },
                { name: "storage", source: null, dependencies: [] },
                { name: "serde", source: "registry+x", dependencies: [] },
            ],
        };
        expect(productCrates(metadata).sort()).toEqual(["engine", "rt", "shm_transport", "storage"]);
    });

    test("rejects pulling source from outside the scanned tree", () => {
        expect(
            forbiddenCoreSources(
                {
                    "a.rs": [
                        "#[path = \"../effects.rs\"]",
                        "mod effects;",
                        "include!(\"../effects.rs\");",
                        "const SPEC: &str = include_str!(\"spec.json\");",
                        "const BLOB: &[u8] = include_bytes!(\"blob.bin\");",
                        "let included = true;",
                        "",
                    ].join("\n"),
                },
                crates,
            ),
        ).toEqual([
            "a.rs:1: #[path = \"../effects.rs\"]",
            "a.rs:3: include!(\"../effects.rs\");",
            "a.rs:4: const SPEC: &str = include_str!(\"spec.json\");",
            "a.rs:5: const BLOB: &[u8] = include_bytes!(\"blob.bin\");",
        ]);
    });

    test("rejects unaliased extern crate and the stdio macros", () => {
        expect(
            forbiddenCoreSources(
                {
                    "a.rs": [
                        "#[macro_use]",
                        "extern crate kernel;",
                        "extern crate tokio;",
                        "extern crate serde;",
                        "println!(\"{x}\");",
                        "let d = dbg!(x);",
                        "eprint!(\"e\");",
                        "let s = format!(\"{x}\");",
                        "write!(out, \"{x}\")?;",
                        "",
                    ].join("\n"),
                },
                crates,
            ),
        ).toEqual([
            "a.rs:2: extern crate kernel;",
            "a.rs:3: extern crate tokio;",
            "a.rs:5: println!(\"{x}\");",
            "a.rs:6: let d = dbg!(x);",
            "a.rs:7: eprint!(\"e\");",
        ]);
    });

    test("refuses to pass on an empty source set", () => {
        expect(forbiddenCoreSources({}, crates)).toEqual([
            "crates/eval-core/src: no source files scanned",
        ]);
    });
});

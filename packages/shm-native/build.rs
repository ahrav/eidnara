//! Links the N-API addon and stamps it with a digest of the sources it is built from.
//!
//! The digest is FNV-1a (32-bit) over every file listed in [`SOURCE_INPUTS`], walked in
//! sorted workspace-relative path order, hashing each path, a NUL, its bytes, and a NUL. The
//! TypeScript witness recomputes the same digest from the checkout, so an addon built from
//! other sources fails the claimed-target check instead of passing as current.

use std::path::{Path, PathBuf};

/// Workspace-relative inputs that shape the addon's code: the addon and the two workspace
/// crates it links, their build scripts and the tokenizer's embedded assets, and the
/// workspace manifest, lockfile, toolchain, and Cargo configuration. Directories are walked
/// recursively. `tests/source-digest.ts` reads this list from this file.
const SOURCE_INPUTS: &[&str] = &[
    ".cargo/config.toml",
    "Cargo.lock",
    "Cargo.toml",
    "crates/shm-transport/Cargo.toml",
    "crates/shm-transport/src",
    "crates/tokenizer/Cargo.toml",
    "crates/tokenizer/assets",
    "crates/tokenizer/build.rs",
    "crates/tokenizer/src",
    "packages/shm-native/Cargo.toml",
    "packages/shm-native/build.rs",
    "packages/shm-native/src",
    "rust-toolchain.toml",
];

fn collect(root: &Path, relative: &Path, files: &mut Vec<PathBuf>) {
    let path = root.join(relative);
    if path.is_dir() {
        let entries = std::fs::read_dir(&path)
            .expect("source directory is readable")
            .map(|entry| relative.join(entry.expect("source entry is readable").file_name()));
        for entry in entries {
            collect(root, &entry, files);
        }
    } else if path.is_file() {
        files.push(relative.to_path_buf());
    }
}

fn fnv1a(hash: &mut u32, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u32::from(*byte);
        *hash = hash.wrapping_mul(0x0100_0193);
    }
}

fn main() {
    napi_build::setup();
    let manifest =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("the addon sits two levels below the workspace root");
    let mut files = Vec::new();
    for input in SOURCE_INPUTS {
        println!("cargo:rerun-if-changed={}", root.join(input).display());
        collect(root, Path::new(input), &mut files);
    }
    files.sort();
    let mut hash = 0x811c_9dc5_u32;
    for file in &files {
        let name = file.to_str().expect("source paths are UTF-8");
        fnv1a(&mut hash, name.as_bytes());
        fnv1a(&mut hash, &[0]);
        fnv1a(
            &mut hash,
            &std::fs::read(root.join(file)).expect("source file is readable"),
        );
        fnv1a(&mut hash, &[0]);
    }
    println!("cargo:rustc-env=EIDNARA_SHM_NATIVE_SOURCE_DIGEST={hash:08x}");
}

use std::process::Command;

/// Whether the host's glibc exports `mallinfo2`, read from `getconf GNU_LIBC_VERSION`.
pub fn exports_mallinfo2() -> bool {
    let output = Command::new("getconf")
        .arg("GNU_LIBC_VERSION")
        .output()
        .expect("getconf runs");
    assert!(output.status.success(), "getconf GNU_LIBC_VERSION failed");
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.split_whitespace().nth(1).expect("a glibc version");
    version_exports_mallinfo2(version)
}

/// `mallinfo2` entered glibc in 2.33; a component is read by its leading digits.
pub fn version_exports_mallinfo2(version: &str) -> bool {
    let mut parts = version.split('.').map(|part| {
        let digits = part
            .trim_start()
            .split(|c: char| !c.is_ascii_digit())
            .next();
        digits
            .and_then(|digits| digits.parse::<u32>().ok())
            .expect("a version number")
    });
    let major = parts.next().expect("major version");
    let minor = parts.next().expect("minor version");
    (major, minor) >= (2, 33)
}

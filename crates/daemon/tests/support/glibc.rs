use std::process::Command;

/// Whether the host's glibc exports `mallinfo2`, read from `getconf GNU_LIBC_VERSION`.
pub fn exports_mallinfo2() -> bool {
    let output = Command::new("getconf")
        .arg("GNU_LIBC_VERSION")
        .output()
        .expect("getconf runs");
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.split_whitespace().nth(1).expect("a glibc version");
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u32>().expect("a number"));
    let major = parts.next().expect("major version");
    let minor = parts.next().expect("minor version");
    (major, minor) >= (2, 33)
}

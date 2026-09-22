use std::path::{Path, PathBuf};

/// The staged file resides beside `path`, keeping `rename` on one filesystem.
pub fn staged_path(path: &Path) -> PathBuf {
    let mut staged = path.as_os_str().to_owned();
    staged.push(".staged");
    PathBuf::from(staged)
}

/// A reader sees the whole file or none of it. `create_new` rejects a
/// pre-existing staged entry instead of following a link or truncating a
/// file; mode `0o600` restricts the staged bytes to the owner. The staged
/// file is linked into place, which fails with `AlreadyExists` rather than
/// replacing a file that arrived at `path` meanwhile (`rename` would replace
/// it), then unlinked. The function syncs file bytes before the link and
/// directory metadata afterward; a refused link leaves the staged file for
/// the next run to refuse on.
pub fn write_then_rename(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let staged = staged_path(path);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::hard_link(&staged, path)?;
    std::fs::remove_file(&staged)?;
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    std::fs::File::open(directory.unwrap_or(Path::new(".")))?.sync_all()
}

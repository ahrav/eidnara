//! Reads the configured project documents into canonical hash and escaped XML forms.
//!
//! The user-tier `project_docs.files` list names the documents, in order; its default is
//! `ARCHITECTURE.md`, then `STRUCTURE.md`. Each path is resolved beneath the project root with
//! `openat2` under [`RESOLVE`]. A host that refuses `openat2` reads with a no-follow `openat`
//! relative to the root descriptor, restricting reads to top-level paths. Each file is limited to
//! 256 KiB, checked on the opened descriptor.

use std::io::Read as _;
use std::os::fd::{AsFd as _, BorrowedFd, OwnedFd};
use std::path::Path;

use rustix::fs::{Mode, OFlags, ResolveFlags};

/// The documents read when the user tier names none.
const DEFAULT_PROJECT_DOC_FILES: [&str; 2] = ["ARCHITECTURE.md", "STRUCTURE.md"];
/// Entries one `project_docs.files` list may hold.
const MAX_PROJECT_DOC_FILES: usize = 8;
const MAX_DOCUMENT_PATH_COMPONENTS: usize = 16;
/// Components that name no ordinary file of the tree.
const REFUSED_COMPONENTS: [&str; 3] = ["", ".", ".."];
/// The project's own Eidnara directory and its repository metadata, refused in any ASCII case so a
/// case-folding filesystem reaches neither.
const REFUSED_DIRECTORIES: [&str; 2] = [".eidnara", ".git"];
const PROJECT_DOCS_DELIMITER: &str = "\n\n---\n\n";
const MAX_PROJECT_DOC_BYTES: u64 = 256 * 1024;
/// Every component of a document path resolves as an ordinary name beneath the root, on the
/// root's device.
const RESOLVE: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

/// A configured document's path relative to the project root: `/`-separated ordinary components,
/// at most 16, none of them `.`, `..`, `.eidnara`, or `.git`, with no variable reference. The path
/// is the `relative_path` identity value of the document's source lineage, so it is also a
/// well-formed Kernel identity value: 1 to 512 bytes with no control character.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocumentPath(String);

impl DocumentPath {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The default list, shared by every test that composes a block the way the default configuration does.
#[cfg(test)]
pub(crate) fn default_document_paths_for_test() -> &'static [DocumentPath] {
    static DEFAULT: std::sync::LazyLock<Vec<DocumentPath>> =
        std::sync::LazyLock::new(default_document_paths);
    &DEFAULT
}

/// The default list.
pub(crate) fn default_document_paths() -> Vec<DocumentPath> {
    DEFAULT_PROJECT_DOC_FILES
        .into_iter()
        .map(|path| DocumentPath(path.to_owned()))
        .collect()
}

/// Admits `entries` as the configured list, or names the zero-based index of the first entry past
/// [`MAX_PROJECT_DOC_FILES`], outside the path rule, or byte-equal to an earlier entry.
pub(crate) fn admit_document_paths(entries: &[&str]) -> Result<Vec<DocumentPath>, usize> {
    let mut admitted: Vec<DocumentPath> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        if index == MAX_PROJECT_DOC_FILES
            || !admissible(entry)
            || admitted.iter().any(|path| path.0 == *entry)
        {
            return Err(index);
        }
        admitted.push(DocumentPath((*entry).to_owned()));
    }
    Ok(admitted)
}

fn admissible(entry: &str) -> bool {
    kernel::source_identity::well_formed_value(entry)
        && !crate::config::has_variable_reference(entry)
        && entry.split('/').enumerate().all(|(index, component)| {
            index < MAX_DOCUMENT_PATH_COMPONENTS
                && !REFUSED_COMPONENTS.contains(&component)
                && !REFUSED_DIRECTORIES
                    .iter()
                    .any(|refused| component.eq_ignore_ascii_case(refused))
        })
}

/// Both fields are empty when neither configured document is included.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectDocs {
    pub rendered_block: String,
    pub canonical_hash: String,
    /// Each configured document's read, in list order: the raw bytes the canonical block was built from, or why it holds none.
    pub(crate) reads: Vec<(DocumentPath, DocumentRead)>,
}

/// Where a pass leaves the configured documents it read for its prompt, for source capture to take once the response is built.
pub(crate) type DocumentReads = std::sync::Mutex<Option<Vec<(DocumentPath, DocumentRead)>>>;

/// Appends the canonical form of `raw` to `out`: the leading byte order mark dropped, each line's `\r\n` ending folded to `\n`, trailing spaces and tabs trimmed from every line, and trailing newlines dropped.
fn canonicalize_doc_content_into(raw: &str, out: &mut String) {
    let body = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let bytes = body.as_bytes();
    // Line breaks owed before the next nonempty line; a run of empty lines at the end owes breaks that are never written.
    let mut breaks = 0;
    let mut start = 0;
    let ends = memchr::memchr_iter(b'\n', bytes)
        .map(|end| (end, true))
        .chain(std::iter::once((bytes.len(), false)));
    for (end, terminated) in ends {
        let mut kept = end;
        if terminated && kept > start && bytes[kept - 1] == b'\r' {
            kept -= 1;
        }
        while kept > start && matches!(bytes[kept - 1], b' ' | b'\t') {
            kept -= 1;
        }
        if kept > start {
            for _ in 0..breaks {
                out.push('\n');
            }
            breaks = 0;
            // Both ends sit next to an ASCII byte, so the slice is on character boundaries.
            out.push_str(&body[start..kept]);
        }
        breaks += 1;
        start = end + 1;
    }
}

/// Appends `text` to `out` with `&`, `<`, and `>` escaped, and `"` too when `quote` is set.
fn escape_xml_into(text: &str, quote: bool, out: &mut String) {
    let mut rest = text;
    loop {
        let found = if quote {
            rest.find(['&', '<', '>', '"'])
        } else {
            memchr::memchr3(b'&', b'<', b'>', rest.as_bytes())
        };
        let Some(at) = found else {
            out.push_str(rest);
            return;
        };
        out.push_str(&rest[..at]);
        out.push_str(match rest.as_bytes()[at] {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            _ => "&quot;",
        });
        rest = &rest[at + 1..];
    }
}

/// Lowercase hex SHA-256 of `bytes`. `ring` uses the CPU's SHA-256 instructions where the CPU has them.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

fn hex(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(2 * digest.len());
    for &byte in digest {
        hex.push(char::from(HEX[usize::from(byte >> 4)]));
        hex.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    hex
}

/// What one read of a configured document found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DocumentRead {
    /// The raw UTF-8 text, exactly as read.
    Text(String),
    /// The path does not exist in a directory that does.
    Missing,
    /// The path is a symlink or another non-regular file.
    NotRegular,
    /// The file holds more than `MAX_PROJECT_DOC_BYTES`.
    Oversize,
    /// The bytes are not UTF-8.
    NotUtf8,
    /// The read failed for another reason, such as a permission error.
    Unreadable,
}

/// Reads `path` beneath `root` as a regular file whose bytes fit `MAX_PROJECT_DOC_BYTES` and decode as UTF-8.
#[cfg(test)]
pub(crate) fn read_document(root: &Path, path: &DocumentPath) -> DocumentRead {
    open_root(root).map_or(DocumentRead::Unreadable, |root| {
        read_beneath(root.as_fd(), path, &open_beneath)
    })
}

/// The project root as a directory descriptor every document of a pass resolves against, so one
/// pass reads one tree. `None` when the root is gone, such as an unmounted volume: no read of the
/// pass verifies a removal.
fn open_root(root: &Path) -> Option<OwnedFd> {
    rustix::fs::open(
        root,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()
}

fn open_beneath(dir: BorrowedFd<'_>, name: &str, flags: OFlags) -> rustix::io::Result<OwnedFd> {
    rustix::fs::openat2(dir, name, flags, Mode::empty(), RESOLVE)
}

/// Treats errors consistent with syscall unavailability, unsupported resolve flags, or seccomp
/// denial as a host-level refusal of `openat2`. These error codes can also arise from other causes.
fn openat2_refused(errno: rustix::io::Errno) -> bool {
    use rustix::io::Errno;
    matches!(
        errno,
        Errno::NOSYS | Errno::PERM | Errno::INVAL | Errno::OPNOTSUPP
    )
}

/// An `openat2` call relative to a directory descriptor, replaceable in tests by one that fails.
type OpenBeneath<'a> = &'a dyn Fn(BorrowedFd<'_>, &str, OFlags) -> rustix::io::Result<OwnedFd>;

/// Reads `path` beneath `root`.
///
/// The parent directory is opened first and the final component relative to it, both under
/// [`RESOLVE`], so `Missing` means the final component is absent from a parent that resolves
/// beneath the root. A parent that is absent, a symlink, across a mount, or unopenable is
/// `Unreadable`; a final-component symlink is `NotRegular`. On a host that refuses `openat2`
/// itself, a top-level path opens with `openat` relative to the root descriptor and every nested
/// path is `Unreadable`.
fn read_beneath(root: BorrowedFd<'_>, path: &DocumentPath, open: OpenBeneath<'_>) -> DocumentRead {
    let (parent, name) = match path.as_str().rsplit_once('/') {
        Some((parent, name)) => (Some(parent), name),
        None => (None, path.as_str()),
    };
    let opened_parent;
    let dir = match parent {
        None => root,
        Some(parent) => match open(
            root,
            parent,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        ) {
            Ok(fd) => {
                opened_parent = fd;
                opened_parent.as_fd()
            }
            Err(_) => return DocumentRead::Unreadable,
        },
    };
    // A FIFO or device at a project-controlled path would block the open or the read; `NONBLOCK` keeps the open from waiting on a writer, and the descriptor's own type settles what was reached before any byte is read.
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let opened = match open(dir, name, flags) {
        // The single-component fallback resolves one ordinary entry relative to the root descriptor, and `NOFOLLOW` rejects a final symlink; a mount at that entry is crossed.
        Err(errno) if parent.is_none() && openat2_refused(errno) => {
            rustix::fs::openat(dir, name, flags, Mode::empty())
        }
        opened => opened,
    };
    let file = match opened {
        Ok(fd) => std::fs::File::from(fd),
        Err(rustix::io::Errno::NOENT) => return DocumentRead::Missing,
        Err(rustix::io::Errno::LOOP) => return DocumentRead::NotRegular,
        Err(_) => return DocumentRead::Unreadable,
    };
    let Ok(metadata) = file.metadata() else {
        return DocumentRead::Unreadable;
    };
    if !metadata.is_file() {
        return DocumentRead::NotRegular;
    }
    if metadata.len() > MAX_PROJECT_DOC_BYTES {
        return DocumentRead::Oversize;
    }
    let mut raw = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(0)
            .saturating_add(1),
    );
    match file.take(MAX_PROJECT_DOC_BYTES + 1).read_to_end(&mut raw) {
        Err(_) => DocumentRead::Unreadable,
        Ok(_) if raw.len() as u64 > MAX_PROJECT_DOC_BYTES => DocumentRead::Oversize,
        Ok(_) => String::from_utf8(raw).map_or(DocumentRead::NotUtf8, DocumentRead::Text),
    }
}

/// Moves `docs`' reads into `sink`, replacing any earlier read of the pass, so capture sees the bytes the prompt was built from last.
pub(crate) fn leave_reads(sink: Option<&DocumentReads>, docs: &mut ProjectDocs) {
    if let Some(sink) = sink {
        *sink.lock().expect("document reads mutex") = Some(std::mem::take(&mut docs.reads));
    }
}

/// Empties `sink`, so a composition the pass discards leaves no reads.
pub(crate) fn clear_reads(sink: Option<&DocumentReads>) {
    if let Some(sink) = sink {
        *sink.lock().expect("document reads mutex") = None;
    }
}

pub(crate) fn take_reads(
    sink: Option<&DocumentReads>,
) -> Option<Vec<(DocumentPath, DocumentRead)>> {
    sink.and_then(|sink| sink.lock().expect("document reads mutex").take())
}

pub(crate) fn restore_reads(
    sink: Option<&DocumentReads>,
    reads: Option<Vec<(DocumentPath, DocumentRead)>>,
) {
    if let (Some(sink), Some(reads)) = (sink, reads) {
        sink.lock()
            .expect("document reads mutex")
            .get_or_insert(reads);
    }
}

/// Reads configured project documents and returns their deterministic rendering, with each document's raw read.
///
/// Missing, non-regular, unreadable, and non-UTF-8 files are skipped, as are
/// files over 256 KiB; the cap holds on the bytes read from one no-follow
/// descriptor, so a concurrent swap or growth cannot bypass it.
/// The hash covers canonical unescaped content in list order under each configured path, while
/// `rendered_block` XML-escapes file names and content. This function does not
/// panic for filesystem or decoding failures.
pub fn read_project_docs_canonical(project_directory: &str, files: &[DocumentPath]) -> ProjectDocs {
    let root = open_root(Path::new(project_directory));
    let reads: Vec<(DocumentPath, DocumentRead)> = files
        .iter()
        .map(|path| {
            let read = root.as_ref().map_or(DocumentRead::Unreadable, |root| {
                read_beneath(root.as_fd(), path, &open_beneath)
            });
            (path.clone(), read)
        })
        .collect();
    let texts: usize = reads
        .iter()
        .map(|(_, read)| match read {
            DocumentRead::Text(raw) => raw.len(),
            _ => 0,
        })
        .sum();
    // The hash covers each `file:{name}\n{canonical}` piece, the pieces joined by the delimiter.
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut rendered_block = String::new();
    let mut canonical = String::new();
    let mut sections = 0;
    for (filename, read) in &reads {
        let DocumentRead::Text(raw) = read else {
            continue;
        };
        canonical.clear();
        canonical.reserve(raw.len());
        canonicalize_doc_content_into(raw, &mut canonical);
        if sections == 0 {
            // Escaping grows content by a few bytes in the usual document; the reserve covers the tags and that growth.
            rendered_block.reserve(texts + texts / 16 + 128);
            rendered_block.push_str("<project-docs>\n");
        } else {
            hash.update(PROJECT_DOCS_DELIMITER.as_bytes());
            rendered_block.push_str("\n\n");
        }
        sections += 1;
        hash.update(b"file:");
        hash.update(filename.as_str().as_bytes());
        hash.update(b"\n");
        hash.update(canonical.as_bytes());
        rendered_block.push_str("<file name=\"");
        escape_xml_into(filename.as_str(), true, &mut rendered_block);
        rendered_block.push_str("\">\n");
        escape_xml_into(&canonical, false, &mut rendered_block);
        rendered_block.push_str("\n</file>");
    }
    let canonical_hash = if sections == 0 {
        String::new()
    } else {
        rendered_block.push_str("\n</project-docs>");
        hex(hash.finish().as_ref())
    };

    ProjectDocs {
        rendered_block,
        canonical_hash,
        reads,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::io::Write;

    fn doc(path: &str) -> DocumentPath {
        admit_document_paths(&[path]).unwrap().remove(0)
    }

    fn write_doc(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir.join(name).parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(dir.join(name)).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    #[test]
    fn symlinked_doc_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret.txt");
        write_doc(dir.path(), "secret.txt", "TOP SECRET");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret, dir.path().join("ARCHITECTURE.md")).unwrap();
        // A symlinked `ARCHITECTURE.md` is skipped; a regular `STRUCTURE.md` is included only when it is no larger than `MAX_PROJECT_DOC_BYTES` and can be read as UTF-8.
        write_doc(dir.path(), "STRUCTURE.md", "real struct");
        let docs = read_project_docs_canonical(
            dir.path().to_str().unwrap(),
            default_document_paths_for_test(),
        );
        #[cfg(unix)]
        {
            assert!(
                !docs.rendered_block.contains("TOP SECRET"),
                "symlink exfil blocked"
            );
            assert!(docs.rendered_block.contains("real struct"));
        }
    }

    /// The cap is enforced on the bytes read from the opened descriptor, so a
    /// file whose metadata fit but whose bytes do not is still skipped.
    #[cfg(unix)]
    #[test]
    fn a_doc_that_outgrows_the_cap_under_the_read_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ARCHITECTURE.md");
        // Sparse extent past the cap: metadata reports the full length, and
        // so does the bounded read, which is what the cap has to see.
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_PROJECT_DOC_BYTES + 1).unwrap();
        drop(file);
        assert_eq!(
            read_document(dir.path(), &doc("ARCHITECTURE.md")),
            DocumentRead::Oversize
        );

        let fifo = dir.path().join("STRUCTURE.md");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        assert_eq!(
            read_document(dir.path(), &doc("STRUCTURE.md")),
            DocumentRead::NotRegular,
            "a FIFO is refused without blocking on a writer"
        );
    }

    /// The hash capture compares with a live artifact digest is the kernel's payload identifier of the same bytes.
    #[test]
    fn sha256_hex_is_the_kernel_payload_id() {
        let long = "\u{e9}x\r\n".repeat(16 * 1024);
        for text in ["", "a", "# Architecture\n\nOne daemon.\n", long.as_str()] {
            assert_eq!(
                sha256_hex(text.as_bytes()),
                kernel::source_identity::payload_id(text.as_bytes())
            );
        }
    }

    #[test]
    fn a_file_is_missing_only_from_a_directory_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_document(dir.path(), &doc("ARCHITECTURE.md")),
            DocumentRead::Missing
        );
        assert_eq!(
            read_document(&dir.path().join("gone"), &doc("ARCHITECTURE.md")),
            DocumentRead::Unreadable,
            "a vanished root verifies no removal"
        );
    }

    #[test]
    fn oversized_doc_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat((MAX_PROJECT_DOC_BYTES + 1) as usize);
        write_doc(dir.path(), "ARCHITECTURE.md", &big);
        write_doc(dir.path(), "STRUCTURE.md", "small");
        let docs = read_project_docs_canonical(
            dir.path().to_str().unwrap(),
            default_document_paths_for_test(),
        );
        assert!(!docs.rendered_block.contains(&big));
        assert!(docs.rendered_block.contains("small"));
    }

    fn reference_canonical(raw: &str) -> String {
        let no_bom = raw.strip_prefix('\u{feff}').unwrap_or(raw);
        let lf = no_bom.replace("\r\n", "\n");
        let trimmed_lines: Vec<&str> = lf
            .split('\n')
            .map(|line| line.trim_end_matches([' ', '\t']))
            .collect();
        let joined = trimmed_lines.join("\n");
        joined.trim_end_matches('\n').to_string()
    }

    fn reference_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    /// The block and hash as the per-piece formatting builds them: each piece canonicalized, formatted, and joined.
    fn reference_docs(files: &[(&str, &str)]) -> (String, String) {
        use sha2::Digest as _;
        let mut hash_pieces = Vec::new();
        let mut sections = Vec::new();
        for (name, raw) in files {
            let canonical = reference_canonical(raw);
            hash_pieces.push(format!("file:{name}\n{canonical}"));
            sections.push(format!(
                "<file name=\"{}\">\n{}\n</file>",
                reference_escape(name).replace('"', "&quot;"),
                reference_escape(&canonical)
            ));
        }
        if hash_pieces.is_empty() {
            return (String::new(), String::new());
        }
        (
            format!("<project-docs>\n{}\n</project-docs>", sections.join("\n\n")),
            format!(
                "{:x}",
                sha2::Sha256::digest(hash_pieces.join(PROJECT_DOCS_DELIMITER).as_bytes())
            ),
        )
    }

    fn doc_text() -> impl proptest::strategy::Strategy<Value = String> {
        proptest::collection::vec(
            proptest::sample::select(vec![
                "a", "\u{e9}", " ", "\t", "\r", "\n", "\r\n", "&", "<", ">", "\"", "\u{feff}",
                "\n\n",
            ]),
            0..64,
        )
        .prop_map(|pieces| pieces.concat())
    }

    use proptest::strategy::Strategy as _;

    proptest::proptest! {
        /// The single-pass canonical form and escape equal the replace-and-join reference on text built from line breaks, carriage returns, trailing blanks, markup characters, and byte order marks.
        #[test]
        fn canonical_form_and_escape_match_the_reference(raw in doc_text()) {
            let mut canonical = String::new();
            canonicalize_doc_content_into(&raw, &mut canonical);
            proptest::prop_assert_eq!(&canonical, &reference_canonical(&raw));
            let mut escaped = String::new();
            escape_xml_into(&raw, false, &mut escaped);
            proptest::prop_assert_eq!(escaped, reference_escape(&raw));
        }

        /// The rendered block and canonical hash equal the per-piece reference for either, both, or neither document.
        #[test]
        fn rendered_block_and_hash_match_the_reference(
            architecture in proptest::option::of(doc_text()),
            structure in proptest::option::of(doc_text()),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let mut files = Vec::new();
            for (name, body) in [("ARCHITECTURE.md", &architecture), ("STRUCTURE.md", &structure)] {
                if let Some(body) = body {
                    write_doc(dir.path(), name, body);
                    files.push((name, body.as_str()));
                }
            }
            let docs = read_project_docs_canonical(dir.path().to_str().unwrap(), default_document_paths_for_test());
            let (block, hash) = reference_docs(&files);
            proptest::prop_assert_eq!(docs.rendered_block, block);
            proptest::prop_assert_eq!(docs.canonical_hash, hash);
        }
    }

    #[derive(Deserialize)]
    struct DocCase {
        /// The configured list; a case without one reads the default.
        #[serde(default)]
        paths: Option<Vec<String>>,
        files: Vec<(String, String)>, // (filename, raw body)
        rendered_block: String,
        canonical_hash: String,
    }
    #[derive(Deserialize)]
    struct DocsGolden {
        cases: Vec<DocCase>,
    }

    #[test]
    fn project_docs_golden_matches_reference() {
        let raw = include_str!("../testdata/project-docs-golden.json");
        let golden: DocsGolden = serde_json::from_str(raw).expect("parse project-docs-golden.json");
        assert!(!golden.cases.is_empty());
        for (n, case) in golden.cases.iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            for (name, body) in &case.files {
                write_doc(dir.path(), name, body);
            }
            let configured = case.paths.as_ref().map(|paths| {
                admit_document_paths(&paths.iter().map(String::as_str).collect::<Vec<_>>()).unwrap()
            });
            let files = configured
                .as_deref()
                .unwrap_or(default_document_paths_for_test());
            let docs = read_project_docs_canonical(dir.path().to_str().unwrap(), files);
            assert_eq!(
                docs.rendered_block, case.rendered_block,
                "render mismatch case {n}"
            );
            assert_eq!(
                docs.canonical_hash, case.canonical_hash,
                "hash mismatch case {n}"
            );
        }
    }

    /// Every class the admission rule refuses, each in the first position the rule reaches it.
    #[test]
    fn admission_names_the_first_refused_entry() {
        let long = "a".repeat(513);
        let deep = vec!["d"; 17].join("/");
        let nine: Vec<String> = (0..9).map(|index| format!("d{index}.md")).collect();
        let nine: Vec<&str> = nine.iter().map(String::as_str).collect();
        for (entries, index) in [
            (vec!["../x.md"], 0),
            (vec!["/abs.md"], 0),
            (vec!["a//b.md"], 0),
            (vec!["a/./b.md"], 0),
            (vec![".git/HEAD"], 0),
            (vec![".GIT/HEAD"], 0),
            (vec!["a/.Eidnara/x"], 0),
            (vec!["src/.git"], 0),
            (vec![".eidnara/x"], 0),
            (vec!["a\0b"], 0),
            (vec!["docs/a\tb.md"], 0),
            (vec!["a\u{7f}.md"], 0),
            (vec!["a\u{85}.md"], 0),
            (vec!["{env:HOME}/x.md"], 0),
            (vec!["docs/{file:x}"], 0),
            (vec![""], 0),
            (vec![long.as_str()], 0),
            (vec![deep.as_str()], 0),
            (vec!["A.md", "A.md"], 1),
            (nine.clone(), 8),
        ] {
            assert_eq!(admit_document_paths(&entries), Err(index), "{entries:?}");
        }
        let sixteen = vec!["d"; 16].join("/");
        let admitted =
            admit_document_paths(&["docs/ARCHITECTURE.md", "A.md", "a.md", &sixteen]).unwrap();
        assert_eq!(admitted[0].as_str(), "docs/ARCHITECTURE.md");
        assert_eq!(
            admitted.len(),
            4,
            "byte-distinct names are distinct entries"
        );
        assert_eq!(admit_document_paths(&nine[..8]).unwrap().len(), 8);
        assert_eq!(admit_document_paths(&[]).unwrap(), Vec::new());
        assert_eq!(admit_document_paths(&[&"a".repeat(512)]).unwrap().len(), 1);
    }

    #[test]
    fn every_admitted_path_is_a_kernel_identity_value() {
        let candidates = [
            "a".repeat(512),
            "a".repeat(513),
            "a".repeat(1024),
            "docs/a\tb.md".to_owned(),
            "a\nb.md".to_owned(),
            "a\u{7f}.md".to_owned(),
            "a\u{85}.md".to_owned(),
            "docs/\u{e9}t\u{e9}.md".to_owned(),
        ];
        for candidate in &candidates {
            if admit_document_paths(&[candidate]).is_ok() {
                assert!(
                    kernel::source_identity::well_formed_value(candidate),
                    "admitted {candidate:?} is not a Kernel identity value"
                );
            }
        }
        assert!(admit_document_paths(&["docs/\u{e9}t\u{e9}.md"]).is_ok());
    }

    /// A symlinked or absent parent, or any path through one, is unreadable; a final-component
    /// symlink is not a regular file; a missing file in a present nested parent is missing.
    #[cfg(unix)]
    #[test]
    fn nested_reads_resolve_beneath_the_root() {
        let outside = tempfile::tempdir().unwrap();
        write_doc(outside.path(), "A.md", "OUTSIDE");
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("docs")).unwrap();
        assert_eq!(
            read_document(dir.path(), &doc("docs/A.md")),
            DocumentRead::Unreadable
        );
        let docs = read_project_docs_canonical(
            dir.path().to_str().unwrap(),
            &admit_document_paths(&["docs/A.md"]).unwrap(),
        );
        assert!(docs.rendered_block.is_empty(), "{}", docs.rendered_block);
        assert_eq!(
            read_document(dir.path(), &doc("absent/A.md")),
            DocumentRead::Unreadable,
            "an absent parent verifies no removal"
        );
        write_doc(dir.path(), "real/A.md", "inside");
        assert_eq!(
            read_document(dir.path(), &doc("real/A.md")),
            DocumentRead::Text("inside".to_owned())
        );
        assert_eq!(
            read_document(dir.path(), &doc("real/B.md")),
            DocumentRead::Missing
        );
        std::os::unix::fs::symlink(outside.path().join("A.md"), dir.path().join("real/link.md"))
            .unwrap();
        assert_eq!(
            read_document(dir.path(), &doc("real/link.md")),
            DocumentRead::NotRegular
        );
        std::os::unix::fs::symlink(
            outside.path().join("A.md"),
            dir.path().join("ARCHITECTURE.md"),
        )
        .unwrap();
        assert_eq!(
            read_document(dir.path(), &doc("ARCHITECTURE.md")),
            DocumentRead::NotRegular
        );
    }

    /// Every component resolves beneath the root with no symlink, magic link, or mount crossing; a
    /// mount inside the root needs privileges a test host lacks, so the flag set is pinned here.
    #[test]
    fn reads_resolve_with_every_confinement_flag() {
        assert_eq!(
            RESOLVE,
            ResolveFlags::BENEATH
                | ResolveFlags::NO_SYMLINKS
                | ResolveFlags::NO_MAGICLINKS
                | ResolveFlags::NO_XDEV
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_host_without_openat2_reads_top_level_documents_and_refuses_nested_ones() {
        let outside = tempfile::tempdir().unwrap();
        write_doc(outside.path(), "A.md", "OUTSIDE");
        let dir = tempfile::tempdir().unwrap();
        write_doc(dir.path(), "ARCHITECTURE.md", "text");
        write_doc(dir.path(), "docs/A.md", "text");
        std::os::unix::fs::symlink(outside.path().join("A.md"), dir.path().join("LINK.md"))
            .unwrap();
        let root = open_root(dir.path()).unwrap();
        for errno in [
            rustix::io::Errno::NOSYS,
            rustix::io::Errno::PERM,
            rustix::io::Errno::INVAL,
            rustix::io::Errno::OPNOTSUPP,
        ] {
            let read = |path: &str| read_beneath(root.as_fd(), &doc(path), &|_, _, _| Err(errno));
            assert_eq!(
                read("ARCHITECTURE.md"),
                DocumentRead::Text("text".to_owned()),
                "{errno:?}"
            );
            assert_eq!(read("STRUCTURE.md"), DocumentRead::Missing, "{errno:?}");
            assert_eq!(read("LINK.md"), DocumentRead::NotRegular, "{errno:?}");
            assert_eq!(read("docs/A.md"), DocumentRead::Unreadable, "{errno:?}");
        }
        assert_eq!(
            read_beneath(root.as_fd(), &doc("ARCHITECTURE.md"), &|_, _, _| Err(
                rustix::io::Errno::XDEV
            )),
            DocumentRead::Unreadable,
            "a refusal of the entry itself takes no fallback"
        );
    }
}

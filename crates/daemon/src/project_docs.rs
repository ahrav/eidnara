//! Reads optional project documents into canonical hash and escaped XML forms.
//!
//! Documents are processed in fixed `ARCHITECTURE.md`, then `STRUCTURE.md`
//! order. Each file is limited to 256 KiB. Metadata checks reject symlinks and
//! narrow, but do not eliminate, the path-swap window before reading.

use sha2::{Digest, Sha256};
use std::path::Path;

pub(crate) const PROJECT_DOC_FILES: [&str; 2] = ["ARCHITECTURE.md", "STRUCTURE.md"];
const PROJECT_DOCS_DELIMITER: &str = "\n\n---\n\n";
const MAX_PROJECT_DOC_BYTES: u64 = 256 * 1024;

/// Both fields are empty when neither configured document is included.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectDocs {
    pub rendered_block: String,
    pub canonical_hash: String,
    /// Each configured document's read, in the fixed file order: the raw bytes the canonical block was built from, or why it holds none.
    pub(crate) reads: Vec<(&'static str, DocumentRead)>,
}

/// Where a pass leaves the configured documents it read for its prompt, for source capture to take once the response is built.
pub(crate) type DocumentReads = std::sync::Mutex<Option<Vec<(&'static str, DocumentRead)>>>;

fn canonicalize_doc_content(raw: &str) -> String {
    let no_bom = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let lf = no_bom.replace("\r\n", "\n");
    let trimmed_lines: Vec<&str> = lf
        .split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect();
    let joined = trimmed_lines.join("\n");
    joined.trim_end_matches('\n').to_string()
}

fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_xml_content(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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

/// Reads `path`, without following a symlink, as a regular file whose bytes fit `MAX_PROJECT_DOC_BYTES` and decode as UTF-8. The descriptor is validated and the cap enforced on bytes read, so a swap or growth after the open cannot change what is read; a failed read is classified from the path's own metadata.
pub(crate) fn read_document(path: &Path) -> DocumentRead {
    match crate::config::read_bounded_bytes(path, MAX_PROJECT_DOC_BYTES) {
        Ok(bytes) => String::from_utf8(bytes).map_or(DocumentRead::NotUtf8, DocumentRead::Text),
        // A file is verifiably missing only from a directory that exists; a vanished project root, such as an unmounted volume, is unreadable.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if path
                .parent()
                .is_some_and(|parent| std::fs::metadata(parent).is_ok_and(|meta| meta.is_dir()))
            {
                DocumentRead::Missing
            } else {
                DocumentRead::Unreadable
            }
        }
        Err(_) => match std::fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.file_type().is_file() => DocumentRead::NotRegular,
            Ok(metadata) if metadata.len() > MAX_PROJECT_DOC_BYTES => DocumentRead::Oversize,
            _ => DocumentRead::Unreadable,
        },
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

/// Reads configured project documents and returns their deterministic rendering, with each document's raw read.
///
/// Missing, non-regular, unreadable, and non-UTF-8 files are skipped, as are
/// files over 256 KiB; the cap holds on the bytes read from one no-follow
/// descriptor, so a concurrent swap or growth cannot bypass it.
/// The hash covers canonical unescaped content in fixed filename order, while
/// `rendered_block` XML-escapes file names and content. This function does not
/// panic for filesystem or decoding failures.
pub fn read_project_docs_canonical(project_directory: &str) -> ProjectDocs {
    let dir = Path::new(project_directory);
    let mut hash_pieces: Vec<String> = Vec::new();
    let mut rendered_sections: Vec<String> = Vec::new();

    let reads: Vec<(&'static str, DocumentRead)> = PROJECT_DOC_FILES
        .into_iter()
        .map(|filename| (filename, read_document(&dir.join(filename))))
        .collect();
    for (filename, read) in &reads {
        let DocumentRead::Text(raw) = read else {
            continue;
        };
        let canonical = canonicalize_doc_content(raw);
        hash_pieces.push(format!("file:{filename}\n{canonical}"));
        rendered_sections.push(format!(
            "<file name=\"{}\">\n{}\n</file>",
            escape_xml_attr(filename),
            escape_xml_content(&canonical)
        ));
    }

    let canonical_hash = if hash_pieces.is_empty() {
        String::new()
    } else {
        let mut hasher = Sha256::new();
        hasher.update(hash_pieces.join(PROJECT_DOCS_DELIMITER).as_bytes());
        format!("{:x}", hasher.finalize())
    };
    let rendered_block = if rendered_sections.is_empty() {
        String::new()
    } else {
        format!(
            "<project-docs>\n{}\n</project-docs>",
            rendered_sections.join("\n\n")
        )
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

    fn write_doc(dir: &Path, name: &str, body: &str) {
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
        let docs = read_project_docs_canonical(dir.path().to_str().unwrap());
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
        assert_eq!(read_document(&path), DocumentRead::Oversize);

        let fifo = dir.path().join("STRUCTURE.md");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        assert_eq!(
            read_document(&fifo),
            DocumentRead::NotRegular,
            "a FIFO is refused without blocking on a writer"
        );
    }

    #[test]
    fn a_file_is_missing_only_from_a_directory_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_document(&dir.path().join("ARCHITECTURE.md")),
            DocumentRead::Missing
        );
        assert_eq!(
            read_document(&dir.path().join("gone").join("ARCHITECTURE.md")),
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
        let docs = read_project_docs_canonical(dir.path().to_str().unwrap());
        assert!(!docs.rendered_block.contains(&big));
        assert!(docs.rendered_block.contains("small"));
    }

    #[derive(Deserialize)]
    struct DocCase {
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
            let docs = read_project_docs_canonical(dir.path().to_str().unwrap());
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
}

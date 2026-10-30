//! A repository built with gix write APIs; no git CLI. Commit ids are fixed
//! by the message and the signature time, so the same inputs yield the same
//! oid in every test.

use std::path::{Path, PathBuf};

use daemon::git_sources::RepositoryBinding;
use gix::bstr::BString;

fn signature(seconds: i64) -> gix::actor::Signature {
    gix::actor::Signature {
        name: "fixture".into(),
        email: "fixture@example.com".into(),
        time: gix::date::Time::new(seconds, 0),
    }
}

fn sha1(bytes: &[u8]) -> gix::ObjectId {
    let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
    hasher.update(bytes);
    hasher.try_finalize().unwrap()
}

fn zlib(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut writer =
        gix::zlib::stream::deflate::Write::new(Vec::new(), gix::zlib::Compression::DEFAULT);
    writer.write_all(bytes).unwrap();
    writer.flush().unwrap();
    writer.into_inner()
}

/// The little-endian base-128 integer the delta format uses for its base and result sizes.
fn leb128(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// A pack entry header: three type bits and the low four size bits, then seven size bits per continuation byte.
fn entry_header(kind: u8, mut size: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut byte = (kind << 4) | (size & 0x0f) as u8;
    size >>= 4;
    while size > 0 {
        out.push(byte | 0x80);
        byte = (size & 0x7f) as u8;
        size >>= 7;
    }
    out.push(byte);
    out
}

/// The big-endian offset-encoding an ofs-delta entry carries: every continuation byte after the first stands for one more than its seven bits.
fn ofs_delta_distance(mut distance: u64) -> Vec<u8> {
    let mut bytes = vec![(distance & 0x7f) as u8];
    loop {
        distance >>= 7;
        if distance == 0 {
            break;
        }
        distance -= 1;
        bytes.push(0x80 | (distance & 0x7f) as u8);
    }
    bytes.reverse();
    bytes
}

/// A repository built with gix write APIs; no git CLI.
pub struct Repo {
    pub root: PathBuf,
    repo: gix::Repository,
}

impl Repo {
    pub fn init(root: &Path) -> Self {
        std::fs::create_dir_all(root).unwrap();
        gix::init(root).unwrap();
        let repo = gix::open_opts(root, gix::open::Options::isolated()).unwrap();
        Self {
            root: root.to_path_buf(),
            repo,
        }
    }

    pub fn tree(&self) -> gix::ObjectId {
        self.repo
            .write_object(gix::objs::Tree::empty())
            .unwrap()
            .detach()
    }

    /// A commit with `message`, its id determined by the message and `seconds`. Written as an object, not through a ref, since identity here is the object id alone.
    pub fn commit(&self, message: &str, seconds: i64) -> String {
        self.write(message.as_bytes(), None, seconds, Vec::new())
    }

    /// The committer time git recorded for `oid`, read back from the object store.
    pub fn committer_seconds(&self, oid: &str) -> i64 {
        let id = gix::ObjectId::from_hex(oid.as_bytes()).unwrap();
        let commit = self.repo.find_object(id).unwrap().into_commit();
        commit.committer().unwrap().time().unwrap().seconds
    }

    /// A commit object written raw, so its message bytes and encoding header are exactly `message` and `encoding`.
    pub fn raw_commit(&self, message: &[u8], encoding: Option<&str>) -> String {
        self.write(message, encoding, 1, Vec::new())
    }

    /// A commit whose `encoding` header follows a `gpgsig` header, the order JGit writes signed commits in. Git honors the header wherever it stands.
    pub fn late_encoding_commit(&self, message: &[u8], encoding: &str) -> String {
        self.write(
            message,
            None,
            1,
            vec![
                (
                    BString::from("gpgsig"),
                    BString::from("-----BEGIN PGP SIGNATURE-----"),
                ),
                (BString::from("encoding"), BString::from(encoding)),
            ],
        )
    }

    fn write(
        &self,
        message: &[u8],
        encoding: Option<&str>,
        seconds: i64,
        extra_headers: Vec<(BString, BString)>,
    ) -> String {
        let commit = self.object(message, encoding, seconds, extra_headers);
        self.repo
            .write_object(&commit)
            .unwrap()
            .detach()
            .to_string()
    }

    fn object(
        &self,
        message: &[u8],
        encoding: Option<&str>,
        seconds: i64,
        extra_headers: Vec<(BString, BString)>,
    ) -> gix::objs::Commit {
        let signature = signature(seconds);
        gix::objs::Commit {
            tree: self.tree(),
            parents: Default::default(),
            author: signature.clone(),
            committer: signature,
            encoding: encoding.map(BString::from),
            message: BString::from(message),
            extra_headers,
        }
    }

    /// Writes one pack holding `base_message`'s commit whole and `delta_message`'s commit as an ofs-delta against it, with the pack's index. Neither commit exists loose, so the delta commit reads back only through its base. Returns `(base id, delta id)`.
    pub fn pack_delta_commit(&self, base_message: &str, delta_message: &str) -> (String, String) {
        use gix::objs::WriteTo as _;
        let serialize = |commit: gix::objs::Commit| {
            let mut bytes = Vec::new();
            commit.write_to(&mut bytes).unwrap();
            bytes
        };
        let base = serialize(self.object(base_message.as_bytes(), None, 1, Vec::new()));
        let result = serialize(self.object(delta_message.as_bytes(), None, 2, Vec::new()));
        let (base_id, delta_id) = write_delta_pack(
            &self.root.join(".git/objects"),
            gix::objs::Kind::Commit,
            &base,
            &result,
        );
        (base_id.to_string(), delta_id.to_string())
    }

    pub fn binding(&self, repository_id: &str) -> RepositoryBinding {
        RepositoryBinding {
            repository_id: repository_id.to_string(),
            path: self.root.clone(),
        }
    }
}

/// The delta instructions that reproduce `result` from a base of `base_len` bytes by inserting every result byte; the base contributes only its declared size.
fn insert_delta(base_len: u64, result: &[u8]) -> Vec<u8> {
    let mut delta = Vec::new();
    leb128(&mut delta, base_len);
    leb128(&mut delta, result.len() as u64);
    for chunk in result.chunks(127) {
        delta.push(chunk.len() as u8);
        delta.extend_from_slice(chunk);
    }
    delta
}

/// A version 2 pack header announcing `count` entries.
fn pack_header(count: u32) -> Vec<u8> {
    let mut pack = b"PACK".to_vec();
    pack.extend_from_slice(&2u32.to_be_bytes());
    pack.extend_from_slice(&count.to_be_bytes());
    pack
}

/// Appends the trailer to `pack`, the header and entries, and writes it into `objects/pack` with a version 2 index of `entries`: each entry's id and offset, in pack order.
fn write_pack_and_index(objects: &Path, mut pack: Vec<u8>, entries: &[(gix::ObjectId, usize)]) {
    let entries_end = pack.len();
    let mut indexed: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(position, (id, offset))| {
            let end = entries.get(position + 1).map_or(entries_end, |next| next.1);
            (
                *id,
                *offset,
                gix::features::hash::crc32(&pack[*offset..end]),
            )
        })
        .collect();
    let pack_checksum = sha1(&pack);
    pack.extend_from_slice(pack_checksum.as_slice());

    indexed.sort_by_key(|entry| entry.0);
    let mut idx = vec![0xff, b't', b'O', b'c'];
    idx.extend_from_slice(&2u32.to_be_bytes());
    for first in 0..=255u8 {
        let count = indexed
            .iter()
            .filter(|(id, _, _)| id.as_slice()[0] <= first)
            .count() as u32;
        idx.extend_from_slice(&count.to_be_bytes());
    }
    for (id, _, _) in &indexed {
        idx.extend_from_slice(id.as_slice());
    }
    for (_, _, crc) in &indexed {
        idx.extend_from_slice(&crc.to_be_bytes());
    }
    for (_, offset, _) in &indexed {
        idx.extend_from_slice(&(*offset as u32).to_be_bytes());
    }
    idx.extend_from_slice(pack_checksum.as_slice());
    let idx_checksum = sha1(&idx);
    idx.extend_from_slice(idx_checksum.as_slice());

    let pack_dir = objects.join("pack");
    std::fs::create_dir_all(&pack_dir).unwrap();
    let stem = format!("pack-{pack_checksum}");
    std::fs::write(pack_dir.join(format!("{stem}.pack")), &pack).unwrap();
    std::fs::write(pack_dir.join(format!("{stem}.idx")), &idx).unwrap();
}

/// Writes one pack into `objects` holding `base` whole and `result` as an ofs-delta against it, both as objects of `kind`, with the pack's index. Neither object exists loose, so `result` reads back only through its base. Returns `(base id, result id)`.
pub fn write_delta_pack(
    objects: &Path,
    kind: gix::objs::Kind,
    base: &[u8],
    result: &[u8],
) -> (gix::ObjectId, gix::ObjectId) {
    let base_id = gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, base).unwrap();
    let delta_id = gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, result).unwrap();
    let type_code = match kind {
        gix::objs::Kind::Commit => 1,
        gix::objs::Kind::Tree => 2,
        gix::objs::Kind::Blob => 3,
        gix::objs::Kind::Tag => 4,
    };
    let delta = insert_delta(base.len() as u64, result);
    let mut pack = pack_header(2);
    let base_offset = pack.len();
    pack.extend_from_slice(&entry_header(type_code, base.len() as u64));
    pack.extend_from_slice(&zlib(base));
    let delta_offset = pack.len();
    pack.extend_from_slice(&entry_header(6, delta.len() as u64));
    pack.extend_from_slice(&ofs_delta_distance((delta_offset - base_offset) as u64));
    pack.extend_from_slice(&zlib(&delta));
    write_pack_and_index(
        objects,
        pack,
        &[(base_id, base_offset), (delta_id, delta_offset)],
    );
    (base_id, delta_id)
}

/// Writes one pack into `objects` holding `id` as a ref-delta whose base is `id` itself, with the pack's index. The delta inserts `result`, so each hop declares `result`'s length as both base and result size; the chain never reaches a whole object.
pub fn write_self_delta_pack(objects: &Path, id: gix::ObjectId, result: &[u8]) {
    let delta = insert_delta(result.len() as u64, result);
    let mut pack = pack_header(1);
    let offset = pack.len();
    pack.extend_from_slice(&entry_header(7, delta.len() as u64));
    pack.extend_from_slice(id.as_slice());
    pack.extend_from_slice(&zlib(&delta));
    write_pack_and_index(objects, pack, &[(id, offset)]);
}

/// The zero-byte delta payload makes delta-header parsing reach end-of-input at the base-size field.
pub fn write_empty_self_delta_pack(objects: &Path, id: gix::ObjectId) {
    let mut pack = pack_header(1);
    let offset = pack.len();
    pack.extend_from_slice(&entry_header(7, 0));
    pack.extend_from_slice(id.as_slice());
    pack.extend_from_slice(&zlib(&[]));
    write_pack_and_index(objects, pack, &[(id, offset)]);
}

/// Resolving the ref-delta for `id` requires `base` from outside this pack.
pub fn write_ref_delta_pack(objects: &Path, id: gix::ObjectId, base: gix::ObjectId, result: &[u8]) {
    let delta = insert_delta(result.len() as u64, result);
    let mut pack = pack_header(1);
    let offset = pack.len();
    pack.extend_from_slice(&entry_header(7, delta.len() as u64));
    pack.extend_from_slice(base.as_slice());
    pack.extend_from_slice(&zlib(&delta));
    write_pack_and_index(objects, pack, &[(id, offset)]);
}

pub fn write_truncated_blob_pack(objects: &Path, content: &[u8]) -> gix::ObjectId {
    let id =
        gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, content).unwrap();
    let stream = zlib(content);
    let mut pack = pack_header(1);
    let offset = pack.len();
    pack.extend_from_slice(&entry_header(3, content.len() as u64));
    pack.extend_from_slice(&stream[..stream.len() / 2]);
    write_pack_and_index(objects, pack, &[(id, offset)]);
    id
}

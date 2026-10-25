use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use gix::odb::store::init::Slots;
use serde::{Deserialize, Serialize};

use crate::git_sources::{self, CyclicDeltaChain, SlotsExhausted};

const HEAD_OBJECT_BYTES: NonZeroU64 = NonZeroU64::new(1 << 20).unwrap();
const HEAD_INDEX_SLOTS: u16 = 128;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FrozenHead {
    /// The 40-character lowercase hex id of the commit HEAD names, directly
    /// when detached or through its branch.
    Commit(String),
    /// `HeadUnavailable` indicates an unborn or unreadable HEAD, a HEAD naming
    /// a non-commit or absent object, a HEAD stored in a pack whose delta
    /// chain revisits an entry, an unsupported object format, or a root that
    /// `git_sources::open_with_slots` refuses.
    #[default]
    HeadUnavailable,
}

/// The pack index that answered the last packed HEAD read; it orders the next
/// pack search only.
static LAST_HEAD_INDEX: Mutex<Option<PathBuf>> = Mutex::new(None);

pub(crate) fn observe_head(root: &Path) -> FrozenHead {
    let slots = Slots::Given(HEAD_INDEX_SLOTS);
    let Ok(repo) = git_sources::open_with_slots(root, HEAD_OBJECT_BYTES, slots) else {
        return FrozenHead::HeadUnavailable;
    };
    let hash = repo.object_hash();
    if git_sources::object_format(hash).is_err() {
        return FrozenHead::HeadUnavailable;
    }
    let Some(id) = head_id(&repo) else {
        return FrozenHead::HeadUnavailable;
    };
    let kind = match git_sources::file_object_kind(&repo, &id, &LAST_HEAD_INDEX) {
        Ok(Some(kind)) => Some(kind),
        Ok(None) => git_sources::store_object_kind(&repo, &id)
            .unwrap_or_else(|SlotsExhausted| reopened_kind(root, &id)),
        Err(CyclicDeltaChain) => None,
    };
    let hex = id.to_hex().to_string();
    if kind == Some(gix::object::Kind::Commit) && git_sources::parse_oid(&hex, hash).is_some() {
        FrozenHead::Commit(hex)
    } else {
        FrozenHead::HeadUnavailable
    }
}

/// Resolves HEAD through its symbolic-reference chain and returns the first
/// object id the chain reaches, a tag object's id included. A branch that is
/// itself symbolic, such as one aliased with `git symbolic-ref`, follows the
/// rest of the chain.
fn head_id(repo: &gix::Repository) -> Option<gix::ObjectId> {
    let head = repo.head().ok()?;
    if let Some(id) = head.id() {
        return Some(id.detach());
    }
    let mut referent = head.try_into_referent()?;
    referent.follow_to_object().ok().map(gix::Id::detach)
}

fn reopened_kind(root: &Path, id: &gix::oid) -> Option<gix::object::Kind> {
    let repo = git_sources::open(root, HEAD_OBJECT_BYTES).ok()?;
    git_sources::store_object_kind(&repo, id).ok().flatten()
}

#[cfg(test)]
pub(crate) mod fixtures {
    use std::path::Path;

    fn signature() -> gix::actor::Signature {
        gix::actor::Signature {
            name: "fixture".into(),
            email: "fixture@example.com".into(),
            time: gix::date::Time::new(1_700_000_000, 0),
        }
    }

    /// Commits one file on the branch HEAD names, parented on the current
    /// HEAD when it exists, and returns the new commit id.
    pub(crate) fn commit(root: &Path, text: &str) -> String {
        let repo = gix::open_opts(root, gix::open::Options::isolated()).unwrap();
        let parents: Vec<gix::ObjectId> = repo
            .head_id()
            .ok()
            .map(|id| id.detach())
            .into_iter()
            .collect();
        let blob = repo.write_blob(text).unwrap().detach();
        let tree = gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: "file.txt".into(),
                oid: blob,
            }],
        };
        let tree = repo.write_object(&tree).unwrap().detach();
        let signature = signature();
        repo.commit_as(
            signature.to_ref(&mut Default::default()),
            signature.to_ref(&mut Default::default()),
            "HEAD",
            text,
            tree,
            parents,
        )
        .unwrap()
        .to_string()
    }

    /// Initializes a repository at `root` with `count` commits and returns
    /// their ids in order.
    pub(crate) fn repository(root: &Path, count: usize) -> Vec<String> {
        std::fs::create_dir_all(root).unwrap();
        gix::init(root).unwrap();
        (0..count)
            .map(|index| commit(root, &format!("revision {index}\n")))
            .collect()
    }

    pub(crate) fn detach(root: &Path, oid: &str) {
        std::fs::write(root.join(".git/HEAD"), format!("{oid}\n")).unwrap();
    }

    pub(crate) fn declare_sha256(root: &Path) {
        std::fs::write(
            root.join(".git/config"),
            "[core]\n\trepositoryformatversion = 1\n\tbare = false\n[extensions]\n\tobjectformat = sha256\n",
        )
        .unwrap();
    }

    fn sha1(bytes: &[u8]) -> gix::ObjectId {
        let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
        hasher.update(bytes);
        hasher.try_finalize().unwrap()
    }

    fn zlib(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut deflate =
            gix::zlib::stream::deflate::Write::new(Vec::new(), gix::zlib::Compression::DEFAULT);
        deflate.write_all(bytes).unwrap();
        deflate.flush().unwrap();
        deflate.into_inner()
    }

    fn leb128(out: &mut Vec<u8>, mut value: u64) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    /// The delta encoding [`write_pack`] uses for its last object, with the
    /// first object as the base.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Delta {
        Offset,
        Id,
        /// The delta references the first object by id as an external base.
        OutsideId,
        /// The delta names its own id as the base, so the chain revisits
        /// its only entry.
        SelfId,
    }

    fn first_written(delta: Option<Delta>) -> usize {
        usize::from(matches!(delta, Some(Delta::OutsideId)))
    }

    pub(crate) type PackedObject = (gix::ObjectId, gix::object::Kind, Vec<u8>);

    pub(crate) fn stored_object(root: &Path, oid: &str) -> PackedObject {
        let repo = gix::open_opts(root, gix::open::Options::isolated()).unwrap();
        let object = repo
            .find_object(gix::ObjectId::from_hex(oid.as_bytes()).unwrap())
            .unwrap();
        (object.id, object.kind, object.data.clone())
    }

    /// Writes `objects` into one pack with a version 2 index and returns the
    /// index path. With `delta`, the pack stores the last object as a delta
    /// against the first.
    pub(crate) fn write_pack(
        root: &Path,
        objects: &[PackedObject],
        delta: Option<Delta>,
    ) -> std::path::PathBuf {
        use gix::odb::pack::data::entry::Header;
        let written = &objects[first_written(delta)..];
        let mut pack = b"PACK".to_vec();
        pack.extend_from_slice(&2u32.to_be_bytes());
        pack.extend_from_slice(&(written.len() as u32).to_be_bytes());
        let mut offsets = Vec::new();
        for (position, (id, kind, data)) in written.iter().enumerate() {
            let offset = pack.len();
            offsets.push(offset);
            let base = if delta == Some(Delta::SelfId) {
                &written[position]
            } else {
                &objects[0]
            };
            let is_last = position + 1 == written.len();
            let header = match delta
                .filter(|delta| is_last && (objects.len() > 1 || *delta == Delta::SelfId))
            {
                None => None,
                Some(Delta::Offset) => Some(Header::OfsDelta {
                    base_distance: (offset - offsets[0]) as u64,
                }),
                Some(Delta::Id | Delta::OutsideId) => Some(Header::RefDelta { base_id: base.0 }),
                Some(Delta::SelfId) => Some(Header::RefDelta { base_id: *id }),
            };
            let body = match header {
                None => data.clone(),
                Some(_) => {
                    let mut instructions = Vec::new();
                    leb128(&mut instructions, base.2.len() as u64);
                    leb128(&mut instructions, data.len() as u64);
                    for chunk in data.chunks(127) {
                        instructions.push(chunk.len() as u8);
                        instructions.extend_from_slice(chunk);
                    }
                    instructions
                }
            };
            let header = header.unwrap_or(match kind {
                gix::object::Kind::Commit => Header::Commit,
                gix::object::Kind::Tree => Header::Tree,
                gix::object::Kind::Blob => Header::Blob,
                gix::object::Kind::Tag => Header::Tag,
            });
            header.write_to(body.len() as u64, &mut pack).unwrap();
            pack.extend_from_slice(&zlib(&body));
        }
        offsets.push(pack.len());
        let mut entries: Vec<(gix::ObjectId, u32, usize)> = written
            .iter()
            .enumerate()
            .map(|(position, (id, _, _))| {
                let bytes = &pack[offsets[position]..offsets[position + 1]];
                (*id, gix::features::hash::crc32(bytes), offsets[position])
            })
            .collect();
        entries.sort_by_key(|entry| entry.0);
        let pack_checksum = sha1(&pack);
        pack.extend_from_slice(pack_checksum.as_slice());
        let mut idx = vec![0xff, b't', b'O', b'c'];
        idx.extend_from_slice(&2u32.to_be_bytes());
        for first in 0..=255u8 {
            let count = entries
                .iter()
                .filter(|(id, _, _)| id.as_slice()[0] <= first)
                .count() as u32;
            idx.extend_from_slice(&count.to_be_bytes());
        }
        for (id, _, _) in &entries {
            idx.extend_from_slice(id.as_slice());
        }
        for (_, crc, _) in &entries {
            idx.extend_from_slice(&crc.to_be_bytes());
        }
        for (_, _, offset) in &entries {
            idx.extend_from_slice(&(*offset as u32).to_be_bytes());
        }
        idx.extend_from_slice(pack_checksum.as_slice());
        let idx_checksum = sha1(&idx);
        idx.extend_from_slice(idx_checksum.as_slice());
        let pack_dir = root.join(".git/objects/pack");
        std::fs::create_dir_all(&pack_dir).unwrap();
        let stem = pack_dir.join(format!("pack-{pack_checksum}"));
        std::fs::write(stem.with_extension("pack"), &pack).unwrap();
        std::fs::write(stem.with_extension("idx"), &idx).unwrap();
        stem.with_extension("idx")
    }

    /// Packs the stored objects `oids` through [`write_pack`] and removes the
    /// loose files of the objects it wrote.
    pub(crate) fn pack_objects(
        root: &Path,
        oids: &[&str],
        delta: Option<Delta>,
    ) -> std::path::PathBuf {
        let objects: Vec<PackedObject> = oids.iter().map(|oid| stored_object(root, oid)).collect();
        let index = write_pack(root, &objects, delta);
        for oid in &oids[first_written(delta)..] {
            let loose = root.join(".git/objects").join(&oid[..2]).join(&oid[2..]);
            match std::fs::remove_file(loose) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => panic!("{error}"),
                _ => {}
            }
        }
        index
    }

    /// Writes `count` packs that each hold one blob.
    pub(crate) fn pack_blobs(root: &Path, count: u16) {
        for index in 0..count {
            let data = format!("blob {index}\n").into_bytes();
            let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::object::Kind::Blob, &data)
                .unwrap();
            write_pack(root, &[(id, gix::object::Kind::Blob, data)], None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{
        Delta, declare_sha256, detach, pack_blobs, pack_objects, repository, stored_object,
        write_pack,
    };
    use super::*;

    fn open(root: &Path) -> gix::Repository {
        git_sources::open(root, HEAD_OBJECT_BYTES).unwrap()
    }

    fn oid(hex: &str) -> gix::ObjectId {
        gix::ObjectId::from_hex(hex.as_bytes()).unwrap()
    }

    const COMMIT: Result<Option<gix::object::Kind>, CyclicDeltaChain> =
        Ok(Some(gix::object::Kind::Commit));
    const ABSENT: Result<Option<gix::object::Kind>, CyclicDeltaChain> = Ok(None);

    fn head_id(root: &Path) -> String {
        gix::open_opts(root, gix::open::Options::isolated())
            .unwrap()
            .head_id()
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_branch_head_and_a_detached_head_freeze_their_commit() {
        let dir = tempfile::tempdir().unwrap();
        let commits = repository(dir.path(), 3);
        assert_eq!(
            observe_head(dir.path()),
            FrozenHead::Commit(head_id(dir.path()))
        );
        assert_eq!(head_id(dir.path()), commits[2]);
        std::fs::remove_dir(dir.path().join(".git/objects/pack")).unwrap();
        let none = Mutex::new(None);
        assert_eq!(
            git_sources::file_object_kind(&open(dir.path()), &oid(&commits[2]), &none),
            COMMIT
        );
        detach(dir.path(), &commits[0]);
        assert_eq!(
            observe_head(dir.path()),
            FrozenHead::Commit(commits[0].clone())
        );
    }

    #[test]
    fn a_head_through_a_branch_alias_freezes_the_aliased_commit() {
        let dir = tempfile::tempdir().unwrap();
        let commits = repository(dir.path(), 1);
        let git = dir.path().join(".git");
        let branch = std::fs::read_to_string(git.join("HEAD")).unwrap();
        let branch = branch.trim().strip_prefix("ref: ").unwrap().to_owned();
        let point = |name: &str, target: &str| {
            std::fs::write(git.join(name), format!("ref: {target}\n")).unwrap();
        };
        point("refs/heads/alias", &branch);
        point("HEAD", "refs/heads/alias");
        assert_eq!(
            observe_head(dir.path()),
            FrozenHead::Commit(commits[0].clone())
        );
        point("refs/heads/outer", "refs/heads/alias");
        point("HEAD", "refs/heads/outer");
        assert_eq!(
            observe_head(dir.path()),
            FrozenHead::Commit(commits[0].clone())
        );
        std::fs::remove_file(git.join(&branch)).unwrap();
        std::fs::write(
            git.join("packed-refs"),
            format!("{} {branch}\n", commits[0]),
        )
        .unwrap();
        assert_eq!(
            observe_head(dir.path()),
            FrozenHead::Commit(commits[0].clone()),
            "an aliased branch held in packed-refs"
        );
        point("refs/heads/alias", "refs/heads/outer");
        assert_eq!(
            observe_head(dir.path()),
            FrozenHead::HeadUnavailable,
            "a cyclic alias"
        );
        point("refs/heads/alias", "refs/heads/unborn");
        assert_eq!(observe_head(dir.path()), FrozenHead::HeadUnavailable);
    }

    #[test]
    fn unborn_unsupported_non_commit_absent_and_non_repository_roots_are_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let unborn = dir.path().join("unborn");
        std::fs::create_dir_all(&unborn).unwrap();
        gix::init(&unborn).unwrap();
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let sha256 = dir.path().join("sha256");
        repository(&sha256, 1);
        declare_sha256(&sha256);
        let tree_head = dir.path().join("tree-head");
        repository(&tree_head, 1);
        let tree = gix::open_opts(&tree_head, gix::open::Options::isolated())
            .unwrap()
            .head_commit()
            .unwrap()
            .tree_id()
            .unwrap()
            .to_string();
        detach(&tree_head, &tree);
        let absent_head = dir.path().join("absent-head");
        repository(&absent_head, 1);
        detach(&absent_head, &"ab".repeat(20));
        let cyclic = dir.path().join("cyclic-alternates");
        repository(&cyclic, 1);
        let objects = cyclic.join(".git/objects");
        std::fs::write(
            objects.join("info/alternates"),
            format!("{}\n", objects.display()),
        )
        .unwrap();
        let pack_file = dir.path().join("pack-file");
        repository(&pack_file, 1);
        let pack = pack_file.join(".git/objects/pack");
        std::fs::remove_dir(&pack).unwrap();
        std::fs::write(&pack, b"not a directory").unwrap();
        let borrower = dir.path().join("alternate-pack-file");
        repository(&borrower, 1);
        std::fs::write(
            borrower.join(".git/objects/info/alternates"),
            format!("{}\n", pack_file.join(".git/objects").display()),
        )
        .unwrap();
        for refused in [&cyclic, &pack_file, &borrower] {
            assert!(git_sources::open(refused, HEAD_OBJECT_BYTES).is_err());
        }
        for root in [
            unborn,
            plain,
            sha256,
            tree_head,
            absent_head,
            cyclic,
            pack_file,
            borrower,
            dir.path().join("missing"),
        ] {
            assert_eq!(
                observe_head(&root),
                FrozenHead::HeadUnavailable,
                "{}",
                root.display()
            );
        }
    }

    /// A status scan reads the index and every tracked file, and a commit
    /// parse reads the commit's tree. The observation still answers with the
    /// index replaced by garbage and the HEAD commit's tree and blob deleted.
    #[test]
    fn the_observation_reads_at_most_one_object_and_neither_the_index_nor_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let commits = repository(dir.path(), 1);
        let repo = gix::open_opts(dir.path(), gix::open::Options::isolated()).unwrap();
        let tree = repo.head_commit().unwrap().tree().unwrap();
        let mut doomed = vec![tree.id.to_string()];
        doomed.extend(
            tree.decode()
                .unwrap()
                .entries
                .iter()
                .map(|entry| entry.oid.to_string()),
        );
        for oid in doomed {
            std::fs::remove_file(
                dir.path()
                    .join(".git/objects")
                    .join(&oid[..2])
                    .join(&oid[2..]),
            )
            .unwrap();
        }
        std::fs::write(dir.path().join(".git/index"), b"not an index").unwrap();
        std::fs::remove_file(dir.path().join("file.txt")).ok();
        assert_eq!(
            observe_head(dir.path()),
            FrozenHead::Commit(commits[0].clone())
        );
    }

    #[test]
    fn a_packed_head_commit_freezes_whole_or_as_a_delta_from_the_pack_files() {
        for delta in [None, Some(Delta::Offset), Some(Delta::Id)] {
            let dir = tempfile::tempdir().unwrap();
            let commits = repository(dir.path(), 2);
            pack_blobs(dir.path(), 1);
            pack_objects(dir.path(), &[&commits[0], &commits[1]], delta);
            for commit in [&commits[1], &commits[0]] {
                let none = Mutex::new(None);
                let kind = git_sources::file_object_kind(&open(dir.path()), &oid(commit), &none);
                assert_eq!(kind, COMMIT);
                detach(dir.path(), commit);
                assert_eq!(observe_head(dir.path()), FrozenHead::Commit(commit.clone()));
            }
        }
    }

    #[test]
    fn a_delta_whose_base_is_in_another_pack_is_read_from_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let commits = repository(dir.path(), 2);
        pack_objects(dir.path(), &[&commits[0]], None);
        pack_objects(
            dir.path(),
            &[&commits[0], &commits[1]],
            Some(Delta::OutsideId),
        );
        let none = Mutex::new(None);
        let kind = git_sources::file_object_kind(&open(dir.path()), &oid(&commits[1]), &none);
        assert_eq!(kind, ABSENT);
        assert_eq!(
            observe_head(dir.path()),
            FrozenHead::Commit(commits[1].clone())
        );
    }

    /// Only the hint names the renamed index, because the directory scan reads
    /// files ending in `.idx`; an answer from the renamed index shows the hint
    /// was read.
    #[test]
    fn a_hinted_pack_index_is_read_before_the_directory_scan() {
        let dir = tempfile::tempdir().unwrap();
        let commits = repository(dir.path(), 1);
        let hinted = pack_objects(dir.path(), &[&commits[0]], None).with_extension("hint");
        std::fs::rename(hinted.with_extension("idx"), &hinted).unwrap();
        let hint = Mutex::new(Some(hinted.clone()));
        let kind = git_sources::file_object_kind(&open(dir.path()), &oid(&commits[0]), &hint);
        assert_eq!(kind, COMMIT);
        assert_eq!(*hint.lock().unwrap(), Some(hinted));
    }

    #[test]
    fn a_pack_index_of_another_repository_never_answers() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first");
        let commits = repository(&first, 2);
        let foreign = pack_objects(&first, &[&commits[1]], None);
        let second = dir.path().join("second");
        let own = repository(&second, 1);
        pack_objects(&second, &[&own[0]], None);
        let pack_dir = second.join(".git/objects/pack");
        let linked = pack_dir.join("pack-linked.idx");
        std::os::unix::fs::symlink(&foreign, &linked).unwrap();
        std::os::unix::fs::symlink(
            foreign.with_extension("pack"),
            linked.with_extension("pack"),
        )
        .unwrap();
        let hint = Mutex::new(Some(foreign));
        let kind = git_sources::file_object_kind(&open(&second), &oid(&commits[1]), &hint);
        assert_eq!(kind, ABSENT);
        let hint = Mutex::new(Some(linked));
        let kind = git_sources::file_object_kind(&open(&second), &oid(&commits[1]), &hint);
        assert_eq!(kind, ABSENT, "a hinted index that became a symlink");
        detach(&second, &commits[1]);
        assert_eq!(observe_head(&second), FrozenHead::HeadUnavailable);
    }

    #[test]
    fn a_delta_chain_that_revisits_an_entry_is_unavailable_without_a_store_read() {
        let dir = tempfile::tempdir().unwrap();
        let commits = repository(dir.path(), 1);
        pack_objects(dir.path(), &[&commits[0]], Some(Delta::SelfId));
        let none = Mutex::new(None);
        let kind = git_sources::file_object_kind(&open(dir.path()), &oid(&commits[0]), &none);
        assert_eq!(kind, Err(CyclicDeltaChain));
        assert_eq!(observe_head(dir.path()), FrozenHead::HeadUnavailable);
    }

    /// The test adds the base pack and more packs than slots after the
    /// store's first lookup, so the second lookup meets the exhaustion while
    /// it resolves the delta base.
    #[test]
    fn slot_exhaustion_while_resolving_a_delta_base_reads_as_exhausted() {
        let dir = tempfile::tempdir().unwrap();
        let commits = repository(dir.path(), 2);
        let base = stored_object(dir.path(), &commits[0]);
        pack_objects(
            dir.path(),
            &[&commits[0], &commits[1]],
            Some(Delta::OutsideId),
        );
        let loose = dir
            .path()
            .join(".git/objects")
            .join(&commits[0][..2])
            .join(&commits[0][2..]);
        std::fs::remove_file(loose).unwrap();
        let slots = Slots::Given(HEAD_INDEX_SLOTS);
        let repo = git_sources::open_with_slots(dir.path(), HEAD_OBJECT_BYTES, slots).unwrap();
        assert!(matches!(
            git_sources::store_object_kind(&repo, &oid(&commits[1])),
            Ok(None)
        ));
        write_pack(dir.path(), &[base], None);
        pack_blobs(dir.path(), HEAD_INDEX_SLOTS);
        assert!(git_sources::store_object_kind(&repo, &oid(&commits[1])).is_err());
    }

    #[test]
    fn the_newest_pack_index_answers_and_a_deleted_hint_falls_back_to_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let commits = repository(dir.path(), 1);
        let base = stored_object(dir.path(), &commits[0]);
        let older = pack_objects(dir.path(), &[&commits[0]], None);
        std::fs::File::options()
            .write(true)
            .open(&older)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH)
            .unwrap();
        let extra = b"extra\n".to_vec();
        let extra_id =
            gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::object::Kind::Blob, &extra)
                .unwrap();
        let newest = write_pack(
            dir.path(),
            &[base, (extra_id, gix::object::Kind::Blob, extra)],
            None,
        );
        let hint = Mutex::new(None);
        let kind = git_sources::file_object_kind(&open(dir.path()), &oid(&commits[0]), &hint);
        assert_eq!(kind, COMMIT);
        assert_eq!(*hint.lock().unwrap(), Some(newest.clone()));
        std::fs::remove_file(&newest).unwrap();
        std::fs::remove_file(newest.with_extension("pack")).unwrap();
        let kind = git_sources::file_object_kind(&open(dir.path()), &oid(&commits[0]), &hint);
        assert_eq!(kind, COMMIT);
        assert_eq!(*hint.lock().unwrap(), Some(older));
    }

    #[test]
    fn a_head_commit_in_an_alternate_freezes_when_the_store_holds_more_indices_than_its_first_slots()
     {
        let dir = tempfile::tempdir().unwrap();
        let alternate = dir.path().join("alternate");
        let commits = repository(&alternate, 1);
        pack_objects(&alternate, &[&commits[0]], None);
        let root = dir.path().join("root");
        repository(&root, 0);
        std::fs::write(
            root.join(".git/objects/info/alternates"),
            format!("{}\n", alternate.join(".git/objects").display()),
        )
        .unwrap();
        std::fs::write(
            root.join(".git/refs/heads/main"),
            format!("{}\n", commits[0]),
        )
        .unwrap();
        pack_blobs(&root, HEAD_INDEX_SLOTS);
        let slots = Slots::Given(HEAD_INDEX_SLOTS);
        let repo = git_sources::open_with_slots(&root, HEAD_OBJECT_BYTES, slots).unwrap();
        assert!(git_sources::store_object_kind(&repo, &oid(&commits[0])).is_err());
        assert_eq!(observe_head(&root), FrozenHead::Commit(commits[0].clone()));
    }

    #[test]
    fn frozen_heads_serialize_as_tagged_values_and_round_trip() {
        assert_eq!(
            serde_json::to_string(&FrozenHead::Commit("ab".repeat(20))).unwrap(),
            format!("{{\"commit\":\"{}\"}}", "ab".repeat(20))
        );
        assert_eq!(
            serde_json::to_string(&FrozenHead::HeadUnavailable).unwrap(),
            "\"head_unavailable\""
        );
        for value in [
            FrozenHead::Commit("cd".repeat(20)),
            FrozenHead::HeadUnavailable,
        ] {
            let text = serde_json::to_string(&value).unwrap();
            assert_eq!(serde_json::from_str::<FrozenHead>(&text).unwrap(), value);
        }
    }
}

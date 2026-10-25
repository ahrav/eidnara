use std::num::NonZeroU64;
use std::path::Path;

use serde::{Deserialize, Serialize};

const HEAD_OBJECT_BYTES: NonZeroU64 = NonZeroU64::new(1 << 20).unwrap();

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FrozenHead {
    /// The 40-character lowercase hex id of the commit HEAD names, directly
    /// when detached or through its branch.
    Commit(String),
    /// An unborn or unreadable HEAD, an unsupported object format, or a
    /// repository-open failure.
    #[default]
    HeadUnavailable,
}

pub(crate) fn observe_head(root: &Path) -> FrozenHead {
    let Ok(repo) = crate::git_sources::open(root, HEAD_OBJECT_BYTES) else {
        return FrozenHead::HeadUnavailable;
    };
    if crate::git_sources::object_format(repo.object_hash()).is_err() {
        return FrozenHead::HeadUnavailable;
    }
    let Some(id) = repo.head().ok().and_then(|head| head.id()) else {
        return FrozenHead::HeadUnavailable;
    };
    let is_commit = repo
        .find_header(id)
        .is_ok_and(|header| header.kind() == gix::object::Kind::Commit);
    let hex = id.to_hex().to_string();
    if is_commit
        && hex.len() == 40
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        FrozenHead::Commit(hex)
    } else {
        FrozenHead::HeadUnavailable
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

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
        detach(dir.path(), &commits[0]);
        assert_eq!(
            observe_head(dir.path()),
            FrozenHead::Commit(commits[0].clone())
        );
    }

    #[test]
    fn unborn_unsupported_non_commit_and_non_repository_roots_are_unavailable() {
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
        for root in [unborn, plain, sha256, tree_head, dir.path().join("missing")] {
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
    fn the_observation_reads_one_object_header_and_neither_the_index_nor_the_tree() {
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

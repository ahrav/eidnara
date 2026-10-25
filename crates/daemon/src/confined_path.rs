use std::os::fd::{BorrowedFd, OwnedFd};

use rustix::fs::{Mode, OFlags, ResolveFlags};

pub(crate) const RESOLVE: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

pub(crate) const MAX_COMPONENTS: usize = 16;

/// The project's own Eidnara directory and its Git store. A case-folding filesystem resolves any
/// ASCII letter case of either name to the same directory, so [`is_protected_component`] matches
/// every case.
const PROTECTED_COMPONENTS: [&str; 2] = [".eidnara", ".git"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathRefusal {
    Invalid,
    Protected,
}

pub(crate) fn is_protected_component(name: &str) -> bool {
    PROTECTED_COMPONENTS
        .iter()
        .any(|protected| name.eq_ignore_ascii_case(protected))
}

pub(crate) fn check(relative: &str) -> Result<(), PathRefusal> {
    if relative.contains('\0') {
        return Err(PathRefusal::Invalid);
    }
    for (index, component) in relative.split('/').enumerate() {
        if component.is_empty() || component == "." || component == ".." {
            return Err(PathRefusal::Invalid);
        }
        if is_protected_component(component) {
            return Err(PathRefusal::Protected);
        }
        if index == MAX_COMPONENTS {
            return Err(PathRefusal::Invalid);
        }
    }
    Ok(())
}

pub(crate) fn open_beneath(
    dir: BorrowedFd<'_>,
    relative: &str,
    flags: OFlags,
) -> rustix::io::Result<OwnedFd> {
    rustix::fs::openat2(dir, relative, flags, Mode::empty(), RESOLVE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_ordinary_components_beneath_the_root() {
        let sixteen = vec!["d"; MAX_COMPONENTS].join("/");
        for accepted in [
            "a",
            "a/b.md",
            "docs/.github/x",
            "a/.gitignore",
            sixteen.as_str(),
        ] {
            assert_eq!(check(accepted), Ok(()), "{accepted:?}");
        }
        let seventeen = vec!["d"; MAX_COMPONENTS + 1].join("/");
        for (refused, refusal) in [
            ("", PathRefusal::Invalid),
            ("/etc/passwd", PathRefusal::Invalid),
            ("a/", PathRefusal::Invalid),
            ("a//b", PathRefusal::Invalid),
            ("./a", PathRefusal::Invalid),
            ("a/../b", PathRefusal::Invalid),
            ("a\0b", PathRefusal::Invalid),
            (seventeen.as_str(), PathRefusal::Invalid),
            (".git", PathRefusal::Protected),
            ("src/.git", PathRefusal::Protected),
            (".GIT/HEAD", PathRefusal::Protected),
            ("a/.Eidnara/x", PathRefusal::Protected),
            (".eidnara/eidnara.jsonc", PathRefusal::Protected),
        ] {
            assert_eq!(check(refused), Err(refusal), "{refused:?}");
        }
    }

    #[test]
    fn opens_resolve_with_every_confinement_flag() {
        assert_eq!(
            RESOLVE,
            ResolveFlags::BENEATH
                | ResolveFlags::NO_SYMLINKS
                | ResolveFlags::NO_MAGICLINKS
                | ResolveFlags::NO_XDEV
        );
    }
}

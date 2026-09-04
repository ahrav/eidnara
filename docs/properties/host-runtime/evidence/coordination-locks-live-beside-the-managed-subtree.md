# `coordination-locks-live-beside-the-managed-subtree`

- **Discovery:** U3, when the coordination directory was renamed to carry the product name.
- **Primary evidence:** `COORDINATION_DIR_NAME` in `crates/host-runtime/src/lifecycle.rs` and `coordination_dir_path`, which joins it to the data root rather than to the managed subtree. `independent_openers_see_one_stable_coordination_identity` spells `.eidnara-coordination/transaction.lock` as a literal, records the file's `(dev, ino)`, renames the managed subtree away, reacquires, and asserts the same identity.
- **Existing evidence:** the test named above and the replaced-subtree tests in the same module (`a_replaced_eidnara_subtree_cannot_admit_an_overlapping_incarnation`, `a_replaced_eidnara_subtree_is_not_reported_stopped_while_the_daemon_lives`).
- **Failure scenario:** a lock under the replaceable subtree.
- **Timing window:** the rename happens between two acquisitions.
- **Instrumentation:** `(dev, ino)` of the lock file.
- **Audit verdict (U3):** pass. The path is spelled in the test, not read from the constant; the identity assertion is independent of the path spelling. The registry `host-locks` family records the same paths.
- **Open-question log:** the directory is a hidden sibling of the managed subtree, outside `<root>/eidnara`; the cutover isolation probe must digest it separately from the predecessor's coordination directory, whose name differs.

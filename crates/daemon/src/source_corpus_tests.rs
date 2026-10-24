use std::path::Path;

use serde_json::{Value, json};

use super::source_corpus::{
    CORPUS_SHA256, ExpectedRecord, ExpectedTool, ExpectedUnit, Instructions, Layout, LossCondition,
    Manifest, NonUnit, NonUnitReason, corpus_digest, corpus_dir, corpus_files, generated_bytes,
    git, manifests, materialize, read_pi_session, redaction_violations, verify_corpus_digest,
};

/// The root commit materializing `repos/ripgrep.json` yields under the fixed git identity and dates.
const RIPGREP_ROOT: &str = "40cf2c897a3cc5b4c2cd5e4c564b44bfb57447ae";

const CANARY: &str = "sk-ant-api03-Zk9Qw2Lm7Pv4Rt8Zw1Yc6Nb3Hd5Kf0JgAbCdEfGh";

const TYPESCRIPT_PEER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/e2e-tests/src/source-corpus/corpus.ts"
);

const TYPESCRIPT_READER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/e2e-tests/src/source-corpus/opencode-db-reader.ts"
);

fn manifest(name: &str) -> Manifest {
    manifests(&corpus_dir())
        .unwrap()
        .into_iter()
        .find(|(path, _)| path == &format!("repos/{name}.json"))
        .map(|(_, manifest)| manifest)
        .unwrap_or_else(|| panic!("no manifest {name}"))
}

fn tree(root: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                paths.push(relative);
            }
        }
    }
    paths.sort();
    paths
}

#[test]
fn the_committed_corpus_hashes_to_the_pin_both_languages_hold() {
    let files = corpus_files(&corpus_dir()).unwrap();
    assert!(files.iter().any(|(path, _)| path == "repos/eidnara.json"));
    assert!(files.iter().all(|(path, _)| path != "README.md"));
    assert_eq!(verify_corpus_digest(&files), Ok(()));

    let peer = std::fs::read_to_string(TYPESCRIPT_PEER).unwrap();
    let pin = peer
        .split("SOURCE_CORPUS_SHA256 =")
        .nth(1)
        .and_then(|rest| rest.split('"').nth(1))
        .expect("the TypeScript peer pins the digest");
    assert_eq!(pin, CORPUS_SHA256);
}

#[test]
fn one_changed_byte_without_a_pin_move_fails_the_digest() {
    let mut files = corpus_files(&corpus_dir()).unwrap();
    let (_, bytes) = files
        .iter_mut()
        .find(|(path, _)| path == "repos/ripgrep.json")
        .unwrap();
    let at = bytes.iter().position(|&b| b == b'5').unwrap();
    bytes[at] = b'6';
    let actual = verify_corpus_digest(&files).unwrap_err();
    assert_ne!(actual, CORPUS_SHA256);
}

#[test]
fn the_digest_sorts_paths_by_bytes_and_normalizes_crlf() {
    let lf = vec![
        ("b".to_owned(), b"x\ny\n".to_vec()),
        ("B".to_owned(), b"z".to_vec()),
    ];
    let crlf = vec![
        ("B".to_owned(), b"z".to_vec()),
        ("b".to_owned(), b"x\r\ny\r\n".to_vec()),
    ];
    assert_eq!(corpus_digest(&lf), corpus_digest(&crlf));
    let expected = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(b"B\nzb\nx\ny\n"))
    };
    assert_eq!(corpus_digest(&lf), expected);
    let lone_cr = vec![("b".to_owned(), b"x\ry".to_vec())];
    assert_ne!(
        corpus_digest(&lone_cr),
        corpus_digest(&[("b".to_owned(), b"x\ny".to_vec())])
    );
}

#[test]
fn every_manifest_parses_and_declares_the_condition_its_documents_hold() {
    let all = manifests(&corpus_dir()).unwrap();
    assert_eq!(
        all.iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        ["repos/eidnara.json", "repos/ripgrep.json"]
    );
    let eidnara = manifest("eidnara");
    assert_eq!(eidnara.layout, Layout::Eidnara);
    assert_eq!(eidnara.instructions, Instructions::NestedDepth3);
    let ripgrep = manifest("ripgrep");
    assert_eq!(ripgrep.layout, Layout::RustWorkspace);
    assert_eq!(ripgrep.instructions, Instructions::None);
    assert!(ripgrep.documents.is_empty());
}

#[test]
fn a_manifest_out_of_order_or_naming_an_unheld_condition_is_refused() {
    let base = json!({
        "schema": "eidnara.source-corpus/v1",
        "name": "t",
        "layout": "go_module",
        "instructions": "root_only",
        "upstream": {"repository": "https://example.invalid/t", "commit": "0".repeat(40), "license": "MIT"},
        "exclusions": [],
        "documents": [{"path": "AGENTS.md", "text": "a"}],
        "files": [{"path": "a", "size": 1, "utf8": true}, {"path": "b", "symlink": "a"}],
    });
    assert!(Manifest::parse(base.to_string().as_bytes()).is_ok());
    let edited = |edit: &dyn Fn(&mut Value)| {
        let mut value = base.clone();
        edit(&mut value);
        Manifest::parse(value.to_string().as_bytes())
    };
    let refused = |reason: &str, edit: &dyn Fn(&mut Value)| {
        let error = edited(edit).unwrap_err();
        assert!(error.contains(reason), "{error:?} names {reason:?}");
    };
    refused("schema", &|v| {
        v["schema"] = json!("eidnara.source-corpus/v2")
    });
    refused("out of path-byte order", &|v| {
        v["files"] = json!([
            {"path": "b", "size": 1, "utf8": true},
            {"path": "a", "size": 1, "utf8": true},
        ]);
    });
    refused("not a normalized relative path", &|v| {
        v["files"] = json!([{"path": "../a", "size": 1, "utf8": true}]);
    });
    refused("not a normalized relative path", &|v| {
        v["files"] = json!([{"path": ".git/hooks/prepare-commit-msg", "symlink": "/bin/sh"}]);
    });
    refused("not a normalized relative path", &|v| {
        v["files"] = json!([{"path": "a/.GIT/config", "size": 1, "utf8": true}]);
    });
    refused("is an instruction document", &|v| {
        v["files"] = json!([{"path": "x/AGENTS.md", "size": 1, "utf8": true}]);
    });
    refused("beneath the symlink", &|v| {
        v["files"] =
            json!([{"path": "d", "symlink": "/tmp"}, {"path": "d/x", "size": 1, "utf8": true}]);
    });
    refused("beneath the symlink", &|v| {
        v["files"] =
            json!([{"path": "D", "symlink": "/tmp"}, {"path": "d/x", "size": 1, "utf8": true}]);
    });
    refused("recorded as non-UTF-8", &|v| {
        v["files"] = json!([{"path": "a", "size": 0, "utf8": false}]);
    });
    refused("manifest declares None", &|v| {
        v["instructions"] = json!("none")
    });
    refused("documents hold None", &|v| {
        v["documents"] = json!([{"path": "docs/AGENTS.md", "text": "a"}]);
    });
    refused("not a blank line", &|v| {
        v["exclusions"] = json!([{"path": "AGENTS.md", "line": 1, "reason": "r"}]);
    });
    refused("full lowercase oid", &|v| {
        v["upstream"]["commit"] = json!("ABC")
    });
    refused("no license", &|v| v["upstream"]["license"] = json!(""));
    refused("unknown field", &|v| v["extra"] = json!(1));

    let held = |documents: Value, instructions: &str| {
        edited(&|v| {
            v["documents"] = documents.clone();
            v["instructions"] = json!(instructions);
        })
    };
    let mixed = json!([
        {"path": "a/.cursorrules", "text": "c"},
        {"path": "a/AGENTS.md", "text": "a"},
        {"path": "a/CLAUDE.md", "text": "b"},
    ]);
    assert!(held(mixed, "mixed").is_ok());
    let deep = json!([{"path": "a/b/AGENTS.md", "text": "a"}]);
    assert!(held(deep, "nested_depth_3").is_ok());
    let shallow = json!([{"path": "a/AGENTS.md", "text": "a"}]);
    assert!(held(shallow, "nested_depth_3").is_err());
    let exclusion = edited(&|v| {
        v["documents"] = json!([{"path": "AGENTS.md", "text": "a\n\nb"}]);
        v["exclusions"] = json!([{"path": "AGENTS.md", "line": 2, "reason": "a fake key"}]);
    });
    assert!(exclusion.is_ok(), "{exclusion:?}");
}

#[test]
fn materializing_eidnara_yields_its_eight_agents_documents_and_no_architecture_document() {
    let manifest = manifest("eidnara");
    let checkout = materialize(&manifest, false).unwrap();
    let paths = tree(&checkout.root);
    let agents: Vec<&String> = paths.iter().filter(|p| p.ends_with("AGENTS.md")).collect();
    assert_eq!(
        agents,
        [
            "AGENTS.md",
            "crates/host-runtime/AGENTS.md",
            "crates/retrieval/AGENTS.md",
            "crates/shm-transport/AGENTS.md",
            "crates/storage/AGENTS.md",
            "docs/properties/AGENTS.md",
            "packages/opencode-plugin/AGENTS.md",
            "packages/shm-native/AGENTS.md",
        ]
    );
    assert!(!paths.iter().any(|p| p.ends_with("ARCHITECTURE.md")));
    assert_eq!(paths.len(), manifest.documents.len() + manifest.files.len());
    let root_agents = std::fs::read_to_string(checkout.root.join("AGENTS.md")).unwrap();
    assert_eq!(root_agents, manifest.documents[0].text);
    let tracked = git(&checkout.root, &["ls-files"]).unwrap();
    assert_eq!(tracked.lines().count(), paths.len());
}

#[test]
fn a_non_git_layout_materializes_without_a_repository() {
    let mut plain = manifest("ripgrep");
    plain.layout = Layout::NonGit;
    let checkout = materialize(&plain, false).unwrap();
    assert!(!checkout.root.join(".git").exists());
    assert_eq!(
        tree(&checkout.root),
        tree(&materialize(&manifest("ripgrep"), false).unwrap().root)
    );
    let error = match materialize(&plain, true) {
        Ok(_) => panic!("a non_git layout accepted a worktree"),
        Err(error) => error,
    };
    assert!(error.contains("non_git"), "{error:?}");
}

#[test]
fn a_materialized_worktree_shares_its_checkout_root_commit() {
    let checkout = materialize(&manifest("ripgrep"), true).unwrap();
    let worktree = checkout.worktree.as_deref().unwrap();
    let roots = |dir: &Path| git(dir, &["rev-list", "--max-parents=0", "HEAD"]).unwrap();
    assert_ne!(checkout.root, worktree);
    assert_eq!(roots(&checkout.root).lines().count(), 1);
    assert_eq!(roots(&checkout.root), roots(worktree));
    let link = std::fs::read_link(checkout.root.join("HomebrewFormula")).unwrap();
    assert_eq!(link, Path::new("pkg/brew"));
    let again = materialize(&manifest("ripgrep"), false).unwrap();
    assert_eq!(roots(&again.root), roots(&checkout.root));
    assert_eq!(roots(&checkout.root).trim(), RIPGREP_ROOT);
    let format = git(&checkout.root, &["rev-parse", "--show-object-format"]).unwrap();
    assert_eq!(format.trim(), "sha1");
}

#[test]
fn generated_bytes_keep_the_recorded_size_and_utf8_validity() {
    for size in [0, 1, 21, 22, 4096] {
        let text = generated_bytes(size, true);
        assert_eq!(text.len() as u64, size);
        assert!(std::str::from_utf8(&text).is_ok());
        let binary = generated_bytes(size, false);
        assert_eq!(binary.len() as u64, size);
        assert_eq!(std::str::from_utf8(&binary).is_err(), size > 0);
    }
}

fn pi_entry(id: &str, message: Value) -> String {
    json!({
        "type": "message",
        "id": id,
        "parentId": "p",
        "timestamp": "2026-01-01T00:00:00.000Z",
        "message": message,
    })
    .to_string()
}

#[test]
fn the_pi_reader_classifies_an_image_bearing_tool_result_whole() {
    let image = json!({"type": "image", "data": "AAAA", "mimeType": "image/png"});
    let session = [
        json!({"type": "session", "id": "s", "version": 3, "cwd": "/w"}).to_string(),
        pi_entry(
            "u1",
            json!({"role": "user", "content": [{"type": "text", "text": "look"}, image], "timestamp": 1}),
        ),
        pi_entry(
            "a1",
            json!({"role": "assistant", "content": [
                {"type": "thinking", "thinking": "hm"},
                {"type": "text", "text": "reading"},
                {"type": "toolCall", "id": "c1", "name": "read", "arguments": {}},
            ], "timestamp": 2}),
        ),
        pi_entry(
            "t1",
            json!({"role": "toolResult", "toolCallId": "c1", "toolName": "read", "isError": false,
                "content": [{"type": "text", "text": "pixels"}, image], "timestamp": 3}),
        ),
        pi_entry(
            "t2",
            json!({"role": "toolResult", "toolCallId": "c2", "toolName": "bash", "isError": true,
                "content": [{"type": "text", "text": "boom"}],
                "details": {"truncation": {"truncated": true}}, "timestamp": 4}),
        ),
        pi_entry("u2", json!({"role": "user", "content": "plain", "timestamp": 5})),
        json!({"type": "model_change", "id": "m", "parentId": "u2"}).to_string(),
        pi_entry(
            "t3",
            json!({"role": "toolResult", "toolCallId": "c3", "toolName": "read", "isError": false,
                "content": [image, {"type": "text", "text": "after"}], "timestamp": 6}),
        ),
        pi_entry(
            "t4",
            json!({"role": "toolResult", "toolCallId": "c4", "toolName": "read", "isError": false,
                "content": [{"type": "text", "text": "ab"}, {"type": "text", "text": "cde"}],
                "timestamp": 7}),
        ),
    ]
    .join("\n");
    let records = read_pi_session(&session).unwrap();
    let text = |index, byte_len| ExpectedUnit {
        index,
        byte_len,
        tool: None,
    };
    let skipped = |index, kind: &str| NonUnit {
        index,
        reason: NonUnitReason::SkippedItem,
        kind: kind.to_owned(),
    };
    let header = NonUnit {
        index: None,
        reason: NonUnitReason::NonMessageEntry,
        kind: "session".to_owned(),
    };
    assert_eq!(records[0].non_units, [header]);
    assert_eq!(records[1].units, [text(0, 4)]);
    assert_eq!(records[1].non_units, [skipped(Some(1), "image")]);
    assert_eq!(records[2].units, [text(1, 7)]);
    assert_eq!(
        records[2].non_units,
        [skipped(Some(0), "thinking"), skipped(Some(2), "toolCall")]
    );
    assert_eq!(
        records[3],
        ExpectedRecord {
            id: "t1".to_owned(),
            role: Some("toolResult".to_owned()),
            units: Vec::new(),
            non_units: vec![NonUnit {
                index: None,
                reason: NonUnitReason::NonTextItem,
                kind: "image".to_owned(),
            }],
        }
    );
    assert_eq!(NonUnitReason::NonTextItem.name(), "non_text_item");
    assert_eq!(
        records[4].units,
        [ExpectedUnit {
            index: 0,
            byte_len: 4,
            tool: Some(ExpectedTool {
                name: "bash".to_owned(),
                call_id: "c2".to_owned(),
                is_error: true,
                exit: None,
                truncated: Some(true),
            }),
        }]
    );
    assert_eq!(records[5].units, [text(0, 5)]);
    assert_eq!(records[7].non_units[0].reason, NonUnitReason::NonTextItem);
    assert!(records[7].units.is_empty());
    assert_eq!(
        records[8]
            .units
            .iter()
            .map(|u| (u.index, u.byte_len))
            .collect::<Vec<_>>(),
        [(0, 2), (1, 3)]
    );
    assert_eq!(records[6].non_units[0].kind, "model_change");
    let expected: usize = records.iter().map(|r| r.units.len()).sum();
    assert_eq!(expected, 6);
}

#[test]
fn the_pi_reader_classifies_a_bash_execution_message_whole_and_reads_its_siblings() {
    let session = [
        pi_entry("u1", json!({"role": "user", "content": "run it", "timestamp": 1})),
        pi_entry(
            "b1",
            json!({"role": "bashExecution", "command": "git status", "output": "clean",
                "exitCode": 0, "cancelled": false, "truncated": false, "timestamp": 2}),
        ),
        pi_entry(
            "a1",
            json!({"role": "assistant", "content": [{"type": "text", "text": "done"}], "timestamp": 3}),
        ),
    ]
    .join("\n");
    let records = read_pi_session(&session).unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].units.len(), 1);
    assert_eq!(records[1].role.as_deref(), Some("bashExecution"));
    assert!(records[1].units.is_empty());
    assert_eq!(
        records[1]
            .non_units
            .iter()
            .map(|n| (n.index, n.reason.name(), n.kind.as_str()))
            .collect::<Vec<_>>(),
        [(None, "uncaptured_role", "bashExecution")]
    );
    assert_eq!(records[2].units.len(), 1);
}

#[test]
fn the_pi_reader_refuses_an_entry_missing_a_field_pi_always_writes() {
    let refused = |jsonl: &str, reason: &str| {
        let error = read_pi_session(jsonl).unwrap_err();
        assert!(error.contains(reason), "{error:?} names {reason:?}");
    };
    refused("{not json", "line 1");
    let no_id = json!({"type": "message", "message": {"role": "user", "content": "x"}});
    refused(&no_id.to_string(), "`id`");
    let no_error_flag = pi_entry(
        "t",
        json!({"role": "toolResult", "toolCallId": "c", "toolName": "read", "content": []}),
    );
    refused(&no_error_flag, "`isError`");
}

#[test]
fn every_fixture_string_is_redaction_negative_outside_canary_units() {
    let files = corpus_files(&corpus_dir()).unwrap();
    let mut violations = Vec::new();
    for (path, bytes) in &files {
        let value: Value = serde_json::from_slice(bytes).unwrap();
        violations.extend(redaction_violations(path, &value));
    }
    assert_eq!(violations, Vec::<String>::new());
}

#[test]
fn a_credential_shape_fails_the_redaction_scan_unless_its_unit_is_a_canary() {
    let plain = json!({"units": [{"text": CANARY}]});
    assert_eq!(redaction_violations("f", &plain).len(), 1);
    let canary = json!({"units": [{"canary": true, "text": CANARY, "id": "u"}]});
    assert_eq!(redaction_violations("f", &canary), Vec::<String>::new());
    let elsewhere = json!({"units": [{"canary": true, "text": CANARY, "note": CANARY}]});
    assert_eq!(redaction_violations("f", &elsewhere).len(), 1);
    let negative_canary = json!({"units": [{"canary": true, "text": "ordinary"}]});
    assert_eq!(redaction_violations("f", &negative_canary).len(), 1);
    let textless = json!({"units": [{"canary": true}]});
    assert_eq!(redaction_violations("f", &textless).len(), 1);
    let key = json!({ CANARY: "value" });
    assert_eq!(redaction_violations("f", &key).len(), 1);
}

#[test]
fn the_loss_conditions_are_a_closed_set_of_distinct_names() {
    let names: Vec<&str> = LossCondition::ALL.iter().map(|c| c.name()).collect();
    assert_eq!(
        names,
        [
            "adapter_refused",
            "unit_too_large",
            "deferred",
            "secret_bearing",
            "re_admission_blocked",
            "refused_retryable",
            "identity_conflict",
            "superseded",
            "window_refused",
            "pass_failed",
        ]
    );
}

#[test]
fn both_readers_name_the_same_closed_non_unit_reasons() {
    let reader = std::fs::read_to_string(TYPESCRIPT_READER).unwrap();
    let union = reader
        .split("export type NonUnitReason =")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .expect("the TypeScript reader declares the reasons");
    let typescript: Vec<&str> = union.split('"').skip(1).step_by(2).collect();
    let rust: Vec<&str> = NonUnitReason::ALL.iter().map(|r| r.name()).collect();
    assert_eq!(typescript, rust);
}

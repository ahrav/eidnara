use proptest::prelude::*;
use serde_json::{Value, json};

use crate::harness_sources::Harness;
use crate::tool_vocabulary::{
    DisplayArg, NameRule, TABLED_NAMES, Tier, ToolClass, classify, diff_keys, display_arg,
    exit_code, is_todo_tool, is_zero_value_note_call, path_arg, tier, truncated_flag,
};

const TRACE: &str = include_str!("../testdata/opencode-tool-outcome-trace.json");

/// Selection uses normalized classes and tiers; the injection read-back matches the to-do name
/// ignoring ASCII case; outcome extraction requires an OpenCode part named exactly `bash` with an
/// integer `exit`.
#[test]
fn each_consumer_keeps_its_name_rule() {
    use ToolClass::{Edit, EidnaraNote, EidnaraReduce, Tabled, TodoWrite, Unknown, ZeroValueMeta};
    let table: [(&str, ToolClass, Tier, bool, bool); 25] = [
        ("Edit", Edit, Tier::T2, false, false),
        ("mcp_edit", Edit, Tier::T2, false, false),
        ("MCP_TodoWrite", TodoWrite, Tier::T1, false, false),
        ("Bash", Tabled, Tier::T3, false, false),
        ("edit", Edit, Tier::T2, false, false),
        ("write", Edit, Tier::T2, false, false),
        ("todowrite", TodoWrite, Tier::T1, true, false),
        ("bash", Tabled, Tier::T3, false, true),
        ("bash_status", ZeroValueMeta, Tier::T3, false, false),
        ("MCP_BASH_KILL", ZeroValueMeta, Tier::T3, false, false),
        ("read", Tabled, Tier::T1, false, false),
        ("task", Tabled, Tier::T1, false, false),
        ("aft_outline", Tabled, Tier::T1, false, false),
        ("aft_zoom", Tabled, Tier::T1, false, false),
        ("apply_patch", Tabled, Tier::T2, false, false),
        ("grep", Tabled, Tier::T2, false, false),
        ("glob", Tabled, Tier::T2, false, false),
        ("aft_search", Tabled, Tier::T2, false, false),
        ("mcp_foo", Unknown, Tier::T3, false, false),
        ("", Unknown, Tier::T3, false, false),
        ("mcp_", Unknown, Tier::T3, false, false),
        ("eidnara_reduce", EidnaraReduce, Tier::T3, false, false),
        ("eidnara_note", EidnaraNote, Tier::T3, false, false),
        ("MCP_READ", Tabled, Tier::T1, false, false),
        ("mcp_mcp_edit", Unknown, Tier::T3, false, false),
    ];
    for (name, class, expected_tier, todo_ignoring_case, exit) in table {
        assert_eq!(classify(name), class, "{name:?}");
        assert_eq!(tier(name), expected_tier, "{name:?}");
        assert_eq!(
            is_todo_tool(name, NameRule::IgnoreAsciiCase),
            todo_ignoring_case,
            "{name:?}"
        );
        assert_eq!(
            is_todo_tool(name, NameRule::Normalized),
            class == TodoWrite,
            "{name:?}"
        );
        let metadata = json!({"exit": 1});
        assert_eq!(
            exit_code(Harness::OpenCode, name, Some(&metadata)).is_some(),
            exit,
            "{name:?}"
        );
        assert_eq!(exit_code(Harness::Pi, name, Some(&metadata)), None);
    }
}

/// Wire extraction and edit supersession read one path key order. For paths, the summary
/// formatter reads `filePath` before `path` and truncates values to 60 characters.
#[test]
fn each_site_keeps_its_key_order() {
    let long = "a/".repeat(40);
    let long_shown = format!("{}…", &long[..60]);
    let table: [(Value, Option<&str>, Option<&str>); 14] = [
        (
            json!({"path": "src/a.rs"}),
            Some("src/a.rs"),
            Some("src/a.rs"),
        ),
        (
            json!({"filePath": "src/a.rs"}),
            Some("src/a.rs"),
            Some("src/a.rs"),
        ),
        (json!({"file_path": "src/a.rs"}), Some("src/a.rs"), None),
        (json!({"path": "p", "filePath": "f"}), Some("f"), Some("f")),
        (json!({"path": "p", "file_path": "f"}), Some("f"), Some("p")),
        (
            json!({"filePath": 1, "file_path": "f", "path": "p"}),
            Some("f"),
            Some("p"),
        ),
        (json!({"pattern": "x*"}), None, Some("x*")),
        (json!({"query": "q"}), None, Some("q")),
        (json!({"symbol": "S", "path": "p"}), Some("p"), Some("p")),
        (json!({"module": "m"}), None, Some("m")),
        (json!({"action": "read"}), None, Some("read")),
        (
            json!({"filePath": long}),
            Some(long.as_str()),
            Some(long_shown.as_str()),
        ),
        (json!({"description": "d"}), None, None),
        (json!("str"), None, None),
    ];
    for (input, path, display) in &table {
        assert_eq!(path_arg(input), *path, "{input}");
        assert_eq!(
            crate::chunk_text::extract_key_arg(input).as_deref(),
            *display,
            "{input}"
        );
    }
    assert_eq!(
        diff_keys(),
        [
            "oldString",
            "newString",
            "content",
            "old_string",
            "new_string"
        ]
    );
    assert!(
        diff_keys()
            .iter()
            .all(|key| path_arg(&json!({ *key: "x" })).is_none())
    );
}

#[test]
fn pi_and_opencode_path_keys_name_one_path() {
    assert_eq!(path_arg(&json!({"path": "src/a.rs"})), Some("src/a.rs"));
    assert_eq!(path_arg(&json!({"filePath": "src/a.rs"})), Some("src/a.rs"));
}

#[test]
fn an_unknown_tool_takes_every_default() {
    let input = json!({"filePath": "src/a.rs", "oldString": "a"});
    assert_eq!(classify("mcp_foo"), ToolClass::Unknown);
    assert_eq!(tier("mcp_foo"), Tier::T3);
    assert_eq!(display_arg(&input), Some(DisplayArg::PathLike("src/a.rs")));
    assert_eq!(
        exit_code(Harness::OpenCode, "mcp_foo", Some(&json!({"exit": 1}))),
        None
    );
}

#[test]
fn exit_codes_come_only_from_opencode_bash_integers() {
    let trace: Value = serde_json::from_str(TRACE).unwrap();
    let cases = trace["cases"].as_array().unwrap();
    let observed: Vec<(&str, Option<i64>)> = cases
        .iter()
        .map(|case| {
            let part = &case["part"];
            let name = part["tool"].as_str().unwrap();
            (
                name,
                exit_code(Harness::OpenCode, name, part["state"].get("metadata")),
            )
        })
        .collect();
    assert_eq!(
        observed,
        [
            ("bash", Some(1)),
            ("bash", Some(0)),
            ("bash", None),
            ("bash", None),
            ("bash", None),
            ("bash", None),
            ("read", None),
        ]
    );
    let exit_zero = json!({"exit": 0});
    assert_eq!(exit_code(Harness::OpenCode, "read", Some(&exit_zero)), None);
    assert_eq!(
        exit_code(Harness::OpenCode, "bash", Some(&json!({"exit": "1"}))),
        None
    );
    assert_eq!(
        exit_code(Harness::OpenCode, "bash", Some(&json!({"exit": 1.5}))),
        None
    );
    assert_eq!(exit_code(Harness::Pi, "bash", Some(&exit_zero)), None);
}

#[test]
fn truncation_flags_are_read_from_each_harness_field() {
    let state = |metadata: Value| json!({"status": "completed", "metadata": metadata});
    assert_eq!(
        truncated_flag(Harness::OpenCode, &state(json!({"truncated": true}))),
        Some(true)
    );
    assert_eq!(
        truncated_flag(Harness::OpenCode, &state(json!({"truncated": "yes"}))),
        None
    );
    assert_eq!(
        truncated_flag(Harness::OpenCode, &json!({"status": "completed"})),
        None
    );
    let message = json!({"role": "toolResult", "details": {"truncation": {"truncated": false}}});
    assert_eq!(truncated_flag(Harness::Pi, &message), Some(false));
    assert_eq!(truncated_flag(Harness::OpenCode, &message), None);
}

#[test]
fn zero_value_note_actions_are_read_and_dismiss() {
    assert!(is_zero_value_note_call(&json!({"action": "read"})));
    assert!(is_zero_value_note_call(&json!({"action": "dismiss"})));
    assert!(!is_zero_value_note_call(&json!({"action": "write"})));
    assert!(!is_zero_value_note_call(&json!({"action": 1})));
    assert!(!is_zero_value_note_call(&json!("read")));
}

#[test]
fn every_tabled_name_reaches_a_non_unknown_answer() {
    for name in TABLED_NAMES.iter().flat_map(|table| table.iter()) {
        assert_ne!(classify(name), ToolClass::Unknown, "{name}");
        assert_ne!(
            classify(&format!("MCP_{name}")),
            ToolClass::Unknown,
            "{name}"
        );
    }
    assert!(is_todo_tool("todowrite", NameRule::Exact));
    assert_eq!(
        exit_code(Harness::OpenCode, "bash", Some(&json!({"exit": 3}))),
        Some(3)
    );
}

#[test]
fn edge_names_classify_as_unknown() {
    for name in [
        String::new(),
        "mcp_foo".to_owned(),
        "x".repeat(10 * 1024),
        "read\0".to_owned(),
        "mcp_\u{e9}dit".to_owned(),
    ] {
        assert_eq!(classify(&name), ToolClass::Unknown, "{name:?}");
        assert_eq!(tier(&name), Tier::T3, "{name:?}");
    }
    assert_eq!(classify("MCP_READ"), ToolClass::Tabled);
}

proptest! {
    #[test]
    fn classify_is_total_and_deterministic(name in any::<String>()) {
        let class = classify(&name);
        prop_assert_eq!(class, classify(&name));
        if class == ToolClass::Unknown {
            prop_assert_eq!(tier(&name), Tier::T3);
        }
    }

    #[test]
    fn classify_ignores_ascii_case(name in "[A-Za-z_]{0,16}") {
        prop_assert_eq!(classify(&name), classify(&name.to_ascii_uppercase()));
        prop_assert_eq!(tier(&name), tier(&name.to_ascii_uppercase()));
    }
}

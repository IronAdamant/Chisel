//! Schema list and next-step hints from the shipped library.

#[test]
fn tools_list_has_29_tools_and_suggest_schema() {
    let list = chisel::schemas::tool_list_json();
    let tools = list.get("tools").and_then(|v| v.as_array()).expect("tools");
    assert_eq!(tools.len(), 29);
    let suggest = tools.iter().find(|t| t.get("name").and_then(|v| v.as_str()) == Some("suggest_tests")).expect("suggest_tests");
    let desc = suggest.get("description").and_then(|v| v.as_str()).unwrap_or("");
    assert!(desc.len() > 20, "{desc}");
    assert!(!desc.starts_with("Chisel tool "), "{desc}");
    let props = suggest.get("inputSchema").and_then(|v| v.get("properties")).and_then(|v| v.as_object()).expect("properties");
    assert!(props.iter().any(|(k, _)| k == "file_path"));
    let required = suggest.get("inputSchema").and_then(|v| v.get("required")).and_then(|v| v.as_array()).expect("required");
    assert!(required.iter().any(|v| v.as_str() == Some("file_path")));
    let impact = tools.iter().find(|t| t.get("name").and_then(|v| v.as_str()) == Some("impact")).unwrap();
    let required = impact.get("inputSchema").and_then(|v| v.get("required")).and_then(|v| v.as_array()).unwrap();
    assert!(required.iter().any(|v| v.as_str() == Some("files")));
}

#[test]
fn schema_text_uses_the_snapshot_contract() {
    let list = chisel::schemas::tool_list_json();
    let tools = list.get("tools").and_then(|v| v.as_array()).expect("tools");
    for tool in tools {
        let mut blob = tool.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if let Some(props) = tool.get("inputSchema").and_then(|v| v.get("properties")).and_then(|v| v.as_object()) {
            for (_, spec) in props {
                if let Some(desc) = spec.get("description").and_then(|v| v.as_str()) {
                    blob.push('\n');
                    blob.push_str(desc);
                }
            }
        }
        let lower = blob.to_lowercase();
        assert!(!lower.contains("pip install"), "{blob}");
        assert!(!lower.contains("sqlite3"), "{blob}");
        assert!(!lower.contains("pragma"), "{blob}");
        assert!(!lower.contains("stdlib"), "{blob}");
        assert!(!lower.contains("vacuum"), "{blob}");
        assert!(!lower.contains("database"), "{blob}");
        assert!(!lower.contains(" db"), "{blob}");
        if lower.contains("chisel.db") {
            assert!(lower.contains("does not open chisel.db"), "{blob}");
        }
        if lower.contains("sqlite") {
            assert!(lower.contains("does not") && lower.contains("sqlite"), "{blob}");
        }
    }
}

#[test]
fn analyze_next_steps_name_risk_map() {
    let result = chisel::json::Value::object(vec![
        ("code_files_scanned".into(), chisel::json::Value::Int(3)),
    ]);
    let steps = chisel::next_steps::compute_next_steps("analyze", &result);
    let text = chisel::json::to_string(&chisel::json::Value::Array(steps)).unwrap();
    assert!(text.contains("risk_map"), "{text}");
}

#[test]
fn no_data_suggests_analyze() {
    let result = chisel::json::Value::object(vec![
        ("status".into(), chisel::json::Value::String("no_data".into())),
    ]);
    let steps = chisel::next_steps::compute_next_steps("risk_map", &result);
    let text = chisel::json::to_string(&chisel::json::Value::Array(steps)).unwrap();
    assert!(text.contains("analyze"), "{text}");
}

#[test]
fn tools_list_is_short_and_every_description_is_distinct() {
    let list = chisel::schemas::tool_list_json();
    let text = chisel::json::to_string(&list).unwrap();
    // 0.23.0 added worktree_root to the 15 tools that answer for a worktree (about 1.8 KB).
    // 0.24.0: record_result's message and platform, stats' result lists, auto_update_limit (0.6 KB).
    assert!(text.len() < 15_500, "tools/list is {} bytes", text.len());
    let tools = list.get("tools").and_then(|v| v.as_array()).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for tool in tools {
        let name = tool.get("name").and_then(|v| v.as_str()).unwrap();
        let desc = tool.get("description").and_then(|v| v.as_str()).unwrap();
        assert!(desc.len() <= 260, "{name}: {} chars", desc.len());
        assert!(seen.insert(desc.to_string()), "{name} repeats another description");
        assert!(!desc.contains("Zero crate dependencies"), "{name}: boilerplate");
        let props = tool.get("inputSchema").and_then(|v| v.get("properties")).and_then(|v| v.as_object()).unwrap();
        for (arg, spec) in props {
            assert!(spec.get("description").and_then(|v| v.as_str()).is_some_and(|d| !d.is_empty()), "{name}.{arg} has no description");
        }
    }
}

/// 0.24.0: a diff_impact reply with `run` or `groups` points at them, not at a generic run.
#[test]
fn diff_impact_steps_point_at_run_and_groups() {
    use chisel::json::{parse, to_string};
    let steps = |text: &str| to_string(&chisel::json::Value::Array(chisel::next_steps::compute_next_steps("diff_impact", &parse(text).unwrap()))).unwrap();
    let with_run = steps(r#"{"items":[{"test_id":"a"}],"run":["cargo test -p a -- t"]}"#);
    assert!(with_run.contains("result.run") && !with_run.contains("run_groups"), "{with_run}");
    let tied = steps(r#"{"items":[],"truncated":true,"total":4,"tied":4,"groups":[{"group":"a","tests":4,"top":1.0,"run":"cargo test -p a"}]}"#);
    assert!(tied.contains("run_groups") && !tied.contains("test_gaps"), "{tied}");
}

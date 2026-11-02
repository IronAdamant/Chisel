//! Schema list and next-step hints from the shipped library.

#[test]
fn tools_list_has_26_tools_and_suggest_schema() {
    let list = chisel::schemas::tool_list_json();
    let tools = list.get("tools").and_then(|v| v.as_array()).expect("tools");
    assert_eq!(tools.len(), 26);
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

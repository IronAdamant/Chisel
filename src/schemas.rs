//! The 26 tool names, in the same order as `chisel/schemas.py`.

pub const TOOL_NAMES: [&str; 26] = [
    "analyze",
    "impact",
    "suggest_tests",
    "churn",
    "ownership",
    "coupling",
    "risk_map",
    "stale_tests",
    "history",
    "who_reviews",
    "record_result",
    "diff_impact",
    "update",
    "stats",
    "optimize_storage",
    "test_gaps",
    "triage",
    "start_job",
    "job_status",
    "cancel_job",
    "acquire_file_lock",
    "release_file_lock",
    "refresh_file_lock",
    "check_file_lock",
    "check_locks",
    "list_file_locks",
];

pub fn tool_list_json() -> crate::json::Value {
    use crate::json::Value;
    let tools = TOOL_NAMES.iter().map(|name| {
        Value::object(vec![
            ("name".into(), Value::String((*name).into())),
            ("description".into(), Value::String(format!("Chisel tool {name}"))),
            ("inputSchema".into(), Value::object(vec![
                ("type".into(), Value::String("object".into())),
                ("properties".into(), Value::object(vec![])),
            ])),
        ])
    }).collect();
    Value::object(vec![("tools".into(), Value::Array(tools))])
}

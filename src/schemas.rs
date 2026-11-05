//! The 26 tool names and their input schemas (`schemas_data.json`, embedded in the binary).
//! Descriptions are short on purpose: every MCP session loads them into the agent's context.

use crate::json::{self, Value};

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

pub fn tool_list_json() -> Value {
    json::parse(include_str!("schemas_data.json")).unwrap_or_else(|_| {
        Value::object(vec![("tools".into(), Value::Array(Vec::new()))])
    })
}

pub fn tool_schema(name: &str) -> Option<Value> {
    let list = tool_list_json();
    let tools = list.get("tools").and_then(|v| v.as_array())?;
    tools.iter().find(|t| t.get("name").and_then(|v| v.as_str()) == Some(name)).cloned()
}

pub fn required_fields(name: &str) -> Vec<String> {
    let Some(schema) = tool_schema(name) else { return Vec::new() };
    schema.get("inputSchema")
        .and_then(|s| s.get("required"))
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default()
}

pub fn field_type(name: &str, field: &str) -> Option<String> {
    tool_schema(name)?
        .get("inputSchema")?
        .get("properties")?
        .get(field)?
        .get("type")?
        .as_str()
        .map(|s| s.to_string())
}

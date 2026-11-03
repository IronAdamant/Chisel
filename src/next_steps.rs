//! Follow-up hints for MCP responses. CLI output does not include these.

use crate::json::Value;

pub fn compute_next_steps(tool_name: &str, result: &Value) -> Vec<Value> {
    if let Some(status) = result.get("status").and_then(|v| v.as_str()) {
        if status == "no_data" {
            return vec![tool_step("analyze", Value::object(vec![]), "Populate analysis data")];
        }
        if status == "stale_db" {
            return vec![tool_step("update", Value::object(vec![]), "Refresh the snapshot")];
        }
    }
    match tool_name {
        "analyze" => hints_analyze(result),
        "update" => hints_update(result),
        "risk_map" => hints_risk_map(result),
        "diff_impact" => hints_diff_impact(result),
        "test_gaps" => hints_list_file(result, "write_tests", "Prioritize highest-churn untested units"),
        "stale_tests" => hints_stale(result),
        "impact" | "suggest_tests" => {
            if result.as_array().is_some_and(|a| !a.is_empty()) {
                vec![action_step("run_tests", "Execute the listed tests"), tool_step("record_result", Value::object(vec![]), "Log outcomes after the run")]
            } else {
                Vec::new()
            }
        }
        "triage" => hints_triage(result),
        "churn" | "history" => {
            if result.as_array().is_some_and(|a| !a.is_empty()) {
                vec![tool_step("risk_map", Value::object(vec![]), "See overall risk context")]
            } else {
                Vec::new()
            }
        }
        "ownership" => nonempty_list(result, || vec![tool_step("who_reviews", Value::object(vec![]), "Find active maintainers")]),
        "who_reviews" => nonempty_list(result, || vec![tool_step("ownership", Value::object(vec![]), "Compare with original authors")]),
        "coupling" => hints_coupling(result),
        "stats" => hints_stats(result),
        "record_result" => {
            if result.get("recorded").is_some() {
                vec![tool_step("suggest_tests", Value::object(vec![]), "Re-rank test suggestions with the new result")]
            } else {
                Vec::new()
            }
        }
        "start_job" => {
            if let Some(id) = result.get("job_id").and_then(|v| v.as_str()) {
                vec![tool_step("job_status", Value::object(vec![("job_id".into(), Value::String(id.into()))]), "Poll until the job finishes")]
            } else {
                Vec::new()
            }
        }
        "job_status" => hints_job_status(result),
        _ => Vec::new(),
    }
}

fn nonempty_list(result: &Value, f: impl FnOnce() -> Vec<Value>) -> Vec<Value> {
    if result.as_array().is_some_and(|a| !a.is_empty()) { f() } else { Vec::new() }
}

fn tool_step(tool: &str, args: Value, reason: &str) -> Value {
    Value::object(vec![
        ("tool".into(), Value::String(tool.into())),
        ("args".into(), args),
        ("reason".into(), Value::String(reason.into())),
    ])
}

fn action_step(action: &str, reason: &str) -> Value {
    Value::object(vec![
        ("action".into(), Value::String(action.into())),
        ("reason".into(), Value::String(reason.into())),
    ])
}

fn hints_analyze(result: &Value) -> Vec<Value> {
    if result.get("code_files_scanned").is_some() {
        vec![
            tool_step("risk_map", Value::object(vec![]), "Identify high-risk files"),
            tool_step("test_gaps", Value::object(vec![]), "Find untested code"),
            tool_step("triage", Value::object(vec![]), "Combined risk + gap + stale overview"),
        ]
    } else {
        Vec::new()
    }
}

fn hints_update(result: &Value) -> Vec<Value> {
    if result.get("files_updated").and_then(|v| v.as_i64()).unwrap_or(0) > 0 {
        vec![
            tool_step("diff_impact", Value::object(vec![]), "See which tests are affected by changes"),
            tool_step("risk_map", Value::object(vec![]), "Check updated risk scores"),
        ]
    } else {
        Vec::new()
    }
}

fn hints_risk_map(result: &Value) -> Vec<Value> {
    let files = result.get("files").and_then(|v| v.as_array()).or_else(|| result.as_array());
    let Some(files) = files else { return Vec::new() };
    if files.is_empty() {
        return vec![tool_step("analyze", Value::object(vec![]), "Populate risk data")];
    }
    let top = files[0].get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let mut steps = vec![tool_step("test_gaps", Value::object(vec![]), "Find missing coverage for high-risk files")];
    if !top.is_empty() {
        steps.push(tool_step("suggest_tests", Value::object(vec![("file_path".into(), Value::String(top.into()))]), "Test recommendations for the riskiest file"));
    }
    if result.get("_meta").and_then(|m| m.get("uniform_components")).and_then(|u| u.get("coupling")).is_some() {
        steps.push(tool_step("stats", Value::object(vec![]), "A risk component is uniform. Check whether the history can support it"));
    }
    steps
}

fn hints_diff_impact(result: &Value) -> Vec<Value> {
    match result.get("status").and_then(|v| v.as_str()) {
        Some("git_error") => vec![action_step("fix_git_context", "Point Chisel at the git repository root so git diff can run")],
        Some("no_changes") => vec![
            tool_step("diff_impact", Value::object(vec![("ref".into(), Value::String("HEAD~1".into()))]), "Try diffing against the previous commit"),
            tool_step("update", Value::object(vec![]), "Re-analyze if the working tree has new files"),
        ],
        _ => {
            if result.as_array().is_some_and(|a| !a.is_empty()) {
                vec![action_step("run_tests", "Execute impacted tests to verify changes")]
            } else if result.as_array().is_some() {
                vec![tool_step("test_gaps", Value::object(vec![]), "Check if new code needs tests")]
            } else {
                Vec::new()
            }
        }
    }
}

fn hints_list_file(result: &Value, action: &str, reason: &str) -> Vec<Value> {
    let Some(items) = result.as_array() else { return Vec::new() };
    if items.is_empty() {
        return vec![action_step("complete", "Nothing left in this list")];
    }
    let file = items[0].get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let mut steps = vec![action_step(action, reason)];
    if !file.is_empty() {
        steps.push(tool_step("churn", Value::object(vec![("file_path".into(), Value::String(file.into()))]), "Check change frequency"));
    }
    steps
}

fn hints_stale(result: &Value) -> Vec<Value> {
    if result.get("status").and_then(|v| v.as_str()) == Some("no_edges") {
        return vec![tool_step("analyze", Value::object(vec![]), "Re-analyze to build test edges")];
    }
    if result.as_array().is_some_and(|a| !a.is_empty()) || result.get("stale_tests").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty()) {
        vec![action_step("fix_tests", "Update or remove stale tests")]
    } else {
        Vec::new()
    }
}

fn hints_triage(result: &Value) -> Vec<Value> {
    let Some(summary) = result.get("summary") else { return Vec::new() };
    let mut steps = Vec::new();
    if summary.get("total_test_gaps").and_then(|v| v.as_i64()).unwrap_or(0) > 0 {
        steps.push(action_step("prioritize", "Focus on files that appear in both risk and gap sections"));
    }
    if let Some(top) = result.get("top_risk_files").and_then(|v| v.as_array()).and_then(|a| a.first()) {
        if let Some(file) = top.get("file_path").and_then(|v| v.as_str()) {
            steps.push(tool_step("suggest_tests", Value::object(vec![("file_path".into(), Value::String(file.into()))]), "Test recommendations for the riskiest file"));
        }
    }
    steps
}

fn hints_coupling(result: &Value) -> Vec<Value> {
    if result.get("co_change_partners").is_some() || result.get("import_partners").is_some() {
        vec![tool_step("risk_map", Value::object(vec![]), "Check risk scores for coupled files")]
    } else {
        Vec::new()
    }
}

fn hints_stats(result: &Value) -> Vec<Value> {
    if result.get("hint").is_some() {
        return vec![tool_step("analyze", Value::object(vec![]), "Populate the snapshot")];
    }
    let mut steps = Vec::new();
    if result.get("code_units").and_then(|v| v.as_i64()).unwrap_or(0) > 0 {
        steps.push(tool_step("triage", Value::object(vec![]), "Get a prioritized overview of risk and gaps"));
    }
    steps
}

fn hints_job_status(result: &Value) -> Vec<Value> {
    match result.get("status").and_then(|v| v.as_str()) {
        Some("running") | Some("queued") => {
            let id = result.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
            vec![tool_step("job_status", Value::object(vec![("job_id".into(), Value::String(id.into()))]), "Job still running. Poll again")]
        }
        Some("completed") => vec![tool_step("diff_impact", Value::object(vec![]), "See test impact of local changes")],
        _ => Vec::new(),
    }
}

/// Warn when a JS/TS file builds code with eval or `new Function`, which static edges cannot see.
pub fn eval_warning(project: &std::path::Path, file_path: Option<&str>) -> Option<Value> {
    let file_path = file_path?;
    let lower = file_path.to_ascii_lowercase();
    if !lower.ends_with(".js") && !lower.ends_with(".jsx") && !lower.ends_with(".ts") && !lower.ends_with(".tsx") && !lower.ends_with(".mjs") && !lower.ends_with(".cjs") {
        return None;
    }
    let text = std::fs::read_to_string(project.join(file_path)).ok()?;
    if !has_eval_or_function(&text) {
        return None;
    }
    Some(Value::object(vec![
        ("action".into(), Value::String("warn_eval_used".into())),
        ("reason".into(), Value::String(format!(
            "{file_path} uses eval(...) or new Function(...). Modules loaded that way are invisible to static analysis, so tests of runtime-generated targets are not in this list. Cross-check risk_map → unknown_require_count for affected files."
        ))),
    ]))
}

fn has_eval_or_function(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"eval") && (i == 0 || !is_ident(bytes[i - 1])) {
            let mut j = i + 4;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() { j += 1; }
            if j < bytes.len() && bytes[j] == b'(' {
                return true;
            }
        }
        if bytes[i..].starts_with(b"new") && (i == 0 || !is_ident(bytes[i - 1])) {
            let mut j = i + 3;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() { j += 1; }
            if bytes.get(j..).is_some_and(|rest| rest.starts_with(b"Function")) {
                let after = j + "Function".len();
                let boundary_ok = after == bytes.len() || !is_ident(bytes[after]);
                if boundary_ok {
                    let mut k = after;
                    while k < bytes.len() && bytes[k].is_ascii_whitespace() { k += 1; }
                    if k < bytes.len() && bytes[k] == b'(' {
                        return true;
                    }
                }
            }
        }
        i += 1;
    }
    false
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

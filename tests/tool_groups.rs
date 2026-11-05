//! CHISEL_TOOLS / CHISEL_DISABLE against the real stdio MCP server.

use chisel::json;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("chisel-groups-{t}-{}", N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Raw response lines for `requests`, with the given variables set (others removed).
fn session(vars: &[(&str, &str)], requests: &[&str]) -> Vec<String> {
    let dir = scratch();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chisel"));
    cmd.args(["serve-mcp", "--project-dir"]).arg(&dir).arg("--storage-dir").arg(dir.join("store"));
    cmd.env_remove("CHISEL_TOOLS").env_remove("CHISEL_DISABLE");
    for (k, v) in vars {
        cmd.env(k, v);
    }
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut lines = Vec::new();
    for req in requests {
        writeln!(stdin, "{req}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        lines.push(line.trim_end().to_string());
    }
    drop(stdin);
    let _ = child.wait();
    lines
}

const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
const LIST: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;

fn names(list_line: &str) -> Vec<String> {
    let doc = json::parse(list_line).unwrap();
    doc.get("result").and_then(|r| r.get("tools")).and_then(|t| t.as_array()).unwrap().iter().map(|t| t.get("name").and_then(|v| v.as_str()).unwrap().to_string()).collect()
}

fn call(name: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"{name}","arguments":{{}}}}}}"#)
}

#[test]
fn default_listing_and_instructions_are_unchanged() {
    let lines = session(&[], &[INIT, LIST]);
    let expected = format!(r#"{{"jsonrpc":"2.0","id":2,"result":{}}}"#, json::to_string(&chisel::schemas::tool_list_json()).unwrap());
    assert_eq!(lines[1], expected, "tools/list changed with no switches set");
    assert_eq!(names(&lines[1]).len(), 26);
    let init = json::parse(&lines[0]).unwrap();
    let instructions = init.get("result").and_then(|r| r.get("instructions")).and_then(|v| v.as_str()).unwrap();
    assert!(!instructions.contains("Tool groups"), "{instructions}");
    // Empty values are the same as unset.
    let blank = session(&[("CHISEL_TOOLS", " "), ("CHISEL_DISABLE", "")], &[INIT, LIST]);
    assert_eq!(blank, lines);
}

#[test]
fn allowlist_and_denylist_filter_by_group_and_by_tool() {
    let lines = session(&[("CHISEL_TOOLS", "Risk, churn")], &[INIT, LIST]);
    let got = names(&lines[1]);
    for keep in ["analyze", "diff_impact", "suggest_tests", "stats", "risk_map", "triage", "churn"] {
        assert!(got.iter().any(|n| n == keep), "{keep} missing: {got:?}");
    }
    for gone in ["ownership", "acquire_file_lock", "start_job"] {
        assert!(!got.iter().any(|n| n == gone), "{gone} listed: {got:?}");
    }
    let init = json::parse(&lines[0]).unwrap();
    let instructions = init.get("result").and_then(|r| r.get("instructions")).and_then(|v| v.as_str()).unwrap();
    assert!(instructions.ends_with("Tool groups on: core, risk, history (partial); off: maintenance, locks."), "{instructions}");

    let lines = session(&[("CHISEL_DISABLE", "locks,history,start_job")], &[LIST]);
    let got = names(&lines[0]);
    assert_eq!(got.len(), 26 - 6 - 5 - 1, "{got:?}");
    assert!(got.iter().any(|n| n == "job_status") && !got.iter().any(|n| n == "start_job" || n == "check_locks" || n == "who_reviews"));

    let lines = session(&[("CHISEL_TOOLS", "all"), ("CHISEL_DISABLE", "locks")], &[LIST]);
    assert_eq!(names(&lines[0]).len(), 20);
}

#[test]
fn a_disabled_tool_says_how_to_turn_it_back_on() {
    let lines = session(&[("CHISEL_DISABLE", "locks")], &[call("acquire_file_lock").as_str(), call("stats").as_str()]);
    let doc = json::parse(&lines[0]).unwrap();
    let result = doc.get("result").unwrap();
    assert_eq!(result.get("isError").and_then(|v| v.as_bool()), Some(true));
    let text = result.get("content").and_then(|c| c.as_array()).unwrap()[0].get("text").and_then(|v| v.as_str()).unwrap();
    assert_eq!(text, "tool \"acquire_file_lock\" is disabled (group \"locks\") by CHISEL_DISABLE; remove \"locks\" from it to enable");
    assert!(lines[1].contains("Tool groups on: core, risk, history, maintenance; off: locks."), "stats does not report groups: {}", lines[1]);
}

#[test]
fn an_unknown_name_stops_the_server_with_exit_2() {
    let dir = scratch();
    let out = Command::new(env!("CARGO_BIN_EXE_chisel"))
        .args(["serve-mcp", "--project-dir"])
        .arg(&dir)
        .env("CHISEL_DISABLE", "lokcs")
        .env_remove("CHISEL_TOOLS")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(err.starts_with("CHISEL_DISABLE: unknown group \"lokcs\"; groups: core, risk, history, maintenance, locks; tools: analyze, impact"), "{err}");
}

#[test]
fn core_cannot_be_disabled() {
    let lines = session(&[("CHISEL_TOOLS", "locks"), ("CHISEL_DISABLE", "core,analyze,diff_impact")], &[LIST]);
    let got = names(&lines[0]);
    for core in ["analyze", "update", "diff_impact", "impact", "suggest_tests", "record_result", "stats"] {
        assert!(got.iter().any(|n| n == core), "{core} missing: {got:?}");
    }
    assert_eq!(got.len(), 7 + 6);
}

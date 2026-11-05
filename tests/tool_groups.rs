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
    session_in(&scratch(), vars, requests)
}

fn server(dir: &std::path::Path, vars: &[(&str, &str)]) -> std::process::Child {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chisel"));
    cmd.args(["serve-mcp", "--project-dir"]).arg(dir).arg("--storage-dir").arg(dir.join("store"));
    cmd.env_remove("CHISEL_TOOLS").env_remove("CHISEL_DISABLE");
    for (k, v) in vars {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap()
}

/// As `session`, in a project directory that may hold an AGENTS.md.
fn session_in(dir: &std::path::Path, vars: &[(&str, &str)], requests: &[&str]) -> Vec<String> {
    let mut child = server(dir, vars);
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
    // initialize + tools/list as the 0.18.0 release answered them, apart from serverInfo.version.
    let release: Vec<String> = include_str!("fixtures/mcp/default_0.18.0.jsonl").lines().map(|l| l.replace("@VERSION@", env!("CARGO_PKG_VERSION"))).collect();
    assert_eq!(lines, release, "default initialize/tools/list differ from the last release");
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
    assert!(instructions.ends_with("\nTool groups on: core, risk, history (only churn). Off: maintenance, locks. Change them with a \"chisel: disable <groups>\" or \"chisel: tools <groups>\" line in the agent-tools block of AGENTS.md; CHISEL_TOOLS / CHISEL_DISABLE override it."), "{instructions}");

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
    assert!(lines[1].contains("Tool groups on: core, risk, history, maintenance. Off: locks."), "stats does not report groups: {}", lines[1]);
}

/// Exit code and stderr of `serve-mcp` started with one variable set.
fn startup(var: &str, value: &str) -> (Option<i32>, String) {
    let dir = scratch();
    let out = Command::new(env!("CARGO_BIN_EXE_chisel"))
        .args(["serve-mcp", "--project-dir"])
        .arg(&dir)
        .env_remove("CHISEL_TOOLS")
        .env_remove("CHISEL_DISABLE")
        .env(var, value)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn an_unknown_name_stops_the_server_with_exit_2() {
    let (code, err) = startup("CHISEL_DISABLE", "lokcs");
    assert_eq!(code, Some(2));
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(err.starts_with("CHISEL_DISABLE: unknown group \"lokcs\"; groups: core, risk, history, maintenance, locks; tools: analyze, impact"), "{err}");
}

#[test]
fn core_cannot_be_disabled() {
    for name in ["core", "analyze", "Diff_Impact"] {
        let (code, err) = startup("CHISEL_DISABLE", name);
        assert_eq!(code, Some(2), "{name}: {err}");
        assert_eq!(err.lines().count(), 1, "{err}");
        assert!(err.contains("group \"core\" is always on and cannot be disabled"), "{err}");
    }
    // An allowlist without core still exposes core.
    let lines = session(&[("CHISEL_TOOLS", "locks")], &[LIST]);
    let got = names(&lines[0]);
    for core in ["analyze", "update", "diff_impact", "impact", "suggest_tests", "record_result", "stats"] {
        assert!(got.iter().any(|n| n == core), "{core} missing: {got:?}");
    }
    assert_eq!(got.len(), 7 + 6);
}

fn project_with(agents_md: &str) -> PathBuf {
    let dir = scratch();
    std::fs::write(dir.join("AGENTS.md"), agents_md).unwrap();
    dir
}

fn instructions(init_line: &str) -> String {
    let doc = json::parse(init_line).unwrap();
    doc.get("result").and_then(|r| r.get("instructions")).and_then(|v| v.as_str()).unwrap().to_string()
}

fn list_changed(init_line: &str) -> Option<bool> {
    let doc = json::parse(init_line).unwrap();
    doc.get("result").and_then(|r| r.get("capabilities")).and_then(|c| c.get("tools")).and_then(|t| t.get("listChanged")).and_then(|v| v.as_bool())
}

const BLOCK: &str = "# Project\n\n```agent-tools\n# one agent; CoordinationHub owns leases\ntrammel: disable locks, notes\nchisel: disable locks history\ncoordinationhub: tools core, locks, tasks\n```\n";

#[test]
fn an_agent_tools_block_in_agents_md_picks_the_groups() {
    let dir = project_with(BLOCK);
    let lines = session_in(&dir, &[], &[INIT, LIST, call("ownership").as_str()]);
    assert_eq!(names(&lines[1]).len(), 26 - 6 - 5, "{:?}", names(&lines[1]));
    assert_eq!(list_changed(&lines[0]), Some(true));
    let text = instructions(&lines[0]);
    assert!(text.ends_with("Tool groups on: core, risk, maintenance. Off: history, locks. Change them with a \"chisel: disable <groups>\" or \"chisel: tools <groups>\" line in the agent-tools block of AGENTS.md; CHISEL_TOOLS / CHISEL_DISABLE override it."), "{text}");
    assert!(lines[2].contains(r#"tool \"ownership\" is disabled (group \"history\") by AGENTS.md agent-tools line \"chisel: disable\"; remove \"history\" from it to enable"#), "{}", lines[2]);
}

#[test]
fn env_vars_override_the_file_key_by_key() {
    let dir = project_with("```agent-tools\nchisel: tools risk\nchisel: disable risk_map\n```\n");
    // CHISEL_DISABLE replaces the file's disable line; the file's tools line still applies.
    let lines = session_in(&dir, &[("CHISEL_DISABLE", "triage")], &[LIST]);
    let got = names(&lines[0]);
    assert!(got.iter().any(|n| n == "risk_map"), "file disable line should be overridden: {got:?}");
    assert!(!got.iter().any(|n| n == "triage" || n == "churn" || n == "check_locks"), "{got:?}");
    // CHISEL_TOOLS replaces the file's tools line; the file's disable line still applies.
    let lines = session_in(&dir, &[("CHISEL_TOOLS", "history")], &[LIST]);
    let got = names(&lines[0]);
    assert!(got.iter().any(|n| n == "churn") && !got.iter().any(|n| n == "risk_map" || n == "triage"), "{got:?}");
}

#[test]
fn file_errors_are_warnings_and_never_stop_the_server() {
    let dir = project_with("```agent-tools\nchisel: disable lokcs\nchisel: disable core\n```\n");
    let lines = session_in(&dir, &[], &[INIT, LIST, call("stats").as_str()]);
    assert_eq!(names(&lines[1]).len(), 26, "bad lines must leave the default");
    let text = instructions(&lines[0]);
    assert!(text.contains("\nAGENTS.md agent-tools: unknown group \"lokcs\" for chisel; groups: core, risk, history, maintenance, locks\n"), "{text}");
    assert!(text.contains("AGENTS.md agent-tools: group \"core\" is always on and cannot be disabled (\"core\") for chisel"), "{text}");
    assert!(text.lines().last().unwrap().starts_with("Tool groups on: core, risk, history, maintenance, locks. Off: none."), "{text}");
    assert!(lines[2].contains("tool_groups_warnings") && lines[2].contains("lokcs"), "{}", lines[2]);
}

#[test]
fn no_block_means_the_release_output() {
    let release: Vec<String> = include_str!("fixtures/mcp/default_0.18.0.jsonl").lines().map(|l| l.replace("@VERSION@", env!("CARGO_PKG_VERSION"))).collect();
    let dir = project_with("# A project\n\n```bash\nchisel: disable locks\n```\n");
    assert_eq!(session_in(&dir, &[], &[INIT, LIST]), release);
    // A block with lines only for other tools changes nothing Chisel lists.
    let dir = project_with("```agent-tools\ntrammel: disable locks\n```\n");
    let lines = session_in(&dir, &[], &[INIT, LIST]);
    assert_eq!(names(&lines[1]).len(), 26);
}

#[test]
fn editing_the_block_mid_session_notifies_and_applies() {
    let dir = scratch();
    let mut child = server(&dir, &[]);
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut send = |req: &str| {
        writeln!(stdin, "{req}").unwrap();
        stdin.flush().unwrap();
    };
    let mut read = || {
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        line.trim_end().to_string()
    };
    send(LIST);
    assert_eq!(names(&read()).len(), 26);
    std::fs::write(dir.join("AGENTS.md"), "```agent-tools\nchisel: disable locks\n```\n").unwrap();
    send(LIST);
    assert_eq!(read(), r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#);
    assert_eq!(names(&read()).len(), 20);
    // A change that leaves the tool set alone sends no notification.
    std::fs::write(dir.join("AGENTS.md"), "```agent-tools\n# still only locks off\nchisel: disable locks\n```\n").unwrap();
    send(LIST);
    assert_eq!(names(&read()).len(), 20);
    drop(stdin);
    let _ = child.wait();
}

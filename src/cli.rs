//! Argv parser and command dispatch. No clap.

use crate::engine::Engine;
use crate::json::{self, Value};
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub struct Args {
    pub command: String,
    pub project_dir: PathBuf,
    pub storage_dir: Option<PathBuf>,
    pub json: bool,
    pub positionals: Vec<String>,
    pub host: String,
    pub port: u16,
    pub ttl: i64,
    pub purpose: Option<String>,
    pub test_command: Vec<String>,
    pub force: bool,
    pub working_tree: bool,
    pub auto_update: bool,
    pub limit: Option<i64>,
    pub shard: Option<String>,
    pub directory: Option<String>,
    pub git_ref: Option<String>,
    pub kind: Option<String>,
    /// Set only by `--passed` or `--failed`. Absent means the caller did not choose.
    pub passed: Option<bool>,
    pub coverage_mode: Option<String>,
    pub proximity_adjustment: Option<bool>,
    pub exclude_new_file_boost: bool,
}

pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut command = String::new();
    let mut project_dir = env::var("CHISEL_PROJECT_DIR").map(PathBuf::from).unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let mut storage_dir = env::var("CHISEL_STORAGE_DIR").ok().map(PathBuf::from);
    let mut json = false;
    let mut positionals = Vec::new();
    let mut host = "127.0.0.1".to_string();
    let mut port = 8377u16;
    let mut ttl = 300i64;
    let mut purpose = None;
    let mut test_command = Vec::new();
    let mut force = false;
    let mut working_tree = false;
    let mut auto_update = false;
    let mut limit = None;
    let mut shard = None;
    let mut directory = None;
    let mut git_ref = None;
    let mut kind = None;
    let mut passed: Option<bool> = None;
    let mut coverage_mode = None;
    let mut proximity_adjustment = None;
    let mut exclude_new_file_boost = false;
    let mut i = 0;
    let mut after_dd = false;
    while i < argv.len() {
        let a = &argv[i];
        if after_dd {
            test_command.push(a.clone());
            i += 1;
            continue;
        }
        if a == "--" {
            after_dd = true;
            i += 1;
            continue;
        }
        if let Some(v) = a.strip_prefix("--project-dir=") {
            project_dir = PathBuf::from(v);
        } else if a == "--project-dir" {
            i += 1;
            project_dir = PathBuf::from(argv.get(i).ok_or("missing --project-dir")?);
        } else if let Some(v) = a.strip_prefix("--storage-dir=") {
            storage_dir = Some(PathBuf::from(v));
        } else if a == "--storage-dir" {
            i += 1;
            storage_dir = Some(PathBuf::from(argv.get(i).ok_or("missing --storage-dir")?));
        } else if a == "--json" {
            json = true;
        } else if a == "--host" {
            i += 1;
            host = argv.get(i).ok_or("missing --host")?.clone();
        } else if a == "--port" {
            i += 1;
            port = argv.get(i).ok_or("missing --port")?.parse().map_err(|_| "bad port")?;
        } else if a == "--ttl" {
            i += 1;
            ttl = argv.get(i).ok_or("missing --ttl")?.parse().map_err(|_| "bad ttl")?;
        } else if a == "--purpose" {
            i += 1;
            purpose = Some(argv.get(i).ok_or("missing --purpose")?.clone());
        } else if a == "--force" {
            force = true;
        } else if a == "--working-tree" {
            working_tree = true;
        } else if a == "--auto-update" {
            auto_update = true;
        } else if a == "--limit" {
            i += 1;
            limit = Some(argv.get(i).ok_or("missing --limit")?.parse().map_err(|_| "bad limit")?);
        } else if a == "--shard" {
            i += 1;
            shard = Some(argv.get(i).ok_or("missing --shard")?.clone());
        } else if a == "--directory" {
            i += 1;
            directory = Some(argv.get(i).ok_or("missing --directory")?.clone());
        } else if a == "--ref" {
            i += 1;
            git_ref = Some(argv.get(i).ok_or("missing --ref")?.clone());
        } else if a == "--kind" {
            i += 1;
            kind = Some(argv.get(i).ok_or("missing --kind")?.clone());
        } else if a == "--job-id" {
            i += 1;
            positionals.push(argv.get(i).ok_or("missing --job-id")?.clone());
        } else if a == "--passed" {
            if passed == Some(false) {
                return Err("status=error pass only one of --passed or --failed".into());
            }
            passed = Some(true);
        } else if a == "--failed" {
            if passed == Some(true) {
                return Err("status=error pass only one of --passed or --failed".into());
            }
            passed = Some(false);
        } else if a == "--coverage-mode" {
            i += 1;
            coverage_mode = Some(argv.get(i).ok_or("missing --coverage-mode")?.clone());
        } else if a == "--no-proximity" {
            proximity_adjustment = Some(false);
        } else if a == "--exclude-new-file-boost" {
            exclude_new_file_boost = true;
        } else if a.starts_with("--") {
            return Err(format!("status=error unknown or unimplemented flag {a}"));
        } else if command.is_empty() {
            command = a.clone();
        } else {
            positionals.push(a.clone());
        }
        i += 1;
    }
    if command.is_empty() {
        return Err("missing command".into());
    }
    Ok(Args {
        command, project_dir, storage_dir, json, positionals, host, port, ttl, purpose, test_command,
        force, working_tree, auto_update, limit, shard, directory, git_ref, kind, passed,
        coverage_mode, proximity_adjustment, exclude_new_file_boost,
    })
}

fn tool_name(command: &str) -> String {
    match command {
        "acquire-lock" => "acquire_file_lock".into(),
        "release-lock" => "release_file_lock".into(),
        "refresh-lock" => "refresh_file_lock".into(),
        "check-lock" => "check_file_lock".into(),
        "check-locks" => "check_locks".into(),
        "list-locks" => "list_file_locks".into(),
        "who-reviews" => "who_reviews".into(),
        "suggest-tests" => "suggest_tests".into(),
        "risk-map" => "risk_map".into(),
        "diff-impact" => "diff_impact".into(),
        "stale-tests" => "stale_tests".into(),
        "test-gaps" => "test_gaps".into(),
        "record-result" => "record_result".into(),
        "start-job" => "start_job".into(),
        "job-status" => "job_status".into(),
        "cancel-job" => "cancel_job".into(),
        other => other.replace('-', "_"),
    }
}

pub fn run(args: &Args) -> Result<i32, String> {
    match args.command.as_str() {
        "serve" => {
            crate::http::serve(&args.project_dir, args.storage_dir.as_deref(), &args.host, args.port)?;
            return Ok(0);
        }
        "serve-mcp" | "mcp" => {
            crate::mcp::serve(&args.project_dir, args.storage_dir.as_deref())?;
            return Ok(0);
        }
        "run" => return run_tests(args),
        _ => {}
    }
    let mut engine = Engine::open(&args.project_dir, args.storage_dir.as_deref())?;
    let tool = tool_name(&args.command);
    let call_args = call_args(args);
    let result = engine.dispatch(&tool, &call_args)?;
    emit(args.json, &args.command, &result)?;
    if result.get("status").and_then(|v| v.as_str()).is_some_and(|s| s == "error" || s == "git_error") {
        return Ok(1);
    }
    Ok(0)
}

fn call_args(args: &Args) -> Value {
    let mut pairs = Vec::new();
    if let Some(first) = args.positionals.first() {
        match args.command.as_str() {
            "suggest-tests" | "churn" | "ownership" | "coupling" | "history" | "who-reviews" => {
                pairs.push(("file_path".into(), Value::String(first.clone())));
            }
            "impact" => {
                pairs.push(("files".into(), Value::Array(args.positionals.iter().cloned().map(Value::String).collect())));
            }
            "record-result" => {
                pairs.push(("test_id".into(), Value::String(first.clone())));
                let outcome = args.passed.or_else(|| {
                    args.positionals.get(1).and_then(|flag| match flag.as_str() {
                        "true" | "1" | "passed" | "PASSED" => Some(true),
                        "false" | "0" | "failed" | "FAILED" => Some(false),
                        _ => None,
                    })
                });
                if let Some(passed) = outcome {
                    pairs.push(("passed".into(), Value::Bool(passed)));
                }
            }
            "analyze" | "risk-map" | "test-gaps" => {
                pairs.push(("directory".into(), Value::String(first.clone())));
            }
            "acquire-lock" | "release-lock" | "refresh-lock" => {
                pairs.push(("file_path".into(), Value::String(first.clone())));
                if let Some(agent) = args.positionals.get(1) {
                    pairs.push(("agent_id".into(), Value::String(agent.clone())));
                }
            }
            "check-lock" => {
                pairs.push(("file_path".into(), Value::String(first.clone())));
            }
            "check-locks" => {
                pairs.push(("file_paths".into(), Value::Array(args.positionals.iter().cloned().map(Value::String).collect())));
            }
            _ => {}
        }
    }
    if matches!(args.command.as_str(), "acquire-lock" | "refresh-lock") {
        pairs.push(("ttl".into(), Value::Int(args.ttl)));
    }
    if let Some(purpose) = &args.purpose {
        pairs.push(("purpose".into(), Value::String(purpose.clone())));
    }
    if args.force {
        pairs.push(("force".into(), Value::Bool(true)));
    }
    if args.working_tree {
        pairs.push(("working_tree".into(), Value::Bool(true)));
    }
    if args.auto_update {
        pairs.push(("auto_update".into(), Value::Bool(true)));
    }
    if let Some(limit) = args.limit {
        pairs.push(("limit".into(), Value::Int(limit)));
    }
    if let Some(shard) = &args.shard {
        pairs.push(("shard".into(), Value::String(shard.clone())));
    }
    if let Some(directory) = &args.directory {
        pairs.push(("directory".into(), Value::String(directory.clone())));
    }
    if let Some(git_ref) = &args.git_ref {
        pairs.push(("ref".into(), Value::String(git_ref.clone())));
    }
    if let Some(kind) = &args.kind {
        pairs.push(("kind".into(), Value::String(kind.clone())));
    }
    if let Some(mode) = &args.coverage_mode {
        pairs.push(("coverage_mode".into(), Value::String(mode.clone())));
    }
    if let Some(proximity) = args.proximity_adjustment {
        pairs.push(("proximity_adjustment".into(), Value::Bool(proximity)));
    }
    if args.exclude_new_file_boost {
        pairs.push(("exclude_new_file_boost".into(), Value::Bool(true)));
    }
    if matches!(args.command.as_str(), "job-status" | "cancel-job") {
        if let Some(id) = args.positionals.first() {
            pairs.push(("job_id".into(), Value::String(id.clone())));
        }
    }
    Value::object(pairs)
}

pub fn parse_jest_json(text: &str) -> Vec<(String, bool)> {
    let Ok(doc) = json::parse(text) else { return Vec::new() };
    let Some(suites) = doc.get("testResults").and_then(|v| v.as_array()) else { return Vec::new() };
    let mut out = Vec::new();
    for suite in suites {
        let name = suite.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let Some(assertions) = suite.get("assertionResults").and_then(|v| v.as_array()) else { continue };
        for assertion in assertions {
            let title = assertion.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let passed = assertion.get("status").and_then(|v| v.as_str()) == Some("passed");
            out.push((format!("{name}:{title}"), passed));
        }
    }
    out
}

fn emit(json_output: bool, command: &str, result: &Value) -> Result<(), String> {
    if json_output {
        println!("{}", json::to_string_pretty(result).map_err(|e| e.message)?);
        return Ok(());
    }
    if let Some(status) = result.get("status").and_then(|v| v.as_str()) {
        if matches!(status, "no_data" | "no_changes" | "no_edges" | "git_error" | "stale_db" | "error") {
            if let Some(msg) = result.get("message").and_then(|v| v.as_str()) {
                println!("{msg}");
            }
            if let Some(hint) = result.get("hint").and_then(|v| v.as_str()) {
                println!("{hint}");
            }
            if let Some(err) = result.get("error").and_then(|v| v.as_str()) {
                println!("Error: {err}");
            }
            if let Some(cwd) = result.get("project_dir").and_then(|v| v.as_str()) {
                println!("Directory: {cwd}");
            }
            return Ok(());
        }
    }
    match command {
        "analyze" | "update" | "stats" => {
            let header = if command == "analyze" { "Analysis complete:" } else if command == "update" { "Update complete:" } else { "Stats:" };
            println!("{header}");
            if let Some(pairs) = result.as_object() {
                for (k, v) in pairs {
                    println!("  {}: {}", title_key(k), fmt_scalar(v));
                }
            }
        }
        "risk-map" => {
            println!("Risk map:");
            if let Some(files) = result.get("files").and_then(|v| v.as_array()) {
                for f in files {
                    let path = f.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                    let score = f.get("risk_score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    println!("  {path}: {score}");
                }
            }
        }
        _ => {
            println!("{}", json::to_string_pretty(result).map_err(|e| e.message)?);
        }
    }
    Ok(())
}

fn title_key(key: &str) -> String {
    key.split('_').map(|w| {
        let mut c = w.chars();
        match c.next() {
            None => String::new(),
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        }
    }).collect::<Vec<_>>().join(" ")
}

fn fmt_scalar(v: &Value) -> String {
    match v {
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => json::to_string(other).unwrap_or_else(|_| "…".into()),
    }
}

fn run_tests(args: &Args) -> Result<i32, String> {
    if args.test_command.is_empty() {
        return Err("chisel run requires a command after --".into());
    }
    let mut cmd_args = args.test_command.clone();
    let program = cmd_args.remove(0);
    let framework = if program == "pytest" || program.ends_with("/pytest") {
        "pytest"
    } else if program == "jest" || program.ends_with("/jest") {
        "jest"
    } else {
        ""
    };
    if framework == "pytest" && !cmd_args.iter().any(|a| a == "-v" || a == "--verbose") {
        cmd_args.push("-v".into());
    }
    let mut child = Command::new(&program)
        .args(&cmd_args)
        .current_dir(&args.project_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut out_buf = Vec::new();
    let mut err_buf = Vec::new();
    if let Some(ref mut o) = stdout {
        use std::io::Read;
        let _ = o.read_to_end(&mut out_buf);
    }
    if let Some(ref mut e) = stderr {
        use std::io::Read;
        let _ = e.read_to_end(&mut err_buf);
    }
    let _ = io::stdout().write_all(&out_buf);
    let _ = io::stderr().write_all(&err_buf);
    let status = child.wait().map_err(|e| e.to_string())?;
    let code = status.code().unwrap_or(1);
    if framework == "pytest" || framework == "jest" {
        let text = String::from_utf8_lossy(&out_buf);
        let rows = if framework == "jest" {
            parse_jest_json(&text)
        } else {
            text.lines().filter_map(parse_pytest_line).collect()
        };
        let mut engine = Engine::open(&args.project_dir, args.storage_dir.as_deref())?;
        let recorded = rows.len();
        if recorded > 0 {
            engine.record_results_batch(&rows)?;
            println!("\n[chisel] Recorded {recorded} test result(s)");
        } else {
            println!("\n[chisel] No test results parsed from output");
        }
    }
    Ok(code)
}

fn parse_pytest_line(line: &str) -> Option<(String, bool)> {
    let line = line.trim_end();
    let mut parts = line.rsplitn(2, ' ');
    let status = parts.next()?.trim();
    let status = status.split_whitespace().next()?;
    if !matches!(status, "PASSED" | "FAILED" | "ERROR" | "SKIPPED" | "XFAIL" | "XPASS") {
        // progress suffix: "PASSED [ 50%]"
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let idx = tokens.iter().position(|t| matches!(*t, "PASSED" | "FAILED" | "ERROR" | "SKIPPED" | "XFAIL" | "XPASS"))?;
        if idx == 0 {
            return None;
        }
        let id = tokens[..idx].join(" ");
        let passed = matches!(tokens[idx], "PASSED" | "XFAIL");
        return Some((id, passed));
    }
    let id = parts.next()?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let passed = matches!(status, "PASSED" | "XFAIL");
    Some((id, passed))
}

pub fn help() {
    println!("Chisel — test impact analysis and code intelligence");
    println!("usage: chisel [--json] [--project-dir DIR] [--storage-dir DIR] <command> ...");
    println!("commands: analyze, impact, suggest-tests, churn, ownership, coupling, risk-map,");
    println!("  stale-tests, history, who-reviews, diff-impact, update, test-gaps, record-result,");
    println!("  run, stats, start-job, job-status, cancel-job, triage, serve, serve-mcp,");
    println!("  acquire-lock, release-lock, refresh-lock, check-lock, check-locks, list-locks");
}

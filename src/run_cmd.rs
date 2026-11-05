//! `chisel run`: stream the test process and record pytest and Jest results.

use crate::cli::Args;
use crate::engine::Engine;
use crate::json;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

pub fn run_tests(args: &Args) -> Result<i32, String> {
    if args.test_command.is_empty() {
        return Err("chisel run requires a command after --".into());
    }
    let mut cmd_args = args.test_command.clone();
    let program = cmd_args.remove(0);
    let framework = detect_framework(&program, &cmd_args);
    if framework.is_empty() {
        println!("Warning: could not detect test framework; results will not be recorded.");
        println!("Running: {} {}", program, cmd_args.join(" "));
    }
    augment(&mut cmd_args, framework);
    let mut jest_file = None;
    if framework == "jest" && !cmd_args.iter().any(|a| a == "--outputFile" || a == "-o") {
        let path = std::env::temp_dir().join(format!("chisel-jest-{}-{}.json", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
        cmd_args.push("--outputFile".into());
        cmd_args.push(path.display().to_string());
        jest_file = Some(path);
    }
    let mut child = Command::new(&program)
        .args(&cmd_args)
        .current_dir(&args.project_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let out_copy = Arc::clone(&out_buf);
    let err_copy = Arc::clone(&err_buf);
    let out_thread = thread::spawn(move || {
        if let Some(mut pipe) = stdout {
            stream_pipe(&mut pipe, &mut io::stdout(), &out_copy);
        }
    });
    let err_thread = thread::spawn(move || {
        if let Some(mut pipe) = stderr {
            stream_pipe(&mut pipe, &mut io::stdout(), &err_copy);
        }
    });
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = out_thread.join();
    let _ = err_thread.join();
    let code = status.code().unwrap_or(1);
    let stdout_text = String::from_utf8_lossy(&out_buf.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
    let stderr_text = String::from_utf8_lossy(&err_buf.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
    let combined = format!("{stdout_text}{stderr_text}");
    let jest_text = jest_file.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).filter(|t| !t.is_empty());
    let mut recorded_here = false;
    if framework == "pytest" || framework == "jest" {
        let rows = if framework == "jest" {
            parse_jest_json(jest_text.as_deref().unwrap_or(&combined), Some(&args.project_dir))
        } else {
            combined.lines().filter_map(parse_pytest_line).collect()
        };
        let mut engine = Engine::open(&args.project_dir, args.storage_dir.as_deref())?;
        if !rows.is_empty() {
            engine.record_results_batch(&rows)?;
            recorded_here = true;
            println!("\n[chisel] Recorded {} test result(s)", rows.len());
        } else {
            println!("\n[chisel] No test results parsed from output");
        }
    }
    // Issues: the same capture the PostToolUse hook runs. Results are recorded here only for
    // runners the block above does not handle.
    let parsed = crate::test_output::parse(jest_text.as_deref().unwrap_or(&combined), Some(&args.project_dir));
    if !parsed.outcomes.is_empty() {
        let mut engine = Engine::open(&args.project_dir, args.storage_dir.as_deref())?;
        let command = args.test_command.join(" ");
        if let Ok(summary) = engine.capture(&parsed, &command, !recorded_here) {
            if summary.recorded > 0 {
                println!("\n[chisel] Recorded {} test result(s)", summary.recorded);
            }
            if let Some(line) = summary.line() {
                println!("[chisel] {line}");
            }
        }
    }
    if let Some(path) = jest_file {
        let _ = std::fs::remove_file(path);
    }
    Ok(code)
}

fn stream_pipe(pipe: &mut impl Read, out: &mut impl Write, buf: &Mutex<Vec<u8>>) {
    let mut tmp = [0u8; 4096];
    loop {
        let n = match pipe.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        let _ = out.write_all(&tmp[..n]);
        let _ = out.flush();
        if let Ok(mut guard) = buf.lock() {
            guard.extend_from_slice(&tmp[..n]);
        }
    }
}

fn detect_framework(program: &str, args: &[String]) -> &'static str {
    let base = Path::new(program).file_name().and_then(|s| s.to_str()).unwrap_or(program).to_ascii_lowercase();
    if base == "pytest" {
        "pytest"
    } else if base == "jest" || base == "npx" {
        "jest"
    } else if base == "go" {
        "go"
    } else if base == "cargo" {
        "cargo"
    } else if args.iter().any(|a| a == "jest" || a.rsplit(['/', '\\']).next() == Some("jest")) {
        "jest"
    } else {
        ""
    }
}

fn augment(args: &mut Vec<String>, framework: &str) {
    match framework {
        "pytest" => {
            if !args.iter().any(|a| a == "-v" || a == "--verbose") {
                args.push("-v".into());
            }
        }
        "jest" => {
            if !args.iter().any(|a| a == "--json") {
                args.push("--json".into());
            }
        }
        "go" => {
            if !args.iter().any(|a| a == "-json") {
                args.push("-json".into());
            }
        }
        "cargo" if !args.iter().any(|a| a == "--message-format=json" || a.starts_with("--message-format")) => {
            args.push("--message-format=json".into());
        }
        _ => {}
    }
}

pub fn parse_jest_json(text: &str, project: Option<&Path>) -> Vec<(String, bool)> {
    let Ok(doc) = json::parse(text) else { return Vec::new() };
    let Some(suites) = doc.get("testResults").and_then(|v| v.as_array()) else { return Vec::new() };
    let mut out = Vec::new();
    for suite in suites {
        let name = suite.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let name = relativize(name, project);
        let Some(assertions) = suite.get("assertionResults").and_then(|v| v.as_array()) else { continue };
        for assertion in assertions {
            let title = assertion.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let passed = assertion.get("status").and_then(|v| v.as_str()) == Some("passed");
            out.push((format!("{name}:{title}"), passed));
        }
    }
    out
}

fn relativize(name: &str, project: Option<&Path>) -> String {
    let name = name.replace('\\', "/");
    let Some(project) = project else { return name };
    let path = PathBuf::from(&name);
    if let Ok(rel) = path.strip_prefix(project) {
        return rel.to_string_lossy().replace('\\', "/");
    }
    if let Ok(root) = project.canonicalize() {
        if let Ok(rel) = PathBuf::from(&name).strip_prefix(&root) {
            return rel.to_string_lossy().replace('\\', "/");
        }
    }
    name
}

pub fn parse_pytest_line(line: &str) -> Option<(String, bool)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let raw: Vec<&str> = line.split_whitespace().collect();
    let tokens = trim_progress(&raw);
    if tokens.len() < 2 {
        return None;
    }
    let (idx, status) = if is_status(tokens[tokens.len() - 1]) {
        (tokens.len() - 1, tokens[tokens.len() - 1])
    } else {
        return None;
    };
    let id = tokens[..idx].join(" ");
    if id.is_empty() || id.chars().all(|c| c == '_' || c == '=' || c == '-') {
        return None;
    }
    if !id.contains(':') && !id.contains('/') && !id.contains('.') {
        return None;
    }
    let passed = matches!(status, "PASSED" | "XFAIL");
    Some((id, passed))
}

fn is_status(token: &str) -> bool {
    matches!(token, "PASSED" | "FAILED" | "ERROR" | "SKIPPED" | "XFAIL" | "XPASS")
}

fn trim_progress<'a>(tokens: &'a [&'a str]) -> &'a [&'a str] {
    if tokens.len() >= 2 {
        let last = tokens[tokens.len() - 1];
        let prev = tokens[tokens.len() - 2];
        if (last.ends_with(']') && prev.starts_with('[')) || (last.starts_with('[') && last.ends_with(']')) {
            let end = if last.starts_with('[') && last.ends_with(']') { tokens.len() - 1 } else { tokens.len() - 2 };
            return &tokens[..end];
        }
    }
    tokens
}

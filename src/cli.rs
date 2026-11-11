//! Argv parser and command dispatch. No clap.

use crate::engine::Engine;
use crate::json::{self, Value};
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;

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
    pub depth: Option<i64>,
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
    /// `chisel watch` checks once and exits.
    pub allow_repo_extractor: bool,
    pub once: bool,
    /// Seconds between worktree polls. Minimum 1.
    pub interval_secs: u64,
    pub functions: Vec<String>,
    pub fallback: bool,
    pub unit: Option<String>,
    pub min_count: Option<i64>,
    pub no_exclude_tests: bool,
    pub top_n: Option<i64>,
    pub agent_id: Option<String>,
    pub duration_ms: Option<i64>,
    pub help: bool,
    /// `chisel capture --command`: the shell command whose output is on stdin.
    pub capture_command: Option<String>,
    /// `chisel capture --hook`: stdin is a Claude Code PostToolUse payload.
    pub hook: bool,
    /// `chisel issues --all`, `--since`, `--brief`; `chisel hook-install --write`.
    pub all: bool,
    pub since: Option<String>,
    pub brief: bool,
    pub write: bool,
    /// `--worktree PATH`: answer for that git worktree (MCP `worktree_root`).
    pub worktree: Option<PathBuf>,
    /// The linked worktree this process started in, when neither the project nor the storage
    /// directory was given: read tools answer for it from the main checkout's snapshot.
    pub default_worktree: Option<PathBuf>,
}

pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut command = String::new();
    let env_project = env::var("CHISEL_PROJECT_DIR").ok().map(PathBuf::from);
    let mut explicit_dirs = env_project.is_some();
    let mut project_dir = env_project.unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let mut storage_dir = env::var("CHISEL_STORAGE_DIR").ok().map(PathBuf::from);
    explicit_dirs |= storage_dir.is_some();
    let mut worktree = None;
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
    let mut depth: Option<i64> = None;
    let mut kind = None;
    let mut passed: Option<bool> = None;
    let mut coverage_mode = None;
    let mut proximity_adjustment = None;
    let mut exclude_new_file_boost = false;
    let mut allow_repo_extractor = false;
    let mut once = false;
    let mut interval_secs = 2u64;
    let mut functions = Vec::new();
    let mut fallback = false;
    let mut unit = None;
    let mut min_count = None;
    let mut no_exclude_tests = false;
    let mut top_n = None;
    let mut agent_id = None;
    let mut duration_ms = None;
    let mut help = false;
    let mut capture_command = None;
    let mut hook = false;
    let mut all = false;
    let mut since = None;
    let mut brief = false;
    let mut write = false;
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
            explicit_dirs = true;
        } else if a == "--project-dir" {
            i += 1;
            project_dir = PathBuf::from(argv.get(i).ok_or("missing --project-dir")?);
            explicit_dirs = true;
        } else if let Some(v) = a.strip_prefix("--storage-dir=") {
            storage_dir = Some(PathBuf::from(v));
            explicit_dirs = true;
        } else if a == "--storage-dir" {
            i += 1;
            storage_dir = Some(PathBuf::from(argv.get(i).ok_or("missing --storage-dir")?));
            explicit_dirs = true;
        } else if let Some(v) = a.strip_prefix("--worktree=") {
            worktree = Some(PathBuf::from(v));
        } else if a == "--worktree" {
            i += 1;
            worktree = Some(PathBuf::from(argv.get(i).ok_or("missing --worktree: pass the absolute path of a git worktree")?));
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
            let r = argv.get(i).ok_or("missing --ref")?.clone();
            if r.starts_with('-') {
                return Err("status=error ref must not start with '-'".into());
            }
            git_ref = Some(r);
        } else if a == "--depth" {
            i += 1;
            depth = Some(argv.get(i).ok_or("missing --depth")?.parse().map_err(|_| "bad depth: use 1, 2 or 3")?);
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
        } else if a == "--allow-repo-extractor" {
            allow_repo_extractor = true;
        } else if a == "--once" {
            once = true;
        } else if a == "--interval" {
            i += 1;
            interval_secs = argv.get(i).ok_or("missing --interval")?.parse().map_err(|_| "bad interval")?;
            if interval_secs == 0 {
                interval_secs = 1;
            }
        } else if a == "--functions" {
            i += 1;
            while i < argv.len() && !argv[i].starts_with('-') {
                functions.push(argv[i].clone());
                i += 1;
            }
            i = i.saturating_sub(1);
        } else if a == "--fallback" {
            fallback = true;
        } else if a == "--unit" {
            i += 1;
            unit = Some(argv.get(i).ok_or("missing --unit")?.clone());
        } else if a == "--min-count" {
            i += 1;
            min_count = Some(argv.get(i).ok_or("missing --min-count")?.parse().map_err(|_| "bad min-count")?);
        } else if a == "--no-exclude-tests" {
            no_exclude_tests = true;
        } else if a == "--proximity" {
            proximity_adjustment = Some(true);
        } else if a == "--top-n" {
            i += 1;
            top_n = Some(argv.get(i).ok_or("missing --top-n")?.parse().map_err(|_| "bad top-n")?);
        } else if a == "--agent-id" {
            i += 1;
            agent_id = Some(argv.get(i).ok_or("missing --agent-id")?.clone());
        } else if a == "--duration" {
            i += 1;
            duration_ms = Some(argv.get(i).ok_or("missing --duration")?.parse().map_err(|_| "bad duration")?);
        } else if a == "--command" {
            i += 1;
            capture_command = Some(argv.get(i).ok_or("missing --command")?.clone());
        } else if a == "--hook" {
            hook = true;
        } else if a == "--all" {
            all = true;
        } else if a == "--since" {
            i += 1;
            since = Some(argv.get(i).ok_or("missing --since")?.clone());
        } else if a == "--brief" {
            brief = true;
        } else if a == "--write" {
            write = true;
        } else if a == "--help" || a == "-h" {
            help = true;
        } else if a.starts_with("--") {
            return Err(format!("status=error unknown flag {a}. `chisel {} --help` lists the flags.", if command.is_empty() { "<command>" } else { command.as_str() }));
        } else if command.is_empty() {
            command = a.clone();
        } else {
            positionals.push(a.clone());
        }
        i += 1;
    }
    if command.is_empty() && !help {
        return Err("missing command. Run `chisel --help` for the command list.".into());
    }
    // Started inside a linked git worktree with no directory given: the main checkout's snapshot
    // is the base and this worktree the overlay, when main has a snapshot and this tree has none.
    let mut default_worktree = None;
    if !explicit_dirs {
        if let Some((main, tree)) = crate::engine::linked_worktree(&project_dir) {
            if main.join(".chisel").join(crate::store::STORE_NAME).is_file() && !tree.join(".chisel").join(crate::store::STORE_NAME).is_file() {
                project_dir = main;
                default_worktree = Some(tree);
            }
        }
    }
    Ok(Args {
        command, project_dir, storage_dir, json, positionals, host, port, ttl, purpose, test_command,
        force, working_tree, auto_update, limit, shard, directory, git_ref, depth, kind, passed,
        coverage_mode, proximity_adjustment, exclude_new_file_boost, allow_repo_extractor, once, interval_secs,
        functions, fallback, unit, min_count, no_exclude_tests, top_n, agent_id, duration_ms, help,
        capture_command, hook, all, since, brief, write, worktree, default_worktree,
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
            crate::http::serve(&args.project_dir, args.storage_dir.as_deref(), args.default_worktree.as_deref(), &args.host, args.port)?;
            return Ok(0);
        }
        "serve-mcp" | "mcp" => {
            crate::mcp::serve(&args.project_dir, args.storage_dir.as_deref(), args.default_worktree.as_deref())?;
            return Ok(0);
        }
        "help" => {
            return Ok(match args.positionals.first() {
                Some(cmd) => if crate::cli_text::command_help(cmd) { 0 } else { 2 },
                None => {
                    help();
                    0
                }
            });
        }
        "run" => return crate::run_cmd::run_tests(args),
        "capture" => return Ok(crate::capture::capture_cmd(args)),
        "hook-install" => return crate::capture::hook_install(args),
        "issues" if args.brief => return Ok(crate::capture::brief(args)),
        "migrate" => return migrate(args),
        "watch" => return watch(args),
        _ => {}
    }
    if crate::cli_text::command_entry(&args.command).is_none() {
        eprintln!("Error: unknown command '{}'. Run `chisel --help` for the command list.", args.command);
        return Ok(2);
    }
    let mut engine = Engine::open(&args.project_dir, args.storage_dir.as_deref())?;
    if args.allow_repo_extractor {
        engine.allow_repo_extractor = true;
    }
    engine.default_worktree = args.default_worktree.clone();
    // The CLI has no client timeout: `analyze --force` runs here instead of as a job, and a job
    // started here is waited for (the process would otherwise exit and take the job with it).
    engine.inline_jobs = true;
    let tool = tool_name(&args.command);
    let call_args = call_args(args);
    let mut result = engine.dispatch(&tool, &call_args)?;
    if tool == "start_job" && result.get("status").and_then(|v| v.as_str()) == Some("queued") {
        if let Some(id) = result.get("job_id").and_then(|v| v.as_str()).map(|s| s.to_string()) {
            result = wait_for_job(&mut engine, &id);
        }
    }
    emit(args.json, &args.command, &result)?;
    Ok(exit_code(&result))
}

/// 0 for answers, including `no_data`, `no_changes`, `no_edges`, `stale_db` and `auto_queued`
/// (the reply says what to do next); 1 for `error` and `git_error`. Usage errors exit 2.
pub fn exit_code(result: &Value) -> i32 {
    match result.get("status").and_then(|v| v.as_str()) {
        Some("error" | "git_error") => 1,
        _ => 0,
    }
}

fn wait_for_job(engine: &mut Engine, id: &str) -> Value {
    loop {
        let status = engine.job_status_value(id);
        match status.get("status").and_then(|v| v.as_str()) {
            Some("queued" | "running") => std::thread::sleep(std::time::Duration::from_millis(100)),
            _ => return status,
        }
    }
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
            "analyze" | "risk-map" | "triage" => {
                pairs.push(("directory".into(), Value::String(first.clone())));
            }
            "test-gaps" => {
                pairs.push(("file_path".into(), Value::String(first.clone())));
            }
            "start-job" => {
                if args.kind.is_none() && (first == "analyze" || first == "update") {
                    pairs.push(("kind".into(), Value::String(first.clone())));
                }
            }
            "acquire-lock" | "release-lock" | "refresh-lock" => {
                pairs.push(("file_path".into(), Value::String(first.clone())));
                if let Some(agent) = args.positionals.get(1) {
                    pairs.push(("agent_id".into(), Value::String(agent.clone())));
                }
            }
            "check-lock" | "trace" => {
                pairs.push(("file_path".into(), Value::String(first.clone())));
            }
            "issue" => {
                pairs.push(("id".into(), Value::String(first.clone())));
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
    if args.all {
        pairs.push(("all".into(), Value::Bool(true)));
    }
    if let Some(since) = &args.since {
        pairs.push(("since".into(), Value::String(since.clone())));
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
    if let Some(depth) = args.depth {
        pairs.push(("depth".into(), Value::Int(depth)));
    }
    if let Some(worktree) = &args.worktree {
        let path = if worktree.is_absolute() { worktree.clone() } else { env::current_dir().map(|d| d.join(worktree)).unwrap_or_else(|_| worktree.clone()) };
        pairs.push(("worktree_root".into(), Value::String(path.to_string_lossy().into_owned())));
    }

    if let Some(kind) = &args.kind {
        pairs.push(("kind".into(), Value::String(kind.clone())));
    } else if args.command == "start-job" {
        if let Some(kind) = args.positionals.iter().find(|p| *p == "analyze" || *p == "update") {
            if !pairs.iter().any(|(k, _)| k == "kind") {
                pairs.push(("kind".into(), Value::String(kind.clone())));
            }
        }
        if args.command == "start-job" {
            if let Some(dir) = args.positionals.iter().find(|p| *p != "analyze" && *p != "update") {
                pairs.push(("directory".into(), Value::String(dir.clone())));
            }
        }
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
    if !args.functions.is_empty() {
        pairs.push(("functions".into(), Value::Array(args.functions.iter().cloned().map(Value::String).collect())));
    }
    if args.fallback {
        pairs.push(("fallback_to_all".into(), Value::Bool(true)));
    }
    if let Some(unit) = &args.unit {
        pairs.push(("unit_name".into(), Value::String(unit.clone())));
    }
    if let Some(min_count) = args.min_count {
        pairs.push(("min_count".into(), Value::Int(min_count)));
    }
    if args.no_exclude_tests {
        pairs.push(("exclude_tests".into(), Value::Bool(false)));
    }
    if let Some(top_n) = args.top_n {
        pairs.push(("top_n".into(), Value::Int(top_n)));
    }
    if let Some(agent) = &args.agent_id {
        pairs.push(("agent_id".into(), Value::String(agent.clone())));
    }
    if let Some(duration) = args.duration_ms {
        pairs.push(("duration_ms".into(), Value::Int(duration)));
    }
    if matches!(args.command.as_str(), "job-status" | "cancel-job") {
        if let Some(id) = args.positionals.first() {
            pairs.push(("job_id".into(), Value::String(id.clone())));
        }
    }
    Value::object(pairs)
}

pub fn parse_jest_json(text: &str) -> Vec<(String, bool)> {
    crate::run_cmd::parse_jest_json(text, None)
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
    crate::cli_text::render(command, result)
}


fn worktree_fingerprint(dir: &PathBuf) -> Result<String, String> {
    let status = Command::new("git")
        .args(["status", "--porcelain=v1", "-uall"])
        .current_dir(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if !status.status.success() {
        return Err(String::from_utf8_lossy(&status.stderr).trim().to_string());
    }
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "{}\n{}",
        String::from_utf8_lossy(&head.stdout).trim(),
        String::from_utf8_lossy(&status.stdout)
    ))
}

fn emit_watch_error(args: &Args, err: &str) -> Result<(), String> {
    let body = Value::object(vec![
        ("status".into(), Value::String("error".into())),
        ("message".into(), Value::String(err.to_string())),
    ]);
    emit(args.json, "watch", &body)?;
    let _ = io::stdout().flush();
    Ok(())
}

fn watch(args: &Args) -> Result<i32, String> {
    use std::thread;
    use std::time::Duration;
    let mut engine = Engine::open(&args.project_dir, args.storage_dir.as_deref())?;
    if args.allow_repo_extractor {
        engine.allow_repo_extractor = true;
    }
    let mut previous = worktree_fingerprint(&args.project_dir)?;
    let ready = Value::object(vec![
        ("status".into(), Value::String("watching".into())),
        ("interval_secs".into(), Value::Int(args.interval_secs as i64)),
        ("watcher".into(), Value::String(crate::fswatch::backend_name().into())),
    ]);
    emit(args.json, "watch", &ready)?;
    let _ = io::stdout().flush();
    loop {
        let slice = Duration::from_secs(args.interval_secs);
        if crate::fswatch::wait_for_change(&args.project_dir, slice).is_err() {
            thread::sleep(slice);
        }
        let current = match worktree_fingerprint(&args.project_dir) {
            Ok(value) => value,
            Err(err) => {
                emit_watch_error(args, &err)?;
                if args.once || !args.project_dir.exists() {
                    return Err(err);
                }
                continue;
            }
        };
        if current != previous {
            match engine.update() {
                Ok(result) => {
                    emit(args.json, "update", &result)?;
                    let _ = io::stdout().flush();
                    previous = current;
                    if args.once {
                        return Ok(0);
                    }
                }
                Err(err) => {
                    emit_watch_error(args, &err)?;
                    if args.once || !args.project_dir.exists() {
                        return Err(err);
                    }
                }
            }
        } else if args.once {
            let quiet = Value::object(vec![
                ("status".into(), Value::String("no_changes".into())),
            ]);
            emit(args.json, "watch", &quiet)?;
            let _ = io::stdout().flush();
            return Ok(0);
        }
    }
}

pub fn argv_is_mcp_binary(argv0: &str) -> bool {
    std::path::Path::new(argv0).file_stem().and_then(|s| s.to_str()) == Some("chisel-mcp")
}

pub fn version() {
    println!("chisel {}", env!("CARGO_PKG_VERSION"));
}

pub fn entry() -> Result<i32, String> {
    let mut argv: Vec<String> = env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--version") {
        version();
        return Ok(0);
    }
    let argv0 = env::args().next().unwrap_or_default();
    if argv_is_mcp_binary(&argv0) {
        argv.insert(0, "serve-mcp".into());
    }
    if argv.is_empty() {
        help();
        return Ok(0);
    }
    let args = match parse(&argv) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("Error: {e}");
            return Ok(2);
        }
    };
    if args.help {
        if args.command.is_empty() {
            help();
        } else if !crate::cli_text::command_help(&args.command) {
            return Ok(2);
        }
        return Ok(0);
    }
    run(&args)
}

fn migrate(args: &Args) -> Result<i32, String> {
    let db = if let Some(path) = args.positionals.first() {
        PathBuf::from(path)
    } else {
        let candidates = [
            args.project_dir.join("chisel.db"),
            args.project_dir.join(".chisel").join("chisel.db"),
            args.storage_dir.clone().unwrap_or_else(|| args.project_dir.join(".chisel")).join("chisel.db"),
        ];
        match candidates.into_iter().find(|p| p.is_file()) {
            Some(path) => path,
            None => {
                let body = Value::object(vec![
                    ("status".into(), Value::String("error".into())),
                    ("message".into(), Value::String("No chisel.db found. Pass the path: chisel migrate path/to/chisel.db".into())),
                ]);
                emit(args.json, "migrate", &body)?;
                return Ok(1);
            }
        }
    };
    let mut engine = Engine::open(&args.project_dir, args.storage_dir.as_deref())?;
    let result = engine.migrate_legacy(&db)?;
    emit(args.json, "migrate", &result)?;
    Ok(0)
}

pub fn help() {
    println!("Chisel — test impact analysis: which tests to run after an edit");
    println!("usage: chisel [--json] [--version] [--project-dir DIR] [--storage-dir DIR] <command> ...");
    println!("After editing: `chisel diff-impact` (committed work too: `--since main`; another worktree: `--worktree PATH`).");
    let mut line = String::from("commands:");
    for (name, _, _) in crate::cli_text::COMMANDS {
        if line.len() + name.len() > 92 {
            println!("{line}");
            line = String::from(" ");
        }
        line.push(' ');
        line.push_str(name);
    }
    println!("{line}");
    println!("Run `chisel <command> --help` for one command. Exit codes: 0 answered, 1 error, 2 usage.");
}

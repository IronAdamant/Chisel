//! Human-readable command output and per-command help.

use crate::json::{self, Value};

pub fn render(command: &str, result: &Value) -> Result<(), String> {
    match command {
        "impact" | "diff-impact" => print_list(result, "No impacted tests found.", if command == "impact" { "Impacted tests:" } else { "Impacted tests from diff:" }, |item| {
            format!("{}  ({})", item.get("test_id").and_then(|v| v.as_str()).unwrap_or(""), item.get("reason").and_then(|v| v.as_str()).unwrap_or(""))
        }),
        "suggest-tests" => render_suggest(result),
        "churn" => render_churn(result),
        "ownership" => print_list(result, "No ownership data.", "Ownership:", |item| {
            format!("{}: {}", item.get("author").and_then(|v| v.as_str()).unwrap_or(""), scalar(item.get("percentage")))
        }),
        "coupling" => render_coupling(result),
        "risk-map" => render_risk(result),
        "stale-tests" => print_list(result, "No stale tests found.", "Stale tests:", |item| {
            format!("{}  ({})", item.get("test_id").and_then(|v| v.as_str()).unwrap_or(""), item.get("edge_type").and_then(|v| v.as_str()).unwrap_or(""))
        }),
        "history" => print_list(result, "No history found.", "History:", |item| {
            let hash = item.get("hash").and_then(|v| v.as_str()).unwrap_or("");
            let short = hash.get(..8).unwrap_or(hash);
            format!("{short}  {}  {}  {}", scalar(item.get("date")), scalar(item.get("author")), scalar(item.get("message")))
        }),
        "who-reviews" => print_list(result, "No reviewer suggestions.", "Suggested reviewers:", |item| {
            format!("{}: {}% ({} commits, {}d ago)", scalar(item.get("author")), scalar(item.get("percentage")), scalar(item.get("recent_commits")), scalar(item.get("days_since_last_commit")))
        }),
        "test-gaps" => print_list(result, "No untested code units found.", "Untested code units:", |item| {
            format!("{}:{} ({}, lines {}-{}, churn: {})", scalar(item.get("file_path")), scalar(item.get("name")), scalar(item.get("unit_type")), scalar(item.get("line_start")), scalar(item.get("line_end")), scalar(item.get("churn_score")))
        }),
        "record-result" => {
            let id = result.get("test_id").and_then(|v| v.as_str()).unwrap_or("");
            let passed = result.get("passed").and_then(|v| v.as_bool()).unwrap_or(false);
            println!("Recorded: {id} — {}", if passed { "PASSED" } else { "FAILED" });
        }
        "triage" => render_triage(result),
        "analyze" | "update" | "stats" | "start-job" | "job-status" | "cancel-job" | "migrate" => {
            let header = match command {
                "analyze" => "Analysis complete:",
                "update" => "Incremental update complete:",
                "stats" => "Chisel database stats:",
                "start-job" => "Background job:",
                "job-status" => "Job status:",
                "cancel-job" => "Cancel job:",
                _ => "Migrate:",
            };
            println!("{header}");
            if let Some(pairs) = result.as_object() {
                for (k, v) in pairs {
                    println!("  {}: {}", title_key(k), scalar(Some(v)));
                }
            }
        }
        "acquire-lock" | "release-lock" | "refresh-lock" | "check-lock" | "check-locks" | "list-locks" => render_lock(command, result),
        _ => println!("{}", json::to_string_pretty(result).map_err(|e| e.message)?),
    }
    Ok(())
}

fn render_suggest(result: &Value) {
    if let Some(pairs) = result.as_object() {
        if pairs.is_empty() {
            println!("No test suggestions.");
            return;
        }
        for (file, items) in pairs {
            println!("\n{file}:");
            if let Some(rows) = items.as_array() {
                for item in rows {
                    println!("  {}  (score: {})", scalar(item.get("name")), scalar(item.get("relevance")));
                }
            }
        }
        return;
    }
    print_list(result, "No test suggestions.", "Suggested tests:", |item| {
        format!("{}  (score: {})", scalar(item.get("name")), scalar(item.get("relevance")))
    });
}

fn render_churn(result: &Value) {
    let Some(rows) = result.as_array() else {
        println!("{}", json::to_string_pretty(result).unwrap_or_default());
        return;
    };
    if rows.is_empty() {
        println!("No churn data available.");
        return;
    }
    let file = rows.first().and_then(|r| r.get("file_path")).and_then(|v| v.as_str()).unwrap_or("");
    println!("Churn stats for {file}:");
    for item in rows {
        if let Some(pairs) = item.as_object() {
            for (k, v) in pairs {
                println!("  {k}: {}", scalar(Some(v)));
            }
            println!();
        }
    }
}

fn render_coupling(result: &Value) {
    let cc = result.get("co_change_partners").and_then(|v| v.as_array()).map(|a| a.to_vec()).unwrap_or_default();
    let imp = result.get("import_partners").and_then(|v| v.as_array()).map(|a| a.to_vec()).unwrap_or_default();
    if cc.is_empty() && imp.is_empty() {
        println!("No coupling data.");
        return;
    }
    if !cc.is_empty() {
        println!("Co-change partners:");
        for p in &cc {
            println!("  {}  ({} co-commits)", scalar(p.get("file_b").or_else(|| p.get("file_a"))), scalar(p.get("co_commit_count")));
        }
    }
    if !imp.is_empty() {
        println!("Import partners:");
        for p in &imp {
            println!("  {}", scalar(p.get("file")));
        }
    }
    if let Some(note) = result.get("note").and_then(|v| v.as_str()) {
        println!("{note}");
    }
}

fn render_risk(result: &Value) {
    let files = result.get("files").and_then(|v| v.as_array());
    let Some(files) = files else {
        println!("No risk data.");
        return;
    };
    if files.is_empty() {
        println!("No risk data.");
        return;
    }
    println!("Risk map:");
    for item in files {
        let locked = item.get("locked_by").and_then(|v| v.as_str()).map(|a| format!("  locked by {a}")).unwrap_or_default();
        println!("  {}: {}{locked}", scalar(item.get("file_path")), scalar(item.get("risk_score")));
    }
    if let Some(uniform) = result.get("_meta").and_then(|m| m.get("uniform_components")).and_then(|v| v.as_object()) {
        if !uniform.is_empty() {
            println!("\nDiagnostics (uniform components — not differentiating):");
            for (comp, info) in uniform {
                println!("  {comp}: {}", info.get("reason").and_then(|v| v.as_str()).unwrap_or(""));
            }
        }
    }
    if let Some(cycles) = result.get("_meta").and_then(|m| m.get("cycles")).and_then(|v| v.as_array()) {
        if !cycles.is_empty() {
            println!("\nImport cycles:");
            for cycle in cycles {
                let files = cycle.get("files").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                println!("  {files}");
            }
        }
    }
}

fn render_triage(result: &Value) {
    let Some(summary) = result.get("summary") else {
        println!("{}", json::to_string_pretty(result).unwrap_or_default());
        return;
    };
    println!("Triage ({} files):", scalar(summary.get("files_triaged")));
    println!("\nTop risk files:");
    if let Some(rows) = result.get("top_risk_files").and_then(|v| v.as_array()) {
        for row in rows {
            println!("  {}: {}", scalar(row.get("file_path")), scalar(row.get("risk_score")));
        }
    }
    if let Some(gaps) = result.get("test_gaps").and_then(|v| v.as_array()) {
        if gaps.is_empty() {
            println!("\nNo test gaps in triaged files.");
        } else {
            println!("\nTest gaps ({}):", scalar(summary.get("total_test_gaps")));
            for gap in gaps {
                println!("  {}:{} ({})", scalar(gap.get("file_path")), scalar(gap.get("name")), scalar(gap.get("unit_type")));
            }
        }
    }
    if let Some(stale) = result.get("stale_tests").and_then(|v| v.as_array()) {
        if stale.is_empty() {
            println!("\nNo stale tests found.");
        } else {
            println!("\nStale tests ({}):", scalar(summary.get("total_stale_tests")));
            for row in stale {
                println!("  {}  ({})", scalar(row.get("test_id")), scalar(row.get("edge_type")));
            }
        }
    }
}

fn render_lock(command: &str, result: &Value) {
    match command {
        "list-locks" => {
            let locks = result.get("locks").and_then(|v| v.as_array()).map(|a| a.to_vec()).unwrap_or_default();
            if locks.is_empty() {
                println!("No active locks");
            } else {
                println!("{} active lock(s):", scalar(result.get("total")));
                for lock in locks {
                    let stale = if lock.get("ttl_remaining").and_then(|v| v.as_f64()).unwrap_or(0.0) < 60.0 { " (STALE)" } else { "" };
                    println!("  {}: {} ({}s remaining){stale}", scalar(lock.get("file_path")), scalar(lock.get("agent_id")), scalar(lock.get("ttl_remaining")));
                }
            }
        }
        "check-locks" => {
            let conflicts = result.get("conflicts").and_then(|v| v.as_array()).map(|a| a.to_vec()).unwrap_or_default();
            if conflicts.is_empty() {
                println!("No locks on {} checked file(s)", scalar(result.get("checked")));
            } else {
                println!("{} lock(s) found:", conflicts.len());
                for row in conflicts {
                    println!("  {}: {}", scalar(row.get("file_path")), scalar(row.get("holder")));
                }
            }
        }
        "check-lock" => {
            if result.get("locked").and_then(|v| v.as_bool()) == Some(true) {
                println!("Locked");
                println!("Holder:   {}", scalar(result.get("holder")));
                println!("TTL rem:  {}s", scalar(result.get("ttl_remaining")));
                println!("Stale:    {}", scalar(result.get("stale")));
            } else {
                println!("Unlocked");
            }
        }
        _ => {
            println!("{}", json::to_string_pretty(result).unwrap_or_default());
        }
    }
}

fn print_list(result: &Value, empty: &str, header: &str, line: impl Fn(&Value) -> String) {
    let Some(rows) = result.as_array() else {
        println!("{}", json::to_string_pretty(result).unwrap_or_default());
        return;
    };
    if rows.is_empty() {
        println!("{empty}");
        return;
    }
    println!("{header}");
    for item in rows {
        println!("  {}", line(item));
    }
}

fn scalar(v: Option<&Value>) -> String {
    match v {
        Some(Value::Int(i)) => i.to_string(),
        Some(Value::Float(f)) => f.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => "null".into(),
        Some(other) => json::to_string(other).unwrap_or_else(|_| "…".into()),
    }
}

pub fn title_key(key: &str) -> String {
    key.split('_').map(|w| {
        let mut c = w.chars();
        match c.next() {
            None => String::new(),
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        }
    }).collect::<Vec<_>>().join(" ")
}

pub fn command_help(command: &str) {
    let text = match command {
        "analyze" => "analyze [DIR] [--force] [--shard NAME]\n  Scan the project and write .chisel/store.",
        "update" => "update [--shard NAME]\n  Recompute changed files. A no-op reports code units found as 0.",
        "impact" => "impact FILE... [--functions NAME...]\n  Tests that reach the named files, optionally limited to those functions.",
        "suggest-tests" => "suggest-tests FILE | --directory DIR [--fallback] [--working-tree] [--auto-update]\n  Rank tests for one file, or return a map for every code file under a directory.",
        "churn" => "churn FILE [--unit NAME]\n  File-level churn, or one unit row with insertions and last changed.",
        "ownership" => "ownership FILE\n  Blame share for a file.",
        "coupling" => "coupling FILE [--min-count N] [--working-tree]\n  Co-change partners. --working-tree scans an untracked file on disk.",
        "risk-map" => "risk-map [DIR] [--no-exclude-tests] [--proximity|--no-proximity] [--coverage-mode unit|line]\n  Risk scores. DIR limits the files. Cycles and locks are included when present.",
        "test-gaps" => "test-gaps [FILE] [--directory DIR] [--no-exclude-tests]\n  Untested units. A file argument or --directory limits the scope.",
        "triage" => "triage [DIR] [--top-n N] [--no-exclude-tests]\n  Top risk files plus their gaps and stale tests.",
        "stale-tests" => "stale-tests\n  Tests whose edges point at code that is gone. Returns a list when edges exist.",
        "record-result" => "record-result TEST_ID (--passed|--failed) [--duration MS]\n  Store a result. Duration feeds the instability variance.",
        "run" => "run -- COMMAND...\n  Run tests, stream their output, record results, and log issues for failures.",
        "capture" => "capture [--hook] [--command CMD] < OUTPUT\n  Read test output (cargo, pytest, jest, vitest, go, node:test), record results and issues.\n  Prints one line only for new, reopened or fixed issues; always exits 0. --hook reads a\n  Claude Code PostToolUse payload. Needs a snapshot (`chisel analyze`).",
        "issues" => "issues [--all] [--since 30m|12h|7d|YYYY-MM-DD] [--limit N] [--brief]\n  Captured test failures, newest first. --brief prints at most two lines, for a SessionStart hook.",
        "issue" => "issue ID\n  One issue: error, location, suspects, occurrences, excerpt, fix.",
        "trace" => "trace FILE [--unit NAME]\n  Issues where the file or function was a suspect, the test, the failure site, or fixed.",
        "hook-install" => "hook-install [--write]\n  Print Claude Code hooks for capture and the SessionStart summary. --write merges them into\n  .claude/settings.local.json with this binary's path.",
        "stats" => "stats\n  Counts, including co_change_query_min as a number and the shadow graph.",
        "start-job" => "start-job analyze|update [DIR] [--force] [--shard NAME]\n  Queue analyze or update. The kind may be the first argument.",
        "job-status" => "job-status JOB_ID",
        "cancel-job" => "cancel-job JOB_ID",
        "list-locks" => "list-locks [--agent-id ID]\n  Active advisory locks, optionally for one agent.",
        "serve" => "serve [--host HOST] [--port PORT]\n  HTTP JSON API. Not an MCP URL.",
        "serve-mcp" => "serve-mcp\n  Stdio MCP. Accepts newline-delimited JSON and Content-Length framing.",
        "migrate" => "migrate [chisel.db]\n  Copy test results from a legacy SQLite chisel.db into .chisel/store.",
        "watch" => "watch [--once] [--interval SECS]\n  Run update when the worktree changes.",
        other => {
            println!("Unknown command '{other}'.");
            println!("Run `chisel --help` for the command list.");
            return;
        }
    };
    println!("chisel {text}");
}

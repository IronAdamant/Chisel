//! `chisel capture`, `chisel issues --brief` and `chisel hook-install`: the zero-token side of
//! issue logging, run by Claude Code hooks.
//!
//! `capture` reads test output on stdin (or a PostToolUse hook payload with `--hook`), records
//! results and issues, prints one line only when an issue is new, reopened or fixed, and always
//! exits 0. Non-test commands and projects without a snapshot return before any work.

use crate::cli::Args;
use crate::engine::Engine;
use crate::json::{self, Value};
use crate::test_output::{is_test_command, parse};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const CAPTURE_HOOK: &str = "chisel capture --hook";
pub const BRIEF_HOOK: &str = "chisel issues --brief";

fn storage(project: &Path, args: &Args) -> PathBuf {
    args.storage_dir.clone().unwrap_or_else(|| project.join(".chisel"))
}

/// Command, output and project dir from a PostToolUse payload. `None` unless it is Bash.
fn from_hook(payload: &str) -> Option<(String, String, Option<PathBuf>)> {
    let doc = json::parse(payload).ok()?;
    if doc.get("tool_name").and_then(|v| v.as_str()) != Some("Bash") {
        return None;
    }
    let command = doc.get("tool_input").and_then(|t| t.get("command")).and_then(|v| v.as_str())?.to_string();
    let response = doc.get("tool_response");
    let text = |key: &str| response.and_then(|r| r.get(key)).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut output = text("stdout");
    let err = text("stderr");
    if !err.is_empty() {
        output.push('\n');
        output.push_str(&err);
    }
    if output.is_empty() {
        output = text("output");
    }
    let cwd = doc.get("cwd").and_then(|v| v.as_str()).map(PathBuf::from);
    Some((command, output, cwd))
}

/// The line to show, or `None` when nothing new happened. Errors are swallowed: a hook must
/// never disturb the session.
fn capture_inner(args: &Args, input: String) -> Option<String> {
    let (command, output, project) = if args.hook {
        let (command, output, cwd) = from_hook(&input)?;
        (command, output, cwd.unwrap_or_else(|| args.project_dir.clone()))
    } else {
        (args.capture_command.clone().unwrap_or_default(), input, args.project_dir.clone())
    };
    if (args.hook || !command.is_empty()) && !is_test_command(&command) {
        return None;
    }
    let store = storage(&project, args);
    if !store.join("store").is_file() {
        return None;
    }
    let parsed = parse(&output, Some(&project));
    if parsed.outcomes.is_empty() && parsed.passed_files.is_empty() && parsed.passed_packages.is_empty() {
        return None;
    }
    // A broken snapshot must not make the hook silently inert: say so in one line.
    let mut engine = match Engine::open(&project, Some(&store)) {
        Ok(engine) => engine,
        Err(e) => return Some(unreadable(&e)),
    };
    match engine.capture(&parsed, &command, true) {
        Ok(summary) => summary.line(),
        Err(e) => Some(format!("chisel: test results not recorded ({}); run `chisel analyze` if this repeats", short_reason(&e))),
    }
}

fn short_reason(e: &str) -> String {
    e.lines().next().unwrap_or(e).chars().take(120).collect()
}

/// The one line for a snapshot that exists but cannot be loaded.
fn unreadable(e: &str) -> String {
    format!("chisel: snapshot unreadable ({}); run `chisel analyze` to rebuild it", short_reason(e))
}

/// `chisel capture`. Always 0.
pub fn capture_cmd(args: &Args) -> i32 {
    // Reading stdin waits for the test run that is piping into it; time only Chisel's work.
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let start = Instant::now();
    std::panic::set_hook(Box::new(|_| {}));
    let line = std::panic::catch_unwind(|| capture_inner(args, input)).ok().flatten();
    if let Some(line) = line {
        if args.hook {
            let out = Value::object(vec![(
                "hookSpecificOutput".into(),
                Value::object(vec![
                    ("hookEventName".into(), Value::String("PostToolUse".into())),
                    ("additionalContext".into(), Value::String(line)),
                ]),
            )]);
            println!("{}", json::to_string(&out).unwrap_or_default());
        } else {
            println!("{line}");
        }
    }
    if std::env::var_os("CHISEL_CAPTURE_TIMING").is_some() {
        eprintln!("chisel capture: {} ms", start.elapsed().as_millis());
    }
    0
}

/// `chisel issues --brief`: at most two lines for a SessionStart hook; nothing when all is well.
pub fn brief(args: &Args) -> i32 {
    std::panic::set_hook(Box::new(|_| {}));
    let lines = std::panic::catch_unwind(|| {
        let store = storage(&args.project_dir, args);
        if !store.join("store").is_file() {
            return Vec::new();
        }
        match Engine::open(&args.project_dir, Some(&store)) {
            Ok(mut engine) => engine.issues_brief(),
            Err(e) => vec![unreadable(&e)],
        }
    })
    .unwrap_or_default();
    for line in lines {
        println!("{line}");
    }
    0
}

/// The hooks block for `.claude/settings.json`, calling `program`.
pub fn hook_settings(program: &str) -> Value {
    let hook = |cmd: String| Value::object(vec![("type".into(), Value::String("command".into())), ("command".into(), Value::String(cmd))]);
    Value::object(vec![(
        "hooks".into(),
        Value::object(vec![
            ("PostToolUse".into(), Value::Array(vec![Value::object(vec![
                ("matcher".into(), Value::String("Bash".into())),
                ("hooks".into(), Value::Array(vec![hook(format!("{program} capture --hook"))])),
            ])])),
            ("SessionStart".into(), Value::Array(vec![Value::object(vec![("hooks".into(), Value::Array(vec![hook(format!("{program} issues --brief"))]))])])),
        ]),
    )])
}

/// Add Chisel's hooks to a settings document, keeping everything else. Idempotent.
pub fn merge_hooks(settings: &mut Value, program: &str) {
    let wanted = hook_settings(program);
    if settings.get("hooks").and_then(|h| h.as_object()).is_none() {
        settings.insert("hooks", Value::object(vec![]));
    }
    let Some(hooks) = settings.get_mut("hooks") else { return };
    for event in ["PostToolUse", "SessionStart"] {
        let entry = wanted.get("hooks").and_then(|h| h.get(event)).and_then(|v| v.as_array()).and_then(|a| a.first()).cloned().unwrap_or(Value::Null);
        let command = entry.get("hooks").and_then(|v| v.as_array()).and_then(|a| a.first()).and_then(|h| h.get("command")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let marker = if event == "PostToolUse" { " capture --hook" } else { " issues --brief" };
        if hooks.get(event).and_then(|v| v.as_array()).is_none() {
            hooks.insert(event, Value::Array(Vec::new()));
        }
        let Some(list) = hooks.get_mut(event).and_then(|v| v.as_array_mut()) else { continue };
        let present = list.iter().any(|e| {
            e.get("hooks").and_then(|v| v.as_array()).is_some_and(|hs| hs.iter().any(|h| h.get("command").and_then(|c| c.as_str()).is_some_and(|c| c.contains("chisel") && c.ends_with(marker))))
        });
        if !present && !command.is_empty() {
            list.push(entry);
        }
    }
}

/// `chisel hook-install`: print the hooks; with `--write`, merge them into
/// `.claude/settings.local.json` (machine-specific: it names this binary's path).
pub fn hook_install(args: &Args) -> Result<i32, String> {
    if !args.write {
        println!("{}", json::to_string_pretty(&hook_settings("chisel")).map_err(|e| e.message)?);
        println!("Add this to .claude/settings.json, or run `chisel hook-install --write` to merge it into .claude/settings.local.json with this binary's path. Run `chisel analyze` once so captures have a snapshot.");
        return Ok(0);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().replace('\\', "/");
    let program = if exe.contains(' ') { format!("\"{exe}\"") } else { exe };
    let dir = args.project_dir.join(".claude");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("settings.local.json");
    let mut settings = match std::fs::read_to_string(&path) {
        Ok(text) => json::parse(&text).map_err(|e| format!("{} is not valid JSON ({}); fix it or move it aside", path.display(), e.message))?,
        Err(_) => Value::object(vec![]),
    };
    merge_hooks(&mut settings, &program);
    std::fs::write(&path, json::to_string_pretty(&settings).map_err(|e| e.message)? + "\n").map_err(|e| e.to_string())?;
    println!("Wrote Chisel's PostToolUse and SessionStart hooks to {}", path.display());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_payload_only_for_bash() {
        let payload = r#"{"session_id":"s","cwd":"C:/work/proj","hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"},"tool_response":{"stdout":"test a ... ok","stderr":"","interrupted":false}}"#;
        let (cmd, out, cwd) = from_hook(payload).unwrap();
        assert_eq!((cmd.as_str(), out.as_str(), cwd.unwrap()), ("cargo test", "test a ... ok", PathBuf::from("C:/work/proj")));
        assert!(from_hook(r#"{"tool_name":"Edit","tool_input":{}}"#).is_none());
    }

    #[test]
    fn merging_hooks_is_idempotent_and_keeps_other_settings() {
        let mut settings = json::parse(r#"{"model":"x","hooks":{"PostToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"fmt"}]}]}}"#).unwrap();
        merge_hooks(&mut settings, "chisel");
        merge_hooks(&mut settings, "chisel");
        let text = json::to_string(&settings).unwrap();
        assert_eq!(text.matches("chisel capture --hook").count(), 1, "{text}");
        assert_eq!(text.matches("chisel issues --brief").count(), 1, "{text}");
        assert!(text.contains(r#""model":"x""#) && text.contains(r#""command":"fmt""#), "{text}");
    }
}

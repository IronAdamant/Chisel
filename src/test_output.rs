//! Test runner output -> per-test outcomes, with no model in the loop.
//!
//! Covers cargo test, pytest (verbose, default and -q), Jest (text and --json), Vitest,
//! go test (text, -v and -json) and node:test (TAP and spec). Each outcome carries what the
//! output says about the failure: a one-line message, `file:line`, and a short excerpt.
//! Runners that do not list passing tests still report passing files or packages, kept in
//! `passed_files` / `passed_packages` so an issue can be closed only on real evidence.

use crate::json;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// Test file relative to the project, when the output names it.
    pub file: Option<String>,
    /// Test name as the runner prints it (cargo: the full module path).
    pub name: String,
    pub passed: bool,
    pub message: String,
    pub location: String,
    pub excerpt: String,
    /// go test: the package path.
    pub package: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub outcomes: Vec<Outcome>,
    /// Files where every test passed (pytest progress, Jest PASS, Vitest checkmark).
    pub passed_files: Vec<String>,
    /// go packages reported `ok`.
    pub passed_packages: Vec<String>,
    /// The run printed its final summary (it was not cut short).
    pub complete: bool,
    /// Runners recognised in the output.
    pub runners: Vec<&'static str>,
}

const EXCERPT_BYTES: usize = 600;

/// Strip ANSI colour codes.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// A path as the project names it: forward slashes, relative to `project` when inside it.
pub fn relative(path: &str, project: Option<&Path>) -> String {
    let p = path.trim().trim_matches('\'').replace("\\\\", "/").replace('\\', "/");
    let p = p.strip_prefix("file:///").map(|s| s.to_string()).unwrap_or(p);
    if let Some(root) = project {
        let root = root.to_string_lossy().replace('\\', "/");
        let root = root.trim_end_matches('/');
        if !root.is_empty() && p.to_ascii_lowercase().starts_with(&format!("{}/", root.to_ascii_lowercase())) {
            return p[root.len() + 1..].to_string();
        }
    }
    p.strip_prefix("./").unwrap_or(&p).to_string()
}

/// `path:line` from `path:line[:col]`, relative to the project.
fn location(raw: &str, project: Option<&Path>) -> String {
    let raw = raw.trim().trim_matches(|c| c == '(' || c == ')' || c == '\'');
    let mut parts: Vec<&str> = raw.rsplitn(3, ':').collect();
    parts.reverse();
    // path may itself hold a drive colon: rsplitn(3) keeps it in parts[0]
    match parts.as_slice() {
        [path, line, col] if line.chars().all(|c| c.is_ascii_digit()) && col.chars().all(|c| c.is_ascii_digit()) => format!("{}:{line}", relative(path, project)),
        [head, path_tail, line] if line.chars().all(|c| c.is_ascii_digit()) => format!("{}:{line}", relative(&format!("{head}:{path_tail}"), project)),
        [path, line] if line.chars().all(|c| c.is_ascii_digit()) => format!("{}:{line}", relative(path, project)),
        _ => relative(raw, project),
    }
}

fn excerpt(lines: &[&str]) -> String {
    let mut s = String::new();
    for l in lines {
        let l = l.trim_end();
        if l.is_empty() && s.is_empty() {
            continue;
        }
        if s.len() + l.len() + 1 > EXCERPT_BYTES {
            let room = EXCERPT_BYTES.saturating_sub(s.len());
            let mut cut = room.min(l.len());
            while cut > 0 && !l.is_char_boundary(cut) {
                cut -= 1;
            }
            s.push_str(&l[..cut]);
            break;
        }
        s.push_str(l);
        s.push('\n');
    }
    s.trim_end().to_string()
}

fn first_text(lines: &[&str]) -> String {
    lines.iter().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("").chars().take(300).collect()
}

/// Does this shell command run tests? (The hook skips everything else without opening a store.)
pub fn is_test_command(command: &str) -> bool {
    let c = command.to_ascii_lowercase();
    ["cargo test", "cargo nextest", "pytest", "py.test", "jest", "vitest", "go test", "node --test", "npm test", "npm run test", "npm t ", "yarn test", "pnpm test", "pnpm run test", "bun test", "chisel run"]
        .iter()
        .any(|k| c.contains(k))
        || c.trim_end() == "npm t"
}

/// Does `command` name test targets (files, dirs, filters), or run the whole suite?
pub fn command_targets(command: &str) -> Vec<String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let Some(start) = tokens.iter().position(|t| {
        let t = t.rsplit(['/', '\\']).next().unwrap_or(t);
        matches!(t, "pytest" | "py.test" | "jest" | "vitest" | "test")
    }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut skip_value = false;
    for t in &tokens[start + 1..] {
        if skip_value {
            skip_value = false;
            continue;
        }
        if matches!(*t, "&&" | "||" | ";" | "|") {
            break;
        }
        if t.starts_with('-') {
            skip_value = matches!(*t, "-k" | "-m" | "-p" | "-c" | "-t" | "--testNamePattern" | "--config" | "--reporter" | "-n");
            if matches!(*t, "-k" | "-t" | "--testNamePattern") {
                out.push(format!("filter:{t}"));
            }
            continue;
        }
        if matches!(*t, "run" | "./..." | "--") {
            continue;
        }
        out.push(t.trim_matches(['"', '\'']).replace('\\', "/"));
    }
    out
}

pub fn parse(text: &str, project: Option<&Path>) -> Parsed {
    let text = strip_ansi(text);
    let mut parsed = Parsed::default();
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') && (text.contains("\"testResults\"") || text.contains("\"Action\"")) {
        if text.contains("\"testResults\"") {
            jest_json(&text, project, &mut parsed);
        }
        if text.contains("\"Action\"") {
            go_json(&text, &mut parsed);
        }
        return parsed;
    }
    let lines: Vec<&str> = text.lines().collect();
    if text.contains("test result: ") || lines.iter().any(|l| l.starts_with("test ") && (l.ends_with(" ... ok") || l.ends_with(" ... FAILED"))) {
        cargo(&lines, project, &mut parsed);
    }
    if text.contains("short test summary info") || lines.iter().any(|l| is_pytest_progress(l).is_some()) || lines.iter().any(|l| crate::run_cmd::parse_pytest_line(l).is_some_and(|(id, _)| id.contains(".py::"))) {
        pytest(&lines, project, &mut parsed);
    }
    if text.contains("Test Files ") || lines.iter().any(|l| l.trim_start().starts_with("FAIL  ") && l.contains(" > ")) {
        vitest(&lines, project, &mut parsed);
    } else if text.contains("Test Suites:") || lines.iter().any(|l| l.starts_with("FAIL ") || l.starts_with("PASS ")) {
        jest(&lines, project, &mut parsed);
    }
    if lines.iter().any(|l| l.trim_start().starts_with("--- FAIL: ") || l.trim_start().starts_with("--- PASS: ") || l.starts_with("ok  \t") || l.starts_with("FAIL\t")) {
        go(&lines, &mut parsed);
    }
    if text.contains("TAP version") || lines.iter().any(|l| l.starts_with("# tests ") || l.starts_with("ℹ tests ")) {
        node(&lines, project, &mut parsed);
    }
    parsed
}

fn cargo(lines: &[&str], project: Option<&Path>, out: &mut Parsed) {
    out.runners.push("cargo");
    let mut file: Option<String> = None;
    let mut by_name: Vec<(usize, String)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Running ") {
            let rest = rest.strip_prefix("unittests ").unwrap_or(rest);
            let path = rest.split(" (").next().unwrap_or(rest);
            file = (path.ends_with(".rs")).then(|| relative(path, project));
            continue;
        }
        if t.starts_with("test result: ") {
            out.complete = true;
            continue;
        }
        let Some(rest) = line.strip_prefix("test ") else { continue };
        let (name, status) = match rest.rsplit_once(" ... ") {
            Some((n, s)) => (n.trim(), s.trim()),
            None => continue,
        };
        let passed = match status {
            "ok" => true,
            "FAILED" => false,
            _ => continue,
        };
        let (tfile, tname) = match name.split_once(" - ") {
            Some((f, n)) if f.ends_with(".rs") => (Some(relative(f, project)), n.to_string()),
            _ => (file.clone(), name.trim_end_matches(" - should panic").to_string()),
        };
        by_name.push((out.outcomes.len(), name.to_string()));
        let _ = i;
        out.outcomes.push(Outcome { file: tfile, name: tname, passed, ..Outcome::default() });
    }
    // Failure details: "---- NAME stdout ----" up to the next "----" or "failures:".
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if let Some(name) = t.strip_prefix("---- ").and_then(|r| r.strip_suffix(" stdout ----")) {
            let start = i + 1;
            let mut end = start;
            while end < lines.len() && !lines[end].trim().starts_with("---- ") && lines[end].trim() != "failures:" {
                end += 1;
            }
            let block = &lines[start..end];
            let (mut message, mut loc) = (String::new(), String::new());
            for (k, l) in block.iter().enumerate() {
                if let Some(at) = l.find(" panicked at ") {
                    let rest = &l[at + " panicked at ".len()..];
                    if let Some(q) = rest.strip_prefix('\'') {
                        // older format: panicked at 'msg', file:line:col
                        if let Some((msg, where_)) = q.rsplit_once("', ") {
                            message = msg.to_string();
                            loc = location(where_, project);
                        }
                    } else {
                        loc = location(rest.trim_end_matches(':'), project);
                        message = first_text(&block[k + 1..]);
                    }
                    break;
                }
            }
            if let Some(&(idx, _)) = by_name.iter().rev().find(|(_, n)| n == name) {
                let o = &mut out.outcomes[idx];
                o.message = message;
                o.location = loc;
                o.excerpt = excerpt(&block.iter().copied().filter(|l| !l.starts_with("note: run with")).collect::<Vec<_>>());
            }
            i = end;
            continue;
        }
        i += 1;
    }
}

fn is_pytest_progress(line: &str) -> Option<(&str, &str)> {
    let t = line.trim_end();
    let t = match t.rfind('[') {
        Some(p) if t.ends_with("%]") => t[..p].trim_end(),
        _ => t,
    };
    let (path, marks) = t.rsplit_once(' ')?;
    let path = path.trim();
    if !path.ends_with(".py") || path.contains(' ') || marks.is_empty() || !marks.chars().all(|c| ".FEsxX".contains(c)) {
        return None;
    }
    Some((path, marks))
}

fn pytest(lines: &[&str], project: Option<&Path>, out: &mut Parsed) {
    out.runners.push("pytest");
    let mut seen: Vec<String> = Vec::new();
    let split_id = |id: &str| -> (Option<String>, String) {
        match id.split_once("::") {
            Some((f, n)) => (Some(relative(f, project)), n.to_string()),
            None => (None, id.to_string()),
        }
    };
    for line in lines {
        if let Some((path, marks)) = is_pytest_progress(line) {
            if !marks.contains('F') && !marks.contains('E') {
                out.passed_files.push(relative(path, project));
            }
            continue;
        }
        let t = line.trim();
        if (t.starts_with("= ") || t.starts_with("==")) && t.contains(" in ") && (t.contains("passed") || t.contains("failed") || t.contains("error")) {
            out.complete = true;
        }
        if !t.starts_with('=') && (t.contains(" passed") || t.contains(" failed")) && t.contains(" in ") && t.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            out.complete = true;
        }
        for (prefix, passed) in [("FAILED ", false), ("ERROR ", false)] {
            if let Some(rest) = t.strip_prefix(prefix) {
                let (id, msg) = rest.split_once(" - ").unwrap_or((rest, ""));
                if !id.contains(".py::") || seen.iter().any(|s| s == id) {
                    continue;
                }
                seen.push(id.to_string());
                let (file, name) = split_id(id);
                out.outcomes.push(Outcome { file, name, passed, message: msg.to_string(), ..Outcome::default() });
            }
        }
        if let Some((id, passed)) = crate::run_cmd::parse_pytest_line(t) {
            if id.contains(".py::") && !seen.iter().any(|s| s == &id) {
                seen.push(id.clone());
                let (file, name) = split_id(&id);
                out.outcomes.push(Outcome { file, name, passed, ..Outcome::default() });
            }
        }
    }
    // Failure sections: "____ test_name ____" blocks with "E   " lines and "path:line: ExcType".
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with("___") && t.ends_with("___") && t.trim_matches('_').trim().len() > 0 {
            let title = t.trim_matches('_').trim().to_string();
            let start = i + 1;
            let mut end = start;
            while end < lines.len() {
                let e = lines[end].trim();
                if (e.starts_with("___") && e.ends_with("___")) || (e.starts_with("===") && e.ends_with("===")) {
                    break;
                }
                end += 1;
            }
            let block = &lines[start..end];
            let e_line = block.iter().find_map(|l| l.trim_start().strip_prefix("E ").map(|s| s.trim().to_string()));
            let loc_line = block.iter().rev().find_map(|l| {
                let l = l.trim();
                let (path_line, exc) = l.rsplit_once(": ")?;
                let (path, line) = path_line.rsplit_once(':')?;
                (path.ends_with(".py") && line.chars().all(|c| c.is_ascii_digit())).then(|| (format!("{}:{line}", relative(path, project)), exc.to_string()))
            });
            let short = title.rsplit('.').next().unwrap_or(&title).to_string();
            if let Some(o) = out.outcomes.iter_mut().find(|o| !o.passed && (o.name == title || o.name == short || o.name.ends_with(&format!("::{short}")))) {
                if let Some((loc, exc)) = &loc_line {
                    o.location = loc.clone();
                    let e = e_line.clone().unwrap_or_default();
                    o.message = if e.starts_with(exc.as_str()) || e.is_empty() { if e.is_empty() { exc.clone() } else { e } } else { format!("{exc}: {e}") };
                } else if o.message.is_empty() {
                    o.message = e_line.clone().unwrap_or_default();
                }
                o.excerpt = excerpt(block);
            }
            i = end;
            continue;
        }
        i += 1;
    }
}

fn js_location(block: &[&str], project: Option<&Path>) -> String {
    for l in block {
        let t = l.trim();
        let inner = if let Some(rest) = t.strip_prefix("at ") {
            rest.rsplit_once('(').map(|(_, r)| r.trim_end_matches(')')).unwrap_or(rest)
        } else if let Some(rest) = t.strip_prefix("❯ ") {
            rest
        } else {
            continue;
        };
        if inner.contains("node_modules") || inner.starts_with("node:") {
            continue;
        }
        if inner.matches(':').count() >= 2 {
            return location(inner, project);
        }
    }
    String::new()
}

fn jest(lines: &[&str], project: Option<&Path>, out: &mut Parsed) {
    out.runners.push("jest");
    let mut file: Option<String> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if let Some(path) = t.strip_prefix("PASS ") {
            let path = path.split_whitespace().next().unwrap_or(path);
            out.passed_files.push(relative(path, project));
            file = Some(relative(path, project));
        } else if let Some(path) = t.strip_prefix("FAIL ") {
            file = Some(relative(path.split_whitespace().next().unwrap_or(path), project));
        } else if t.starts_with("Tests:") {
            out.complete = true;
        } else if let Some(rest) = t.strip_prefix("✓ ").or_else(|| t.strip_prefix("√ ")) {
            out.outcomes.push(Outcome { file: file.clone(), name: strip_duration(rest), passed: true, ..Outcome::default() });
        } else if let Some(rest) = t.strip_prefix("✕ ").or_else(|| t.strip_prefix("× ")) {
            let name = strip_duration(rest);
            if !out.outcomes.iter().any(|o| o.name == name && o.file == file) {
                out.outcomes.push(Outcome { file: file.clone(), name, passed: false, ..Outcome::default() });
            }
        } else if let Some(title) = t.strip_prefix("● ") {
            if title.starts_with("Test suite failed to run") {
                i += 1;
                continue;
            }
            let name = title.rsplit(" › ").next().unwrap_or(title).trim().to_string();
            let start = i + 1;
            let mut end = start;
            while end < lines.len() {
                let e = lines[end].trim();
                if e.starts_with("● ") || e.starts_with("FAIL ") || e.starts_with("PASS ") || e.starts_with("Test Suites:") {
                    break;
                }
                end += 1;
            }
            let block = &lines[start..end];
            let o = Outcome {
                file: file.clone(),
                name: name.clone(),
                passed: false,
                message: first_text(block),
                location: js_location(block, project),
                excerpt: excerpt(block),
                package: None,
            };
            match out.outcomes.iter_mut().find(|x| x.name == name && x.file == file) {
                Some(x) => *x = o,
                None => out.outcomes.push(o),
            }
            i = end;
            continue;
        }
        i += 1;
    }
}

fn strip_duration(s: &str) -> String {
    let s = s.trim();
    match s.rfind(" (") {
        Some(p) if s.ends_with(')') && s[p + 2..s.len() - 1].trim_end_matches("ms").trim().parse::<f64>().is_ok() => s[..p].to_string(),
        _ => s.to_string(),
    }
}

fn jest_json(text: &str, project: Option<&Path>, out: &mut Parsed) {
    out.runners.push("jest");
    let Ok(doc) = json::parse(text) else { return };
    out.complete = true;
    for suite in doc.get("testResults").and_then(|v| v.as_array()).unwrap_or(&[]) {
        let file = relative(suite.get("name").and_then(|v| v.as_str()).unwrap_or(""), project);
        for a in suite.get("assertionResults").and_then(|v| v.as_array()).unwrap_or(&[]) {
            let status = a.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if !matches!(status, "passed" | "failed") {
                continue;
            }
            let failure = a.get("failureMessages").and_then(|v| v.as_array()).and_then(|m| m.first()).and_then(|v| v.as_str()).map(strip_ansi).unwrap_or_default();
            let block: Vec<&str> = failure.lines().collect();
            out.outcomes.push(Outcome {
                file: Some(file.clone()),
                name: a.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                passed: status == "passed",
                message: first_text(&block),
                location: js_location(&block, project),
                excerpt: excerpt(&block),
                package: None,
            });
        }
    }
}

fn vitest(lines: &[&str], project: Option<&Path>, out: &mut Parsed) {
    out.runners.push("vitest");
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with("Test Files ") {
            out.complete = true;
        } else if let Some(rest) = t.strip_prefix("✓ ") {
            if let Some((path, _)) = rest.split_once(" (") {
                if path.contains('.') && !path.contains(' ') {
                    out.passed_files.push(relative(path, project));
                }
            }
        } else if let Some(rest) = t.strip_prefix("FAIL ") {
            let rest = rest.trim();
            let mut parts = rest.split(" > ");
            let path = parts.next().unwrap_or("").trim();
            let name = rest.rsplit(" > ").next().unwrap_or(rest).trim().to_string();
            let start = i + 1;
            let mut end = start;
            while end < lines.len() {
                let e = lines[end].trim();
                if e.starts_with("FAIL ") || e.starts_with("⎯") || e.starts_with("Test Files ") {
                    break;
                }
                end += 1;
            }
            let block = &lines[start..end];
            out.outcomes.push(Outcome {
                file: Some(relative(path, project)),
                name,
                passed: false,
                message: first_text(block),
                location: js_location(block, project),
                excerpt: excerpt(block),
                package: None,
            });
            i = end;
            continue;
        }
        i += 1;
    }
}

fn go(lines: &[&str], out: &mut Parsed) {
    out.runners.push("go");
    let mut pending: Vec<usize> = Vec::new();
    let mut run_buf: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if t.starts_with("=== RUN ") {
            run_buf.clear();
        } else if let Some(rest) = t.strip_prefix("--- FAIL: ").or_else(|| t.strip_prefix("--- PASS: ")) {
            let passed = t.starts_with("--- PASS");
            let name = rest.split(" (").next().unwrap_or(rest).trim().to_string();
            // Messages follow "--- FAIL" without -v, and precede it (inside === RUN) with -v.
            let mut after: Vec<&str> = Vec::new();
            let mut k = i + 1;
            while k < lines.len() && lines[k].starts_with("    ") && !lines[k].trim().starts_with("---") {
                after.push(lines[k]);
                k += 1;
            }
            let block: Vec<&str> = if after.is_empty() { run_buf.clone() } else { after };
            let msg_line = block.iter().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("");
            let (loc, message) = match msg_line.split_once(": ") {
                Some((loc, m)) if loc.contains(".go:") => (loc.to_string(), m.to_string()),
                _ => (String::new(), msg_line.to_string()),
            };
            pending.push(out.outcomes.len());
            out.outcomes.push(Outcome {
                file: None,
                name,
                passed,
                message: if passed { String::new() } else { message },
                location: if passed { String::new() } else { loc },
                excerpt: if passed { String::new() } else { excerpt(&block) },
                package: None,
            });
            run_buf.clear();
            i = k;
            continue;
        } else if let Some(rest) = t.strip_prefix("ok  \t").or_else(|| line.strip_prefix("ok  \t")).or_else(|| line.strip_prefix("ok \t")) {
            let pkg = rest.split('\t').next().unwrap_or(rest).trim().to_string();
            for &p in &pending {
                out.outcomes[p].package = Some(pkg.clone());
            }
            pending.clear();
            out.passed_packages.push(pkg);
            out.complete = true;
        } else if let Some(rest) = line.strip_prefix("FAIL\t") {
            let pkg = rest.split('\t').next().unwrap_or(rest).trim().to_string();
            for &p in &pending {
                out.outcomes[p].package = Some(pkg.clone());
            }
            pending.clear();
            out.complete = true;
        } else {
            run_buf.push(line);
        }
        i += 1;
    }
}

fn go_json(text: &str, out: &mut Parsed) {
    out.runners.push("go");
    let mut output: Vec<(String, String, String)> = Vec::new();
    for line in text.lines() {
        let Ok(ev) = json::parse(line.trim()) else { continue };
        let action = ev.get("Action").and_then(|v| v.as_str()).unwrap_or("");
        let pkg = ev.get("Package").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let test = ev.get("Test").and_then(|v| v.as_str()).map(String::from);
        match (action, test) {
            ("output", Some(t)) => output.push((pkg, t, ev.get("Output").and_then(|v| v.as_str()).unwrap_or("").to_string())),
            ("pass" | "fail", Some(t)) => {
                let passed = action == "pass";
                let lines: Vec<String> = output.iter().filter(|(p, n, _)| *p == pkg && *n == t).map(|(_, _, o)| o.trim_end().to_string()).collect();
                let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).filter(|l| !l.trim().starts_with("=== RUN") && !l.trim().starts_with("--- ")).collect();
                let msg_line = refs.iter().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("");
                let (loc, message) = match msg_line.split_once(": ") {
                    Some((loc, m)) if loc.contains(".go:") => (loc.to_string(), m.to_string()),
                    _ => (String::new(), msg_line.to_string()),
                };
                out.outcomes.push(Outcome {
                    file: None,
                    name: t,
                    passed,
                    message: if passed { String::new() } else { message },
                    location: if passed { String::new() } else { loc },
                    excerpt: if passed { String::new() } else { excerpt(&refs) },
                    package: Some(pkg),
                });
            }
            ("pass", None) => {
                out.passed_packages.push(pkg);
                out.complete = true;
            }
            ("fail", None) => out.complete = true,
            _ => {}
        }
    }
}

fn node(lines: &[&str], project: Option<&Path>, out: &mut Parsed) {
    out.runners.push("node");
    let mut i = 0;
    let mut in_failures = false;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if t.starts_with("# tests ") || t.starts_with("ℹ tests ") {
            out.complete = true;
        }
        // TAP
        let tap = t.strip_prefix("not ok ").map(|r| (false, r)).or_else(|| t.strip_prefix("ok ").map(|r| (true, r)));
        if let Some((passed, rest)) = tap {
            let name = rest.split_once(" - ").map(|(_, n)| n).unwrap_or(rest);
            let name = name.split(" # ").next().unwrap_or(name).trim().to_string();
            // YAML diagnostics follow, indented, between "---" and "...".
            let mut yaml: Vec<&str> = Vec::new();
            let mut k = i + 1;
            if k < lines.len() && lines[k].trim() == "---" {
                k += 1;
                while k < lines.len() && lines[k].trim() != "..." {
                    yaml.push(lines[k]);
                    k += 1;
                }
                k += 1;
            }
            let field = |key: &str| yaml.iter().position(|l| l.trim_start().starts_with(&format!("{key}:")));
            let value = |idx: usize, key: &str| -> String {
                let v = yaml[idx].trim_start()[key.len() + 1..].trim();
                if v == "|-" || v == "|" || v == ">-" {
                    yaml.get(idx + 1).map(|l| l.trim().to_string()).unwrap_or_default()
                } else {
                    v.trim_matches(['\'', '"']).to_string()
                }
            };
            let kind = field("type").map(|p| value(p, "type")).unwrap_or_default();
            if kind != "suite" {
                let loc = field("location").map(|p| value(p, "location")).unwrap_or_default();
                let err = field("error").map(|p| value(p, "error")).unwrap_or_default();
                let ename = field("name").map(|p| value(p, "name")).unwrap_or_default();
                let file = (!loc.is_empty()).then(|| relative(loc.rsplitn(3, ':').nth(2).unwrap_or(&loc), project));
                out.outcomes.push(Outcome {
                    file,
                    name,
                    passed,
                    message: if passed { String::new() } else if ename.is_empty() || err.starts_with(&ename) { err } else { format!("{ename}: {err}") },
                    location: if passed || loc.is_empty() { String::new() } else { location(&loc, project) },
                    excerpt: if passed { String::new() } else { excerpt(&yaml) },
                    package: None,
                });
            }
            i = k.max(i + 1);
            continue;
        }
        // spec reporter
        if t == "✖ failing tests:" {
            in_failures = true;
        } else if let Some(rest) = t.strip_prefix("✔ ") {
            if !in_failures {
                out.outcomes.push(Outcome { name: strip_duration(rest), passed: true, ..Outcome::default() });
            }
        } else if let Some(rest) = t.strip_prefix("✖ ") {
            let name = strip_duration(rest);
            if !in_failures {
                out.outcomes.push(Outcome { name, passed: false, ..Outcome::default() });
            } else {
                let loc = i.checked_sub(1).and_then(|p| lines[p].trim().strip_prefix("test at ")).map(|l| location(l, project)).unwrap_or_default();
                let start = i + 1;
                let mut end = start;
                while end < lines.len() && !lines[end].trim().starts_with("test at ") && !lines[end].trim().starts_with("✖ ") {
                    end += 1;
                }
                let block = &lines[start..end];
                if let Some(o) = out.outcomes.iter_mut().find(|o| !o.passed && o.name == name && o.message.is_empty()) {
                    o.message = first_text(block);
                    if let Some(f) = loc.rsplit_once(':').map(|(f, _)| f.to_string()) {
                        o.file = Some(f);
                    }
                    o.location = loc;
                    o.excerpt = excerpt(block);
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_and_paths() {
        assert_eq!(strip_ansi("\u{1b}[31mFAIL\u{1b}[39m x"), "FAIL x");
        let root = Path::new("C:\\work\\proj");
        assert_eq!(relative("C:\\work\\proj\\test\\a.js", Some(root)), "test/a.js");
        assert_eq!(location("C:\\\\work\\\\proj\\\\test\\\\a.js:5:1", Some(root)), "test/a.js:5");
        assert_eq!(location("src\\lib.rs:10:22", None), "src/lib.rs:10");
    }

    #[test]
    fn test_commands_and_targets() {
        assert!(is_test_command("cargo test --release") && is_test_command("python -m pytest -q") && is_test_command("npx vitest run"));
        assert!(!is_test_command("git status") && !is_test_command("ls tests"));
        assert!(command_targets("python -m pytest -q").is_empty());
        assert_eq!(command_targets("pytest tests/test_a.py -x"), vec!["tests/test_a.py"]);
        assert_eq!(command_targets("npx jest -t adds"), vec!["filter:-t"]);
        assert!(command_targets("npx vitest run").is_empty());
    }
}

//! What `diff_impact` hands back besides ranked rows: tests a project always runs for some
//! paths (AGENTS.md `always-run` rules, and tests that read source files themselves), a
//! ready-to-run command per test runner, and a grouped summary when the rows do not fit.
//!
//! Source-scanning guard tests (`read_dir` over `src/`, `include_str!` of a file) and calls
//! dispatched by a string name are invisible to a call graph. A project declares them in its
//! `agent-tools` block (`chisel: always-run crates/a/src/** -> crates/a/tests/guard.rs`); the
//! analyzer also records, per test, the files it reads by a path written in the test
//! (`reads`), and a change under one of them names the test. A test that reads the whole
//! project or its whole crate gets no `reads`: it would be named for every change.

use super::analyze::crate_dir;
use super::*;

/// Most test names one command filters by; past it the command runs the whole package.
const MAX_FILTER_NAMES: usize = 30;
/// The same for a group's command: a group is the part of the answer that did not fit.
const MAX_GROUP_FILTER_NAMES: usize = 10;
/// Most groups listed when rows are summarised.
const MAX_GROUPS: usize = 20;

/// `*` matches within one folder, `**` across folders, `?` one character. A pattern with no
/// `*` or `?` also matches everything under it as a folder (`crates/a/src`).
pub(crate) fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches("./").trim_end_matches('/');
    if !pattern.contains(['*', '?']) {
        return path == pattern || path.starts_with(&format!("{pattern}/"));
    }
    fn go(p: &[u8], s: &[u8]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(b'*') if p.get(1) == Some(&b'*') => match p.get(2) {
                // `**/x`: `x` here or after any folder.
                Some(b'/') => go(&p[3..], s) || (0..s.len()).any(|i| s[i] == b'/' && go(&p[3..], &s[i + 1..])),
                _ => (0..=s.len()).any(|i| go(&p[2..], &s[i..])),
            },
            Some(b'*') => (0..=s.len()).take_while(|i| *i == 0 || s[i - 1] != b'/').any(|i| go(&p[1..], &s[i..])),
            Some(b'?') => s.first().is_some_and(|c| *c != b'/') && go(&p[1..], &s[1..]),
            Some(c) => s.first() == Some(c) && go(&p[1..], &s[1..]),
        }
    }
    go(pattern.as_bytes(), path.as_bytes())
}

/// Folders and files a test reads by a path written in its text: `include_str!("../src/x.rs")`,
/// `Path::new(env!("CARGO_MANIFEST_DIR")).join("src")`, `concat!(env!("CARGO_MANIFEST_DIR"),
/// "/src/x.rs")`. Only when the text also reads files (`read_to_string`, `read_dir`,
/// `include_str!`, `fs::read`). Patterns are relative to the project (`crates/a/src/**`).
pub(crate) fn source_reads(text: &str, test_file: &str) -> Vec<String> {
    let reads_files = ["read_to_string", "read_dir", "include_str!", "include_bytes!", "fs::read("].iter().any(|m| text.contains(m));
    if !reads_files {
        return Vec::new();
    }
    let krate = crate_dir(test_file);
    let join = |base: &str, rel: &str| -> String {
        let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
        for seg in rel.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                s => parts.push(s),
            }
        }
        parts.join("/")
    };
    // The project root or the whole crate would name the test for any change: no scope.
    let as_pattern = |p: String| -> String {
        if p.is_empty() || p == krate {
            return String::new();
        }
        let leaf = p.rsplit('/').next().unwrap_or("");
        if leaf.contains('.') { p } else { format!("{p}/**") }
    };
    let mut out: Vec<String> = Vec::new();
    let mut push = |p: String| {
        if !p.is_empty() && !out.contains(&p) {
            out.push(p);
        }
    };
    // The string literal starting at or after `from`, and the index past it.
    let literal = |from: usize| -> Option<(String, usize)> {
        let open = text[from..].find('"')? + from;
        let close = text[open + 1..].find('"')? + open + 1;
        Some((text[open + 1..close].to_string(), close + 1))
    };
    for macro_name in ["include_str!(", "include_bytes!("] {
        let mut at = 0;
        while let Some(p) = text[at..].find(macro_name).map(|p| p + at) {
            at = p + macro_name.len();
            let rest = text[at..].trim_start();
            if !rest.starts_with('"') {
                continue;
            }
            if let Some((lit, _)) = literal(at) {
                let dir = test_file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                push(as_pattern(join(dir, &lit)));
            }
        }
    }
    // `.join("a").join("b")` literals right after `from`, up to the end of the statement.
    let joins = |from: usize| -> String {
        let end = text[from..].find([';', '{']).map(|e| e + from).unwrap_or(text.len());
        let mut rel = String::new();
        let mut k = from;
        while let Some(j) = text[k..end].find(".join(").map(|j| j + k) {
            k = j + ".join(".len();
            if !text[k..end].trim_start().starts_with('"') {
                break;
            }
            match literal(k) {
                Some((lit, after)) if after <= end => {
                    if !rel.is_empty() {
                        rel.push('/');
                    }
                    rel.push_str(lit.trim_matches('/'));
                    k = after;
                }
                _ => break,
            }
        }
        rel
    };
    let marker = "env!(\"CARGO_MANIFEST_DIR\")";
    let mut at = 0;
    while let Some(p) = text[at..].find(marker).map(|p| p + at) {
        at = p + marker.len();
        let rel = if text[..p].trim_end().ends_with("concat!(") {
            let end = text[at..].find([';', '{']).map(|e| e + at).unwrap_or(text.len());
            literal(at).filter(|(_, e)| *e <= end).map(|(lit, _)| lit.trim_start_matches('/').to_string()).unwrap_or_default()
        } else {
            joins(at)
        };
        let base = join(krate, &rel);
        // A path kept in a variable (`let root = ...;`) or returned by a helper
        // (`fn repo() -> PathBuf { ... }`) is read through its own joins later on.
        let stmt = text[..p].rfind([';', '{', '}']).map(|i| i + 1).unwrap_or(0);
        let head = text[stmt..p].trim_start();
        let name = if let Some(rest) = head.strip_prefix("let ") {
            let rest = rest.trim_start().strip_prefix("mut ").unwrap_or(rest.trim_start());
            Some(rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<String>())
        } else {
            text[..stmt].rfind("fn ").filter(|f| !text[*f..stmt].contains(';') && stmt - f < 200).map(|f| text[f + 3..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<String>() + "()")
        }
        .filter(|n| !n.is_empty() && n != "()");
        let mut later = Vec::new();
        // The kept path is also used as it is (`read_dir(&src)`): the folder itself counts.
        let mut whole = false;
        if let Some(name) = &name {
            let mut q = 0;
            let defined_at = text[..p].rfind(name.trim_end_matches("()")).unwrap_or(0);
            while let Some(j) = text[q..].find(name.as_str()).map(|j| j + q) {
                q = j + name.len();
                let before = text[..j].chars().next_back();
                let after = text[q..].chars().next();
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') || after.is_some_and(|c| c.is_alphanumeric() || c == '_') || j <= defined_at {
                    continue;
                }
                if text[q..].starts_with(".join(") {
                    let rel = joins(q);
                    if !rel.is_empty() {
                        later.push(join(&base, &rel));
                        continue;
                    }
                }
                whole = true;
            }
        }
        // Used as it is and through joins: the folder and the joined paths.
        if whole && !later.is_empty() {
            later.insert(0, base.clone());
        }
        if !later.is_empty() {
            for l in later {
                push(as_pattern(l));
            }
        } else {
            // The project root or the test's own crate, read whole, would name the test for
            // every change: no scope (an AGENTS.md always-run rule says it when it is meant).
            push(as_pattern(base));
        }
    }
    out
}

impl Engine {
    /// Tests named first because a project rule or the test's own source reads cover a
    /// changed file. Each row says which rule or path brought it in.
    pub(super) fn always_run_rows(&self, changed: &[String]) -> Vec<Value> {
        let agents = fs::read_to_string(self.project_dir.join("AGENTS.md")).map(|t| crate::tool_groups::parse_agents_md(&t)).unwrap_or_default();
        let tests = self.store.rows("test_units");
        let mut out: Vec<Value> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut add = |t: &Value, source: &str, reason: String, out: &mut Vec<Value>| {
            let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if id.is_empty() || !seen.insert(id.clone()) {
                return;
            }
            out.push(Value::object(vec![
                ("test_id".into(), Value::String(id)),
                ("file_path".into(), t.get("file_path").cloned().unwrap_or(Value::String(String::new()))),
                ("name".into(), t.get("name").cloned().unwrap_or(Value::String(String::new()))),
                ("reason".into(), Value::String(reason)),
                ("score".into(), Value::Float(1.0)),
                ("source".into(), Value::String(source.into())),
            ]));
        };
        for rule in &agents.always_run {
            let Some(hit) = changed.iter().find(|f| rule.paths.iter().any(|p| glob_match(p, f))) else { continue };
            for wanted in &rule.tests {
                for t in tests.iter() {
                    let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let file = t.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
                    let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    if id == wanted || file == wanted || name == wanted || glob_match(wanted, file) && wanted.contains('*') {
                        add(t, "always_run", format!("AGENTS.md always-run {} ({hit} changed)", rule.line), &mut out);
                    }
                }
            }
        }
        for t in tests.iter() {
            let Some(reads) = t.get("reads").and_then(|v| v.as_array()) else { continue };
            let own = t.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            for r in reads.iter().filter_map(|v| v.as_str()) {
                if let Some(hit) = changed.iter().find(|f| f.as_str() != own && glob_match(r, f)) {
                    add(t, "reads_source", format!("reads {r} ({hit} changed)"), &mut out);
                    break;
                }
            }
        }
        out
    }

    /// Ready-to-run commands for these rows: per Cargo package `cargo test -p <pkg> -- <names>`,
    /// for pytest the node ids. A package with more than `max_names` names runs whole.
    pub(super) fn run_commands(&self, rows: &[Value], crates: &mut crate::cargo::Crates, max_names: usize) -> Vec<String> {
        let mut cargo: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut pytest: Vec<String> = Vec::new();
        for r in rows {
            let file = r.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if file.is_empty() || name.is_empty() {
                continue;
            }
            if file.ends_with(".rs") {
                let pkg = crates.package_of(file).unwrap_or_default();
                let names = cargo.entry(pkg).or_default();
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
            } else if file.ends_with(".py") {
                let id = format!("{file}::{}", name.replace('.', "::"));
                if !pytest.contains(&id) {
                    pytest.push(id);
                }
            }
        }
        let mut out = Vec::new();
        for (pkg, names) in cargo {
            let p = if pkg.is_empty() { String::new() } else { format!(" -p {pkg}") };
            if names.len() > max_names {
                out.push(format!("cargo test{p}"));
            } else {
                out.push(format!("cargo test{p} -- {}", names.join(" ")));
            }
        }
        if !pytest.is_empty() {
            if pytest.len() > max_names {
                let mut files: Vec<&str> = pytest.iter().filter_map(|i| i.split("::").next()).collect();
                files.dedup();
                out.push(format!("pytest {}", files.join(" ")));
            } else {
                out.push(format!("pytest {}", pytest.join(" ")));
            }
        }
        out
    }

    /// The `diff_impact` reply: rows that fit within `limit` and the commands to run them.
    /// Past the limit the reply stops before the tie that crosses it (rows of equal tier and
    /// score cannot be told apart, so listing some of them would be arbitrary) and summarises
    /// the rest per Cargo package (or per test folder): how many tests, the best score, and a
    /// command for each group.
    pub(super) fn shape_diff_impact(&self, result: Value) -> Value {
        let Value::Array(rows) = result else { return result };
        if rows.is_empty() {
            return Value::Array(rows);
        }
        let mut crates = crate::cargo::Crates::new(&self.project_dir);
        let limit = self.diff_limit.max(1);
        let total = rows.len();
        let key = |r: &Value| (query::row_tier(r), (r.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0) * 1000.0).round() as i64);
        let mut cut = total.min(limit);
        let mut tied = 0;
        if total > limit {
            let boundary = key(&rows[limit - 1]);
            if key(&rows[limit]) == boundary {
                cut = rows.iter().position(|r| key(r) == boundary).unwrap_or(limit);
                tied = rows.iter().filter(|r| key(r) == boundary).count();
            }
        }
        let mut items = rows;
        let rest = items.split_off(cut);
        let mut pairs: Vec<(crate::json::SharedStr, Value)> = Vec::new();
        let run = self.run_commands(&items, &mut crates, MAX_FILTER_NAMES);
        pairs.push(("items".into(), Value::Array(items)));
        if !rest.is_empty() {
            pairs.push(("truncated".into(), Value::Bool(true)));
            pairs.push(("total".into(), Value::Int(total as i64)));
            if tied > 0 {
                pairs.push(("tied".into(), Value::Int(tied as i64)));
            }
            let groups = self.group_rows(&rest, &mut crates);
            let shown = groups.len().min(MAX_GROUPS);
            if groups.len() > shown {
                pairs.push(("groups_total".into(), Value::Int(groups.len() as i64)));
            }
            pairs.push(("groups".into(), Value::Array(groups.into_iter().take(shown).collect())));
        }
        if !run.is_empty() {
            pairs.push(("run".into(), Value::Array(run.into_iter().map(Value::String).collect())));
        }
        Value::Object(pairs)
    }

    /// Rows not listed, per Cargo package (Rust) or test folder: `{group, tests, top, run}`,
    /// best first.
    fn group_rows(&self, rows: &[Value], crates: &mut crate::cargo::Crates) -> Vec<Value> {
        let mut groups: BTreeMap<String, (Vec<&Value>, f64)> = BTreeMap::new();
        for r in rows {
            let file = r.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let label = if file.ends_with(".rs") {
                crates.package_of(file).unwrap_or_else(|| crate_dir(file).to_string())
            } else {
                file.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default()
            };
            let score = r.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let slot = groups.entry(label).or_insert((Vec::new(), 0.0));
            slot.0.push(r);
            slot.1 = slot.1.max(score);
        }
        let mut list: Vec<(String, Vec<&Value>, f64)> = groups.into_iter().map(|(k, (v, s))| (k, v, s)).collect();
        list.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal).then(b.1.len().cmp(&a.1.len())).then(a.0.cmp(&b.0)));
        list.into_iter()
            .map(|(label, members, top)| {
                let owned: Vec<Value> = members.iter().map(|v| (*v).clone()).collect();
                let run = self.run_commands(&owned, crates, MAX_GROUP_FILTER_NAMES);
                let mut row = vec![
                    ("group".into(), Value::String(if label.is_empty() { ".".into() } else { label })),
                    ("tests".into(), Value::Int(members.len() as i64)),
                    ("top".into(), Value::Float((top * 1000.0).round() / 1000.0)),
                ];
                if let Some(cmd) = run.into_iter().next() {
                    row.push(("run".into(), Value::String(cmd)));
                }
                Value::object(row)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_folders_and_files() {
        assert!(glob_match("crates/a/src/**", "crates/a/src/x.rs"));
        assert!(glob_match("crates/a/src/**", "crates/a/src/deep/x.rs"));
        assert!(glob_match("crates/*/src/*.rs", "crates/b/src/x.rs"));
        assert!(!glob_match("crates/*/src/*.rs", "crates/b/src/deep/x.rs"));
        assert!(glob_match("**/*.rs", "x.rs") && glob_match("**/*.rs", "a/b/x.rs"));
        assert!(glob_match("crates/a/src", "crates/a/src/x.rs") && !glob_match("crates/a/src", "crates/a/srcs/x.rs"));
        assert!(glob_match("src/?.rs", "src/a.rs") && !glob_match("src/?.rs", "src/ab.rs"));
    }

    #[test]
    fn source_reads_from_paths_written_in_tests() {
        let guard = "#[test]\nfn no_prints() {\n    let src = Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"src\");\n    for e in std::fs::read_dir(src).unwrap() {}\n}";
        assert_eq!(source_reads(guard, "crates/a/tests/print_guard.rs"), vec!["crates/a/src/**"]);
        let inc = "const T: &str = include_str!(\"../src/table.rs\");";
        assert_eq!(source_reads(inc, "crates/a/tests/t.rs"), vec!["crates/a/src/table.rs"]);
        let cat = "let t = std::fs::read_to_string(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/src/lib.rs\")).unwrap();";
        assert_eq!(source_reads(cat, "tests/x.rs"), vec!["src/lib.rs"]);
        assert!(source_reads("let p = env!(\"CARGO_MANIFEST_DIR\"); let x = 1;", "tests/x.rs").is_empty(), "nothing is read");
        assert!(source_reads("fn t() { std::fs::read_to_string(p).unwrap(); let d = env!(\"CARGO_MANIFEST_DIR\"); }", "tests/x.rs").is_empty(), "the whole crate is no scope");
        let root = "fn t() { let r = Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"../..\"); std::fs::read_dir(r).unwrap(); }";
        assert!(source_reads(root, "crates/a/tests/x.rs").is_empty(), "the whole project is no scope; a rule says it");
        let sub = "fn t() {
    let root = Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"../..\");
    walk(&root.join(\"crates\"));
    walk(&root.join(\"games\").join(\"data\"));
    std::fs::read_to_string(x);
}";
        assert_eq!(source_reads(sub, "crates/a/tests/x.rs"), vec!["crates/**", "games/data/**"]);
        let helper = "fn repo() -> PathBuf {
    Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"../..\")
}
fn t() { for e in std::fs::read_dir(repo().join(\"scripts\")).unwrap() {} }";
        assert_eq!(source_reads(helper, "crates/a/tests/x.rs"), vec!["scripts/**"]);
        let both = "fn t() {
    let src = Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"src\");
    let lib = std::fs::read_to_string(src.join(\"lib.rs\"));
    for e in std::fs::read_dir(&src) {}
}";
        assert_eq!(source_reads(both, "crates/a/tests/x.rs"), vec!["crates/a/src/**", "crates/a/src/lib.rs"], "the folder is read whole as well");
        let rooted = "fn t() {
    let root = Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"../..\");
    walk(&root.join(\"crates\"));
    let rel = p.strip_prefix(&root);
    std::fs::read_to_string(x);
}";
        assert_eq!(source_reads(rooted, "crates/a/tests/x.rs"), vec!["crates/**"], "the root itself is never a scope");
    }
}

#[cfg(test)]
mod classify_tests {
    use super::super::looks_like_test;

    #[test]
    fn crate_tests_folders_are_tests_and_src_test_modules_are_code() {
        assert!(looks_like_test("tests/it.rs"));
        assert!(looks_like_test("crates/specula/tests/disk_guard.rs"));
        assert!(looks_like_test("crates/a/tests/common/mod.rs"));
        assert!(!looks_like_test("crates/a/src/tests.rs"));
        assert!(!looks_like_test("crates/a/src/trace/tests/mod.rs"));
        assert!(!looks_like_test("src/tests/helpers.rs"));
    }
}

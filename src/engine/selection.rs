//! What `diff_impact` hands back besides ranked rows: tests a project always runs for some
//! paths (AGENTS.md `always-run` rules, and tests that read source files themselves), a
//! ready-to-run command per test runner, and a grouped summary when the rows do not fit.
//!
//! Source-scanning guard tests (`read_dir` over `src/`, `include_str!` of a file) and calls
//! dispatched by a string name are invisible to a call graph. A project declares them in its
//! `agent-tools` block (`chisel: always-run crates/a/src/** -> crates/a/tests/guard.rs`); the
//! analyzer also records, per test, the files it reads by a path written in the test
//! (`reads`), and a change under one of them names the test. One rule bounds what is detected:
//! a scope that is the project root, or a folder holding two or more Cargo packages
//! (`crates/**`), is dropped; a test reading that wide needs an always-run rule. A crate's own
//! folders (`crates/a/src/**`, `crates/a/**`) are kept. The reply folds these tests per
//! test file in `guards`, beside the ranked rows.

use super::analyze::crate_dir;
use super::*;

/// `tests/test_a.py` -> `tests.test_a`, the name `python -m unittest` takes.
fn unittest_module(file: &str) -> String {
    file.strip_suffix(".py").unwrap_or(file).trim_start_matches("./").replace(['/', '\\'], ".")
}

/// Whether the project runs its Python tests with pytest: a pytest config (`pytest.ini`,
/// `conftest.py`, `[tool.pytest...]` in `pyproject.toml`, `[tool:pytest]` in `setup.cfg`,
/// `[pytest]` in `tox.ini`), a test file that imports pytest, or pytest importable by the
/// project's Python (`.venv`/`venv`, else `python3`). Otherwise the commands use unittest.
/// The import check runs once per project per process.
pub(crate) fn uses_pytest(project: &Path, test_files: &[&str]) -> bool {
    let read = |name: &str| fs::read_to_string(project.join(name)).unwrap_or_default();
    if project.join("pytest.ini").is_file() || project.join("conftest.py").is_file() {
        return true;
    }
    if read("pyproject.toml").lines().any(|l| l.trim_start().starts_with("[tool.pytest")) {
        return true;
    }
    if read("setup.cfg").lines().any(|l| l.trim() == "[tool:pytest]") || read("tox.ini").lines().any(|l| l.trim() == "[pytest]") {
        return true;
    }
    // A conftest.py beside the tests counts too (pytest finds it there).
    if test_files.iter().any(|f| Path::new(f).parent().is_some_and(|d| !d.as_os_str().is_empty() && project.join(d).join("conftest.py").is_file())) {
        return true;
    }
    if test_files.iter().any(|f| fs::read_to_string(project.join(f)).unwrap_or_default().lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("import pytest") || t.starts_with("from pytest") || t.starts_with("from _pytest")
    })) {
        return true;
    }
    pytest_importable(project)
}

fn pytest_importable(project: &Path) -> bool {
    static SEEN: std::sync::Mutex<Option<HashMap<PathBuf, bool>>> = std::sync::Mutex::new(None);
    if let Some(known) = SEEN.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(project).copied())) {
        return known;
    }
    if std::env::var("CHISEL_PYTEST_PROBE").ok().as_deref() == Some("0") {
        return false;
    }
    let bin = if cfg!(windows) { "Scripts/python.exe" } else { "bin/python" };
    let mut pythons: Vec<PathBuf> = [".venv", "venv"].iter().map(|v| project.join(v).join(bin)).filter(|p| p.is_file()).collect();
    pythons.push(PathBuf::from(if cfg!(windows) { "python" } else { "python3" }));
    let found = pythons.iter().any(|py| {
        std::process::Command::new(py)
            .args(["-c", "import pytest"])
            .current_dir(project)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    });
    if let Ok(mut guard) = SEEN.lock() {
        guard.get_or_insert_with(HashMap::new).insert(project.to_path_buf(), found);
    }
    found
}

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
    // The project root is never a scope; `detected_scopes` drops folders of several packages.
    let as_pattern = |p: String| -> String {
        if p.is_empty() {
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
            push(as_pattern(base));
        }
    }
    out
}

/// Detected read scopes a test keeps (`reads`): `source_reads`, less any that is the project
/// root or a folder holding two or more Cargo packages (`crates/**`, `games/**`). A crate's own
/// folders and files stay (`crates/a/src/**`). A test reading wider than one package is named
/// only by an AGENTS.md always-run rule.
pub(crate) fn detected_scopes(text: &str, test_file: &str, package_dirs: &BTreeSet<String>) -> Vec<String> {
    source_reads(text, test_file).into_iter().filter(|p| !spans_packages(p, package_dirs)).collect()
}

/// Whether a scope is the project root or a folder with two or more packages in it.
pub(crate) fn spans_packages(pattern: &str, package_dirs: &BTreeSet<String>) -> bool {
    let Some(dir) = pattern.strip_suffix("**").map(|d| d.trim_end_matches('/')) else { return false };
    if dir.is_empty() {
        return true;
    }
    package_dirs.iter().filter(|d| d.as_str() == dir || d.starts_with(&format!("{dir}/"))).take(2).count() >= 2
}

/// Most guard entries listed; the rest are counted in `guards_total`.
const MAX_GUARDS: usize = 10;

impl Engine {
    /// Tests a project rule or the test's own read paths name for these changed files, one row
    /// per test. `scope` (the rule or the read path) and `changed` (the file that matched) say
    /// why; `diff_impact` folds the rows per test file into `guards`.
    pub(super) fn always_run_rows(&self, changed: &[String]) -> Vec<Value> {
        let agents = fs::read_to_string(self.project_dir.join("AGENTS.md")).map(|t| crate::tool_groups::parse_agents_md(&t)).unwrap_or_default();
        let tests = self.store.rows("test_units");
        let mut out: Vec<Value> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut add = |t: &Value, source: &str, scope: &str, hit: &str, out: &mut Vec<Value>| {
            let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if id.is_empty() || !seen.insert(id.clone()) {
                return;
            }
            out.push(Value::object(vec![
                ("test_id".into(), Value::String(id)),
                ("file_path".into(), t.get("file_path").cloned().unwrap_or(Value::String(String::new()))),
                ("name".into(), t.get("name").cloned().unwrap_or(Value::String(String::new()))),
                ("source".into(), Value::String(source.into())),
                ("scope".into(), Value::String(scope.into())),
                ("changed".into(), Value::String(hit.into())),
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
                        add(t, "always_run", &rule.line, hit, &mut out);
                    }
                }
            }
        }
        for t in tests.iter() {
            let Some(reads) = t.get("reads").and_then(|v| v.as_array()) else { continue };
            let own = t.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            for r in reads.iter().filter_map(|v| v.as_str()) {
                if let Some(hit) = changed.iter().find(|f| f.as_str() != own && glob_match(r, f)) {
                    add(t, "reads_source", r, hit, &mut out);
                    break;
                }
            }
        }
        out
    }

    /// The Cargo target a test file is built in: `--test <name>` for `tests/<name>.rs` or
    /// `tests/<name>/main.rs`, `--lib` for a library's `src/`, `--bins` for a binary-only
    /// crate's `src/`. `None` when Chisel cannot tell (then no target is named).
    fn cargo_target(&self, file: &str, crates: &mut crate::cargo::Crates) -> Option<String> {
        let krate = crates.crate_of(file)?;
        let rel = if krate.is_empty() { file } else { file.strip_prefix(&format!("{krate}/"))? };
        if let Some(rest) = rel.strip_prefix("tests/") {
            return match rest.split_once('/') {
                None => rest.strip_suffix(".rs").map(|n| format!("--test {n}")),
                Some((dir, "main.rs")) => Some(format!("--test {dir}")),
                Some(_) => None,
            };
        }
        if rel.starts_with("src/") {
            let root = if krate.is_empty() { self.project_dir.clone() } else { self.project_dir.join(&krate) };
            if root.join("src/lib.rs").is_file() {
                return Some("--lib".into());
            }
            if root.join("src/main.rs").is_file() {
                return Some("--bins".into());
            }
        }
        None
    }

    /// Ready-to-run commands for these rows, and notes for any that could not be narrowed.
    /// Cargo: per package, the test targets the rows live in (`--lib`, `--test <name>`) and
    /// their names as filters: `cargo test -p <pkg> --lib --test it -- a b`. Past `max_names`
    /// names the filter is dropped (the targets still narrow it) and a note says so. pytest:
    /// the node ids.
    pub(super) fn run_commands(&self, rows: &[Value], crates: &mut crate::cargo::Crates, max_names: usize) -> (Vec<String>, Vec<String>) {
        // package -> (targets, every row had a known target, names)
        let mut cargo: BTreeMap<String, (BTreeSet<String>, bool, Vec<String>)> = BTreeMap::new();
        let mut pytest: Vec<String> = Vec::new();
        for r in rows {
            let file = r.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if file.is_empty() || name.is_empty() {
                continue;
            }
            if file.ends_with(".rs") {
                let pkg = crates.package_of(file).unwrap_or_default();
                let target = self.cargo_target(file, crates);
                let slot = cargo.entry(pkg).or_insert((BTreeSet::new(), true, Vec::new()));
                match target {
                    Some(t) => {
                        slot.0.insert(t);
                    }
                    None => slot.1 = false,
                }
                if !slot.2.iter().any(|n| n == name) {
                    slot.2.push(name.to_string());
                }
            } else if file.ends_with(".py") {
                let id = format!("{file}::{}", name.replace('.', "::"));
                if !pytest.contains(&id) {
                    pytest.push(id);
                }
            }
        }
        let (mut out, mut notes) = (Vec::new(), Vec::new());
        for (pkg, (targets, known, names)) in cargo {
            let mut cmd = "cargo test".to_string();
            if !pkg.is_empty() {
                cmd.push_str(&format!(" -p {pkg}"));
            }
            if known {
                for t in &targets {
                    cmd.push(' ');
                    cmd.push_str(t);
                }
            }
            if names.len() > max_names {
                let label = if pkg.is_empty() { "the package".to_string() } else { pkg.clone() };
                notes.push(format!("{label}: {} tests, too many to name; the command runs {}", names.len(), if known { "their targets whole" } else { "the package whole" }));
            } else {
                cmd.push_str(" -- ");
                cmd.push_str(&names.join(" "));
            }
            out.push(cmd);
        }
        if !pytest.is_empty() {
            let mut files: Vec<&str> = pytest.iter().filter_map(|i| i.split("::").next()).collect();
            files.dedup();
            // A module-level `def test_x()` runs only under pytest; unittest finds TestCase methods.
            let bare = pytest.iter().any(|id| id.split_once("::").is_some_and(|(_, rest)| !rest.contains("::") && rest.starts_with("test")));
            if bare || uses_pytest(&self.project_dir, &files) {
                if pytest.len() > max_names {
                    notes.push(format!("pytest: {} tests, too many to name; the command runs their files", pytest.len()));
                    out.push(format!("pytest {}", files.join(" ")));
                } else {
                    out.push(format!("pytest {}", pytest.join(" ")));
                }
            } else {
                // unittest names a test by its dotted module path: `tests.test_a.TestX.test_y`.
                let python = if cfg!(windows) { "python" } else { "python3" };
                let ids: Vec<String> = if pytest.len() > max_names {
                    notes.push(format!("unittest: {} tests, too many to name; the command runs their modules", pytest.len()));
                    files.iter().map(|f| unittest_module(f)).collect()
                } else {
                    pytest.iter().map(|id| match id.split_once("::") {
                        Some((file, rest)) => format!("{}.{}", unittest_module(file), rest.replace("::", ".")),
                        None => unittest_module(id),
                    }).collect()
                };
                out.push(format!("{python} -m unittest {}", ids.join(" ")));
            }
        }
        (out, notes)
    }

    /// Guard rows folded per test file: `{file, tests, source, rule | reads, changed}` (`changed`
    /// when the diff has more than one file), rules
    /// first, at most `MAX_GUARDS` (and the total). The commands: a Cargo test file all of
    /// whose tests are named runs as `--test <name>`, one command per package; a file of which
    /// only some are named (a rule naming tests, an inline test in `src/`) runs them by name.
    fn guard_section(&self, rows: &[Value], crates: &mut crate::cargo::Crates) -> (Vec<Value>, usize, Vec<String>) {
        let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        // (file, source, scope) -> (changed, tests)
        let mut by: BTreeMap<(String, String, String), (String, usize)> = BTreeMap::new();
        for r in rows {
            let slot = by.entry((s(r, "file_path"), s(r, "source"), s(r, "scope"))).or_insert((s(r, "changed"), 0));
            slot.1 += 1;
        }
        let mut per_file: BTreeMap<String, usize> = BTreeMap::new();
        for t in self.store.rows("test_units") {
            *per_file.entry(s(t, "file_path")).or_insert(0) += 1;
        }
        let mut entries: Vec<(bool, Value)> = Vec::new();
        for ((file, source, scope), (hit, n)) in &by {
            let rule = source == "always_run";
            let mut entry = vec![
                ("file".into(), Value::String(file.clone())),
                ("tests".into(), Value::Int(*n as i64)),
                ("source".into(), Value::String(source.clone())),
                ((if rule { "rule" } else { "reads" }).into(), Value::String(scope.clone())),
            ];
            // With one changed file, that file is why; name it only when there is a choice.
            if self.diff_paths != 1 {
                entry.push(("changed".into(), Value::String(hit.clone())));
            }
            entries.push((rule, Value::object(entry)));
        }
        entries.sort_by_key(|a| std::cmp::Reverse(a.0));
        let total = entries.len();
        let list: Vec<Value> = entries.into_iter().take(MAX_GUARDS).map(|(_, v)| v).collect();
        let mut whole: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut partial: Vec<Value> = Vec::new();
        let files: BTreeSet<String> = rows.iter().map(|r| s(r, "file_path")).collect();
        for file in files {
            let named: Vec<&Value> = rows.iter().filter(|r| s(r, "file_path") == file).collect();
            let target = if file.ends_with(".rs") { self.cargo_target(&file, crates) } else { None };
            match target.filter(|t| t.starts_with("--test ") && per_file.get(&file).copied().unwrap_or(0) == named.len()) {
                Some(t) => {
                    whole.entry(crates.package_of(&file).unwrap_or_default()).or_default().insert(t);
                }
                None => partial.extend(named.into_iter().cloned()),
            }
        }
        let mut run = Vec::new();
        for (pkg, targets) in whole {
            let p = if pkg.is_empty() { String::new() } else { format!(" -p {pkg}") };
            run.push(format!("cargo test{p} {}", targets.into_iter().collect::<Vec<_>>().join(" ")));
        }
        run.extend(self.run_commands(&partial, crates, MAX_FILTER_NAMES).0);
        (list, total, run)
    }

    /// The `diff_impact` reply: rows that fit within `limit`, the guard tests folded per file
    /// (`guards`), and the commands to run both. Past the limit the reply stops before the tie
    /// that crosses it (rows of equal tier and score cannot be told apart, so listing some of
    /// them would be arbitrary) and summarises the rest per Cargo package (or per test folder):
    /// how many tests, the best score, and a command for each group.
    pub(super) fn shape_diff_impact(&mut self, result: Value) -> Value {
        let guard_rows = std::mem::take(&mut self.guard_rows);
        let Value::Array(rows) = result else { return result };
        if rows.is_empty() && guard_rows.is_empty() {
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
        let (mut run, notes) = self.run_commands(&items, &mut crates, MAX_FILTER_NAMES);
        pairs.push(("items".into(), Value::Array(items)));
        if !guard_rows.is_empty() {
            let (guards, guards_total, guard_run) = self.guard_section(&guard_rows, &mut crates);
            if guards_total > guards.len() {
                pairs.push(("guards_total".into(), Value::Int(guards_total as i64)));
            }
            pairs.push(("guards".into(), Value::Array(guards)));
            run.extend(guard_run);
        }
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
        if !notes.is_empty() {
            pairs.push(("run_note".into(), Value::Array(notes.into_iter().map(Value::String).collect())));
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
                let (run, _) = self.run_commands(&owned, crates, MAX_GROUP_FILTER_NAMES);
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
    fn a_scope_over_two_packages_or_the_root_is_too_wide() {
        let pkgs: BTreeSet<String> = ["crates/a", "crates/b", "games/one"].iter().map(|s| s.to_string()).collect();
        assert!(spans_packages("**", &pkgs) && spans_packages("crates/**", &pkgs));
        assert!(!spans_packages("crates/a/src/**", &pkgs) && !spans_packages("crates/a/**", &pkgs) && !spans_packages("games/**", &pkgs));
        assert!(!spans_packages("crates/a/src/lib.rs", &pkgs) && !spans_packages("docs/**", &pkgs));
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
        assert!(source_reads("fn t() { std::fs::read_to_string(p).unwrap(); let d = env!(\"CARGO_MANIFEST_DIR\"); }", "tests/x.rs").is_empty(), "the root crate is the project root: no scope");
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
        assert!(looks_like_test("crates/app/tests/disk_guard.rs"));
        assert!(looks_like_test("crates/a/tests/common/mod.rs"));
        assert!(!looks_like_test("crates/a/src/tests.rs"));
        assert!(!looks_like_test("crates/a/src/trace/tests/mod.rs"));
        assert!(!looks_like_test("src/tests/helpers.rs"));
    }
}

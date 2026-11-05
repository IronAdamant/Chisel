//! Project-scoped source scanners. Python block structure plus brace languages.
//! No regex crate. Output matches `CodeUnit` ids `{path}:{name}:{unit_type}`.

mod brace;
mod deps;
pub mod calls;
mod framework;
mod python;

pub use deps::{extract_deps, extract_deps_in, Dep, is_test_name, matches_import_path, matches_relative_import, proximity_weight};
pub use framework::{annotated_names, content_has_cpp_test, cpp_macro_name, framework_for};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeUnit {
    pub file_path: String,
    pub name: String,
    pub unit_type: String,
    pub line_start: usize,
    pub line_end: usize,
}

pub fn detect_language(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "py" | "pyw" => "python",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "go" => "go",
        "rs" => "rust",
        "cs" => "csharp",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hxx" => "cpp",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "php" => "php",
        "rb" => "ruby",
        "dart" => "dart",
        _ => return None,
    })
}

pub fn path_has_code_extension(path: &str) -> bool {
    detect_language(path).is_some()
}

pub fn extract_code_units(file_path: &str, content: &str) -> Vec<CodeUnit> {
    extract_code_units_in(file_path, content, None, false)
}

/// `project`, when set, is where `.chisel/extensions.toml` is read.
/// `CHISEL_EXTRACTOR` still wins over that file.
/// The file is ignored unless `allow_repo_extractor` is true.
pub fn extract_code_units_in(
    file_path: &str,
    content: &str,
    project: Option<&std::path::Path>,
    allow_repo_extractor: bool,
) -> Vec<CodeUnit> {
    if let Some(units) = external_units(file_path, project, allow_repo_extractor) {
        return units;
    }
    builtin_units(file_path, content)
}

fn builtin_units(file_path: &str, content: &str) -> Vec<CodeUnit> {
    let Some(lang) = detect_language(file_path) else {
        return Vec::new();
    };
    if lang == "python" {
        python::extract_python(file_path, content)
    } else if lang == "ruby" {
        brace::extract_ruby(file_path, content)
    } else {
        brace::extract_brace(file_path, content, lang)
    }
}

thread_local! {
    static EXTRACTOR_WARNINGS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub fn clear_extractor_warnings() {
    EXTRACTOR_WARNINGS.with(|w| w.borrow_mut().clear());
    SKIP_NOTED.with(|n| n.set(false));
}

pub fn take_extractor_warnings() -> Vec<String> {
    EXTRACTOR_WARNINGS.with(|w| std::mem::take(&mut *w.borrow_mut()))
}

fn note_extractor_warning(message: String) {
    EXTRACTOR_WARNINGS.with(|w| w.borrow_mut().push(message));
}

thread_local! {
    static SKIP_NOTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn note_once(message: &str) {
    SKIP_NOTED.with(|n| {
        if !n.get() {
            n.set(true);
            note_extractor_warning(message.to_string());
        }
    });
}

/// `CHISEL_EXTRACTOR` reads `{"path","language"}` on stdin and writes a JSON unit list.
/// A missing variable reads `extractor` from `<project>/.chisel/extensions.toml`.
/// A missing file keeps the built-in scanner. A bad run records a warning and falls back.
fn external_units(file_path: &str, project: Option<&std::path::Path>, allow_repo_extractor: bool) -> Option<Vec<CodeUnit>> {
    let bin = configured_extractor(project, allow_repo_extractor)?;
    let lang = detect_language(file_path).unwrap_or("");
    match run_extractor(&bin, file_path, lang) {
        Ok(units) => Some(units),
        Err(err) => {
            note_extractor_warning(format!("CHISEL_EXTRACTOR failed for {file_path}: {err}"));
            None
        }
    }
}

fn configured_extractor(project: Option<&std::path::Path>, allow_repo_extractor: bool) -> Option<String> {
    if let Ok(bin) = std::env::var("CHISEL_EXTRACTOR") {
        let bin = bin.trim();
        if !bin.is_empty() {
            return Some(bin.to_string());
        }
    }
    if !allow_repo_extractor {
        if let Some(project) = project {
            if let Ok(text) = std::fs::read_to_string(project.join(".chisel").join("extensions.toml")) {
                if extractor_from_toml(&text).is_some() {
                    note_once("repo extractor skipped; pass --allow-repo-extractor");
                }
            }
        }
        return None;
    }
    let project = project?;
    let text = std::fs::read_to_string(project.join(".chisel").join("extensions.toml")).ok()?;
    let Some(raw) = extractor_from_toml(&text) else {
        note_once("extensions.toml has no extractor");
        return None;
    };
    let path = std::path::PathBuf::from(&raw);
    if path.is_absolute() || raw.split(['/', '\\']).any(|part| part == "..") {
        note_once("repo extractor path must stay inside the project");
        return None;
    }
    let resolved = project.join(path);
    let Ok(root) = std::fs::canonicalize(project) else {
        note_once("repo extractor path must stay inside the project");
        return None;
    };
    let Ok(canon) = std::fs::canonicalize(&resolved) else {
        note_once("repo extractor path must stay inside the project");
        return None;
    };
    if !canon.starts_with(&root) {
        note_once("repo extractor path must stay inside the project");
        return None;
    }
    Some(canon.to_string_lossy().into_owned())
}

pub(crate) fn toml_value(text: &str, want: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != want {
            continue;
        }
        let value = rest.trim().trim_matches('"').trim_matches('\'');
        if value.is_empty() {
            return None;
        }
        return Some(value.to_string());
    }
    None
}

pub(crate) fn extractor_from_toml(text: &str) -> Option<String> {
    toml_value(text, "extractor")
}

pub(crate) fn dep_extractor_from_toml(text: &str) -> Option<String> {
    toml_value(text, "dep_extractor")
}

/// Custom dependency extractor. Replaces `register_dep_extractor` and `CHISEL_BOOTSTRAP`.
/// Stdin is `{"path","language","content"}`. Stdout is a JSON list of `{name,dep_type,module_path,confidence}`.
pub(crate) fn custom_deps(path: &str, content: &str, project: Option<&std::path::Path>, allow_repo: bool) -> Option<Vec<deps::Dep>> {
    let bin = configured_dep_extractor(project, allow_repo)?;
    let lang = detect_language(path).unwrap_or("");
    match run_dep_extractor(&bin, path, lang, content) {
        Ok(rows) => Some(rows),
        Err(err) => {
            note_extractor_warning(format!("CHISEL_DEP_EXTRACTOR failed for {path}: {err}"));
            None
        }
    }
}

fn configured_dep_extractor(project: Option<&std::path::Path>, allow_repo: bool) -> Option<String> {
    if let Ok(bin) = std::env::var("CHISEL_DEP_EXTRACTOR") {
        let bin = bin.trim();
        if !bin.is_empty() {
            return Some(bin.to_string());
        }
    }
    if let Ok(boot) = std::env::var("CHISEL_BOOTSTRAP") {
        let boot = boot.trim();
        if !boot.is_empty() {
            let path = std::path::Path::new(boot);
            if path.exists() {
                return Some(boot.to_string());
            }
            note_once("CHISEL_BOOTSTRAP is a Python import path. Set CHISEL_DEP_EXTRACTOR to an executable instead.");
        }
    }
    if !allow_repo {
        return None;
    }
    let project = project?;
    let text = std::fs::read_to_string(project.join(".chisel").join("extensions.toml")).ok()?;
    let raw = dep_extractor_from_toml(&text)?;
    let path = std::path::PathBuf::from(&raw);
    if path.is_absolute() || raw.split(['/', '\\']).any(|part| part == "..") {
        note_once("repo dep extractor path must stay inside the project");
        return None;
    }
    let resolved = project.join(path);
    let root = std::fs::canonicalize(project).ok()?;
    let canon = std::fs::canonicalize(&resolved).ok()?;
    if !canon.starts_with(&root) {
        note_once("repo dep extractor path must stay inside the project");
        return None;
    }
    Some(canon.to_string_lossy().into_owned())
}

fn run_dep_extractor(bin: &str, file_path: &str, lang: &str, content: &str) -> Result<Vec<deps::Dep>, String> {
    let payload = crate::json::Value::object(vec![
        ("path".into(), crate::json::Value::String(file_path.into())),
        ("language".into(), crate::json::Value::String(lang.into())),
        ("content".into(), crate::json::Value::String(content.into())),
    ]);
    let body = crate::json::to_string(&payload).map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new(bin)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(body.as_bytes()).map_err(|e| format!("stdin: {e}"))?;
    }
    let output = child.wait_with_output().map_err(|e| format!("wait: {e}"))?;
    if !output.status.success() {
        return Err(format!("exit {}", output.status.code().unwrap_or(-1)));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let parsed = crate::json::parse(text.trim()).map_err(|e| format!("bad JSON: {e}"))?;
    let rows = parsed.as_array().ok_or_else(|| "bad JSON: expected a dependency array".to_string())?;
    let mut deps_out = Vec::new();
    for row in rows {
        let name = row.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let dep_type = row.get("dep_type").and_then(|v| v.as_str()).unwrap_or("import").to_string();
        let module_path = row.get("module_path").and_then(|v| v.as_str()).map(|s| s.to_string());
        let confidence = row.get("confidence").and_then(|v| v.as_f64()).unwrap_or(1.0);
        deps_out.push(deps::Dep { name, dep_type, module_path, confidence });
    }
    Ok(deps_out)
}

fn run_extractor(bin: &str, file_path: &str, lang: &str) -> Result<Vec<CodeUnit>, String> {
    let payload = crate::json::Value::object(vec![
        ("path".into(), crate::json::Value::String(file_path.into())),
        ("language".into(), crate::json::Value::String(lang.into())),
    ]);
    let body = crate::json::to_string(&payload).map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new(bin)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(body.as_bytes()).map_err(|e| format!("stdin: {e}"))?;
    }
    let output = child.wait_with_output().map_err(|e| format!("wait: {e}"))?;
    if !output.status.success() {
        return Err(format!("exit {}", output.status.code().unwrap_or(-1)));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let parsed = crate::json::parse(text.trim()).map_err(|e| format!("bad JSON: {e}"))?;
    let rows = parsed.as_array().ok_or_else(|| "bad JSON: expected a unit array".to_string())?;
    let mut units = Vec::new();
    for row in rows {
        let name = row.get("name").and_then(|v| v.as_str()).ok_or_else(|| "bad JSON: unit.name".to_string())?;
        let unit_type = row.get("unit_type").and_then(|v| v.as_str()).ok_or_else(|| "bad JSON: unit.unit_type".to_string())?;
        let line_start = row.get("line_start").and_then(|v| v.as_i64()).ok_or_else(|| "bad JSON: unit.line_start".to_string())? as usize;
        let line_end = row.get("line_end").and_then(|v| v.as_i64()).ok_or_else(|| "bad JSON: unit.line_end".to_string())? as usize;
        units.push(CodeUnit {
            file_path: file_path.to_string(),
            name: name.to_string(),
            unit_type: unit_type.to_string(),
            line_start,
            line_end,
        });
    }
    Ok(units)
}

pub fn unit_id(rel: &str, unit: &CodeUnit) -> String {
    format!("{rel}:{}:{}", unit.name, unit.unit_type)
}


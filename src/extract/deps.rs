use super::*;

#[derive(Clone, Debug)]
pub struct Dep {
    pub name: String,
    pub dep_type: String,
    pub module_path: Option<String>,
    pub confidence: f64,
}

pub fn extract_deps(path: &str, content: &str) -> Vec<Dep> {
    extract_deps_in(path, content, None, false)
}

/// `project` lets `.chisel/extensions.toml` name a dependency extractor.
/// `CHISEL_DEP_EXTRACTOR` still wins, and does not need `allow_repo`.
pub fn extract_deps_in(path: &str, content: &str, project: Option<&std::path::Path>, allow_repo: bool) -> Vec<Dep> {
    if let Some(custom) = super::custom_deps(path, content, project, allow_repo) {
        return custom;
    }
    match detect_language(path) {
        Some("python") => python_deps(content),
        Some("javascript" | "typescript") => js_deps(content),
        Some("go") => go_deps(content),
        Some("java" | "kotlin") => import_lines(content, &["import "], &["."], false),
        Some("csharp") => import_lines(content, &["using "], &["."], false),
        Some("c" | "cpp") => include_lines(content),
        Some("ruby") => ruby_requires(content),
        Some("php") => import_lines(content, &["use "], &["\\"], false),
        Some("swift") => import_lines(content, &["import "], &["."], false),
        Some("dart") => import_lines(content, &["import "], &["/"], false),
        _ => name_calls(content),
    }
}

fn import_lines(content: &str, prefixes: &[&str], seps: &[&str], with_calls: bool) -> Vec<Dep> {
    let mut deps = Vec::new();
    for line in content.lines() {
        let t = line.trim().trim_end_matches(';');
        let mut rest = None;
        for prefix in prefixes {
            if let Some(r) = t.strip_prefix(prefix) {
                rest = Some(r.trim());
                break;
            }
        }
        let Some(rest) = rest else { continue };
        let rest = rest.trim_start_matches("static ");
        let path = rest.split_whitespace().next().unwrap_or("").trim_matches(|c| c == '"' || c == '\'' || c == ';');
        if path.is_empty() || path == "*" {
            continue;
        }
        let mut name = path;
        for sep in seps {
            if let Some(tail) = path.rsplit_once(sep) {
                name = tail.1;
                break;
            }
        }
        if name.is_empty() || name == "*" {
            continue;
        }
        deps.push(Dep {
            name: name.to_string(),
            dep_type: "import".into(),
            module_path: Some(path.to_string()),
            confidence: 1.0,
        });
    }
    if with_calls {
        deps.extend(name_calls(content));
    }
    dedupe(deps)
}

fn include_lines(content: &str) -> Vec<Dep> {
    let mut deps = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("#include") else { continue };
        let rest = rest.trim();
        let path = rest.trim_matches(|c| c == '<' || c == '>' || c == '"' || c == ' ');
        if path.is_empty() {
            continue;
        }
        let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let name = base.split('.').next().unwrap_or(base);
        deps.push(Dep {
            name: name.to_string(),
            dep_type: "import".into(),
            module_path: Some(path.to_string()),
            confidence: 1.0,
        });
    }
    dedupe(deps)
}

fn ruby_requires(content: &str) -> Vec<Dep> {
    let mut deps = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        let rest = t.strip_prefix("require_relative ").or_else(|| t.strip_prefix("require "));
        let Some(rest) = rest else { continue };
        let Some(path) = quoted_token(rest) else { continue };
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path);
        deps.push(Dep {
            name: name.to_string(),
            dep_type: "import".into(),
            module_path: Some(path),
            confidence: 1.0,
        });
    }
    dedupe(deps)
}

fn python_deps(content: &str) -> Vec<Dep> {
    let mut deps = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("from ") {
            if let Some((module, names)) = rest.split_once(" import ") {
                for name in names.split(',') {
                    let name = name.split_whitespace().next().unwrap_or("").trim();
                    let name = name.split_once(" as ").map(|(n, _)| n).unwrap_or(name);
                    if name.is_empty() || name == "*" {
                        continue;
                    }
                    deps.push(Dep {
                        name: name.to_string(),
                        dep_type: "import".into(),
                        module_path: Some(module.trim().to_string()),
                        confidence: 1.0,
                    });
                }
            }
        } else if let Some(rest) = t.strip_prefix("import ") {
            for part in rest.split(',') {
                let name = part.split_whitespace().next().unwrap_or("").trim();
                if name.is_empty() {
                    continue;
                }
                let base = name.split('.').next_back().unwrap_or(name);
                deps.push(Dep {
                    name: base.to_string(),
                    dep_type: "import".into(),
                    module_path: Some(name.to_string()),
                    confidence: 1.0,
                });
            }
        }
    }
    deps.extend(name_calls(content).into_iter().filter(|d| d.dep_type == "call"));
    dedupe(deps)
}

const PY_KEYWORDS: &[&str] = &[
    "if", "for", "while", "with", "return", "print", "class", "def", "import", "from", "raise",
    "assert", "del", "yield", "lambda", "elif", "except", "async", "await", "not", "and", "or",
    "in", "is", "pass", "break", "continue", "try", "finally", "global", "nonlocal",
];

fn is_definition(content: &str, start: usize) -> bool {
    let before = content[..start].trim_end();
    before.ends_with("def")
        || before.ends_with("class")
        || before.ends_with("fn")
        || before.ends_with("func")
        || before.ends_with("function")
        || before.ends_with("fun")
        || before.ends_with(')')
}

fn name_calls(content: &str) -> Vec<Dep> {
    let mut deps = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let name = &content[start..i];
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'(' && !PY_KEYWORDS.contains(&name) && !is_definition(content, start) {
                deps.push(Dep {
                    name: name.to_string(),
                    dep_type: "call".into(),
                    module_path: None,
                    confidence: 1.0,
                });
            }
        } else {
            i += 1;
        }
    }
    dedupe(deps)
}

pub(super) fn quoted_token(s: &str) -> Option<String> {
    let t = s.trim_start();
    let q = t.chars().next()?;
    if q != '\'' && q != '"' && q != '`' {
        return None;
    }
    let rest = &t[q.len_utf8()..];
    let end = rest.find(q)?;
    Some(rest[..end].to_string())
}

fn import_names(clause: &str) -> Vec<String> {
    let clause = clause.trim().trim_end_matches(';');
    let mut names = Vec::new();
    if let Some(start) = clause.find('{') {
        if let Some(end) = clause[start..].find('}') {
            for part in clause[start + 1..start + end].split(',') {
                let part = part.trim();
                if part.is_empty() { continue; }
                let name = part.split_whitespace().next().unwrap_or(part);
                let name = name.split(" as ").next().unwrap_or(name).trim();
                if !name.is_empty() && name != "*" {
                    names.push(name.to_string());
                }
            }
        }
    }
    let head = clause.split('{').next().unwrap_or("").trim().trim_end_matches(',');
    if !head.is_empty() && head != "*" && !head.starts_with("type ") {
        if let Some(name) = head.split_whitespace().next() {
            if name != "type" && name != "{" {
                names.push(name.to_string());
            }
        }
    }
    names
}

fn js_deps(content: &str) -> Vec<Dep> {
    let mut deps = name_calls(content);
    for line in content.lines() {
        let t = line.trim().trim_start_matches("export ");
        let Some(rest) = t.strip_prefix("import ") else { continue };
        let Some((clause, after)) = rest.split_once(" from ") else { continue };
        let Some(path) = quoted_token(after) else { continue };
        let names = import_names(clause);
        let names = if names.is_empty() {
            vec![path.rsplit(['/', '.']).find(|s| !s.is_empty() && *s != "ts" && *s != "js" && *s != "tsx" && *s != "jsx").unwrap_or(&path).to_string()]
        } else {
            names
        };
        for name in names {
            deps.push(Dep {
                name,
                dep_type: "import".into(),
                module_path: Some(path.clone()),
                confidence: 1.0,
            });
        }
    }
    push_js_requires(content, &mut deps);
    dedupe(deps)
}

fn push_js_requires(content: &str, deps: &mut Vec<Dep>) {
    let taint = js_assign_paths(content);
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if ident_start(bytes[i]) && (i == 0 || !ident_cont(bytes[i - 1])) {
            let start = i;
            i += 1;
            while i < bytes.len() && ident_cont(bytes[i]) {
                i += 1;
            }
            let word = &content[start..i];
            if word == "require" {
                let mut j = i;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'(' {
                    if let Some(dep) = require_arg(&content[j + 1..], &taint) {
                        if dep.dep_type == "import" {
                            if let Some(bound) = cjs_binding(&content[..start], dep.module_path.as_deref().unwrap_or("")) {
                                for name in bound {
                                    deps.push(Dep {
                                        name,
                                        dep_type: "import".into(),
                                        module_path: dep.module_path.clone(),
                                        confidence: 1.0,
                                    });
                                }
                            }
                        }
                        deps.push(dep);
                    }
                }
            } else if word == "Function" {
                let before = content[..start].trim_end();
                if before.ends_with("new") {
                    deps.push(Dep {
                        name: "new Function(...)".into(),
                        dep_type: "eval_import".into(),
                        module_path: None,
                        confidence: 0.0,
                    });
                }
            } else if word == "import" {
                let mut j = i;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'(' {
                    if let Some(path) = quoted_token(content[j + 1..].trim_start()) {
                        if !path.contains("${") {
                            deps.push(Dep {
                                name: format!("import({path})"),
                                dep_type: "dynamic_import".into(),
                                module_path: Some(path),
                                confidence: 1.0,
                            });
                        }
                    }
                }
            }
        } else {
            i += 1;
        }
    }
}

fn ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

fn ident_cont(b: u8) -> bool {
    ident_start(b) || b.is_ascii_digit()
}

fn js_assign_paths(content: &str) -> std::collections::BTreeMap<String, String> {
    let mut declared = std::collections::BTreeMap::new();
    let mut simple = std::collections::BTreeMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("const ").or_else(|| trimmed.strip_prefix("let ")).or_else(|| trimmed.strip_prefix("var ")) {
            if let Some((name, path)) = assign_string(rest) {
                declared.insert(name, path);
                continue;
            }
        }
        if let Some((name, path)) = assign_string(trimmed) {
            simple.entry(name).or_insert(path);
        }
    }
    for (name, path) in simple {
        declared.entry(name).or_insert(path);
    }
    declared
}

fn assign_string(text: &str) -> Option<(String, String)> {
    let (name, rest) = split_ident(text.trim())?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=')?;
    if rest.starts_with('=') {
        return None;
    }
    let path = quoted_token(rest.trim_start())?;
    Some((name, path))
}

fn split_ident(text: &str) -> Option<(String, &str)> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || !ident_start(bytes[0]) {
        return None;
    }
    let mut i = 1;
    while i < bytes.len() && ident_cont(bytes[i]) {
        i += 1;
    }
    Some((text[..i].to_string(), &text[i..]))
}

fn require_arg(after_paren: &str, taint: &std::collections::BTreeMap<String, String>) -> Option<Dep> {
    let s = after_paren.trim_start();
    if let Some(dep) = path_join_require(s) {
        return Some(dep);
    }
    if let Some(dep) = conditional_require(s) {
        return Some(dep);
    }
    if s.starts_with('\'') || s.starts_with('"') {
        let path = quoted_token(s)?;
        let rest = after_quoted(s)?.trim_start();
        if rest.starts_with('+') {
            return Some(Dep {
                name: format!("require('{path}' + ...)"),
                dep_type: "dynamic_import".into(),
                module_path: Some(path),
                confidence: 0.2,
            });
        }
        return Some(Dep {
            name: module_stem(&path),
            dep_type: "import".into(),
            module_path: Some(path),
            confidence: 1.0,
        });
    }
    if s.starts_with('`') {
        let path = quoted_token(s)?;
        return Some(Dep {
            name: format!("require(`{path}`)"),
            dep_type: "dynamic_import".into(),
            module_path: Some(path),
            confidence: 0.4,
        });
    }
    let (name, _) = split_ident(s)?;
    if matches!(name.as_str(), "if" | "for" | "while" | "switch" | "function" | "return" | "new") {
        return None;
    }
    if let Some(path) = taint.get(&name) {
        return Some(Dep {
            name: format!("require({name})"),
            dep_type: "tainted_import".into(),
            module_path: Some(path.clone()),
            confidence: 1.0,
        });
    }
    Some(Dep {
        name: format!("require({name})"),
        dep_type: "dynamic_import".into(),
        module_path: Some(name),
        confidence: 0.3,
    })
}

fn module_stem(path: &str) -> String {
    path.rsplit(['/', '.']).find(|s| !s.is_empty() && !matches!(*s, "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs")).unwrap_or(path).to_string()
}

fn after_quoted(s: &str) -> Option<&str> {
    let s = s.trim_start();
    let q = s.chars().next()?;
    let rest = &s[q.len_utf8()..];
    let end = rest.find(q)?;
    Some(&rest[end + q.len_utf8()..])
}

fn conditional_require(s: &str) -> Option<Dep> {
    if s.starts_with(['\'', '"', '`']) {
        return None;
    }
    let qmark = s.find('?')?;
    let before = s[..qmark].trim();
    if before.is_empty() || before.split_whitespace().count() != 1 {
        return None;
    }
    let after = s[qmark + 1..].trim_start();
    let true_path = quoted_token(after)?;
    let rest = after_quoted(after)?.trim_start().strip_prefix(':')?.trim_start();
    let false_path = quoted_token(rest)?;
    Some(Dep {
        name: format!("require(cond ? '{true_path}' : '{false_path}')"),
        dep_type: "dynamic_import".into(),
        module_path: Some(format!("{true_path}|{false_path}")),
        confidence: 0.3,
    })
}

fn path_join_require(s: &str) -> Option<Dep> {
    let rest = s.strip_prefix("path.join")?.trim_start().strip_prefix('(')?.trim_start();
    if !(rest.starts_with("__dirname") || rest.starts_with("__filename")) {
        return None;
    }
    let after = rest.split_once(',')?.1.trim_start();
    let dir = quoted_token(after)?;
    Some(Dep {
        name: format!("require(path.join(__dirname, '{dir}', ...))"),
        dep_type: "dynamic_import".into(),
        module_path: Some(dir),
        confidence: 0.2,
    })
}

fn cjs_binding(before: &str, _module: &str) -> Option<Vec<String>> {
    let line = before.rsplit('\n').next().unwrap_or(before);
    let line = line.trim();
    let rest = line.strip_prefix("const ").or_else(|| line.strip_prefix("let ")).or_else(|| line.strip_prefix("var "))?;
    let rest = rest.trim_start();
    if let Some(inner) = rest.strip_prefix('{') {
        let inner = inner.split('}').next().unwrap_or("");
        let names: Vec<String> = inner.split(',').filter_map(|part| {
            let name = part.trim().split(':').next().unwrap_or("").trim();
            if name.is_empty() { None } else { Some(name.to_string()) }
        }).collect();
        if names.is_empty() { None } else { Some(names) }
    } else {
        let (name, _) = split_ident(rest)?;
        Some(vec![name])
    }
}

fn go_deps(content: &str) -> Vec<Dep> {
    let mut deps = Vec::new();
    let mut in_import = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with("import (") {
            in_import = true;
            continue;
        }
        if in_import {
            if t.starts_with(')') {
                in_import = false;
                continue;
            }
            if let Some(path) = t.split('"').nth(1) {
                let name = path.rsplit('/').next().unwrap_or(path);
                deps.push(Dep {
                    name: name.to_string(),
                    dep_type: "import".into(),
                    module_path: Some(path.to_string()),
                    confidence: 1.0,
                });
            }
            continue;
        }
        if let Some(rest) = t.strip_prefix("import ") {
            if let Some(path) = rest.split('"').nth(1) {
                let name = path.rsplit('/').next().unwrap_or(path);
                deps.push(Dep {
                    name: name.to_string(),
                    dep_type: "import".into(),
                    module_path: Some(path.to_string()),
                    confidence: 1.0,
                });
            }
        }
    }
    deps.extend(name_calls(content));
    dedupe(deps)
}

fn dedupe(deps: Vec<Dep>) -> Vec<Dep> {
    let mut out = Vec::new();
    for d in deps {
        if out.iter().any(|e: &Dep| e.name == d.name && e.dep_type == d.dep_type && e.module_path == d.module_path) {
            continue;
        }
        out.push(d);
    }
    out
}

pub fn matches_relative_import(from_file: &str, code_file: &str, module_path: &str) -> bool {
    if !module_path.starts_with('.') {
        return false;
    }
    let from = from_file.replace('\\', "/");
    let from_dir = from.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let mut parts: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in module_path.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            parts.pop();
            continue;
        }
        let seg = seg.split('.').next().unwrap_or(seg);
        parts.push(seg);
    }
    let joined = parts.join("/");
    let code = code_file.replace('\\', "/");
    let code_stem = code.rsplit('.').next().map(|ext| {
        let known = matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "py");
        if known { code.trim_end_matches(&format!(".{ext}")).to_string() } else { code.clone() }
    }).unwrap_or(code);
    code_stem == joined
}

pub fn matches_import_path(code_file: &str, module_path: &str) -> bool {
    if module_path.is_empty() {
        return false;
    }
    let module_as_path = module_path.replace('.', "/") + ".py";
    let normalized = code_file.replace('\\', "/");
    normalized == module_as_path || normalized.ends_with(&format!("/{module_as_path}"))
}

pub fn proximity_weight(test_path: &str, code_path: &str) -> f64 {
    let test_norm = test_path.replace('\\', "/");
    let code_norm = code_path.replace('\\', "/");
    let test_dir = test_norm.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let code_dir = code_norm.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let test_parts: Vec<&str> = test_dir.split('/').filter(|s| !s.is_empty()).collect();
    let code_parts: Vec<&str> = code_dir.split('/').filter(|s| !s.is_empty()).collect();
    let mut common = 0;
    for (t, c) in test_parts.iter().zip(code_parts.iter()) {
        if t == c {
            common += 1;
        } else {
            break;
        }
    }
    if test_parts.is_empty() && code_parts.is_empty() {
        return 1.0;
    }
    if common == test_parts.len() && common == code_parts.len() {
        return 1.0;
    }
    let max_depth = test_parts.len().max(code_parts.len()).max(1) as f64;
    let ratio = common as f64 / max_depth;
    if ratio >= 0.5 {
        0.8
    } else if common >= 1 {
        0.6
    } else {
        0.4
    }
}

pub fn is_test_name(name: &str, framework: &str) -> bool {
    let bare = name.rsplit('.').next().unwrap_or(name);
    match framework {
        "pytest" => bare.starts_with("test_") || bare.starts_with("Test"),
        "jest" | "playwright" => matches!(bare, "describe" | "it" | "test") || bare.starts_with("test"),
        "go" => bare.starts_with("Test") || bare.starts_with("Benchmark"),
        "rust" => bare.starts_with("test_"),
        "csharp_test" => bare.starts_with("Test") || bare.ends_with("Test") || bare.ends_with("Tests"),
        "gtest" => true,
        "junit" | "xctest" | "minitest" | "phpunit" | "dart_test" => bare.starts_with("test") || bare.starts_with("Test"),
        "rspec" => matches!(bare, "describe" | "it" | "context") || bare.starts_with("test"),
        _ => bare.to_ascii_lowercase().starts_with("test"),
    }
}

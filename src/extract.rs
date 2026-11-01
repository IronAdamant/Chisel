//! Project-scoped source scanners. Python block structure plus brace languages.
//! No regex crate. Output matches `CodeUnit` ids `{path}:{name}:{unit_type}`.

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
    let Some(lang) = detect_language(file_path) else {
        return Vec::new();
    };
    if lang == "python" {
        extract_python(file_path, content)
    } else if lang == "ruby" {
        extract_ruby(file_path, content)
    } else {
        extract_brace(file_path, content, lang)
    }
}

pub fn unit_id(rel: &str, unit: &CodeUnit) -> String {
    format!("{rel}:{}:{}", unit.name, unit.unit_type)
}

fn extract_python(file_path: &str, content: &str) -> Vec<CodeUnit> {
    let lines: Vec<&str> = content.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let mut units = Vec::new();
    let mut depth = 0i32;
    let mut class_stack: Vec<(String, usize)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (clean, next_depth) = strip_line_py(&lines[i], depth);
        let trimmed = clean.trim();
        if depth == 0 {
            class_stack.retain(|(_, ind)| *ind < indent_of(&lines[i]));
            if let Some(name) = class_name(trimmed) {
                let ind = indent_of(&lines[i]);
                let end = py_block_end(&lines, i, ind);
                units.push(CodeUnit {
                    file_path: file_path.to_string(),
                    name,
                    unit_type: "class".into(),
                    line_start: i + 1,
                    line_end: end,
                });
                class_stack.push((units.last().unwrap().name.clone(), ind));
            } else if let Some((name, is_async)) = def_name(trimmed) {
                let ind = indent_of(&lines[i]);
                let parent = class_stack.iter().rev().find(|(_, cind)| ind > *cind).map(|(n, _)| n.clone());
                let qname = match parent {
                    Some(p) => format!("{p}.{name}"),
                    None => name,
                };
                let end = py_block_end(&lines, i, ind);
                units.push(CodeUnit {
                    file_path: file_path.to_string(),
                    name: qname,
                    unit_type: if is_async { "async_function" } else { "function" }.into(),
                    line_start: i + 1,
                    line_end: end,
                });
            }
        }
        depth = next_depth;
        i += 1;
    }
    let _ = lines;
    units
}

fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

fn class_name(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix("class ")?;
    let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if name.is_empty() { None } else { Some(name) }
}

fn def_name(trimmed: &str) -> Option<(String, bool)> {
    let (rest, is_async) = if let Some(r) = trimmed.strip_prefix("async def ") {
        (r, true)
    } else if let Some(r) = trimmed.strip_prefix("def ") {
        (r, false)
    } else {
        return None;
    };
    let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if name.is_empty() { None } else { Some((name, is_async)) }
}

fn py_block_end(lines: &[&str], start: usize, ind: usize) -> usize {
    let mut end = start + 1;
    let mut i = start + 1;
    let mut depth = bracket_delta(lines[start]);
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            if depth == 0 {
                // blank lines inside a block still belong to it until dedent of real code
            }
            i += 1;
            continue;
        }
        if depth == 0 && indent_of(line) <= ind && !trimmed.starts_with('@') {
            break;
        }
        end = i + 1;
        depth += bracket_delta(line);
        if depth < 0 {
            depth = 0;
        }
        i += 1;
    }
    end
}

fn bracket_delta(line: &str) -> i32 {
    let mut d = 0i32;
    let mut chars = line.chars().peekable();
    let mut in_str: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = in_str {
            if c == '\\' {
                chars.next();
                continue;
            }
            if c == q {
                in_str = None;
            }
            continue;
        }
        if c == '#' {
            break;
        }
        if c == '"' || c == '\'' {
            in_str = Some(c);
            continue;
        }
        match c {
            '(' | '[' | '{' => d += 1,
            ')' | ']' | '}' => d -= 1,
            _ => {}
        }
    }
    d
}

fn strip_line_py(line: &str, depth_in: i32) -> (String, i32) {
    let mut out = String::new();
    let mut depth = depth_in;
    let mut chars = line.chars().peekable();
    let mut in_str: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = in_str {
            if c == '\\' {
                chars.next();
                continue;
            }
            if c == q {
                in_str = None;
            }
            continue;
        }
        if depth > 0 {
            match c {
                '"' | '\'' => in_str = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
            continue;
        }
        if c == '#' {
            break;
        }
        if c == '"' || c == '\'' {
            in_str = Some(c);
            continue;
        }
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                out.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    let _ = depth_in;
    (out, depth.max(0))
}

fn extract_ruby(file_path: &str, content: &str) -> Vec<CodeUnit> {
    let lines: Vec<&str> = content.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let mut units = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        let (kind, rest) = if let Some(r) = t.strip_prefix("class ") {
            ("class", r)
        } else if let Some(r) = t.strip_prefix("module ") {
            ("class", r)
        } else if let Some(r) = t.strip_prefix("def ") {
            ("function", r)
        } else {
            continue;
        };
        let name: String = rest
            .trim_start_matches("self.")
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '?' || *c == '!')
            .collect();
        if name.is_empty() {
            continue;
        }
        let end = ruby_end(&lines, i);
        units.push(CodeUnit {
            file_path: file_path.into(),
            name,
            unit_type: kind.into(),
            line_start: i + 1,
            line_end: end,
        });
    }
    units
}

fn ruby_end(lines: &[&str], start: usize) -> usize {
    let mut depth = 0i32;
    for (i, line) in lines.iter().enumerate().skip(start) {
        let t = line.trim();
        if t.starts_with("class ") || t.starts_with("module ") || t.starts_with("def ") || t.starts_with("do") || t == "do" {
            depth += 1;
        }
        if t == "end" || t.starts_with("end ") || t.starts_with("end#") {
            depth -= 1;
            if depth == 0 {
                return i + 1;
            }
        }
    }
    lines.len()
}

fn extract_brace(file_path: &str, content: &str, lang: &str) -> Vec<CodeUnit> {
    let raw: Vec<&str> = content.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let cleaned = strip_brace_comments(&raw);
    let mut units = Vec::new();
    for (i, line) in cleaned.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some((name, kind)) = decl_on_line(t, lang) {
            let end = brace_end(&cleaned, i);
            units.push(CodeUnit {
                file_path: file_path.into(),
                name,
                unit_type: kind,
                line_start: i + 1,
                line_end: end,
            });
        }
    }
    units
}

fn decl_on_line(t: &str, lang: &str) -> Option<(String, String)> {
    let t = t.trim_start_matches(|c: char| c == '@' || c.is_whitespace());
    // Skip pure attributes.
    if t.starts_with('@') || t.starts_with('[') {
        return None;
    }
    if let Some(name) = keyword_name(t, &["class ", "struct ", "interface ", "enum ", "trait ", "protocol ", "actor ", "object ", "mixin ", "extension "]) {
        let kind = if t.starts_with("struct ") {
            "struct"
        } else if t.starts_with("interface ") || t.starts_with("protocol ") {
            "interface"
        } else if t.starts_with("enum ") {
            "enum"
        } else {
            "class"
        };
        return Some((name, kind.into()));
    }
    if lang == "go" {
        if let Some(name) = go_func_name(t) {
            return Some((name, "function".into()));
        }
    }
    if lang == "rust" {
        if let Some(name) = rust_fn_name(t) {
            return Some((name, "function".into()));
        }
    }
    if matches!(lang, "swift" | "kotlin" | "dart") {
        if let Some(name) = after_keyword_fn(t, &["func ", "fun ", "function "]) {
            return Some((name, "function".into()));
        }
    }
    if matches!(lang, "javascript" | "typescript" | "php" | "kotlin" | "dart" | "swift" | "java" | "csharp" | "c" | "cpp") {
        if let Some(name) = after_keyword_fn(t, &["function ", "func ", "fun "]) {
            return Some((name, "function".into()));
        }
        if let Some(name) = method_name(t) {
            return Some((name, "function".into()));
        }
    }
    None
}

fn keyword_name(t: &str, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(rest) = t.strip_prefix(k) {
            return ident(rest);
        }
        // modifiers before the keyword: "public class Foo"
        if let Some(idx) = t.find(k) {
            let before = t[..idx].trim();
            if before.split_whitespace().all(is_modifier) && !before.is_empty() || before.is_empty() {
                if idx == 0 || t.as_bytes().get(idx - 1).is_some_and(|b| b.is_ascii_whitespace()) {
                    return ident(&t[idx + k.len()..]);
                }
            }
        }
    }
    None
}

fn is_modifier(w: &str) -> bool {
    matches!(
        w,
        "public" | "private" | "protected" | "internal" | "static" | "final"
            | "abstract" | "sealed" | "open" | "data" | "override" | "virtual"
            | "async" | "partial" | "readonly" | "extern" | "unsafe" | "export"
            | "default" | "synchronized" | "native" | "strictfp" | "inline"
    )
}

fn ident(rest: &str) -> Option<String> {
    let name: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() { None } else { Some(name) }
}

fn go_func_name(t: &str) -> Option<String> {
    let rest = t.strip_prefix("func ")?;
    let rest = rest.trim_start();
    if rest.starts_with('(') {
        let close = rest.find(')')?;
        ident(rest[close + 1..].trim_start())
    } else {
        ident(rest)
    }
}

fn rust_fn_name(t: &str) -> Option<String> {
    let mut s = t;
    for prefix in ["pub(crate) ", "pub(super) ", "pub ", "async ", "const ", "unsafe ", "extern "] {
        if let Some(r) = s.strip_prefix(prefix) {
            s = r.trim_start();
        }
    }
    // stacked prefixes
    loop {
        let next = ["pub ", "async ", "const ", "unsafe "]
            .iter()
            .find_map(|p| s.strip_prefix(p).map(|r| r.trim_start()));
        match next {
            Some(n) => s = n,
            None => break,
        }
    }
    let rest = s.strip_prefix("fn ")?;
    ident(rest)
}

fn after_keyword_fn(t: &str, keys: &[&str]) -> Option<String> {
    let mut s = t;
    loop {
        let next = s.split_whitespace().next()?;
        if is_modifier(next) {
            s = s[next.len()..].trim_start();
            continue;
        }
        break;
    }
    for k in keys {
        if let Some(rest) = s.strip_prefix(k) {
            return ident(rest);
        }
    }
    None
}

fn method_name(t: &str) -> Option<String> {
    // public void Name(   or   void Name(   or   String name(
    // Skip lines that are calls (no type-ish token before the name) by requiring '('
    // and a preceding word.
    if t.contains('=') && !t.contains('(') {
        return None;
    }
    let paren = t.find('(')?;
    let head = t[..paren].trim();
    if head.is_empty() || head.ends_with("if") || head.ends_with("for") || head.ends_with("while") || head.ends_with("switch") || head.ends_with("catch") {
        return None;
    }
    let mut parts: Vec<&str> = head.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }
    // drop modifiers
    while parts.len() > 1 && is_modifier(parts[0]) {
        parts.remove(0);
    }
    if parts.len() < 2 {
        return None;
    }
    let name = parts.last()?.trim_end_matches(')');
    let name = name.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
    if name.is_empty() || is_modifier(name) || matches!(name, "if" | "for" | "while" | "switch" | "return" | "new") {
        return None;
    }
    // The token before the name should look like a type, not an operator.
    Some(name.to_string())
}

fn brace_end(lines: &[String], start: usize) -> usize {
    let mut depth = 0i32;
    let mut seen = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        for c in line.chars() {
            if c == '{' {
                depth += 1;
                seen = true;
            } else if c == '}' {
                depth -= 1;
                if seen && depth == 0 {
                    return i + 1;
                }
            }
        }
    }
    start + 1
}

fn strip_brace_comments(lines: &[&str]) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    let mut block = false;
    for line in lines {
        let mut cleaned = String::new();
        let mut chars = line.chars().peekable();
        let mut in_str: Option<char> = None;
        while let Some(c) = chars.next() {
            if block {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    block = false;
                }
                continue;
            }
            if let Some(q) = in_str {
                cleaned.push(c);
                if c == '\\' {
                    if let Some(n) = chars.next() {
                        cleaned.push(n);
                    }
                    continue;
                }
                if c == q {
                    in_str = None;
                }
                continue;
            }
            if c == '"' || c == '\'' {
                in_str = Some(c);
                cleaned.push(c);
                continue;
            }
            if c == '/' && chars.peek() == Some(&'/') {
                break;
            }
            if c == '/' && chars.peek() == Some(&'*') {
                chars.next();
                block = true;
                continue;
            }
            cleaned.push(c);
        }
        out.push(cleaned);
    }
    out
}

#[derive(Clone, Debug)]
pub struct Dep {
    pub name: String,
    pub dep_type: String,
    pub module_path: Option<String>,
    pub confidence: f64,
}

pub fn extract_deps(path: &str, content: &str) -> Vec<Dep> {
    match detect_language(path) {
        Some("python") => python_deps(content),
        Some("javascript" | "typescript") => js_deps(content),
        Some("go") => go_deps(content),
        _ => name_calls(content),
    }
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

fn js_deps(content: &str) -> Vec<Dep> {
    let mut deps = name_calls(content);
    // require('path') and from 'path'
    let mut i = 0;
    let b = content.as_bytes();
    while i + 8 < b.len() {
        if content[i..].starts_with("require") {
            if let Some(path) = quoted_after(&content[i + "require".len()..]) {
                deps.push(Dep {
                    name: path.rsplit(['/', '.']).next().unwrap_or(&path).to_string(),
                    dep_type: if path.contains("${") || path.contains('+') { "dynamic_import".into() } else { "import".into() },
                    module_path: Some(path.clone()),
                    confidence: if path.contains("${") { 0.4 } else { 1.0 },
                });
            }
        }
        i += 1;
    }
    dedupe(deps)
}

fn quoted_after(s: &str) -> Option<String> {
    let t = s.trim_start();
    let t = t.strip_prefix('(')?.trim_start();
    let q = t.chars().next()?;
    if q != '\'' && q != '"' && q != '`' {
        return None;
    }
    let rest = &t[1..];
    let end = rest.find(q)?;
    Some(rest[..end].to_string())
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

pub fn is_test_file(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if name.starts_with("test_") && name.ends_with(".py") {
        return true;
    }
    if name.ends_with("_test.py") || name.ends_with("_test.go") {
        return true;
    }
    if name.ends_with("Test.java") || name.ends_with("Tests.cs") || name.ends_with("Tests.swift") {
        return true;
    }
    false
}

pub fn is_test_name(name: &str, framework: &str) -> bool {
    let bare = name.rsplit('.').next().unwrap_or(name);
    match framework {
        "pytest" => bare.starts_with("test_"),
        "go" => bare.starts_with("Test") || bare.starts_with("Benchmark"),
        _ => bare.to_ascii_lowercase().starts_with("test"),
    }
}

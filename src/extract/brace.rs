use super::*;
use super::deps::quoted_token;

pub(super) fn extract_ruby(file_path: &str, content: &str) -> Vec<CodeUnit> {
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

pub(super) fn extract_brace(file_path: &str, content: &str, lang: &str) -> Vec<CodeUnit> {
    let raw: Vec<&str> = content.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let cleaned = strip_brace_comments(&raw);
    let mut units = Vec::new();
    for (i, line) in cleaned.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some((name, mut kind)) = decl_on_line(t, lang) {
            if lang == "rust" && kind == "function" && rust_test_above(&cleaned, i) {
                kind = "test_case".into();
            }
            // `struct Unit;` and `struct Pair(u8, u8);` have no body: they end at the `;`.
            let end = if kind != "function" && kind != "test_case" && body_less(&cleaned, i) { i + 1 } else { brace_end(&cleaned, i) };
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
    for key in ["class ", "struct ", "interface ", "enum ", "trait ", "protocol ", "actor ", "object ", "mixin ", "extension "] {
        if let Some(name) = keyword_name(t, &[key]) {
            // The keyword decides the kind, after any modifiers (`pub struct`, `public enum`).
            let kind = match key {
                "struct " => "struct",
                "interface " | "protocol " => "interface",
                "enum " => "enum",
                _ => "class",
            };
            return Some((name, kind.into()));
        }
    }
    if lang == "go" {
        if let Some(name) = go_func_name(t) {
            return Some((name, "function".into()));
        }
    }
    if lang == "rust" {
        let body = strip_leading_rust_attrs(t);
        if let Some(name) = rust_fn_name(body) {
            return Some((name, "function".into()));
        }
    }
    if matches!(lang, "swift" | "kotlin" | "dart") {
        if let Some(name) = after_keyword_fn(t, &["func ", "fun ", "function "]) {
            return Some((name, "function".into()));
        }
    }
    if matches!(lang, "javascript" | "typescript") {
        if let Some(found) = jest_block(t) {
            return Some(found);
        }
    }
    if matches!(lang, "c" | "cpp") {
        if let Some(name) = super::framework::cpp_macro_name(t) {
            return Some((name, "test_case".into()));
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

fn jest_block(t: &str) -> Option<(String, String)> {
    let (kind, rest) = jest_keyword(t)?;
    let rest = rest.trim_start();
    let rest = if let Some(after_dot) = rest.strip_prefix('.') {
        let after_dot = after_dot.trim_start();
        let mod_len = after_dot.chars().take_while(|c| c.is_ascii_alphabetic()).count();
        let modifier = &after_dot[..mod_len];
        if !matches!(modifier, "only" | "skip" | "todo") {
            return None;
        }
        after_dot[mod_len..].trim_start()
    } else {
        rest
    };
    let rest = rest.strip_prefix('(')?.trim_start();
    let name = quoted_token(rest)?;
    if name.is_empty() {
        return None;
    }
    let unit_type = if kind == "describe" { "test_suite" } else { "test_case" };
    Some((name, unit_type.into()))
}

fn jest_keyword(t: &str) -> Option<(&'static str, &str)> {
    for kw in ["describe", "it", "test"] {
        if let Some(rest) = t.strip_prefix(kw) {
            let boundary = rest.chars().next().unwrap_or('(');
            if boundary.is_ascii_alphanumeric() || boundary == '_' || boundary == '$' {
                continue;
            }
            return Some((if kw == "describe" { "describe" } else { kw }, rest));
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
            if (before.split_whitespace().all(is_modifier) && !before.is_empty() || before.is_empty())
                && (idx == 0 || t.as_bytes().get(idx - 1).is_some_and(|b| b.is_ascii_whitespace()))
            {
                return ident(&t[idx + k.len()..]);
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
            | "default" | "synchronized" | "native" | "strictfp" | "inline" | "pub"
    ) || (w.starts_with("pub(") && w.ends_with(')'))
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

fn strip_leading_rust_attrs(t: &str) -> &str {
    let mut s = t.trim();
    while s.starts_with("#[") {
        let Some(end) = s.find(']') else { break };
        s = s[end + 1..].trim_start();
    }
    s
}

fn rust_attr_head(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("#[")?.trim_start();
    let end = rest.find(']')?;
    let head = rest[..end].split('(').next()?.trim();
    if head.is_empty() { None } else { Some(head) }
}

fn is_rust_test_attr(head: &str) -> bool {
    head == "test"
        || head == "rstest"
        || head == "test_case"
        || head.ends_with("::test")
        || head.ends_with("::rstest")
        || head.ends_with("::test_case")
}

fn rust_test_above(lines: &[String], index: usize) -> bool {
    if line_has_rust_test_attr(&lines[index]) {
        return true;
    }
    let mut i = index;
    while i > 0 {
        i -= 1;
        let t = lines[i].trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        if let Some(head) = rust_attr_head(t) {
            if is_rust_test_attr(head) {
                return true;
            }
            continue;
        }
        return false;
    }
    false
}

fn line_has_rust_test_attr(line: &str) -> bool {
    let mut rest = line.trim();
    while let Some(start) = rest.find("#[") {
        let after = &rest[start..];
        let Some(head) = rust_attr_head(after) else { break };
        if is_rust_test_attr(head) {
            return true;
        }
        let Some(end) = after.find(']') else { break };
        rest = after[end + 1..].trim_start();
    }
    false
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
    if name.is_empty() || is_modifier(name) || matches!(name, "if" | "for" | "while" | "switch" | "return" | "new" | "require" | "include") {
        return None;
    }
    // `const x = require(...)` is a call, not a declaration.
    let before = parts[parts.len() - 2];
    if before == "=" || before.ends_with('=') {
        return None;
    }
    Some(name.to_string())
}

/// Whether a declaration ends with `;` before any `{` (a unit or tuple struct, a forward declaration).
fn body_less(lines: &[String], start: usize) -> bool {
    for line in lines.iter().skip(start).take(20) {
        for c in line.chars() {
            match c {
                '{' => return false,
                ';' => return true,
                _ => {}
            }
        }
    }
    false
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

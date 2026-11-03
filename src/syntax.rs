//! Homegrown syntax tree. No tree-sitter crate.
//! Python and JavaScript/TypeScript become a module node with function and class children.
//! Python unit extraction goes through this tree. Other languages keep their scanners.

use crate::extract::CodeUnit;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub kind: &'static str,
    pub name: String,
    pub line_start: usize,
    pub line_end: usize,
    pub children: Vec<Node>,
}

pub fn parse(language: &str, source: &str) -> Option<Node> {
    match language {
        "python" => Some(parse_python(source)),
        "javascript" | "typescript" => Some(parse_javascript(source)),
        _ => None,
    }
}

pub fn python_units(file_path: &str, source: &str) -> Vec<CodeUnit> {
    flatten(file_path, &parse_python(source))
}

fn flatten(file_path: &str, node: &Node) -> Vec<CodeUnit> {
    let mut units = Vec::new();
    walk(file_path, node, &mut units);
    units
}

fn walk(file_path: &str, node: &Node, units: &mut Vec<CodeUnit>) {
    if node.kind == "function" || node.kind == "async_function" || node.kind == "class" {
        units.push(CodeUnit {
            file_path: file_path.to_string(),
            name: node.name.clone(),
            unit_type: node.kind.to_string(),
            line_start: node.line_start,
            line_end: node.line_end,
        });
    }
    for child in &node.children {
        walk(file_path, child, units);
    }
}

fn parse_python(source: &str) -> Node {
    let lines: Vec<&str> = source.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let mut module = Node { kind: "module", name: String::new(), line_start: 1, line_end: lines.len().max(1), children: Vec::new() };
    let mut depth = 0i32;
    let mut quote: Quote = Quote::None;
    // (indent, is_class, bare class name). Functions are pushed so a nested def
    // is not qualified; only a def whose enclosing block is a class becomes Class.method.
    let mut stack: Vec<(usize, bool, String)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (clean, next_depth, next_quote) = scan_py_line(lines[i], depth, quote);
        let trimmed = clean.trim();
        let raw_trim = lines[i].trim();
        let skip_stack = raw_trim.is_empty() || raw_trim.starts_with('#') || quote != Quote::None;
        if depth == 0 && quote == Quote::None && !skip_stack {
            let indent = indent_of(lines[i]);
            stack.retain(|(ind, _, _)| *ind < indent);
            if let Some(name) = class_name(trimmed) {
                let end = block_end(&lines, i, indent);
                module.children.push(Node {
                    kind: "class",
                    name: name.clone(),
                    line_start: i + 1,
                    line_end: end,
                    children: Vec::new(),
                });
                stack.push((indent, true, name));
            } else if let Some((name, is_async)) = def_name(trimmed) {
                let parent = match stack.last() {
                    Some((_, true, class_name)) => Some(class_name.clone()),
                    _ => None,
                };
                let qname = match &parent {
                    Some(p) => format!("{p}.{name}"),
                    None => name,
                };
                let end = block_end(&lines, i, indent);
                module.children.push(Node {
                    kind: if is_async { "async_function" } else { "function" },
                    name: qname,
                    line_start: i + 1,
                    line_end: end,
                    children: Vec::new(),
                });
                stack.push((indent, false, String::new()));
            }
        }
        depth = next_depth;
        quote = next_quote;
        i += 1;
    }
    module
}

fn parse_javascript(source: &str) -> Node {
    let lines: Vec<&str> = source.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let mut module = Node { kind: "module", name: String::new(), line_start: 1, line_end: lines.len().max(1), children: Vec::new() };
    let mut depth = 0i32;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = strip_js(line);
        if depth == 0 {
            if let Some(name) = js_name(&trimmed, "class ") {
                module.children.push(Node { kind: "class", name, line_start: i + 1, line_end: js_end(&lines, i), children: Vec::new() });
            } else if let Some(name) = js_name(&trimmed, "async function ") {
                module.children.push(Node { kind: "async_function", name, line_start: i + 1, line_end: js_end(&lines, i), children: Vec::new() });
            } else if let Some(name) = js_name(&trimmed, "function ") {
                module.children.push(Node { kind: "function", name, line_start: i + 1, line_end: js_end(&lines, i), children: Vec::new() });
            }
        }
        depth += brace_delta(line);
        if depth < 0 {
            depth = 0;
        }
        i += 1;
    }
    module
}

fn js_name(trimmed: &str, prefix: &str) -> Option<String> {
    let rest = trimmed.trim_start().strip_prefix(prefix)?;
    let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$').collect();
    if name.is_empty() { None } else { Some(name) }
}

fn js_end(lines: &[&str], start: usize) -> usize {
    let mut depth = 0i32;
    let mut seen = false;
    let mut i = start;
    while i < lines.len() {
        depth += brace_delta(lines[i]);
        if lines[i].contains('{') {
            seen = true;
        }
        if seen && depth <= 0 {
            return i + 1;
        }
        i += 1;
    }
    lines.len().max(start + 1)
}

fn brace_delta(line: &str) -> i32 {
    let mut d = 0i32;
    for c in strip_js(line).chars() {
        match c {
            '{' => d += 1,
            '}' => d -= 1,
            _ => {}
        }
    }
    d
}

fn strip_js(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    let mut in_str: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = in_str {
            if c == '\\' { chars.next(); continue; }
            if c == q { in_str = None; }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            break;
        }
        if c == '"' || c == '\'' || c == '`' {
            in_str = Some(c);
            continue;
        }
        out.push(c);
    }
    out
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
    } else {
        let r = trimmed.strip_prefix("def ")?;
        (r, false)
    };
    let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if name.is_empty() { None } else { Some((name, is_async)) }
}

fn block_end(lines: &[&str], start: usize, ind: usize) -> usize {
    let mut end = start + 1;
    let mut i = start + 1;
    let mut depth = bracket_delta(lines[start]);
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
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
            if c == '\\' { chars.next(); continue; }
            if c == q { in_str = None; }
            continue;
        }
        if c == '#' { break; }
        if c == '"' || c == '\'' { in_str = Some(c); continue; }
        match c {
            '(' | '[' | '{' => d += 1,
            ')' | ']' | '}' => d -= 1,
            _ => {}
        }
    }
    d
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    None,
    Single(char),
    Triple(char),
}

/// Track parentheses and quotes, including triple-quoted strings that span lines.
/// Parentheses inside strings do not hide the next `def`.
fn scan_py_line(line: &str, depth_in: i32, mut quote: Quote) -> (String, i32, Quote) {
    let mut out = String::new();
    let mut depth = depth_in;
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if let Quote::Triple(q) = quote {
            if bytes[i..].starts_with(&[q as u8, q as u8, q as u8]) {
                quote = Quote::None;
                i += 3;
                continue;
            }
            i += 1;
            continue;
        }
        if let Quote::Single(q) = quote {
            if c == '\\' {
                i += 1;
                if i < bytes.len() { i += 1; }
                continue;
            }
            if c == q {
                quote = Quote::None;
            }
            i += 1;
            continue;
        }
        if c == '#' && depth == 0 {
            break;
        }
        if c == '"' || c == '\'' {
            if i + 2 < bytes.len() && bytes[i + 1] == bytes[i] && bytes[i + 2] == bytes[i] {
                quote = Quote::Triple(c);
                i += 3;
                continue;
            }
            quote = Quote::Single(c);
            i += 1;
            continue;
        }
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                if depth_in == 0 || depth == 1 {
                    out.push(c);
                }
            }
            ')' | ']' | '}' => {
                depth -= 1;
                out.push(c);
            }
            _ => {
                if depth <= 0 {
                    out.push(c);
                }
            }
        }
        i += 1;
    }
    (out, depth.max(0), quote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triple_quoted_parens_do_not_hide_later_defs() {
        let src = r#"
class MigrationTests:
    def test_create_model(self):
        sql = """
        CREATE TABLE x (id int)
        """
        return 1

    def test_create_model2(self):
        return 2

    def test_outer(self):
        def test_inner():
            return 3

def test_after():
    pass
"#;
        let units = python_units("t.py", src);
        let names: Vec<&str> = units.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "MigrationTests",
                "MigrationTests.test_create_model",
                "MigrationTests.test_create_model2",
                "MigrationTests.test_outer",
                "test_inner",
                "test_after",
            ]
        );
    }
}

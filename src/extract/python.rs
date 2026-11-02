use super::*;

pub(super) fn extract_python(file_path: &str, content: &str) -> Vec<CodeUnit> {
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


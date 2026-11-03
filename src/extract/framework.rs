//! Test-framework labels and annotation names. Filename rules stay in `looks_like_test`.

use std::collections::BTreeSet;

/// Framework label for a path, using file content when the name is ambiguous.
/// `None` means the built-in filename rules did not mark it and the content has no test macro.
pub fn framework_for(path: &str, content: &str) -> Option<&'static str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if (name.starts_with("test_") && name.ends_with(".py")) || name.ends_with("_test.py") {
        return Some("pytest");
    }
    if name.ends_with("_test.go") {
        return Some("go");
    }
    if name.ends_with(".test.js") || name.ends_with(".test.jsx") || name.ends_with(".test.ts") || name.ends_with(".test.tsx") {
        return Some("jest");
    }
    if name.ends_with(".spec.js") || name.ends_with(".spec.jsx") || name.ends_with(".spec.ts") || name.ends_with(".spec.tsx") {
        return Some(if content_has_playwright(content) { "playwright" } else { "jest" });
    }
    if name.ends_with("Test.cs") || name.ends_with("Tests.cs") {
        return Some("csharp_test");
    }
    if name.ends_with("Test.java") || (name.starts_with("Test") && name.ends_with(".java")) || name.ends_with("Test.kt") || name.ends_with("Test.kts") {
        return Some("junit");
    }
    if name.ends_with("Test.swift") || name.ends_with("Tests.swift") {
        return Some("xctest");
    }
    if name.ends_with("Test.php") {
        return Some("phpunit");
    }
    if name.ends_with("_spec.rb") {
        return Some("rspec");
    }
    if name.starts_with("test_") && name.ends_with(".rb") {
        return Some("minitest");
    }
    if name.ends_with("_test.dart") {
        return Some("dart_test");
    }
    if name.ends_with("_test.rs") || name.ends_with(".rs") && path_under_tests(path) {
        return Some("rust");
    }
    if name.ends_with(".rs") && content_has_rust_test(content) {
        return Some("rust");
    }
    if is_c_family(name) && content_has_cpp_test(content) {
        return Some("gtest");
    }
    None
}

fn path_under_tests(path: &str) -> bool {
    path.replace('\\', "/").split('/').next() == Some("tests")
}

fn is_c_family(name: &str) -> bool {
    name.ends_with(".c")
        || name.ends_with(".h")
        || name.ends_with(".cc")
        || name.ends_with(".cpp")
        || name.ends_with(".cxx")
        || name.ends_with(".hpp")
        || name.ends_with(".hxx")
}

pub fn content_has_playwright(content: &str) -> bool {
    content.contains("playwright") || content.contains("@playwright")
}

pub fn content_has_rust_test(content: &str) -> bool {
    content.contains("#[test]") || content.contains("#[tokio::test]") || content.contains("#[rstest]") || content.contains("#[cfg(test)]")
}

pub fn content_has_cpp_test(content: &str) -> bool {
    content.contains("TEST(") || content.contains("TEST_F(") || content.contains("TEST_CASE(") || content.contains("BOOST_AUTO_TEST_CASE(")
}

/// `TEST(Suite, Name)` and the Catch2 / Boost forms become one test unit name.
pub fn cpp_macro_name(line: &str) -> Option<String> {
    let t = line.trim();
    if let Some(rest) = macro_rest(t, "BOOST_AUTO_TEST_CASE") {
        return ident_arg(rest);
    }
    if let Some(rest) = macro_rest(t, "BOOST_FIXTURE_TEST_CASE") {
        return ident_arg(rest);
    }
    if let Some(rest) = macro_rest(t, "TEST_CASE") {
        return quoted_or_ident(rest);
    }
    if let Some(rest) = macro_rest(t, "SCENARIO") {
        return quoted_or_ident(rest);
    }
    if let Some(rest) = macro_rest(t, "TEST_F") {
        return suite_case(rest);
    }
    if let Some(rest) = macro_rest(t, "TEST_P") {
        return suite_case(rest);
    }
    if let Some(rest) = macro_rest(t, "TEST") {
        return suite_case(rest);
    }
    None
}

fn macro_rest<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(name)?;
    let rest = rest.trim_start();
    rest.strip_prefix('(')
}

fn ident_arg(rest: &str) -> Option<String> {
    let name: String = rest.trim_start().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if name.is_empty() { None } else { Some(name) }
}

fn quoted_or_ident(rest: &str) -> Option<String> {
    let t = rest.trim_start();
    if t.starts_with('"') || t.starts_with('\'') {
        let q = t.chars().next()?;
        let body = &t[q.len_utf8()..];
        let end = body.find(q)?;
        let name = body[..end].trim();
        if name.is_empty() { None } else { Some(name.to_string()) }
    } else {
        ident_arg(t)
    }
}

fn suite_case(rest: &str) -> Option<String> {
    let suite = ident_arg(rest)?;
    let after = rest.trim_start()[suite.len()..].trim_start();
    let after = after.strip_prefix(',')?.trim_start();
    let name = ident_arg(after)?;
    Some(format!("{suite}.{name}"))
}

/// Names marked with `@Test`, `[Fact]` / `[Theory]` / `[Test]` / `[TestMethod]`, `@Test`, or `#[test]`.
pub fn annotated_names(content: &str, framework: &str) -> BTreeSet<String> {
    match framework {
        "junit" => java_annotated(content),
        "csharp_test" => csharp_annotated(content),
        "xctest" => swift_annotated(content),
        "rust" => rust_annotated(content),
        _ => BTreeSet::new(),
    }
}

fn java_annotated(content: &str) -> BTreeSet<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut names = BTreeSet::new();
    let mut i = 0;
    while i < lines.len() {
        if java_test_attr(lines[i]) {
            if let Some(name) = method_after_attrs(&lines[i..]) {
                names.insert(name);
            }
        }
        i += 1;
    }
    names
}

fn java_test_attr(line: &str) -> bool {
    let t = line.trim();
    t.contains("@Test") || t.contains("@ParameterizedTest") || t.contains("@RepeatedTest")
}

fn csharp_annotated(content: &str) -> BTreeSet<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut names = BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        if csharp_test_attr(line) {
            if let Some(name) = method_after_attrs(&lines[i..]) {
                names.insert(name);
            }
        }
    }
    names
}

fn csharp_test_attr(line: &str) -> bool {
    let t = line.trim();
    for attr in ["[Fact", "[Theory", "[TestMethod", "[Test"] {
        if let Some(rest) = t.strip_prefix(attr) {
            let next = rest.chars().next().unwrap_or(']');
            if next == ']' || next == '(' || next.is_whitespace() {
                return true;
            }
        }
    }
    false
}

fn swift_annotated(content: &str) -> BTreeSet<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut names = BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("@Test") {
            if let Some(name) = swift_func_from(t) {
                names.insert(name);
                continue;
            }
            if let Some(name) = method_after_attrs(&lines[i..]) {
                names.insert(name);
            }
        }
    }
    names
}

fn swift_func_from(line: &str) -> Option<String> {
    let idx = line.find("func ")?;
    let rest = line[idx + 5..].trim_start();
    let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if name.is_empty() { None } else { Some(name) }
}

fn rust_annotated(content: &str) -> BTreeSet<String> {
    let lines: Vec<&str> = content.lines().collect();
    let mut names = BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("#[test]") || t.starts_with("#[tokio::test]") || t.starts_with("#[rstest]") || t.starts_with("#[test_case") {
            for next in lines.iter().skip(i).take(6) {
                let n = next.trim().trim_start_matches("async ").trim_start();
                if let Some(rest) = n.strip_prefix("fn ") {
                    let name: String = rest.trim_start().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                    if !name.is_empty() {
                        names.insert(name);
                    }
                    break;
                }
            }
        }
    }
    names
}

/// Walk from an attribute line to the next method signature and take its name.
fn method_after_attrs(lines: &[&str]) -> Option<String> {
    for line in lines.iter().take(8) {
        let t = line.trim();
        if t.is_empty() || is_attr_line(t) {
            continue;
        }
        if let Some(name) = signature_name(t) {
            return Some(name);
        }
    }
    None
}

fn is_attr_line(t: &str) -> bool {
    t.starts_with('@') || t.starts_with('[') || t.starts_with('#')
}

fn signature_name(line: &str) -> Option<String> {
    let mut t = line.trim();
    if let Some(idx) = t.find("func ") {
        return swift_func_from(&t[idx..]);
    }
    if let Some(rest) = t.strip_prefix("fn ").or_else(|| t.strip_prefix("fun ")) {
        t = rest;
    }
    let paren = t.find('(')?;
    let head = t[..paren].trim();
    if head.contains('=') {
        return None;
    }
    let mut parts: Vec<&str> = head.split_whitespace().collect();
    while parts.len() > 1 && is_sig_modifier(parts[0]) {
        parts.remove(0);
    }
    let name = parts.last()?.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
    if name.is_empty() || is_sig_modifier(name) {
        None
    } else {
        Some(name.to_string())
    }
}

fn is_sig_modifier(w: &str) -> bool {
    matches!(
        w,
        "public" | "private" | "protected" | "internal" | "static" | "final" | "abstract"
            | "async" | "override" | "virtual" | "open" | "func" | "fun" | "fn" | "void"
    )
}

//! How a name is called, for languages whose dependencies are bare call names (Rust, C, C++).
//!
//! A test that calls `Vec::new()` and `x.len()` must not be linked to every `new` and `len` in
//! the workspace. `call_sites` records, per called name, the `Type::` / `module::` qualifiers it
//! was called through and whether it was called as a method or as a free function; `use_paths`
//! records where `use` lines import names from. The edge builder narrows ambiguous names with
//! them.

use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CallSite {
    /// `T` in `T::name(` (also `T::<..>::name(`).
    pub quals: BTreeSet<String>,
    /// Called as `x.name(` or `x->name(`.
    pub method: bool,
    /// Called as `name(`.
    pub free: bool,
    /// What each `.name(` was called on: a variable (`atlas`), `self`, a field of self
    /// (`self.atlas`), or `?` when it is the result of another call or an index.
    pub recvs: BTreeSet<String>,
}

/// `text` with strings, chars and comments replaced by spaces (newlines kept), so offsets and
/// line numbers still match the original.
pub fn code_only(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some(next) = skip_non_code(text, i) {
            let next = next.min(bytes.len());
            out.extend(bytes[i..next].iter().map(|b| if *b == b'\n' { b'\n' } else { b' ' }));
            i = next;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

/// What `.name(` at `dot` (the index of the `.`) is called on.
fn receiver(text: &str, dot: usize) -> String {
    let bytes = text.as_bytes();
    let end = skip_ws_back(bytes, dot);
    let Some(start) = ident_before(bytes, end) else { return "?".into() };
    let word = &text[start..end];
    let before = skip_ws_back(bytes, start);
    if before >= 1 && bytes[before - 1] == b'.' {
        let inner_end = skip_ws_back(bytes, before - 1);
        if let Some(inner) = ident_before(bytes, inner_end) {
            let outer = skip_ws_back(bytes, inner);
            let chained = outer >= 1 && (bytes[outer - 1] == b'.' || bytes[outer - 1] == b':');
            if &text[inner..inner_end] == "self" && !chained {
                return format!("self.{word}");
            }
        }
        return "?".into();
    }
    if before >= 2 && &bytes[before - 2..before] == b"::" {
        return "?".into();
    }
    word.to_string()
}

const NOT_CALLS: [&str; 12] = ["if", "while", "for", "match", "return", "fn", "loop", "in", "as", "move", "unsafe", "where"];

fn ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Index of the first byte of the identifier that ends just before `end`, if any.
fn ident_before(bytes: &[u8], end: usize) -> Option<usize> {
    let mut i = end;
    while i > 0 && ident_char(bytes[i - 1]) {
        i -= 1;
    }
    (i < end && !bytes[i].is_ascii_digit()).then_some(i)
}

fn skip_ws_back(bytes: &[u8], mut i: usize) -> usize {
    while i > 0 && bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    i
}

/// If a string, raw string, char literal or comment starts at `i`, the index just past it.
fn skip_non_code(text: &str, i: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let rest = &bytes[i..];
    let at_word_start = i == 0 || !ident_char(bytes[i - 1]);
    if rest.starts_with(b"//") {
        return Some(text[i..].find('\n').map(|n| i + n).unwrap_or(bytes.len()));
    }
    if rest.starts_with(b"/*") {
        return Some(text[i + 2..].find("*/").map(|n| i + 2 + n + 2).unwrap_or(bytes.len()));
    }
    // r"..", r#".."#, br#".."#
    if at_word_start && (rest.starts_with(b"r") || rest.starts_with(b"br")) {
        let start = if rest[0] == b'b' { i + 2 } else { i + 1 };
        let hashes = bytes[start..].iter().take_while(|b| **b == b'#').count();
        if bytes.get(start + hashes) == Some(&b'"') {
            let close = format!("\"{}", "#".repeat(hashes));
            let body = start + hashes + 1;
            return Some(text[body..].find(&close).map(|n| body + n + close.len()).unwrap_or(bytes.len()));
        }
        return None;
    }
    match rest.first() {
        Some(b'"') => {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'"' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            Some((j + 1).min(bytes.len()))
        }
        Some(b'\'') => {
            // A char literal ('x', '\n', '\u{..}', 'é'); otherwise a lifetime or label.
            if rest.get(1) == Some(&b'\\') {
                return Some(text.get(i + 3..).and_then(|t| t.find('\'')).map(|n| i + 3 + n + 1).unwrap_or(bytes.len()));
            }
            let c = text[i + 1..].chars().next()?;
            let after = i + 1 + c.len_utf8();
            (bytes.get(after) == Some(&b'\'')).then_some(after + 1)
        }
        _ => None,
    }
}

pub fn call_sites(text: &str) -> HashMap<String, CallSite> {
    let bytes = text.as_bytes();
    let mut out: HashMap<String, CallSite> = HashMap::new();
    let mut i = 0;
    while i < bytes.len() {
        // Text in strings, chars and comments is not code: `"front (+Z)"` calls nothing.
        if let Some(next) = skip_non_code(text, i) {
            i = next;
            continue;
        }
        if !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') || (i > 0 && ident_char(bytes[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && ident_char(bytes[i]) {
            i += 1;
        }
        let name = &text[start..i];
        let mut j = i;
        // turbofish on the call itself: name::<T>(
        if text[j..].starts_with("::<") {
            let mut depth = 0;
            while j < bytes.len() {
                match bytes[j] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
        }
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b'(' || NOT_CALLS.contains(&name) {
            continue;
        }
        let before = skip_ws_back(bytes, start);
        // `fn name(` is a definition.
        if let Some(w) = ident_before(bytes, before) {
            if &text[w..before] == "fn" {
                continue;
            }
        }
        let site = out.entry(name.to_string()).or_default();
        if before >= 1 && bytes[before - 1] == b'.' || before >= 2 && &bytes[before - 2..before] == b"->" {
            site.method = true;
            site.recvs.insert(if bytes[before - 1] == b'.' { receiver(text, before - 1) } else { "?".to_string() });
        } else if before >= 2 && &bytes[before - 2..before] == b"::" {
            let mut k = skip_ws_back(bytes, before - 2);
            if k > 0 && bytes[k - 1] == b'>' {
                let mut depth = 0;
                while k > 0 {
                    k -= 1;
                    match bytes[k] {
                        b'>' => depth += 1,
                        b'<' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if k >= 2 && &bytes[k - 2..k] == b"::" {
                    k -= 2;
                }
            }
            match ident_before(bytes, k) {
                Some(q) => {
                    site.quals.insert(text[q..k].to_string());
                }
                None => site.free = true,
            }
        } else {
            site.free = true;
        }
    }
    out
}

/// Imported name (or `*` for a glob) -> the path segments before it, from Rust `use` lines.
pub fn use_paths(text: &str) -> HashMap<String, Vec<Vec<String>>> {
    let mut out: HashMap<String, Vec<Vec<String>>> = HashMap::new();
    let mut add = |name: &str, path: &[String]| {
        out.entry(name.to_string()).or_default().push(path.to_vec());
    };
    for line in text.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("use ").or_else(|| t.strip_prefix("pub use ")) else { continue };
        let rest = rest.trim_end_matches(';').trim();
        let (prefix, group) = match rest.find('{') {
            Some(p) => (&rest[..p], Some(rest[p + 1..].trim_end_matches('}'))),
            None => (rest, None),
        };
        let mut base: Vec<String> = prefix.split("::").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let items: Vec<String> = match group {
            Some(g) => g.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            None => vec![base.pop().unwrap_or_default()],
        };
        for item in items {
            let (path_part, alias) = match item.split_once(" as ") {
                Some((p, a)) => (p.trim().to_string(), Some(a.trim().to_string())),
                None => (item.clone(), None),
            };
            let mut segs = base.clone();
            let mut parts: Vec<String> = path_part.split("::").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            let leaf = parts.pop().unwrap_or_default();
            segs.extend(parts);
            let name = alias.unwrap_or_else(|| leaf.clone());
            if leaf == "*" {
                add("*", &segs);
            } else if leaf == "self" {
                if let Some(last) = segs.last().cloned() {
                    add(&last, &segs);
                }
            } else {
                segs.push(leaf);
                add(&name, &segs);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualifiers_methods_and_free_calls() {
        let text = "fn helper() {}\nlet v = Vec::new();\nlet b = Box3::new(1);\nlet r = geom::yaw_toward(a, b);\nv.len();\np->step();\nlet m = HashMap::<u8, u8>::with_capacity(4);\nfree_fn(1);\nif (x) {}\nSelf::make();";
        let sites = call_sites(text);
        assert_eq!(sites["new"].quals, ["Box3".to_string(), "Vec".to_string()].into_iter().collect());
        assert_eq!(sites["yaw_toward"].quals.iter().collect::<Vec<_>>(), vec!["geom"]);
        assert!(sites["len"].method && !sites["len"].free && sites["step"].method);
        assert_eq!(sites["with_capacity"].quals.iter().collect::<Vec<_>>(), vec!["HashMap"]);
        assert!(sites["free_fn"].free && sites["make"].quals.contains("Self"));
        assert!(!sites.contains_key("helper") && !sites.contains_key("if"));
        assert_eq!(sites["len"].recvs.iter().collect::<Vec<_>>(), vec!["v"]);
    }

    #[test]
    fn receivers_of_method_calls() {
        let text = "atlas.glyph(1);\nself.clear();\nself.atlas.take_dirty();\nmake().finish();\nself.a.b.deep();\nlist[0].first();\nx\n    .chained();\nx.y().z();";
        let sites = call_sites(text);
        let recv = |n: &str| sites[n].recvs.iter().cloned().collect::<Vec<_>>();
        assert_eq!(recv("glyph"), vec!["atlas"]);
        assert_eq!(recv("clear"), vec!["self"]);
        assert_eq!(recv("take_dirty"), vec!["self.atlas"]);
        assert_eq!(recv("finish"), vec!["?"]);
        assert_eq!(recv("deep"), vec!["?"]);
        assert_eq!(recv("first"), vec!["?"]);
        assert_eq!(recv("chained"), vec!["x"]);
        assert_eq!(recv("z"), vec!["?"]);
        assert_eq!(code_only("a(\"x{\") // }\nb"), "a(    )     \nb");
    }

    #[test]
    fn strings_chars_and_comments_call_nothing() {
        let text = r##"assert!(x < 1.0, "front (+Z) away: {}", y);
let s = r#"back(1)"#;
// left(2)
/* right(3) */
let c = '(';
let q = '\'';
fn f<'a>(v: &'a str) { real(v); }
let e = "esc \" up(4)";"##;
        let sites = call_sites(text);
        for name in ["front", "back", "left", "right", "up"] {
            assert!(!sites.contains_key(name), "{name} is in a literal or comment: {sites:?}");
        }
        assert!(sites["real"].free, "code after a lifetime is still read: {sites:?}");
    }

    #[test]
    fn use_lines() {
        let text = "use specula_locus::geom::yaw_toward;\nuse crate::solve::{solve, Plan as P};\nuse super::*;\nuse a::b::{self, c::d};";
        let u = use_paths(text);
        assert_eq!(u["yaw_toward"], vec![vec!["specula_locus".to_string(), "geom".into(), "yaw_toward".into()]]);
        assert_eq!(u["P"], vec![vec!["crate".to_string(), "solve".into(), "Plan".into()]]);
        assert_eq!(u["*"], vec![vec!["super".to_string()]]);
        assert_eq!(u["b"], vec![vec!["a".to_string(), "b".into()]]);
        assert_eq!(u["d"], vec![vec!["a".to_string(), "b".into(), "c".into(), "d".into()]]);
    }
}

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

/// Names the text binds itself: `let` patterns, closure parameters (`|a, b: T|`) and function
/// parameters (`fn f(write: impl Fn())`). A bare `write(..)` on such a name calls the local
/// value, not a function of the project.
pub fn local_bindings(text: &str) -> BTreeSet<String> {
    let code = code_only(text);
    let bytes = code.as_bytes();
    let mut out = BTreeSet::new();
    // In a pattern or a parameter, the bound names are its lowercase identifiers: `Some(x)`
    // binds `x`, `Point { x: a }` binds `a` (a word ending in `:` is a field), `a::B` binds none.
    let add_idents = |seg: &str, out: &mut BTreeSet<String>| {
        for w in seg.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')) {
            if w.is_empty() || w.contains("::") || w.ends_with(':') {
                continue;
            }
            let name = w.trim_start_matches(':');
            if name.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_') && !matches!(name, "mut" | "ref" | "_" | "self" | "impl" | "dyn" | "move") {
                out.insert(name.to_string());
            }
        }
    };
    let mut i = 0;
    while i < bytes.len() {
        let word_start = i == 0 || !ident_char(bytes[i - 1]);
        if word_start && code[i..].starts_with("let") && bytes.get(i + 3).is_some_and(|b| !ident_char(*b)) {
            // `let PATTERN [: T] = ..;` / `if let PATTERN = ..`
            let rest = &code[i + 3..];
            let end = rest.find(['=', ';']).unwrap_or(rest.len());
            let pat = &rest[..end];
            // The pattern ends at a top-level `:` (its type); `::` is a path.
            let mut cut = pat.len();
            let pb = pat.as_bytes();
            let mut depth = 0i32;
            let mut k = 0;
            while k < pb.len() {
                match pb[k] {
                    b'(' | b'[' | b'{' | b'<' => depth += 1,
                    b')' | b']' | b'}' | b'>' => depth -= 1,
                    b':' if depth == 0 => {
                        if pb.get(k + 1) == Some(&b':') {
                            k += 2;
                            continue;
                        }
                        cut = k;
                        break;
                    }
                    _ => {}
                }
                k += 1;
            }
            add_idents(&pat[..cut], &mut out);
            i += 3;
            continue;
        }
        if word_start && code[i..].starts_with("fn") && bytes.get(i + 2).is_some_and(|b| !ident_char(*b)) {
            // `fn name<..>(params)`: the parameter names, each written `name: Type`.
            let mut angle = 0i32;
            let open = bytes.iter().enumerate().skip(i + 2).find_map(|(k, b)| {
                match b {
                    b'<' => angle += 1,
                    b'>' => angle -= 1,
                    b'(' if angle <= 0 => return Some(Some(k)),
                    b'{' | b';' => return Some(None),
                    _ => {}
                }
                None
            }).flatten();
            if let Some(open) = open {
                let mut depth = 0i32;
                let mut close = open;
                for (k, b) in bytes.iter().enumerate().skip(open) {
                    match b {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                close = k;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let params = &code[open + 1..close.max(open + 1)];
                let mut d = 0i32;
                let mut part = String::new();
                for c in params.chars().chain(std::iter::once(',')) {
                    match c {
                        '(' | '<' | '[' => d += 1,
                        ')' | '>' | ']' => d -= 1,
                        _ => {}
                    }
                    if c == ',' && d <= 0 {
                        if let Some((p, _)) = part.split_once(':').filter(|(_, t)| !t.starts_with(':')) {
                            add_idents(p, &mut out);
                        }
                        part.clear();
                    } else {
                        part.push(c);
                    }
                }
                i = close.max(i + 2);
                continue;
            }
        }
        if bytes[i] == b'|' && bytes.get(i + 1) != Some(&b'|') && (i == 0 || bytes[i - 1] != b'|') {
            // A closure's `|params|`, only where an expression starts (not `a | b`).
            let before = skip_ws_back(bytes, i);
            let opens = before == 0 || matches!(bytes[before - 1], b'=' | b'(' | b',' | b'{' | b';' | b'[') || ident_before(bytes, before).is_some_and(|w| matches!(&code[w..before], "move" | "return"));
            if opens {
                if let Some(close) = code[i + 1..].find('|').map(|p| i + 1 + p) {
                    let params = &code[i + 1..close];
                    if !params.contains(['\n', ';']) {
                        for p in params.split(',') {
                            let p = p.split_once(':').map(|(a, _)| a).unwrap_or(p);
                            add_idents(p, &mut out);
                        }
                    }
                    i = close + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// `call_sites`, except that a bare call of a name the text binds itself (a closure in a
/// `let`, a parameter) is not a free call: `let page = || ..; page()` calls no project `page`.
pub fn call_sites_scoped(text: &str) -> HashMap<String, CallSite> {
    let mut sites = call_sites(text);
    let bound = local_bindings(text);
    sites.retain(|name, site| {
        if bound.contains(name) {
            site.free = false;
        }
        site.free || site.method || !site.quals.is_empty()
    });
    sites
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
    fn names_bound_in_the_text_are_not_free_calls() {
        let text = "fn helper<F: Fn(u8)>(write: F, (a, b): (u8, u8)) { write(1); }\n#[test]\nfn t() {\n    let err = |m: &str| m.len();\n    let mut page = move || 3;\n    let Some(found) = lookup() else { return };\n    let Point { x: px, .. } = p;\n    err(\"x\");\n    page();\n    found(1);\n    px(2);\n    real(1);\n    crate::io::write(2);\n    let total = a | b;\n    total(3);\n}";
        let bound = local_bindings(text);
        for n in ["write", "a", "b", "err", "m", "page", "found", "px", "total"] {
            assert!(bound.contains(n), "{n} is bound: {bound:?}");
        }
        for n in ["real", "lookup", "Point", "x", "Some", "helper", "t"] {
            assert!(!bound.contains(n), "{n} is not bound: {bound:?}");
        }
        let sites = call_sites_scoped(text);
        for n in ["err", "page", "found", "px", "total"] {
            assert!(!sites.contains_key(n), "{n}(..) calls a local: {sites:?}");
        }
        assert!(sites["real"].free && sites["lookup"].free);
        assert!(!sites["write"].free && sites["write"].quals.contains("io"), "a qualified call of a bound name stays");
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

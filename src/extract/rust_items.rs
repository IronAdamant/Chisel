//! Rust items a call can be resolved through, read from the source text without a parser.
//!
//! `x.glyph()` and `GlyphAtlas::new()` name a method, not a file. To link them to the right
//! definition Chisel needs to know which type owns each method (`impl_blocks`), what type a
//! local variable or parameter has (`local_types`), and what type a struct field has
//! (`struct_fields`, for `self.atlas.glyph()`). Strings, chars and comments are blanked first
//! (`calls::code_only`), so braces and keywords inside them do not count.

use super::calls::code_only;
use std::collections::{BTreeSet, HashMap};

/// An `impl` or `trait` block. Lines are 1-based and inclusive.
#[derive(Clone, Debug, PartialEq)]
pub struct ImplBlock {
    pub start: usize,
    pub end: usize,
    /// The implementing type (`impl Trait for Type`, `impl Type`), or the trait's own name for a
    /// `trait` block (its default methods).
    pub ty: String,
    /// `Trait` in `impl Trait for Type`.
    pub trait_name: Option<String>,
    pub is_trait: bool,
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The bare type a written type names: `&'a mut crate::x::Foo<T>` -> `Foo`; `Box<Foo>`,
/// `Rc<Foo>` and `Arc<Foo>` -> `Foo` (method calls see through them).
pub fn type_name(written: &str) -> Option<String> {
    let mut t = written.trim();
    loop {
        let before = t;
        t = t.trim_start_matches('&').trim_start();
        if let Some(rest) = t.strip_prefix('\'') {
            t = rest.trim_start_matches(ident_char).trim_start();
        }
        for kw in ["mut ", "dyn ", "impl ", "const "] {
            if let Some(rest) = t.strip_prefix(kw) {
                t = rest.trim_start();
            }
        }
        if t == before {
            break;
        }
    }
    let path_end = t.find(|c: char| !(ident_char(c) || c == ':')).unwrap_or(t.len());
    let path = &t[..path_end];
    let last = path.rsplit("::").next().unwrap_or(path);
    if last.is_empty() || last.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    if matches!(last, "Box" | "Rc" | "Arc") {
        let rest = t[path_end..].trim_start();
        if let Some(inner) = rest.strip_prefix('<') {
            let mut depth = 1;
            let mut end = inner.len();
            for (i, c) in inner.char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i;
                            break;
                        }
                    }
                    ',' if depth == 1 => {
                        end = i;
                        break;
                    }
                    _ => {}
                }
            }
            return type_name(&inner[..end]);
        }
    }
    Some(last.to_string())
}

/// Byte offset of each line start, for offset -> 1-based line.
fn line_starts(text: &str) -> Vec<usize> {
    let mut out = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            out.push(i + 1);
        }
    }
    out
}

fn line_of(starts: &[usize], offset: usize) -> usize {
    match starts.binary_search(&offset) {
        Ok(i) => i + 1,
        Err(i) => i,
    }
}

/// Leading attributes, visibility and qualifiers before an item keyword.
fn strip_item_prefix(mut t: &str) -> &str {
    loop {
        let before = t;
        t = t.trim_start();
        if t.starts_with("#[") {
            match t.find(']') {
                Some(e) => t = &t[e + 1..],
                None => return "",
            }
        }
        if let Some(rest) = t.strip_prefix("pub(") {
            t = rest.find(')').map(|e| &rest[e + 1..]).unwrap_or("");
        }
        for kw in ["pub ", "unsafe ", "default ", "auto "] {
            if let Some(rest) = t.strip_prefix(kw) {
                t = rest;
            }
        }
        if t == before {
            return t;
        }
    }
}

/// Split at the first top-level ` for ` (outside `<..>`).
fn split_for(header: &str) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    let bytes = header.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => depth += 1,
            b'>' => depth -= 1,
            b'f' if depth == 0 && header[i..].starts_with("for") && (i == 0 || !ident_char(bytes[i - 1] as char)) && bytes.get(i + 3).is_some_and(|b| b.is_ascii_whitespace()) => {
                return Some((&header[..i], &header[i + 3..]));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Skip a leading `<...>` generic list.
fn skip_generics(t: &str) -> &str {
    let t = t.trim_start();
    if !t.starts_with('<') {
        return t;
    }
    let mut depth = 0;
    for (i, c) in t.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return &t[i + 1..];
                }
            }
            _ => {}
        }
    }
    ""
}

/// Every `impl` and `trait` block in a Rust file.
pub fn impl_blocks(text: &str) -> Vec<ImplBlock> {
    let code = code_only(text);
    let starts = line_starts(&code);
    // (block, offset of its opening brace)
    let mut pending: Vec<(ImplBlock, usize)> = Vec::new();
    for (index, &line_start) in starts.iter().enumerate() {
        let line_end = code[line_start..].find('\n').map(|n| line_start + n).unwrap_or(code.len());
        let line = &code[line_start..line_end];
        let item = strip_item_prefix(line);
        let (is_trait, rest) = if let Some(r) = item.strip_prefix("impl") {
            if !(r.starts_with(' ') || r.starts_with('<')) {
                continue;
            }
            (false, r)
        } else if let Some(r) = item.strip_prefix("trait ") {
            (true, r)
        } else {
            continue;
        };
        let header_start = line_end - rest.len();
        let Some(brace_rel) = code[header_start..].find(['{', ';']) else { continue };
        let brace = header_start + brace_rel;
        if code.as_bytes()[brace] != b'{' {
            continue;
        }
        let header = &code[header_start..brace];
        let header = match header.find(" where") .or_else(|| header.find("\nwhere")) {
            Some(w) => &header[..w],
            None => header,
        };
        let block = if is_trait {
            let name: String = header.trim_start().chars().take_while(|c| ident_char(*c)).collect();
            if name.is_empty() {
                continue;
            }
            ImplBlock { start: index + 1, end: index + 1, ty: name, trait_name: None, is_trait: true }
        } else {
            let header = skip_generics(header);
            let (trait_part, ty_part) = match split_for(header) {
                Some((t, ty)) => (Some(t), ty),
                None => (None, header),
            };
            let Some(ty) = type_name(ty_part) else { continue };
            let trait_name = trait_part.and_then(|t| type_name(t.trim().trim_start_matches('!')));
            ImplBlock { start: index + 1, end: index + 1, ty, trait_name, is_trait: false }
        };
        pending.push((block, brace));
    }
    if pending.is_empty() {
        return Vec::new();
    }
    // One pass over the braces closes every block.
    let mut by_brace: HashMap<usize, usize> = HashMap::new();
    for (i, (_, brace)) in pending.iter().enumerate() {
        by_brace.insert(*brace, i);
    }
    let mut stack: Vec<Option<usize>> = Vec::new();
    for (offset, b) in code.bytes().enumerate() {
        match b {
            b'{' => stack.push(by_brace.get(&offset).copied()),
            b'}' => {
                if let Some(Some(i)) = stack.pop() {
                    pending[i].0.end = line_of(&starts, offset);
                }
            }
            _ => {}
        }
    }
    pending.into_iter().map(|(b, _)| b).collect()
}

/// The innermost block holding 1-based `line`.
pub fn owner_at(blocks: &[ImplBlock], line: usize) -> Option<&ImplBlock> {
    blocks.iter().filter(|b| b.start <= line && line <= b.end).min_by_key(|b| b.end - b.start)
}

/// Struct name -> field name -> field type, for structs with named fields.
pub fn struct_fields(text: &str) -> HashMap<String, HashMap<String, String>> {
    let code = code_only(text);
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut search = 0;
    while let Some(rel) = code[search..].find("struct ") {
        let at = search + rel;
        search = at + 7;
        if at > 0 && code.as_bytes()[at - 1].is_ascii_alphanumeric() {
            continue;
        }
        let rest = &code[at + 7..];
        let name: String = rest.trim_start().chars().take_while(|c| ident_char(*c)).collect();
        if name.is_empty() {
            continue;
        }
        let Some(open_rel) = rest.find(['{', ';', '(']) else { break };
        if rest.as_bytes()[open_rel] != b'{' {
            continue;
        }
        let body_start = at + 7 + open_rel + 1;
        let mut depth = 1i32;
        let mut field_start = body_start;
        let mut fields: HashMap<String, String> = HashMap::new();
        let bytes = code.as_bytes();
        let mut i = body_start;
        let mut angle = 0i32;
        while i < bytes.len() && depth > 0 {
            match bytes[i] {
                b'{' | b'(' | b'[' => depth += 1,
                b'}' | b')' | b']' => depth -= 1,
                b'<' => angle += 1,
                b'>' if i > 0 && bytes[i - 1] != b'-' => angle -= 1,
                _ => {}
            }
            if (depth == 1 && angle <= 0 && bytes[i] == b',') || depth == 0 {
                let field = strip_item_prefix(&code[field_start..i]);
                if let Some((fname, fty)) = field.split_once(':') {
                    let fname = fname.trim();
                    if !fname.is_empty() && fname.chars().all(ident_char) {
                        if let Some(t) = type_name(fty) {
                            fields.insert(fname.to_string(), t);
                        }
                    }
                }
                field_start = i + 1;
            }
            i += 1;
        }
        search = i.max(search);
        if !fields.is_empty() {
            out.entry(name).or_default().extend(fields);
        }
    }
    out
}

/// Words that start an expression but are not a type: `let x = Some(Foo::new())` says nothing.
const NOT_TYPES: [&str; 6] = ["Some", "None", "Ok", "Err", "Self", "Default"];

/// Local variable and parameter name -> type, from `let x = Type::f(..)`, `let x = Type { .. }`,
/// `let x: Type`, and `fn f(x: &Type)`. A name bound to two different types is left out.
pub fn local_types(text: &str) -> HashMap<String, String> {
    let code = code_only(text);
    let mut found: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut add = |name: &str, ty: Option<String>| {
        if let Some(ty) = ty {
            if !name.is_empty() && name != "_" && name != "self" && !NOT_TYPES.contains(&ty.as_str()) {
                found.entry(name.to_string()).or_default().insert(ty);
            }
        }
    };
    let bytes = code.as_bytes();
    let word_at = |i: usize, w: &str| code[i..].starts_with(w) && (i == 0 || !ident_char(bytes[i - 1] as char)) && code[i + w.len()..].starts_with(|c: char| c.is_whitespace() || c == '(' || c == '<');
    let mut i = 0;
    while i < bytes.len() {
        if word_at(i, "let") {
            let mut rest = code[i + 3..].trim_start();
            rest = rest.strip_prefix("mut ").unwrap_or(rest).trim_start();
            let name: String = rest.chars().take_while(|c| ident_char(*c)).collect();
            let after = rest[name.len()..].trim_start();
            if let Some(ty) = after.strip_prefix(':') {
                let ty_text = ty.split(['=', ';']).next().unwrap_or("");
                add(&name, type_name(ty_text));
            } else if let Some(expr) = after.strip_prefix('=') {
                add(&name, constructed_type(expr));
            }
            i += 3;
            continue;
        }
        if word_at(i, "fn") {
            // Parameters: the first top-level `( .. )` after the name.
            let rest = &code[i + 2..];
            if let Some(open) = rest.find('(') {
                let params_start = i + 2 + open + 1;
                let mut depth = 1i32;
                let mut j = params_start;
                let mut piece = params_start;
                let mut angle = 0i32;
                while j < bytes.len() && depth > 0 {
                    match bytes[j] {
                        b'(' | b'[' | b'{' => depth += 1,
                        b')' | b']' | b'}' => depth -= 1,
                        b'<' => angle += 1,
                        b'>' if bytes[j - 1] != b'-' => angle -= 1,
                        _ => {}
                    }
                    if (depth == 1 && angle <= 0 && bytes[j] == b',') || depth == 0 {
                        if let Some((pname, pty)) = code[piece..j].split_once(':') {
                            let pname = pname.trim().trim_start_matches("mut ").trim();
                            if pname.chars().all(ident_char) {
                                add(pname, type_name(pty));
                            }
                        }
                        piece = j + 1;
                    }
                    j += 1;
                }
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    found.into_iter().filter(|(_, tys)| tys.len() == 1).map(|(k, tys)| (k, tys.into_iter().next().unwrap_or_default())).collect()
}

/// The type an expression constructs: `Type::f(`, `path::Type::f(`, `Type {`, `Type(`.
fn constructed_type(expr: &str) -> Option<String> {
    let e = expr.trim_start().trim_start_matches('&').trim_start();
    let e = e.strip_prefix("mut ").unwrap_or(e);
    let end = e.find(|c: char| !(ident_char(c) || c == ':')).unwrap_or(e.len());
    let path = &e[..end];
    let next = e[end..].trim_start();
    let segs: Vec<&str> = path.split("::").filter(|s| !s.is_empty()).collect();
    let upper = |s: &str| s.chars().next().is_some_and(|c| c.is_ascii_uppercase());
    let ty = if next.starts_with('(') && segs.len() >= 2 {
        // `Type::new(`: the type is the segment before the function.
        segs[..segs.len() - 1].iter().rev().find(|s| upper(s)).copied()
    } else if next.starts_with('{') || next.starts_with('(') {
        segs.last().filter(|s| upper(s)).copied()
    } else {
        None
    }?;
    if NOT_TYPES.contains(&ty) {
        return None;
    }
    Some(ty.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impl_and_trait_blocks_own_their_methods() {
        let text = "pub struct GlyphAtlas;\n\nimpl GlyphAtlas {\n    pub fn new() -> Self {\n        let s = \"}\";\n        GlyphAtlas\n    }\n}\n\nimpl<T: Clone> Default for Wrap<T> where T: Copy {\n    fn default() -> Self { todo() }\n}\n\npub(crate) trait Shape {\n    fn area(&self) -> f32 { 0.0 }\n}\n\nimpl<'a> From<&'a str> for Box<Name> {\n    fn from(s: &str) -> Self { x }\n}\n\nfn ret() -> impl Iterator<Item = u8> {\n    std::iter::empty()\n}\n";
        let blocks = impl_blocks(text);
        let tys: Vec<(&str, Option<&str>, bool)> = blocks.iter().map(|b| (b.ty.as_str(), b.trait_name.as_deref(), b.is_trait)).collect();
        assert_eq!(tys, vec![("GlyphAtlas", None, false), ("Wrap", Some("Default"), false), ("Shape", None, true), ("Name", Some("From"), false)]);
        assert_eq!((blocks[0].start, blocks[0].end), (3, 8), "a brace in a string does not close the impl");
        assert_eq!(owner_at(&blocks, 4).map(|b| b.ty.as_str()), Some("GlyphAtlas"));
        assert_eq!(owner_at(&blocks, 11).map(|b| b.ty.as_str()), Some("Wrap"));
        assert_eq!(owner_at(&blocks, 23), None, "-> impl Trait is not an impl block");
    }

    #[test]
    fn local_and_parameter_types() {
        let text = "fn helper(atlas: &mut GlyphAtlas, n: usize, cfg: specula::Cfg<u8>) {}\nfn t() {\n    let mut a = specula_text::GlyphAtlas::new(AtlasConfig::default());\n    let k = GlyphKey { size: 1 };\n    let b: Box<Board> = make();\n    let v = Vec::new();\n    let o = Some(Foo::new());\n    let x = Foo::new();\n    let x = Bar::new();\n}\n";
        let t = local_types(text);
        assert_eq!(t.get("atlas").map(String::as_str), Some("GlyphAtlas"));
        assert_eq!(t.get("cfg").map(String::as_str), Some("Cfg"));
        assert_eq!(t.get("a").map(String::as_str), Some("GlyphAtlas"));
        assert_eq!(t.get("k").map(String::as_str), Some("GlyphKey"));
        assert_eq!(t.get("b").map(String::as_str), Some("Board"));
        assert_eq!(t.get("v").map(String::as_str), Some("Vec"));
        assert!(!t.contains_key("o"), "Some(..) names no type");
        assert!(!t.contains_key("x"), "a name bound to two types is ambiguous");
        assert_eq!(t.get("n").map(String::as_str), Some("usize"));
    }

    #[test]
    fn named_struct_fields() {
        let text = "pub struct Overlay {\n    pub atlas: Option<GlyphAtlas>,\n    raster: Rasterizer,\n    map: HashMap<u32, Vec<u8>>,\n    shared: Rc<Board>,\n}\nstruct Pair(u8, u8);\n";
        let f = struct_fields(text);
        let o = &f["Overlay"];
        assert_eq!(o.get("raster").map(String::as_str), Some("Rasterizer"));
        assert_eq!(o.get("shared").map(String::as_str), Some("Board"));
        assert_eq!(o.get("map").map(String::as_str), Some("HashMap"));
        assert!(!f.contains_key("Pair"));
    }
}

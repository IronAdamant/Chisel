//! Which Rust definition a call names, by type and path rather than by name alone.
//!
//! A name such as `new`, `default` or `clear` is defined in hundreds of places. A call through
//! a type (`GlyphAtlas::new`, `Self::step`) keeps only the methods that type owns; a method call
//! on a known receiver (`self`, a field of `self`, a local or parameter whose type is written
//! down) does the same with the receiver's type. A `Trait::method` call through a standard
//! trait (`Default::default`), or a qualifier imported from `std`, names nothing in the project.
//! What cannot be resolved is matched by name only when the name is rare and not a standard
//! method, at a lower weight (`NAME_ONLY`), so resolved edges rank first.

use super::analyze::{declared_types, narrow_by_call_site};
use crate::extract::calls::CallSite;
use crate::extract::rust_items::{self, ImplBlock};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;

/// Weight factor of an edge matched by name only (the receiver type is unknown).
pub(super) const NAME_ONLY: f64 = 0.5;
/// Weight factor of a method call on an unknown receiver whose owner type the caller names.
pub(super) const TYPE_MENTIONED: f64 = 0.75;

/// Methods and associated functions of the standard library that projects call constantly on
/// values of std types. On an unknown receiver they almost always mean the std method.
const STD_METHODS: &[&str] = &[
    "abs", "all", "and_then", "any", "as_bytes", "as_mut", "as_ref", "as_slice", "as_str", "borrow", "borrow_mut", "bytes",
    "capacity", "ceil", "chain", "chars", "chunks", "clamp", "clear", "clone", "cloned", "cmp", "collect", "contains",
    "contains_key", "copied", "count", "default", "drain", "drop", "end", "ends_with", "entry", "enumerate", "eq", "err",
    "expect", "extend", "filter", "filter_map", "find", "first", "flat_map", "flatten", "floor", "fmt", "fold", "for_each",
    "from", "get", "get_mut", "hash", "insert", "into", "into_iter", "is_empty", "is_err", "is_none", "is_ok", "is_some",
    "iter", "iter_mut", "join", "keys", "last", "len", "lines", "lock", "map", "map_err", "max", "max_by", "min", "min_by",
    "ne", "next", "nth", "ok", "ok_or", "or_default", "or_insert", "or_insert_with", "parse", "partial_cmp", "pop", "position",
    "powf", "powi", "push", "push_str", "read", "recv", "remove", "replace", "reserve", "resize", "retain", "rev", "round",
    "send", "set", "skip", "sort", "sort_by", "sort_by_key", "sort_unstable", "split", "sqrt", "start", "starts_with",
    "step_by", "sum", "swap", "take", "to_lowercase", "to_owned", "to_string", "to_uppercase", "to_vec", "trim", "truncate",
    "try_into", "unwrap", "unwrap_or", "unwrap_or_default", "unwrap_or_else", "values", "values_mut", "windows",
    "with_capacity", "write", "zip",
];

/// Standard library types and traits. A `Name::f` call through one of them names no project
/// code unless the project defines that name itself.
const STD_TYPES: &[&str] = &[
    "Add", "Arc", "AsMut", "AsRef", "BTreeMap", "BTreeSet", "BinaryHeap", "Borrow", "Box", "Cell", "Clone", "Copy", "Cow",
    "Debug", "Default", "Deref", "DerefMut", "Display", "Div", "Drop", "Duration", "Eq", "Err", "Error", "Extend", "File",
    "Fn", "FnMut", "FnOnce", "From", "FromIterator", "FromStr", "Hash", "HashMap", "HashSet", "Hasher", "Index",
    "IndexMut", "Instant", "Into", "IntoIterator", "Iterator", "Mul", "Mutex", "Neg", "None", "Not", "Ok", "Option",
    "Ord", "PartialEq", "PartialOrd", "Path", "PathBuf", "PhantomData", "Rc", "Read", "RefCell", "Rem", "Result",
    "RwLock", "Send", "Sized", "Some", "String", "Sub", "Sync", "SystemTime", "ToOwned", "ToString", "TryFrom", "TryInto",
    "Vec", "VecDeque", "Write",
];

/// Where a call is made from.
pub(super) struct Ctx<'a> {
    /// The calling file.
    pub file: &'a str,
    pub uses: &'a HashMap<String, Vec<Vec<String>>>,
    /// Local variable and parameter types of the calling text.
    pub locals: &'a HashMap<String, String>,
    /// The type of `self` (the impl block the caller sits in).
    pub self_type: Option<&'a str>,
    /// Capitalised identifiers in the calling text (types it names).
    pub idents: &'a HashSet<String>,
}

/// Capitalised identifiers in `text`, plus the names it imports.
pub(super) fn type_idents(text: &str, uses: &HashMap<String, Vec<Vec<String>>>) -> HashSet<String> {
    let mut out: HashSet<String> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| w.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
        .map(|w| w.to_string())
        .collect();
    for (name, paths) in uses {
        out.insert(name.clone());
        for p in paths {
            if let Some(last) = p.last() {
                out.insert(last.clone());
            }
        }
    }
    out
}

#[derive(Clone)]
struct Owner {
    ty: String,
    trait_name: Option<String>,
}

pub(super) struct RustResolver {
    sources: super::Sources,
    blocks: HashMap<String, Vec<ImplBlock>>,
    declared: HashMap<String, BTreeSet<String>>,
    /// Struct name -> field -> type, read lazily from the files declaring the struct.
    fields: HashMap<String, HashMap<String, String>>,
    fields_read: HashSet<String>,
    /// Every definition behind an id (`file:new:function` can be several `new`s): start line, kind.
    defs: HashMap<String, Vec<(usize, String)>>,
    owners: HashMap<String, Vec<Option<Owner>>>,
    type_files: std::rc::Rc<HashMap<String, BTreeSet<String>>>,
}

impl RustResolver {
    pub(super) fn new(sources: super::Sources, code_units: &[crate::json::Value], type_files: HashMap<String, BTreeSet<String>>) -> Self {
        let mut defs: HashMap<String, Vec<(usize, String)>> = HashMap::new();
        for u in code_units {
            let Some(id) = u.get("id").and_then(|v| v.as_str()) else { continue };
            let line = u.get("line_start").and_then(|v| v.as_i64()).unwrap_or(0).max(0) as usize;
            let kind = u.get("unit_type").and_then(|v| v.as_str()).unwrap_or("").to_string();
            defs.entry(id.to_string()).or_default().push((line, kind));
        }
        RustResolver {
            sources,
            blocks: HashMap::new(),
            declared: HashMap::new(),
            fields: HashMap::new(),
            fields_read: HashSet::new(),
            defs,
            owners: HashMap::new(),
            type_files: std::rc::Rc::new(type_files),
        }
    }

    fn read(&self, file: &str) -> String {
        fs::read_to_string(self.sources.path(file)).unwrap_or_default()
    }

    /// Impl and trait blocks of a file, read once.
    pub(super) fn blocks_of(&mut self, file: &str) -> &[ImplBlock] {
        if !self.blocks.contains_key(file) {
            let text = self.read(file);
            self.blocks.insert(file.to_string(), rust_items::impl_blocks(&text));
        }
        &self.blocks[file]
    }

    /// The type owning the function at 1-based `line` of `file` (its impl or trait block).
    pub(super) fn self_type_at(&mut self, file: &str, line: usize) -> Option<String> {
        rust_items::owner_at(self.blocks_of(file), line).map(|b| b.ty.clone())
    }

    /// The owner of each definition behind `id`: `None` for a free function or a type.
    fn owners_of(&mut self, id: &str, file: &str) -> Vec<Option<Owner>> {
        if let Some(o) = self.owners.get(id) {
            return o.clone();
        }
        let defs = self.defs.get(id).cloned().unwrap_or_default();
        let mut owners = Vec::new();
        for (line, kind) in defs {
            let owner = if matches!(kind.as_str(), "function" | "async_function" | "method") {
                rust_items::owner_at(self.blocks_of(file), line).map(|b| Owner { ty: b.ty.clone(), trait_name: b.trait_name.clone() })
            } else {
                None
            };
            owners.push(owner);
        }
        if owners.is_empty() {
            owners.push(None);
        }
        self.owners.insert(id.to_string(), owners.clone());
        owners
    }

    fn field_type(&mut self, ty: &str, field: &str) -> Option<String> {
        let files: Vec<String> = self.type_files.get(ty).map(|f| f.iter().cloned().collect()).unwrap_or_default();
        for f in files {
            if self.fields_read.insert(f.clone()) {
                let text = self.read(&f);
                for (name, fields) in rust_items::struct_fields(&text) {
                    self.fields.entry(name).or_default().extend(fields);
                }
            }
        }
        self.fields.get(ty).and_then(|m| m.get(field)).cloned()
    }

    fn declares(&mut self, file: &str, ty: &str) -> bool {
        if !self.declared.contains_key(file) {
            let text = self.read(file);
            self.declared.insert(file.to_string(), declared_types(&text));
        }
        self.declared[file].contains(ty)
    }

    /// `use a::Plan as P` makes `P::f` a call through `Plan`.
    fn unalias(q: &str, uses: &HashMap<String, Vec<Vec<String>>>) -> String {
        match uses.get(q).and_then(|paths| paths.first()).and_then(|p| p.last()) {
            Some(real) if real != "self" && real != "*" => real.clone(),
            _ => q.to_string(),
        }
    }

    /// Of `matched` (every unit called `name`), the ones this call can reach. Each comes with a
    /// weight factor (1 when resolved through a type or path, `TYPE_MENTIONED` or `NAME_ONLY`
    /// when only the name matched) and the owner type it was resolved through (empty when none:
    /// an id can stand for several same-named methods of one file, and the owner says which).
    pub(super) fn resolve(&mut self, ctx: &Ctx, name: &str, site: Option<&CallSite>, matched: &[String], id_file: &HashMap<&str, &str>) -> Vec<Resolved> {
        let Some(site) = site else { return Vec::new() };
        let mut out: BTreeMap<(String, String), f64> = BTreeMap::new();
        let keep = |out: &mut BTreeMap<(String, String), f64>, id: &str, owner: &str, conf: f64| {
            let slot = out.entry((id.to_string(), owner.to_string())).or_insert(0.0);
            if conf > *slot {
                *slot = conf;
            }
        };
        struct Cand {
            id: String,
            file: String,
            owners: Vec<Option<Owner>>,
        }
        let mut cands: Vec<Cand> = Vec::new();
        for id in matched {
            let Some(file) = id_file.get(id.as_str()).copied() else { continue };
            let owners = self.owners_of(id, file);
            cands.push(Cand { id: id.clone(), file: file.to_string(), owners });
        }
        let owned_by = |cands: &[Cand], ty: &str| -> Vec<String> {
            cands.iter().filter(|c| c.owners.iter().flatten().any(|o| o.ty == ty)).map(|c| c.id.clone()).collect()
        };
        let free_ids: Vec<String> = cands.iter().filter(|c| c.owners.iter().any(|o| o.is_none())).map(|c| c.id.clone()).collect();
        let from_std = |q: &str| ctx.uses.get(q).is_some_and(|paths| paths.iter().all(|p| p.first().is_some_and(|s| matches!(s.as_str(), "std" | "core" | "alloc"))));
        // Free functions through a module path or a bare call: the file-level narrowing.
        let free_narrow = |this: &mut Self, quals: BTreeSet<String>, free: bool| -> Vec<String> {
            if free_ids.is_empty() {
                return Vec::new();
            }
            let path_site = CallSite { quals, method: false, free, recvs: BTreeSet::new() };
            let type_files = std::rc::Rc::clone(&this.type_files);
            let mut declares = |f: &str, ty: &str| this.declares(f, ty);
            narrow_by_call_site(ctx.file, name, free_ids.clone(), Some(&path_site), ctx.uses, id_file, &type_files, &mut declares)
        };
        for q in &site.quals {
            if from_std(q) {
                continue;
            }
            if q == "Self" {
                match ctx.self_type {
                    Some(ty) => {
                        for id in owned_by(&cands, ty) {
                            keep(&mut out, &id, ty, 1.0);
                        }
                    }
                    None => {
                        for c in cands.iter().filter(|c| c.file == ctx.file) {
                            keep(&mut out, &c.id, "", 1.0);
                        }
                    }
                }
                continue;
            }
            if !q.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                for id in free_narrow(self, [q.clone()].into_iter().collect(), false) {
                    keep(&mut out, &id, "", 1.0);
                }
                continue;
            }
            let ty = Self::unalias(q, ctx.uses);
            let owned = owned_by(&cands, &ty);
            if !owned.is_empty() {
                for id in owned {
                    keep(&mut out, &id, &ty, 1.0);
                }
                continue;
            }
            if STD_TYPES.contains(&ty.as_str()) {
                // `Default::default()`, `From::from(x)`: any implementation in the project.
                continue;
            }
            // `Trait::method(x)` through a project trait: its implementations, when there are few.
            let impls: Vec<(&str, &str, &str)> = cands
                .iter()
                .flat_map(|c| c.owners.iter().flatten().filter(|o| o.trait_name.as_deref() == Some(ty.as_str())).map(move |o| (c.id.as_str(), c.file.as_str(), o.ty.as_str())))
                .collect();
            if !impls.is_empty() {
                let files: BTreeSet<&str> = impls.iter().map(|i| i.1).collect();
                if files.len() <= 3 {
                    for (id, _, owner) in impls {
                        keep(&mut out, id, owner, NAME_ONLY);
                    }
                }
                continue;
            }
            if self.type_files.contains_key(&ty) || STD_TYPES.contains(&ty.as_str()) {
                // A project or std type that defines no such method here: derived, generic or std.
                continue;
            }
            // A name Chisel did not see declared (a type alias, a macro-made type): the files whose
            // text declares it, by name only.
            let files: BTreeSet<String> = cands.iter().map(|c| c.file.clone()).collect();
            for f in files {
                if self.declares(&f, &ty) {
                    for c in cands.iter().filter(|c| c.file == f) {
                        keep(&mut out, &c.id, "", NAME_ONLY);
                    }
                }
            }
        }
        let mut unresolved = site.method && site.recvs.is_empty();
        for recv in &site.recvs {
            let ty = match recv.as_str() {
                "?" => None,
                "self" => ctx.self_type.map(|s| s.to_string()),
                r => match r.strip_prefix("self.") {
                    Some(field) => ctx.self_type.and_then(|st| self.field_type(st, field)),
                    None => ctx.locals.get(r).cloned(),
                },
            };
            // A type Chisel knows: a project type, a std type, a primitive, or one owning a candidate.
            let known = ty.as_ref().is_some_and(|t| {
                self.type_files.contains_key(t) || STD_TYPES.contains(&t.as_str()) || t.chars().next().is_some_and(|c| c.is_ascii_lowercase()) || !owned_by(&cands, t).is_empty()
            });
            match ty {
                Some(t) if known => {
                    for id in owned_by(&cands, &t) {
                        keep(&mut out, &id, &t, 1.0);
                    }
                }
                _ => unresolved = true,
            }
        }
        if unresolved && !STD_METHODS.contains(&name) {
            let methods: Vec<(&str, &str, &Owner)> = cands.iter().flat_map(|c| c.owners.iter().flatten().map(move |o| (c.id.as_str(), c.file.as_str(), o))).collect();
            let mentioned: Vec<&(&str, &str, &Owner)> = methods.iter().filter(|(_, _, o)| ctx.idents.contains(&o.ty) || o.trait_name.as_ref().is_some_and(|t| ctx.idents.contains(t))).collect();
            let mentioned_files: BTreeSet<&str> = mentioned.iter().map(|m| m.1).collect();
            if !mentioned.is_empty() && mentioned_files.len() <= 3 {
                for (id, _, o) in mentioned {
                    keep(&mut out, id, &o.ty, TYPE_MENTIONED);
                }
            } else {
                let files: BTreeSet<&str> = methods.iter().map(|m| m.1).collect();
                if files.len() <= 3 {
                    for (id, _, o) in methods {
                        keep(&mut out, id, &o.ty, NAME_ONLY);
                    }
                }
            }
        }
        if site.free {
            for id in free_narrow(self, BTreeSet::new(), true) {
                keep(&mut out, &id, "", 1.0);
            }
        }
        // One row per id: its best factor, and the owner when only one owner was resolved.
        let mut by_id: BTreeMap<String, Resolved> = BTreeMap::new();
        for ((id, owner), conf) in out {
            match by_id.get_mut(&id) {
                Some(r) => {
                    if r.owner != owner {
                        r.owner.clear();
                    }
                    r.conf = r.conf.max(conf);
                }
                None => {
                    by_id.insert(id.clone(), Resolved { id, conf, owner });
                }
            }
        }
        by_id.into_values().collect()
    }
}

/// The owner type of each definition starting at these 1-based lines of a Rust file (`""` for a
/// free function), or `None` when a line is unknown (0) and any owner may be meant.
pub(super) fn owner_types(text: &str, starts: &[usize]) -> Option<BTreeSet<String>> {
    if starts.contains(&0) {
        return None;
    }
    let blocks = rust_items::impl_blocks(text);
    Some(starts.iter().map(|s| rust_items::owner_at(&blocks, *s).map(|b| b.ty.clone()).unwrap_or_default()).collect())
}

/// One definition a call reaches.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Resolved {
    pub id: String,
    /// Weight factor: 1 resolved, `TYPE_MENTIONED` or `NAME_ONLY` by name.
    pub conf: f64,
    /// The type it was reached through, or empty.
    pub owner: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::calls::{call_sites, use_paths};
    use crate::json::Value;
    use std::path::PathBuf;

    fn unit(id: &str, file: &str, kind: &str, line: i64) -> Value {
        let name = id.split(':').nth(1).unwrap_or("");
        Value::object(vec![
            ("id".into(), Value::String(id.into())),
            ("file_path".into(), Value::String(file.into())),
            ("name".into(), Value::String(name.into())),
            ("unit_type".into(), Value::String(kind.into())),
            ("line_start".into(), Value::Int(line)),
        ])
    }

    fn project() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chisel-resolve-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let files: &[(&str, &str)] = &[
            ("src/atlas.rs", "pub struct GlyphAtlas { pages: Vec<u8> }\npub struct GlyphKey;\n\nimpl GlyphKey {\n    pub fn new() -> GlyphKey { GlyphKey }\n}\n\nimpl GlyphAtlas {\n    pub fn new() -> GlyphAtlas { GlyphAtlas { pages: Vec::new() } }\n    pub fn clear(&mut self) { self.pages.clear(); }\n    pub fn glyph(&mut self) -> u8 { 0 }\n}\n\nimpl Default for GlyphAtlas {\n    fn default() -> Self { GlyphAtlas::new() }\n}\n"),
            ("src/ui.rs", "use crate::atlas::GlyphAtlas;\npub struct Ui { atlas: GlyphAtlas }\n\nimpl Ui {\n    pub fn new() -> Ui { Ui { atlas: GlyphAtlas::new() } }\n    pub fn clear(&mut self) { self.atlas.clear(); }\n}\n\nimpl Default for Ui {\n    fn default() -> Self { Ui::new() }\n}\n"),
        ];
        for (p, t) in files {
            let path = dir.join(p);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, t).unwrap();
        }
        dir
    }

    #[test]
    fn calls_resolve_through_types_receivers_and_fields() {
        let dir = project();
        let units = vec![
            unit("src/atlas.rs:GlyphAtlas:struct", "src/atlas.rs", "struct", 1),
            unit("src/atlas.rs:GlyphKey:struct", "src/atlas.rs", "struct", 2),
            unit("src/atlas.rs:new:function", "src/atlas.rs", "function", 5),
            unit("src/atlas.rs:new:function", "src/atlas.rs", "function", 9),
            unit("src/atlas.rs:clear:function", "src/atlas.rs", "function", 10),
            unit("src/atlas.rs:glyph:function", "src/atlas.rs", "function", 11),
            unit("src/atlas.rs:default:function", "src/atlas.rs", "function", 15),
            unit("src/ui.rs:Ui:struct", "src/ui.rs", "struct", 2),
            unit("src/ui.rs:new:function", "src/ui.rs", "function", 5),
            unit("src/ui.rs:clear:function", "src/ui.rs", "function", 6),
            unit("src/ui.rs:default:function", "src/ui.rs", "function", 10),
        ];
        let id_file: HashMap<&str, &str> = units.iter().map(|u| (u.get("id").unwrap().as_str().unwrap(), u.get("file_path").unwrap().as_str().unwrap())).collect();
        let type_files = super::super::analyze::type_files(&units);
        let mut r = RustResolver::new(super::super::Sources::new(&dir, None), &units, type_files);
        let all = |name: &str| -> Vec<String> { units.iter().filter(|u| u.get("name").and_then(|v| v.as_str()) == Some(name)).map(|u| u.get("id").unwrap().as_str().unwrap().to_string()).collect() };
        let test = "use crate::atlas::GlyphAtlas;\nfn t() {\n    let mut a = GlyphAtlas::new();\n    a.glyph();\n    let cfg = Config { ..Default::default() };\n    a.clear();\n    let v: Vec<u8> = Vec::new();\n    v.len();\n}\n";
        let sites = call_sites(test);
        let uses = use_paths(test);
        let locals = rust_items::local_types(test);
        let idents = type_idents(test, &uses);
        let ctx = Ctx { file: "tests/t.rs", uses: &uses, locals: &locals, self_type: None, idents: &idents };
        let ids = |v: Vec<Resolved>| v.into_iter().map(|r| format!("{}@{}{}", r.id, r.conf, if r.owner.is_empty() { String::new() } else { format!("/{}", r.owner) })).collect::<Vec<_>>();
        // GlyphAtlas::new, not GlyphKey::new or Ui::new.
        assert_eq!(ids(r.resolve(&ctx, "new", sites.get("new"), &all("new"), &id_file)), vec!["src/atlas.rs:new:function@1/GlyphAtlas"], "the id is shared with GlyphKey::new; the owner says which");
        assert_eq!(ids(r.resolve(&ctx, "glyph", sites.get("glyph"), &all("glyph"), &id_file)), vec!["src/atlas.rs:glyph:function@1/GlyphAtlas"]);
        assert_eq!(ids(r.resolve(&ctx, "clear", sites.get("clear"), &all("clear"), &id_file)), vec!["src/atlas.rs:clear:function@1/GlyphAtlas"], "a typed receiver resolves even a std method name");
        assert!(r.resolve(&ctx, "default", sites.get("default"), &all("default"), &id_file).is_empty(), "Default::default names no project code");
        assert!(r.resolve(&ctx, "len", sites.get("len"), &all("len"), &id_file).is_empty());
        assert!(r.resolve(&ctx, "new", None, &all("new"), &id_file).is_empty(), "a name only in a string or comment is no call");

        // Inside `impl Ui`: `self.atlas.clear()` goes through the field's type.
        let body = "    pub fn clear(&mut self) { self.atlas.clear(); }";
        let sites = call_sites(body);
        let uses = use_paths("use crate::atlas::GlyphAtlas;");
        let locals = rust_items::local_types(body);
        let idents = type_idents(body, &uses);
        let ctx = Ctx { file: "src/ui.rs", uses: &uses, locals: &locals, self_type: Some("Ui"), idents: &idents };
        assert_eq!(ids(r.resolve(&ctx, "clear", sites.get("clear"), &all("clear"), &id_file)), vec!["src/atlas.rs:clear:function@1/GlyphAtlas"]);
        let sites = call_sites("fn default() -> Self { Self::new() }");
        assert_eq!(ids(r.resolve(&ctx, "new", sites.get("new"), &all("new"), &id_file)), vec!["src/ui.rs:new:function@1/Ui"]);

        // Unknown receivers: a rare name whose owner the caller names; a std method name never.
        let text = "fn t(x: Thing) { make().glyph(); make().clear(); }";
        let sites = call_sites(text);
        let empty = HashMap::new();
        let locals = rust_items::local_types(text);
        let idents: HashSet<String> = ["GlyphAtlas".to_string()].into_iter().collect();
        let ctx = Ctx { file: "tests/t.rs", uses: &empty, locals: &locals, self_type: None, idents: &idents };
        assert_eq!(ids(r.resolve(&ctx, "glyph", sites.get("glyph"), &all("glyph"), &id_file)), vec![format!("src/atlas.rs:glyph:function@{TYPE_MENTIONED}/GlyphAtlas")]);
        assert!(r.resolve(&ctx, "clear", sites.get("clear"), &all("clear"), &id_file).is_empty());
        let none: HashSet<String> = HashSet::new();
        let ctx = Ctx { file: "tests/t.rs", uses: &empty, locals: &locals, self_type: None, idents: &none };
        assert_eq!(ids(r.resolve(&ctx, "glyph", sites.get("glyph"), &all("glyph"), &id_file)), vec![format!("src/atlas.rs:glyph:function@{NAME_ONLY}/GlyphAtlas")]);
        let _ = fs::remove_dir_all(dir);
    }
}

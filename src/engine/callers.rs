//! Callers of changed functions, a few calls deep, for transitive test impact.
//!
//! Test edges link a test to the functions it calls itself. A change to `yaw_toward` reaches a
//! test that only calls `aligned`, which calls `yaw_toward`. `callers` walks that call graph
//! backwards from the changed functions, read from the source at query time (the snapshot
//! stores no code-to-code calls). Rust, C and C++ callers anywhere in the crates that can reach
//! the changed file are matched through the same call-site narrowing as test edges; in other
//! languages only callers in the changed file are followed.

use super::analyze::{declared_types, narrow_by_call_site, type_files};
use super::*;
use crate::extract::calls::{call_sites, use_paths, CallSite};

/// A function that calls a changed function, `hops` calls away (1: calls it directly).
pub(super) struct Caller {
    pub id: String,
    pub name: String,
    /// The changed function this caller reaches.
    pub reaches: String,
    /// The function it calls on the way (the changed one at hop 1).
    pub via: String,
    pub hops: usize,
}

/// A function with more callers than this is a utility: walking past it reaches most tests and
/// says little about this change.
/// `file` as a key that lives as long as the snapshot rows (the text cache is keyed by those).
fn file_key<'a>(funcs: &BTreeMap<&'a str, Vec<Unit<'a>>>, file: &str) -> &'a str {
    funcs.get_key_value(file).map(|(k, _)| *k).unwrap_or("")
}

pub(super) const MAX_CALLERS_PER_FUNCTION: usize = 40;

struct Unit<'a> {
    id: &'a str,
    name: &'a str,
    start: usize,
    end: usize,
}

/// Where a Rust function can be called from.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Reach {
    /// Private: its module (the file, and child modules under the file's directory).
    Module,
    /// `pub(crate)`, `pub(super)`, `pub(in ..)`: its crate.
    Crate,
    Anywhere,
}

/// The reach of the function defined at 1-based `line`: its own `pub`, or anywhere when it is a
/// method of a trait impl or a trait (callable wherever the trait is).
pub(super) fn rust_reach(text: &str, line: usize) -> Reach {
    let lines: Vec<&str> = text.lines().collect();
    let Some(def) = lines.get(line.saturating_sub(1)) else { return Reach::Anywhere };
    let mut sig = def.trim_start();
    while sig.starts_with("#[") {
        sig = sig.find(']').map(|e| sig[e + 1..].trim_start()).unwrap_or("");
    }
    let indent = def.len() - def.trim_start().len();
    if indent > 0 {
        // The enclosing item: the nearest line above with less indentation.
        for above in lines[..line.saturating_sub(1)].iter().rev() {
            let t = above.trim_start();
            if t.is_empty() || above.len() - t.len() >= indent || t.starts_with("//") || t.starts_with('#') || t.starts_with('}') {
                continue;
            }
            let item = t.trim_start_matches("pub ").trim_start_matches("unsafe ");
            if item.starts_with("trait ") || (item.starts_with("impl") && item.split_whitespace().any(|w| w == "for")) {
                return Reach::Anywhere;
            }
            break;
        }
    }
    if sig.starts_with("pub(") {
        Reach::Crate
    } else if sig.starts_with("pub ") {
        Reach::Anywhere
    } else {
        Reach::Module
    }
}

/// Files a private item in `file` is visible from: the file and its child modules.
fn in_module(file: &str, other: &str) -> bool {
    if file == other {
        return true;
    }
    let dir = match file.rsplit_once('/') {
        Some((d, "lib.rs" | "main.rs" | "mod.rs")) => d.to_string(),
        _ => file.trim_end_matches(".rs").to_string(),
    };
    other.starts_with(&format!("{dir}/"))
}

impl Engine {
    /// Callers of `seeds` (`(file, function name)`), up to `depth - 1` calls away.
    pub(super) fn callers(&self, seeds: &[(String, String)], depth: usize) -> Vec<Caller> {
        if depth < 2 || seeds.is_empty() {
            return Vec::new();
        }
        let code_units = self.store.rows("code_units");
        let mut id_file: HashMap<&str, &str> = HashMap::new();
        let mut by_name: HashMap<&str, Vec<String>> = HashMap::new();
        let mut funcs: BTreeMap<&str, Vec<Unit>> = BTreeMap::new();
        let mut starts: HashMap<&str, usize> = HashMap::new();
        for u in code_units {
            let (Some(id), Some(file), Some(name)) = (u.get("id").and_then(|v| v.as_str()), u.get("file_path").and_then(|v| v.as_str()), u.get("name").and_then(|v| v.as_str())) else { continue };
            id_file.insert(id, file);
            if is_test_unit_row(u) || !matches!(u.get("unit_type").and_then(|v| v.as_str()), Some("function" | "async_function" | "method")) {
                continue;
            }
            by_name.entry(name).or_default().push(id.to_string());
            let start = u.get("line_start").and_then(|v| v.as_i64()).unwrap_or(0).max(1) as usize;
            let end = u.get("line_end").and_then(|v| v.as_i64()).unwrap_or(0).max(0) as usize;
            funcs.entry(file).or_default().push(Unit { id, name, start, end });
            starts.insert(id, start);
        }
        let types = type_files(code_units);
        let mut crates = crate::cargo::Crates::new(&self.project_dir);
        let mut texts: HashMap<&str, String> = HashMap::new();
        let mut uses: HashMap<String, HashMap<String, Vec<Vec<String>>>> = HashMap::new();
        let mut declared: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut file_sites: HashMap<&str, HashMap<String, CallSite>> = HashMap::new();
        let mut verdicts: HashMap<String, bool> = HashMap::new();
        let project_dir = self.project_dir.clone();
        let mut out: Vec<Caller> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        // (file, name, ids of that function, the changed function it leads to)
        let mut frontier: Vec<(String, String, Vec<String>, String)> = seeds
            .iter()
            .map(|(f, n)| {
                let ids: Vec<String> = funcs.get(f.as_str()).into_iter().flatten().filter(|u| u.name == n).map(|u| u.id.to_string()).collect();
                (f.clone(), n.clone(), ids, n.clone())
            })
            .filter(|(_, _, ids, _)| !ids.is_empty())
            .collect();
        for (_, _, ids, _) in &frontier {
            seen.extend(ids.iter().cloned());
        }
        for hops in 1..depth {
            let mut next = Vec::new();
            for (file, name, ids, reaches) in &frontier {
                let lang = extract::detect_language(file);
                let narrowing = matches!(lang, Some("rust" | "c" | "cpp"));
                let candidates: Vec<&str> = if narrowing {
                    let own = crates.crate_of(file);
                    let text = texts.entry(file_key(&funcs, file)).or_insert_with(|| fs::read_to_string(project_dir.join(file.as_str())).unwrap_or_default());
                    // A private Rust function is called only from its module; pub(crate) only from its crate.
                    let reach = if lang == Some("rust") { ids.iter().filter_map(|i| starts.get(i.as_str())).map(|l| rust_reach(text, *l)).max_by_key(|r| *r as u8).unwrap_or(Reach::Anywhere) } else { Reach::Anywhere };
                    let mut c: Vec<&str> = funcs
                        .keys()
                        .copied()
                        .filter(|f| match reach {
                            Reach::Module => in_module(file, f),
                            Reach::Crate => crates.crate_of(f) == own,
                            Reach::Anywhere => extract::detect_language(f) == lang && crates.allows(f, file),
                        })
                        .collect();
                    c.sort_by_cached_key(|f| (*f != file.as_str(), crates.crate_of(f) != own));
                    c
                } else {
                    funcs.keys().copied().filter(|f| *f == file.as_str()).collect()
                };
                let mut found: Vec<(String, String)> = Vec::new();
                let matched = by_name.get(name.as_str()).cloned().unwrap_or_default();
                // Whether a call site in `cand` points at the changed function. In its own file a
                // call by that name is to it (`self.f()` or `x.f()` there has nothing to narrow by).
                let mut points_here = |cand: &str, site: &CallSite, text: &str| -> bool {
                    if !narrowing || cand == file.as_str() {
                        return true;
                    }
                    let key = format!("{cand}|{name}|{:?}|{}|{}", site.quals, site.method, site.free);
                    if let Some(hit) = verdicts.get(&key) {
                        return *hit;
                    }
                    let file_uses = uses.entry(cand.to_string()).or_insert_with(|| use_paths(text));
                    let mut declares = |f: &str, ty: &str| declared.entry(f.to_string()).or_insert_with(|| declared_types(&fs::read_to_string(project_dir.join(f)).unwrap_or_default())).contains(ty);
                    let kept = narrow_by_call_site(cand, name, matched.clone(), Some(site), file_uses, &id_file, &types, &mut declares);
                    let hit = kept.iter().any(|k| ids.contains(k));
                    verdicts.insert(key, hit);
                    hit
                };
                'files: for cand in candidates {
                    let text = texts.entry(cand).or_insert_with(|| fs::read_to_string(project_dir.join(cand)).unwrap_or_default());
                    if !text.contains(name.as_str()) {
                        continue;
                    }
                    // One pass over the file first: most files that mention a name never call it
                    // in a way that can reach this definition.
                    let whole = file_sites.entry(cand).or_insert_with(|| call_sites(text));
                    let Some(any) = whole.get(name.as_str()) else { continue };
                    if !points_here(cand, any, text) {
                        continue;
                    }
                    let lines: Vec<&str> = text.lines().collect();
                    for u in funcs.get(cand).into_iter().flatten() {
                        if ids.iter().any(|i| i == u.id) || u.end < u.start {
                            continue;
                        }
                        let body = lines.iter().skip(u.start - 1).take(u.end + 1 - u.start).copied().collect::<Vec<_>>().join("
");
                        if !body.contains(name.as_str()) {
                            continue;
                        }
                        let sites = call_sites(&body);
                        let Some(site) = sites.get(name.as_str()) else { continue };
                        if !points_here(cand, site, text) {
                            continue;
                        }
                        found.push((u.id.to_string(), u.name.to_string()));
                        if found.len() > MAX_CALLERS_PER_FUNCTION {
                            break 'files;
                        }
                    }
                }
                if found.len() > MAX_CALLERS_PER_FUNCTION {
                    continue;
                }
                for (id, caller) in found {
                    if !seen.insert(id.clone()) {
                        continue;
                    }
                    let cfile = id_file.get(id.as_str()).copied().unwrap_or("").to_string();
                    out.push(Caller { id: id.clone(), name: caller.clone(), reaches: reaches.clone(), via: name.clone(), hops });
                    next.push((cfile, caller, vec![id], reaches.clone()));
                }
            }
            frontier = next;
            if frontier.is_empty() {
                break;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_visibility_bounds_where_callers_can_be() {
        let text = "pub fn open() {}\nfn helper() {}\npub(crate) fn shared() {}\nimpl Spec {\n    fn private(&self) {}\n    pub fn names(&self) {}\n}\nimpl Add for Vec3 {\n    fn add(self, o: Vec3) -> Vec3 { o }\n}\npub trait Twice {\n    fn twice(&self) -> f32 { 0.0 }\n}\n";
        let at = |name: &str| text.lines().position(|l| l.contains(&format!("fn {name}("))).unwrap() + 1;
        assert_eq!(rust_reach(text, at("open")), Reach::Anywhere);
        assert_eq!(rust_reach(text, at("helper")), Reach::Module);
        assert_eq!(rust_reach(text, at("shared")), Reach::Crate);
        assert_eq!(rust_reach(text, at("private")), Reach::Module);
        assert_eq!(rust_reach(text, at("names")), Reach::Anywhere);
        // Trait methods have no `pub` but are callable wherever the trait is.
        assert_eq!(rust_reach(text, at("add")), Reach::Anywhere);
        assert_eq!(rust_reach(text, at("twice")), Reach::Anywhere);
        assert!(in_module("src/geom.rs", "src/geom/shapes.rs") && in_module("src/a/mod.rs", "src/a/b.rs"));
        assert!(!in_module("src/geom.rs", "src/solve.rs") && in_module("src/lib.rs", "src/solve.rs"));
    }
}

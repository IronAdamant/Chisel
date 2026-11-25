//! Where a cargo test lives when the snapshot does not know it yet.
//!
//! cargo prints a unit test by its module path (`tests::recursive::walks`) under the crate root
//! it was built from (`Running unittests src/lib.rs`). A test added since the last `analyze` has
//! no test unit, so its id is derived here the way the next `analyze` will name it:
//! `<file that defines it>:<function name>` (`src/tests/recursive.rs:walks`). The file is
//! found by following the module path from the crate root with Rust's own file rules
//! (`a.rs` or `a/mod.rs`); a module with no file of its own is inline in the last file found.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// The id the next `analyze` gives cargo test `name` (a module path), built from `root` (the
/// crate root cargo named) or, when the output named none, from every crate root in `crates`
/// (crate folders, `""` for the project root): `src/lib.rs`, `src/main.rs`, `tests/*.rs`.
/// `None` unless exactly one file defines the function.
pub(super) fn derived_id(project: &Path, name: &str, root: Option<&str>, crates: &BTreeSet<String>) -> Option<String> {
    let segments: Vec<&str> = name.split("::").collect();
    if segments.iter().any(|s| s.is_empty() || !s.chars().all(|c| c.is_alphanumeric() || c == '_')) {
        return None;
    }
    let (func, mods) = segments.split_last()?;
    let roots: Vec<String> = match root {
        Some(r) if r.ends_with(".rs") => vec![r.to_string()],
        Some(_) => return None,
        None => crate_roots(project, crates),
    };
    let mut found: BTreeSet<String> = BTreeSet::new();
    for r in &roots {
        let file = follow(project, r, mods);
        if defines_fn(project, &file, func) {
            found.insert(file);
        }
    }
    if found.len() != 1 {
        return None;
    }
    found.into_iter().next().map(|file| format!("{file}:{func}"))
}

fn crate_roots(project: &Path, crates: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for dir in crates {
        let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        for f in ["src/lib.rs", "src/main.rs"] {
            if project.join(format!("{prefix}{f}")).is_file() {
                out.push(format!("{prefix}{f}"));
            }
        }
        let mut tests: Vec<String> = fs::read_dir(project.join(format!("{prefix}tests")))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str().filter(|n| n.ends_with(".rs")).map(|n| format!("{prefix}tests/{n}")))
            .collect();
        tests.sort();
        out.extend(tests);
    }
    out
}

/// The file module path `mods` leads to from crate root `root`.
fn follow(project: &Path, root: &str, mods: &[&str]) -> String {
    let mut file = root.to_string();
    // A crate root, like a `mod.rs`, keeps its child modules beside it.
    let mut dir = parent(root).to_string();
    for m in mods {
        let flat = join(&dir, &format!("{m}.rs"));
        let nested = join(&dir, &format!("{m}/mod.rs"));
        if project.join(&flat).is_file() {
            file = flat;
        } else if project.join(&nested).is_file() {
            file = nested;
        } else {
            break;
        }
        dir = join(&dir, m);
    }
    file
}

fn parent(file: &str) -> &str {
    file.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

fn join(dir: &str, rest: &str) -> String {
    if dir.is_empty() {
        rest.to_string()
    } else {
        format!("{dir}/{rest}")
    }
}

/// `fn <name>` followed by `(` or `<` somewhere in the file.
fn defines_fn(project: &Path, file: &str, name: &str) -> bool {
    let Ok(text) = fs::read_to_string(project.join(file)) else { return false };
    let mut rest = text.as_str();
    while let Some(at) = rest.find("fn ") {
        let before_ok = at == 0 || !rest[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after = rest[at + 3..].trim_start();
        if before_ok {
            if let Some(tail) = after.strip_prefix(name) {
                if tail.trim_start().starts_with(['(', '<']) {
                    return true;
                }
            }
        }
        rest = &rest[at + 3..];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("chisel-rust-tests-{}-{nanos}", std::process::id()));
        for (path, text) in files {
            let p = dir.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        dir
    }

    fn crates(dirs: &[&str]) -> BTreeSet<String> {
        dirs.iter().map(|d| d.to_string()).collect()
    }

    #[test]
    fn module_path_names_the_file_that_defines_the_test() {
        let dir = tree(&[
            ("src/lib.rs", "#[cfg(test)]\nmod tests;\npub fn add() {}\n"),
            ("src/tests/mod.rs", "mod recursive;\n"),
            ("src/tests/recursive.rs", "#[test]\nfn walks() {}\n"),
            ("src/util.rs", "#[cfg(test)]\nmod tests {\n    #[test]\n    fn inline_one() {}\n}\n"),
        ]);
        let none = crates(&[""]);
        assert_eq!(derived_id(&dir, "tests::recursive::walks", Some("src/lib.rs"), &none).as_deref(), Some("src/tests/recursive.rs:walks"));
        // No `Running` line (stdout and stderr read separately): the crate roots are tried.
        assert_eq!(derived_id(&dir, "tests::recursive::walks", None, &none).as_deref(), Some("src/tests/recursive.rs:walks"));
        // An inline `mod tests` stays in its file.
        assert_eq!(derived_id(&dir, "util::tests::inline_one", None, &none).as_deref(), Some("src/util.rs:inline_one"));
        // Nothing defines it: no guess.
        assert_eq!(derived_id(&dir, "tests::recursive::missing", None, &none), None);
        assert_eq!(derived_id(&dir, "tests::recursive::walks", Some("Cargo.toml"), &none), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn integration_tests_and_workspace_crates() {
        let dir = tree(&[
            ("crates/a/src/lib.rs", "mod deep;\n"),
            ("crates/a/src/deep/mod.rs", "#[test]\nfn deep_test() {}\n"),
            ("crates/a/tests/it.rs", "mod sub;\n#[test]\nfn top_level() {}\n"),
            ("crates/a/tests/sub.rs", "#[test]\nfn in_sub() {}\n"),
            ("crates/b/tests/other.rs", "#[test]\nfn top_level() {}\n"),
        ]);
        let ws = crates(&["crates/a", "crates/b"]);
        assert_eq!(derived_id(&dir, "deep::deep_test", None, &ws).as_deref(), Some("crates/a/src/deep/mod.rs:deep_test"));
        assert_eq!(derived_id(&dir, "sub::in_sub", Some("crates/a/tests/it.rs"), &ws).as_deref(), Some("crates/a/tests/sub.rs:in_sub"));
        assert_eq!(derived_id(&dir, "top_level", Some("crates/a/tests/it.rs"), &ws).as_deref(), Some("crates/a/tests/it.rs:top_level"));
        // Two files define it and the output named neither: ambiguous, so no guess.
        assert_eq!(derived_id(&dir, "top_level", None, &ws), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fn_detection_needs_the_whole_name() {
        let dir = tree(&[("src/lib.rs", "fn walks_far() {}\nfn  walks <T>() {}\n")]);
        assert!(defines_fn(&dir, "src/lib.rs", "walks"));
        assert!(!defines_fn(&dir, "src/lib.rs", "walk"));
        let _ = fs::remove_dir_all(dir);
    }
}

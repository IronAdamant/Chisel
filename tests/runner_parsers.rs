//! `chisel::test_output` against output captured from real runs (cargo 1.99, pytest 8.2,
//! Jest 30, Vitest 4/5, node 22 `--test`). Project paths were replaced with C:\work\proj.
//! The go fixtures are written from `go test`'s documented formats: no Go toolchain was
//! available when they were made.

use chisel::test_output::{parse, Outcome, Parsed};
use std::path::Path;

fn run(fixture: &str) -> Parsed {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/runners").join(fixture)).unwrap();
    parse(&text, Some(Path::new("C:\\work\\proj")))
}

fn find<'a>(p: &'a Parsed, name: &str) -> &'a Outcome {
    p.outcomes.iter().find(|o| o.name == name).unwrap_or_else(|| panic!("{name} not in {:#?}", p.outcomes))
}

fn summary(p: &Parsed) -> Vec<(String, bool)> {
    p.outcomes.iter().map(|o| (o.name.clone(), o.passed)).collect()
}

#[test]
fn cargo_test() {
    let p = run("cargo.txt");
    assert!(p.complete && p.runners == ["cargo"]);
    assert_eq!(summary(&p), vec![("tests::adds".into(), true), ("tests::subtracts".into(), false), ("api_adds".into(), true), ("api_panics".into(), false)]);
    let sub = find(&p, "tests::subtracts");
    assert_eq!(sub.file.as_deref(), Some("src/lib.rs"));
    assert_eq!((sub.location.as_str(), sub.message.as_str()), ("src/lib.rs:10", "assertion `left == right` failed"));
    let api = find(&p, "api_panics");
    assert_eq!(api.file.as_deref(), Some("tests/api.rs"));
    assert_eq!((api.location.as_str(), api.message.as_str()), ("tests/api.rs:4", "index out of bounds: the len is 0 but the index is 3"));
    assert!(api.excerpt.len() <= 600 && api.excerpt.contains("index out of bounds"));
}

#[test]
fn pytest_default_quiet_and_verbose() {
    for fixture in ["pytest.txt", "pytest-q.txt", "pytest-v.txt"] {
        let p = run(fixture);
        assert!(p.complete, "{fixture}");
        let sub = find(&p, "test_sub");
        assert_eq!(sub.file.as_deref(), Some("tests/test_calc.py"), "{fixture}");
        assert!(!sub.passed);
        assert_eq!(sub.message, "AssertionError: assert 8 == 2", "{fixture}");
        assert_eq!(sub.location, "tests/test_calc.py:7", "{fixture}");
        let err = find(&p, "test_error");
        assert_eq!((err.message.as_str(), err.location.as_str()), ("KeyError: 'missing'", "tests/test_calc.py:10"), "{fixture}");
    }
    // Only -v lists the passing test; the default run shows its file as a progress line.
    assert!(find(&run("pytest-v.txt"), "test_add").passed);
    assert!(run("pytest.txt").passed_files.is_empty(), "a file with F marks is not a passing file");
}

#[test]
fn jest_text() {
    let p = run("jest.txt");
    assert!(p.complete && p.runners == ["jest"]);
    let sub = find(&p, "subtracts");
    assert_eq!(sub.file.as_deref(), Some("jtest/calc.test.js"));
    assert_eq!((sub.message.as_str(), sub.location.as_str()), ("expect(received).toBe(expected) // Object.is equality", "jtest/calc.test.js:4"));
    let throws = find(&p, "throws");
    assert_eq!((throws.message.as_str(), throws.location.as_str()), ("TypeError: Cannot read properties of null (reading 'x')", "jtest/calc.test.js:6"));
    assert!(run("jest-pass.txt").complete);
}

#[test]
fn vitest_text() {
    let p = run("vitest.txt");
    assert!(p.complete && p.runners == ["vitest"]);
    let sub = find(&p, "subtracts");
    assert_eq!(sub.file.as_deref(), Some("vtest/calc.test.mjs"));
    assert_eq!((sub.message.as_str(), sub.location.as_str()), ("AssertionError: expected 8 to be 2 // Object.is equality", "vtest/calc.test.mjs:5"));
    assert_eq!(find(&p, "throws").location, "vtest/calc.test.mjs:7");
}

#[test]
fn node_test_tap_and_spec() {
    let tap = run("node-tap.txt");
    assert!(tap.complete);
    assert_eq!(summary(&tap), vec![("adds".into(), true), ("subtracts".into(), false), ("throws".into(), false)]);
    let sub = find(&tap, "subtracts");
    assert_eq!(sub.file.as_deref(), Some("test/calc.test.js"));
    assert_eq!((sub.message.as_str(), sub.location.as_str()), ("AssertionError: Expected values to be strictly equal:", "test/calc.test.js:5"));
    assert_eq!(find(&tap, "throws").message, "TypeError: Cannot read properties of null (reading 'x')");

    let spec = run("node-spec.txt");
    assert!(spec.complete);
    assert_eq!(summary(&spec), vec![("adds".into(), true), ("subtracts".into(), false), ("throws".into(), false)]);
    let sub = find(&spec, "subtracts");
    assert_eq!((sub.file.as_deref(), sub.location.as_str()), (Some("test/calc.test.js"), "test/calc.test.js:5"));
    assert!(sub.message.starts_with("AssertionError [ERR_ASSERTION]: Expected values to be strictly equal"), "{}", sub.message);
}

#[test]
fn go_test_text_verbose_and_json() {
    for fixture in ["go.txt", "go-v.txt", "go-json.txt"] {
        let p = run(fixture);
        assert!(p.complete, "{fixture}");
        let sub = find(&p, "TestSub");
        assert!(!sub.passed);
        assert_eq!((sub.message.as_str(), sub.location.as_str()), ("Sub(5, 3) = 8, want 2", "calc_test.go:12"), "{fixture}");
        assert_eq!(sub.package.as_deref(), Some("example.com/calc"), "{fixture}");
    }
    assert_eq!(run("go.txt").passed_packages, vec!["example.com/calc/util"]);
    assert!(find(&run("go-v.txt"), "TestAdd").passed);
    assert!(find(&run("go-json.txt"), "TestAdd").passed);
}

#[test]
fn unrelated_output_yields_nothing() {
    let p = parse("Compiling chisel v0.20.0\nwarning: unused variable\nok\n", None);
    assert!(p.outcomes.is_empty() && !p.complete);
}

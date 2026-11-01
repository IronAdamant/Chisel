use chisel::extract::{extract_code_units, CodeUnit};

fn units(path: &str) -> Vec<(String, usize, usize, String)> {
    let text = std::fs::read_to_string(path).unwrap();
    extract_code_units(path, &text)
        .into_iter()
        .map(|u: CodeUnit| (u.unit_type, u.line_start, u.line_end, u.name))
        .collect()
}

#[test]
fn language_fixtures_match_python_0_15_1() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/languages");
    let expected = [
        (
            "csharp/BasicTests.cs",
            vec![
                ("class", 3, 21, "BasicTests"),
                ("function", 6, 9, "TestAddition"),
                ("function", 13, 16, "TestTheory"),
                ("function", 18, 20, "PlainHelper"),
            ],
        ),
        (
            "go/basic_test.go",
            vec![
                ("function", 5, 9, "TestAdd"),
                ("function", 11, 15, "TestSubtract"),
                ("function", 17, 19, "plainHelper"),
            ],
        ),
        (
            "java/BasicTest.java",
            vec![
                ("class", 5, 19, "BasicTest"),
                ("function", 7, 9, "testAddition"),
                ("function", 13, 15, "testParameterized"),
                ("function", 17, 18, "plainHelper"),
            ],
        ),
        (
            "rust/basic.rs",
            vec![
                ("function", 2, 4, "test_addition"),
                ("function", 7, 9, "test_async_fetch"),
                ("function", 12, 14, "test_with_fixture"),
                ("function", 16, 18, "plain_helper"),
            ],
        ),
        (
            "swift/BasicTests.swift",
            vec![
                ("function", 4, 6, "testAddition"),
                ("function", 9, 11, "testStrings"),
                ("function", 13, 15, "plainHelper"),
            ],
        ),
    ];
    for (rel, exp) in expected {
        let got = units(&format!("{root}/{rel}"));
        let exp: Vec<(String, usize, usize, String)> = exp
            .into_iter()
            .map(|(k, a, b, n)| (k.to_string(), a, b, n.to_string()))
            .collect();
        assert_eq!(got, exp, "{rel}");
    }
}

#[test]
fn dependency_tables_are_empty() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    for section in ["[dependencies]", "[dev-dependencies]"] {
        let Some(rest) = text.split(section).nth(1) else {
            continue;
        };
        let body = rest.split("\n[").next().unwrap_or("");
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            panic!("dependency table {section} is not empty: {line}");
        }
    }
    assert!(!std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs")).exists());
}

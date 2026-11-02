use super::*;

pub(super) fn extract_python(file_path: &str, content: &str) -> Vec<CodeUnit> {
    crate::syntax::python_units(file_path, content)
}

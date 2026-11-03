pub fn add_values(left: i32, right: i32) -> i32 {
    left + right
}

#[cfg(test)]
mod unit_checks {
    use super::add_values;

    #[test]
    fn checks_add_values() {
        assert_eq!(add_values(1, 1), 2);
    }
}

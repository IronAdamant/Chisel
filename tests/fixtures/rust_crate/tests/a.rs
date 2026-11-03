use sample_crate::a::add_values;

fn helper_not_a_test() -> i32 {
    1
}

#[test]
fn checks_add_values_from_crate_tests() {
    assert_eq!(add_values(2, 3), 5);
    let _ = helper_not_a_test();
}

#[tokio::test]
async fn checks_add_values_async() {
    assert_eq!(add_values(4, 1), 5);
}

#[rstest]
fn checks_add_values_cases() {
    assert_eq!(add_values(0, 0), 0);
}

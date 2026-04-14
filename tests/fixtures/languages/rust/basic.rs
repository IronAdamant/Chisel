#[test]
fn test_addition() {
    assert_eq!(2 + 2, 4);
}

#[tokio::test]
async fn test_async_fetch() {
    assert!(true);
}

#[rstest]
fn test_with_fixture() {
    assert!(true);
}

fn plain_helper() -> i32 {
    42
}

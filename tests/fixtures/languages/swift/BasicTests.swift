import Testing

@Test
func testAddition() {
    #expect(2 + 2 == 4)
}

@Test
func testStrings() {
    #expect("hello".isEmpty == false)
}

func plainHelper() -> Int {
    return 42
}

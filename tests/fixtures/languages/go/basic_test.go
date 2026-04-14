package basic

import "testing"

func TestAdd(t *testing.T) {
    if 2+2 != 4 {
        t.Fail()
    }
}

func TestSubtract(t *testing.T) {
    if 4-2 != 2 {
        t.Fail()
    }
}

func plainHelper() int {
    return 42
}

import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;

public class BasicTest {
    @Test
    public void testAddition() {
        assertEquals(4, 2 + 2);
    }

    @ParameterizedTest
    @ValueSource(strings = {"hello"})
    public void testParameterized(String s) {
        assertNotNull(s);
    }

    public void plainHelper() {
    }
}

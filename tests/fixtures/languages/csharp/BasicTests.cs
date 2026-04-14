using Xunit;

public class BasicTests
{
    [Fact]
    public void TestAddition()
    {
        Assert.Equal(4, 2 + 2);
    }

    [Theory]
    [InlineData(1)]
    public void TestTheory(int x)
    {
        Assert.True(x > 0);
    }

    public void PlainHelper()
    {
    }
}

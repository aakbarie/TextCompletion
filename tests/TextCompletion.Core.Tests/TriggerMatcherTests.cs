using TextCompletion.Core.Matching;
using TextCompletion.Core.Models;

namespace TextCompletion.Core.Tests;

public sealed class TriggerMatcherTests
{
    [Fact]
    public void MatchSuffix_ReturnsMatchingSnippet()
    {
        var snippet = Snippet.Create(";sig", "Akbar Esfahani");
        var matcher = new TriggerMatcher([snippet]);

        var result = matcher.MatchSuffix("hello ;sig");

        Assert.Equal(snippet, result);
    }

    [Fact]
    public void MatchSuffix_PrefersLongestMatchingTrigger()
    {
        var shortSnippet = Snippet.Create("sig", "short");
        var longSnippet = Snippet.Create(";sig", "long");
        var matcher = new TriggerMatcher([shortSnippet, longSnippet]);

        var result = matcher.MatchSuffix(";sig");

        Assert.Equal(longSnippet, result);
    }

    [Fact]
    public void DisabledSnippet_IsIgnored()
    {
        var snippet = new Snippet(Guid.NewGuid(), ";sig", "Akbar", false);
        var matcher = new TriggerMatcher([snippet]);

        Assert.Null(matcher.MatchSuffix(";sig"));
    }
}

using TextCompletion.Core.Models;

namespace TextCompletion.Core.Matching;

public sealed class TriggerMatcher
{
    private readonly Dictionary<string, Snippet> _snippets;
    private readonly int _maxTriggerLength;

    public TriggerMatcher(IEnumerable<Snippet> snippets, StringComparer? comparer = null)
    {
        comparer ??= StringComparer.Ordinal;
        _snippets = snippets
            .Where(s => s.IsEnabled && !string.IsNullOrEmpty(s.Trigger))
            .GroupBy(s => s.Trigger, comparer)
            .ToDictionary(g => g.Key, g => g.Last(), comparer);

        _maxTriggerLength = _snippets.Count == 0 ? 0 : _snippets.Keys.Max(k => k.Length);
    }

    public int MaxTriggerLength => _maxTriggerLength;

    public Snippet? MatchSuffix(string text)
    {
        if (string.IsNullOrEmpty(text) || _snippets.Count == 0)
            return null;

        var start = Math.Max(0, text.Length - _maxTriggerLength);
        var candidateWindow = text[start..];

        Snippet? bestMatch = null;
        foreach (var pair in _snippets)
        {
            if (!candidateWindow.EndsWith(pair.Key, StringComparison.Ordinal))
                continue;

            if (bestMatch is null || pair.Key.Length > bestMatch.Trigger.Length)
                bestMatch = pair.Value;
        }

        return bestMatch;
    }
}

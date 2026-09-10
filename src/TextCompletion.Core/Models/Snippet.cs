namespace TextCompletion.Core.Models;

public sealed record Snippet(
    Guid Id,
    string Trigger,
    string Expansion,
    bool IsEnabled = true)
{
    public static Snippet Create(string trigger, string expansion) =>
        new(Guid.NewGuid(), trigger, expansion, true);
}

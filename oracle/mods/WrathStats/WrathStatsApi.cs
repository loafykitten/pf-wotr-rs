using System.Collections.Concurrent;
using System.Threading;

namespace WrathStats;

// Public hooks for other mods (planned: Pathviewer) to mark work they do inside the game,
// so WrathStats can tell whether a hitch overlapped it. Callers that don't want a hard
// dependency can bind these by reflection: type "WrathStats.WrathStatsApi" in assembly
// "WrathStats". Safe to call from any thread; cheap; never throws.
//
//   WrathStatsApi.BeginWork("pathviewer");
//   try { ... } finally { WrathStatsApi.EndWork("pathviewer"); }
public static class WrathStatsApi
{
    private static readonly ConcurrentQueue<string> Notes = new ConcurrentQueue<string>();
    private static int _active;
    private static int _touched; // work began or ended since the recorder last looked
    private static string _lastSource;

    public static void BeginWork(string source)
    {
        _lastSource = source;
        Interlocked.Increment(ref _active);
        Volatile.Write(ref _touched, 1);
    }

    public static void EndWork(string source)
    {
        if (Interlocked.Decrement(ref _active) < 0) Interlocked.Exchange(ref _active, 0);
        Volatile.Write(ref _touched, 1);
    }

    // Adds a note to the current session's events.jsonl (e.g. a benchmark step or a
    // timing another mod measured). Any thread; allocates, so not for per-frame use.
    public static void Note(string text)
    {
        if (!string.IsNullOrEmpty(text)) Notes.Enqueue(text);
    }

    internal static bool TryTakeNote(out string text) => Notes.TryDequeue(out text);

    // Recorder, once per frame: was any external work active during this frame?
    internal static bool ConsumeFrame(out string source)
    {
        var touched = Interlocked.Exchange(ref _touched, 0) != 0;
        var busy = touched || Volatile.Read(ref _active) > 0;
        source = busy ? _lastSource : null;
        return busy;
    }
}

using System;
using System.Collections.Generic;
using System.Linq;
using System.Diagnostics;
using System.Reflection;
using System.Threading;
using HarmonyLib;
using Pathfinding;

namespace WrathOracle;

// Pathfinding results in turn-based windows: observed in a recording, reproduced in a replay.
//
// A* computes paths on a worker thread and hands the finished ones to the game in
// AstarPath.Update (PathReturnQueue.ReturnPaths), so a path requested in one frame is called
// back in the next frame if the thread was quick and one frame later if it was not: wall-clock
// scheduling that moves the tick at which a unit's movement starts. A recording changes nothing:
// it notes, for every pinned path requested through AstarPath.StartPath (keyed "<requester>#<n>",
// see RequesterKey), its end point ("path_start"), the tick and the ordinal of the ReturnPaths call
// within the tick that delivered it ("path_ret"), and the number of pinned requests (the end
// record's paths_tracked).
//
// A replay delivers each path at exactly that call. A finished path never enters the return queue
// on its own: PathReturnQueue.Enqueue (the worker's hand-over) holds every tracked path, and before
// each ReturnPaths the paths due at this call are delivered by themselves, in the order they finished
// (DeliverNow, as the drain delivers each path), so nothing that arrives during the drain can be
// delivered early and the native drain keeps its own time slice for untracked paths. If a due path hasn't finished,
// the call waits for it (a bounded wait, never while the graph work-item lock is held, which blocks
// the workers, and never without worker threads). Any departure invalidates the replay: a path due
// but not finished in time, a request the recording didn't make (or to another end point) or one it
// made that the replay didn't, a recorded delivery that didn't happen by the window's end (of a strict
// request, or of a lenient one the replay matched; lenient requests it didn't match are native). Paths are
// identified by their pathID (unique per request), since A* pools Path objects. Held paths go back
// to the game when the window ends. The callbacks are the game's own.
internal static class PathSync
{
    // Held: handed over by the worker and held; Released: put into the return queue at its due call;
    // Done: its callback ran (ReturnPath), which is what counts as delivered.
    private sealed class Track { public string K; public int ReqTick; public Path P; public ushort Id; public bool Strict, Owed, Held, Released, Done; }

    private static readonly List<Track> Outstanding = new List<Track>();
    // Matched lenient requests with a recorded delivery (CheckComplete requires each delivered).
    private static readonly List<Track> Owed = new List<Track>();
    // Recorded deliveries by request ordinal: tick, call within the tick, and the request's tick (-1 in
    // recordings before path_start; then the delivery tick moves with the gate offset instead).
    private struct Ret { public int Tick, Call, Req; }
    private static readonly Dictionary<string, Ret> Tape = new Dictionary<string, Ret>();
    // Recorded requests' end points by ordinal, which a replay's requests must match (else the ordinals
    // name different requests and delivery scheduling is meaningless).
    private static readonly Dictionary<string, UnityEngine.Vector3> StartTape = new Dictionary<string, UnityEngine.Vector3>();
    private static readonly HashSet<string> StartKeys = new HashSet<string>();
    private static readonly HashSet<string> StrictKeys = new HashSet<string>();
    private const float EndTolerance = 0.25f;
    private static readonly List<Track> HeldOrder = new List<Track>();
    private static readonly Dictionary<string, int> PerRequester = new Dictionary<string, int>();
    private static int _count, _callTick = -1, _callIdx, _recordedTracked = -1, _delivered;
    public static long Waited, Timeouts, Tracked;
    private const int LimitMs = 100;

    private static readonly FieldInfo QueueField = AccessTools.Field(AccessTools.TypeByName("Pathfinding.PathReturnQueue"), "pathReturnQueue");
    private static readonly FieldInfo LockField = AccessTools.Field(typeof(AstarPath), "workItemLock");

    // The return queue the workers hand over to (captured at the first hand-over).
    private static object _owner;
    private static readonly MethodInfo OwnerEnqueue = AccessTools.Method(AccessTools.TypeByName("Pathfinding.PathReturnQueue"), "Enqueue");

    // Hand-overs are intercepted only between ResetWindow and EndWindow; the flag changes under the
    // same lock EnqueuePrefix decides in, so a late hand-over either lands in the held list before the
    // drain or passes through to the game.
    private static bool _intercept;

    public static void ResetWindow()
    {
        EndWindow();
        lock (Outstanding)
        {
            Outstanding.Clear();
            Owed.Clear();
            _intercept = true;
        }
        lock (Keys) Keys.Clear();
        Tape.Clear();
        StartTape.Clear();
        StartKeys.Clear();
        StrictKeys.Clear();
        PerRequester.Clear();
        _count = 0;
        _callTick = -1;
        _recordedTracked = -1;
        _delivered = 0;
        _offThread = 0;
        Waited = Timeouts = Tracked = 0;
    }

    public static void AddTape(string k, int tick, int call, int req)
    {
        if (Tape.ContainsKey(k)) Sched.Invalidate($"recording delivers path {k} twice");
        Tape[k] = new Ret { Tick = tick, Call = call, Req = req };
    }

    public static void AddStartTape(string k, UnityEngine.Vector3? end, bool strict)
    {
        StartKeys.Add(k);
        if (strict) StrictKeys.Add(k);
        if (end.HasValue) StartTape[k] = end.Value;
    }

    // An unidentified request's callbacks (target type and method), noted so its source can be found.
    private static string CallbackNames(Path p)
    {
        try
        {
            var cb = p.callback;
            if (cb == null) return "none";
            var names = new List<string>();
            foreach (var d in cb.GetInvocationList()) names.Add((d.Target?.GetType().FullName ?? "static") + "." + d.Method?.Name);
            return string.Join(";", names);
        }
        catch (Exception) { return "?"; }
    }

    // Who asked: the unit whose Seeker the path calls back (Seeker.StartPath adds its own delegate).
    // Requests are keyed by requester and that requester's own count, and every one is pinned: a
    // unit's path delivery moves it, and a unit outside the fight now may join it later (poc5-tb-35:
    // the archer's pre-combat path came a tick apart under load). Requests of units in combat or of
    // the player's faction, and of anything that isn't a unit (key "?"), are strict: any departure
    // invalidates. Other units (a roaming animal: its roaming draws are audited T1) are lenient: a
    // request the recording doesn't have (by key and end point) is delivered natively, unchecked.
    internal static string RequesterKey(Path p, out bool strict)
    {
        strict = true;
        try
        {
            var cb = p.callback;
            if (cb != null)
                foreach (var d in cb.GetInvocationList())
                    if (d.Target is Seeker sk && sk)
                    {
                        // UnitMovementAgent.Init puts the Seeker on a "_navigation" child of the unit.
                        var u = sk.GetComponentInParent<Kingmaker.View.UnitEntityView>()?.EntityData
                                ?? sk.GetComponentInParent<Kingmaker.View.UnitMovementAgent>()?.Unit?.EntityData;
                        if (u == null) return "?";
                        strict = u.IsInCombat || u.IsPlayerFaction;
                        return u.UniqueId;
                    }
        }
        catch (Exception e) { Health.Failed("path requester", e); }
        return "?";
    }

    internal static UnityEngine.Vector3? EndOf(Path p) => p is ABPath ab ? ab.originalEndPoint : (UnityEngine.Vector3?)null;

    public static void SetRecordedTracked(int n) => _recordedTracked = n;

    // The window closed or aborted: paths still held go back to the return queue (in the order they
    // finished), so their callbacks and claim releases run as the game expects.
    public static void EndWindow()
    {
        List<Track> held;
        lock (Outstanding)
        {
            _intercept = false;
            held = new List<Track>(HeldOrder);
            HeldOrder.Clear();
            foreach (var t in held) t.Done = true; // Find skips them, so Enqueue passes them through
        }
        if (held.Count == 0 || _owner == null) return;
        try { foreach (var t in held) OwnerEnqueue.Invoke(_owner, new object[] { t.P }); }
        catch (Exception e) { Health.Failed("path sync release", e); }
    }

    // Every request's key in this window, by the Path object and its pathID (A* pools Path objects: a pooled one
    // reused for a new request has another pathID).
    private static readonly Dictionary<Path, (ushort id, string key)> Keys = new Dictionary<Path, (ushort, string)>(new RefEq());

    private sealed class RefEq : IEqualityComparer<Path>
    {
        public bool Equals(Path a, Path b) => ReferenceEquals(a, b);
        public int GetHashCode(Path p) => System.Runtime.CompilerServices.RuntimeHelpers.GetHashCode(p);
    }

    // The PathSync key of the request `p` currently is, if it was made in this window.
    internal static string KeyOf(Path p)
    {
        if (p == null) return null;
        lock (Keys) return Keys.TryGetValue(p, out var e) && e.id == p.pathID ? e.key : null;
    }

    // The track of a live request (pathID guards against a pooled Path reused for a new request).
    private static Track Find(Path p)
    {
        foreach (var t in Outstanding)
            if (ReferenceEquals(t.P, p) && t.Id == p.pathID && !t.Done) return t;
        return null;
    }

    // A prefix: StartPath publishes the request to the workers (queue.Push) before it returns, so a
    // worker could hand it back before a postfix ran. A request StartPath rejects is never delivered,
    // in a recording and its replay alike.
    public static void StartPathPrefix(Path path)
    {
        if (!Sched.Window) PrePathSync.OnStart(path); // before a turn-based window (PrePathSync.cs)
        if (!Sched.Window || !Trace.Active || path == null) return;
        // A request from another thread (its requester can't be read off the main thread): noted, and it
        // invalidates a replay (Sched is main-thread only, so at the next ReturnPaths or the close).
        if (!Trace.OnMainThread) { Interlocked.Increment(ref _offThread); return; }
        var who = RequesterKey(path, out var strict);
        string k;
        lock (Outstanding)
        {
            PerRequester.TryGetValue(who, out var n);
            PerRequester[who] = n + 1;
            k = who + "#" + n;
        }
        lock (Keys) Keys[path] = (path.pathID, k);
        var end = EndOf(path);
        var cbs = who == "?" ? CallbackNames(path) : null;
        Sched.Obs("path_start", j => { j.Prop("path", k); j.Prop("strict", strict); if (end.HasValue) j.Prop("e", end.Value); if (cbs != null) j.Prop("cb", cbs); });
        if (Sched.Replay)
        {
            var known = StartKeys.Contains(k) && (!StartTape.TryGetValue(k, out var want0) || (end.HasValue && (end.Value - want0).magnitude <= EndTolerance));
            if (!known)
            {
                if (strict || StrictKeys.Contains(k))
                {
                    if (!StartKeys.Contains(k)) Sched.Invalidate($"path request {k} at tick {Trace.Tick}: the recording made no such request");
                    else Sched.Invalidate($"path request {k} at tick {Trace.Tick} goes to {end?.ToString("F3") ?? "?"}; the recording's went to {StartTape[k]:F3}");
                }
                else Sched.Obs("path_native", j => j.Prop("path", k)); // lenient: delivered as it comes
                if (!strict) return;
            }
            else if (strict != StrictKeys.Contains(k))
                Sched.Invalidate($"path request {k} at tick {Trace.Tick} is {(strict ? "strict" : "lenient")}; the recording's was not");
        }
        lock (Outstanding)
        {
            if (strict) _count++;
            // A replay owes every delivery its recording made of a request it matched, lenient ones included
            // (a matched lenient request is held and released like a strict one); unmatched lenient requests
            // returned above and are delivered natively.
            var owed = Sched.Replay && !strict && Tape.ContainsKey(k);
            var t = new Track { K = k, P = path, Id = path.pathID, ReqTick = Trace.Tick, Strict = strict, Owed = owed };
            Outstanding.Add(t);
            if (owed) Owed.Add(t);
        }
        if (strict) Tracked++;
    }

    private static int _offThread;

    private static void CheckOffThread()
    {
        if (Interlocked.Exchange(ref _offThread, 0) == 0) return;
        Sched.Obs("path_off_thread");
        if (Sched.Replay) Sched.Invalidate("a path was requested off the main thread");
    }

    // The worker's hand-over of a finished path (on an A* worker thread, so not Sched.On, which is
    // main-thread only). A replay holds tracked paths until they are due.
    // Hands a held path back to the queue its worker gave it to (a window's end, PrePathSync's stop).
    internal static void HandBack(object owner, Path p) => OwnerEnqueue.Invoke(owner, new object[] { p });

    public static bool EnqueuePrefix(object __instance, Path path)
    {
        if (!Sched.Window && PrePathSync.Active) return PrePathSync.OnEnqueue(__instance, path);
        if (!Sched.Window || !Trace.Active || !Sched.Replay || path == null) return true;
        lock (Outstanding)
        {
            if (!_intercept) return true;
            _owner = __instance;
            var t = Find(path);
            if (t == null) return true; // requested before the window
            t.Held = true;
            HeldOrder.Add(t);
            return false;
        }
    }

    // Can the workers make progress? Not while the graph work-item lock is held, and not without them.
    internal static bool WorkersCanRun()
    {
        try
        {
            var a = AstarPath.active;
            if (a == null || !a.IsUsingMultithreading) return false;
            var l = LockField?.GetValue(a);
            if (l == null) return true;
            var held = l.GetType().GetProperty("Held");
            return !(held != null && (bool)held.GetValue(l, null));
        }
        catch (Exception) { return false; }
    }

    // Due: the recorded number of ticks after this replay's own request (a request a tick later, as an
    // animation event can be, moves its delivery with it), at the recorded call within that tick.
    private static bool DueNow(Track t, out int dueTick, out int dueCall)
    {
        dueTick = dueCall = 0;
        if (!Tape.TryGetValue(t.K, out var d)) return false;
        dueTick = d.Req >= 0 && t.ReqTick >= 0 ? t.ReqTick + (d.Tick - d.Req) : d.Tick + Sched.Offset;
        dueCall = d.Call;
        return Trace.Tick > dueTick || (Trace.Tick == dueTick && _callIdx >= dueCall);
    }

    public static void ReturnPathsPrefix(object __instance, ref bool timeSlice)
    {
        if (!Sched.On)
        {
            if (!Sched.Window && PrePathSync.Active)
            {
                try { PrePathSync.OnReturnPaths(__instance, QueueField?.GetValue(__instance) as Queue<Path>, ref timeSlice); }
                catch (Exception e) { Health.Failed("pre-window path sync", e); }
            }
            return;
        }
        try
        {
            if (Trace.Tick != _callTick) { _callTick = Trace.Tick; _callIdx = 0; }
            _callIdx++;
            CheckOffThread();
            if (!Sched.Replay)
            {
                lock (Outstanding) Outstanding.RemoveAll(t => t.Done);
                return;
            }
            List<Track> due;
            lock (Outstanding)
            {
                due = new List<Track>();
                foreach (var t in Outstanding)
                    if (!t.Released && DueNow(t, out _, out _)) due.Add(t);
            }
            if (due.Count == 0) return;
            // Wait for the due paths that haven't been handed over yet.
            var canWait = WorkersCanRun();
            var sw = Stopwatch.StartNew();
            foreach (var t in due)
            {
                bool held;
                lock (Outstanding) held = t.Held;
                if (held) continue;
                Waited++;
                while (true)
                {
                    lock (Outstanding) held = t.Held;
                    if (held) break;
                    if (!canWait || sw.ElapsedMilliseconds > LimitMs)
                    {
                        Timeouts++;
                        var k = t.K;
                        Sched.Obs("astar_timeout", j => { j.Prop("path", k); j.Prop("state", t.P.PipelineState.ToString()); });
                        Sched.Invalidate($"path {k} due at tick {Trace.Tick} call {_callIdx} was not finished ({(canWait ? "timed out" : "workers blocked")})");
                        break;
                    }
                    Thread.Sleep(0);
                }
            }
            List<Track> release;
            lock (Outstanding)
            {
                // In the order they finished, as the original queue would hold them.
                release = HeldOrder.FindAll(t => due.Contains(t));
                foreach (var t in release)
                {
                    HeldOrder.Remove(t);
                    t.Released = true;
                }
            }
            // The recording tracked every request of its window, so it has no order between tracked and
            // untracked deliveries of one call; a replay call that has both can't follow it and leaves it.
            var queued = QueuedCount(__instance);
            if (release.Count > 0 && queued > 0)
                Sched.Invalidate($"path {release[0].K} due at tick {Trace.Tick} call {_callIdx} shares the call with {queued} untracked path(s) already queued (order not recorded)");
            foreach (var t in release)
            {
                var why = DeliverNow(__instance, t.P);
                if (why != null) Sched.Invalidate($"path {t.K}: {why}");
            }
        }
        catch (Exception e) { Trace.HookFailed("path sync", e); }
    }

    // A due path is delivered by itself, as PathReturnQueue.ReturnPaths delivers each path (its callback, the
    // Returned state, the queue's silent claim released), before the native drain of this call runs, which then
    // keeps its own wall-clock time slice for the paths nobody tracks. Returns why it failed, or null.
    private static readonly MethodInfo ReturnPathMethod = AccessTools.Method(typeof(Path), "Pathfinding.IPathInternals.ReturnPath");
    private static readonly MethodInfo AdvanceStateMethod = AccessTools.Method(typeof(Path), "Pathfinding.IPathInternals.AdvanceState");
    private static readonly FieldInfo ClaimedBy = AccessTools.Field(AccessTools.TypeByName("Pathfinding.PathReturnQueue"), "pathsClaimedSilentlyBy");

    internal static int QueuedCount(object returnQueue)
    {
        var q = QueueField?.GetValue(returnQueue) as Queue<Path>;
        if (q == null) return 0;
        lock (q) return q.Count;
    }

    internal static string DeliverNow(object returnQueue, Path p)
    {
        try
        {
            ReturnPathMethod.Invoke(p, null);
            AdvanceStateMethod.Invoke(p, new object[] { PathState.Returned });
            p.Release(ClaimedBy.GetValue(returnQueue), true);
            return null;
        }
        catch (TargetInvocationException e) { return "its delivery threw " + e.InnerException?.GetType().Name + ": " + e.InnerException?.Message; }
        catch (Exception e) { return "its delivery failed: " + e.Message; }
    }

    // A recording notes each delivery as it happens (the path's callback is about to run, its
    // pathID still the request's: a later callback in the same drain may reuse a pooled path).
    public static void ReturnPathPrefix(Path __instance)
    {
        if (!Sched.On)
        {
            if (!Sched.Window && Trace.OnMainThread) PrePathSync.OnReturn(__instance);
            return;
        }
        try
        {
            if (Sched.Replay)
            {
                // A replay counts and checks each delivery as its callback runs (a callback that throws
                // ends the native drain; the paths left in the queue come out at a later call, late).
                Track d;
                lock (Outstanding)
                {
                    d = Find(__instance);
                    if (d == null || !d.Released) return;
                    d.Done = true;
                    if (d.Strict) _delivered++;
                }
                DueNow(d, out var dueTick, out var dueCall);
                if (Trace.Tick != dueTick || _callIdx != dueCall)
                    Sched.Invalidate($"path {d.K} delivered at tick {Trace.Tick} call {_callIdx}; recording at tick {dueTick} call {dueCall}");
                return;
            }
            string k;
            int req;
            lock (Outstanding)
            {
                var t = Find(__instance);
                if (t == null) return;
                t.Done = true;
                k = t.K;
                req = t.ReqTick;
            }
            var c = _callIdx;
            Sched.Obs("path_ret", j => { j.Prop("path", k); j.Prop("c", c); j.Prop("req", req); });
        }
        catch (Exception e) { Trace.HookFailed("path sync return", e); }
    }

    // Before the window closes: every recorded request and delivery happened.
    public static void CheckComplete()
    {
        if (!Sched.On) return;
        CheckOffThread();
        if (!Sched.Replay) return;
        if (_recordedTracked >= 0 && _count != _recordedTracked)
            Sched.Invalidate($"replay made {_count} pinned path requests; the recording made {_recordedTracked}");
        else
        {
            var strictDelivered = Tape.Keys.Count(StrictKeys.Contains);
            if (_delivered != strictDelivered)
                Sched.Invalidate($"replay delivered {_delivered} strict paths; the recording delivered {strictDelivered}");
        }
        if (Sched.Invalid != null) return;
        Track missing = null;
        lock (Outstanding) missing = Owed.FirstOrDefault(t => !t.Done);
        if (missing != null && Tape.TryGetValue(missing.K, out var r))
            Sched.Invalidate($"lenient path {missing.K} (matched) was delivered at tick {r.Tick} in the recording, but not in the replay's window");
    }

    public static void Install(Harmony h)
    {
        try
        {
            var rq = AccessTools.TypeByName("Pathfinding.PathReturnQueue");
            h.Patch(AccessTools.Method(typeof(AstarPath), nameof(AstarPath.StartPath), new[] { typeof(Path), typeof(bool) }), prefix: new HarmonyMethod(typeof(PathSync), nameof(StartPathPrefix)));
            h.Patch(AccessTools.Method(rq, "Enqueue"), prefix: new HarmonyMethod(typeof(PathSync), nameof(EnqueuePrefix)));
            h.Patch(AccessTools.Method(rq, "ReturnPaths"), prefix: new HarmonyMethod(typeof(PathSync), nameof(ReturnPathsPrefix)));
            h.Patch(AccessTools.Method(typeof(Path), "Pathfinding.IPathInternals.ReturnPath"), prefix: new HarmonyMethod(typeof(PathSync), nameof(ReturnPathPrefix)));
        }
        catch (Exception e) { Health.Failed("patch path sync", e); }
    }
}

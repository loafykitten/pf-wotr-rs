using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using Newtonsoft.Json.Linq;
using Pathfinding;
using UnityEngine;

namespace WrathOracle;

// Path deliveries before a turn-based window, from the reload's first idle frame to the window's start:
// observed in a recording, reproduced in its replays (PathSync does the same inside the window).
//
// A unit that moves before the window carries the phase of its move into it. [M] The roaming monitor
// lizard (5052) asks for a path 93 frames before tick 0 and gets it back in the same frame when the A*
// worker is quick (every run of poc5-tb-48); under load it can come a frame later, and the lizard is a
// tick behind on its whole move (poc5-tb-46 replay 1: its UnitMoveTo ended at tick 10, not 9, a T0
// difference). The frames from the first idle frame to the window are the same in every run (Runner.Load
// waits 60, the plan's wait is fixed; checked through the anchor-to-window count, PhysicsPhase).
//
// Recording: from the first idle frame, every path request on the main thread is keyed
// "<requester>#<n>" (PathSync.RequesterKey; n counts that requester's requests since the idle frame) and
// its request and delivery are noted by frame and ReturnPaths call; at the window's start they go into
// the trace as "pre_path" observations (dt: frames before tick 0).
// Replay (turn-based, tape read at "prepare"): a request the recording made (same key, end point within
// 0.25 m) is held when the worker hands it over and released into the return queue at the recorded number
// of frames after this replay's own request, at the recorded call, waiting for the worker if it isn't done
// (bounded, never while the graph lock blocks the workers), and delivered by itself before the native drain
// (PathSync.DeliverNow), which keeps its time slice; paths due at one call go in the recording's order.
// Requests the recording didn't make are delivered natively. At the window's start, any recorded delivery
// that didn't happen at its frame and call invalidates the replay. Critters' requests (PathSync.CritterOf)
// are presentation: neither noted nor held, delivered natively, and not counted as untracked paths of a call.
internal static class PrePathSync
{
    private sealed class Track { public string K; public int ReqFrame; public Path P; public ushort Id; public bool Held, Released, Done; }
    // Seq: the delivery's place in the recording (its "ret" note's order).
    private struct Rec { public int Req, Ret, Call, Seq; public Vector3? End; }

    private static readonly Dictionary<string, Rec> Tape = new Dictionary<string, Rec>();
    // _armed: a tape was loaded at "prepare"; _replay: this interval reproduces it (set at its idle frame).
    private static bool _armed, _replay;
    private static volatile bool _active;
    private static readonly List<Track> Tracks = new List<Track>();
    private static readonly List<Track> HeldOrder = new List<Track>();
    private static readonly Dictionary<string, int> PerRequester = new Dictionary<string, int>();
    private static readonly List<(int frame, string ev, string key, int call, Vector3? end)> Notes = new List<(int, string, string, int, Vector3?)>();
    private static int _callFrame = -1, _callIdx;
    private static string _problem;
    private static object _owner;
    public static long Held, Waits;
    public static bool Active => _active;

    // Driver "prepare": the recording's pre-window deliveries for a turn-based replay (else none: observe only).
    public static void LoadTape(string recordTrace)
    {
        Deactivate(); // an interval still observing (the previous load) ends here; its held paths go back
        Tape.Clear();
        _armed = false;
        _problem = null;
        if (recordTrace == null || !System.IO.File.Exists(recordTrace)) return;
        var starts = new Dictionary<string, (int dt, Vector3? end)>();
        var rets = new Dictionary<string, (int dt, int call, int seq)>();
        using (var f = System.IO.File.OpenRead(recordTrace))
        using (var gz = new System.IO.Compression.GZipStream(f, System.IO.Compression.CompressionMode.Decompress))
        using (var r = new System.IO.StreamReader(gz, System.Text.Encoding.UTF8))
        {
            string line;
            while ((line = r.ReadLine()) != null)
            {
                if (line.IndexOf("\"ev\":\"pre_path\"", StringComparison.Ordinal) < 0) continue;
                var o = JObject.Parse(line);
                var key = (string)o["path"];
                if (key == null) continue;
                var what = (string)o["what"];
                if (what == "start") starts[key] = ((int)o["dt"], o["e"] is JArray e && e.Count == 3 ? new Vector3((float)e[0], (float)e[1], (float)e[2]) : (Vector3?)null);
                else if (what == "ret") rets[key] = ((int)o["dt"], (int?)o["c"] ?? 1, rets.Count);
            }
        }
        foreach (var kv in starts)
            if (rets.TryGetValue(kv.Key, out var ret)) Tape[kv.Key] = new Rec { Req = kv.Value.dt, Ret = ret.dt, Call = ret.call, Seq = ret.seq, End = kv.Value.end };
        _armed = true;
    }

    // "prepare" without a turn-based replay: observe only.
    public static void Disarm()
    {
        Deactivate();
        Tape.Clear();
        _armed = false;
        _problem = null;
    }

    // Ends the interval: interception off (under the lock the worker hand-over decides in), then every held
    // path goes back to the queue its worker gave it to, so its callback and claim release run.
    private static void Deactivate()
    {
        List<Track> held;
        lock (Tracks)
        {
            _active = false;
            held = new List<Track>(HeldOrder);
            HeldOrder.Clear();
            foreach (var t in held) t.Done = true;
        }
        if (held.Count > 0 && _owner != null)
        {
            try { foreach (var t in held) PathSync.HandBack(_owner, t.P); }
            catch (Exception e) { Health.Failed("pre-window path release", e); }
        }
    }

    // Runner.Load, the reload's first idle frame (again after a loading restart: the earlier interval ends,
    // and a replay that already pinned a delivery in it is invalid, since the restart moved what followed).
    public static void Start()
    {
        bool pinned;
        lock (Tracks) pinned = _active && _replay && Tracks.Any(t => t.Released);
        Deactivate();
        if (pinned) Problem("the reload restarted after pre-window path deliveries were pinned");
        lock (Tracks)
        {
            Tracks.Clear();
            PerRequester.Clear();
            Notes.Clear();
            _replay = _armed;
            _active = true;
        }
        _callFrame = -1;
        Held = Waits = 0;
    }

    // Fixture.Begin, before the window's own path sync starts: held paths go back to the game, the notes go
    // into the trace (after it started: Flush), and a replay's missing deliveries are its invalid reason.
    public static string Stop()
    {
        if (!_active) { _armed = false; Tape.Clear(); return null; }
        Deactivate();
        if (_replay && _problem == null)
        {
            var delivered = new HashSet<string>();
            lock (Tracks) foreach (var t in Tracks) if (t.Done && t.Released) delivered.Add(t.K);
            var missing = Tape.Keys.Where(k => !delivered.Contains(k)).OrderBy(k => k, StringComparer.Ordinal).ToList();
            if (missing.Count > 0) _problem = $"pre-window path {missing[0]} (and {missing.Count - 1} more) was delivered in the recording but not in the replay";
        }
        var p = _problem;
        _replay = _armed = false;
        _problem = null;
        Tape.Clear();
        return p;
    }

    // After Trace.Begin: the pre-window notes as observations (dt relative to the window's first frame).
    public static void Flush(int frame0)
    {
        List<(int frame, string ev, string key, int call, Vector3? end)> list;
        lock (Tracks) { list = new List<(int, string, string, int, Vector3?)>(Notes); Notes.Clear(); }
        foreach (var (frame, ev, key, call, end) in list)
            Sched.Obs("pre_path", j => { j.Prop("dt", frame - frame0); j.Prop("what", ev); j.Prop("path", key); if (ev == "ret") j.Prop("c", call); if (end.HasValue) j.Prop("e", end.Value); });
    }

    private static void Problem(string why)
    {
        if (_problem == null) _problem = why;
        Main.Note("pre-window path sync: " + why);
    }

    private static Track Find(Path p)
    {
        foreach (var t in Tracks)
            if (ReferenceEquals(t.P, p) && t.Id == p.pathID && !t.Done) return t;
        return null;
    }

    // AstarPath.StartPath (main thread).
    public static void OnStart(Path path)
    {
        if (!_active || path == null) return;
        if (!Trace.OnMainThread) { if (_replay) Problem("a path was requested off the main thread before the window"); return; }
        if (PathSync.IsCritterRequest(path)) return; // presentation: native
        var who = PathSync.RequesterKey(path, out _);
        var end = PathSync.EndOf(path);
        string k;
        lock (Tracks)
        {
            PerRequester.TryGetValue(who, out var n);
            PerRequester[who] = n + 1;
            k = who + "#" + n;
            Notes.Add((Time.frameCount, "start", k, 0, end));
            if (_replay && Tape.TryGetValue(k, out var rec) && (!rec.End.HasValue || (end.HasValue && (end.Value - rec.End.Value).magnitude <= 0.25f)))
                Tracks.Add(new Track { K = k, P = path, Id = path.pathID, ReqFrame = Time.frameCount });
            else if (!_replay)
                Tracks.Add(new Track { K = k, P = path, Id = path.pathID, ReqFrame = Time.frameCount });
        }
    }

    // PathReturnQueue.Enqueue (a worker): a replay holds its tracked paths until they are due.
    public static bool OnEnqueue(object owner, Path path)
    {
        if (!_active || !_replay || path == null) return true;
        lock (Tracks)
        {
            if (!_active) return true;
            var t = Find(path);
            if (t == null) return true;
            _owner = owner;
            t.Held = true;
            HeldOrder.Add(t);
            Held++;
            return false;
        }
    }

    private static bool Due(Track t, out int dueFrame, out int dueCall)
    {
        dueFrame = dueCall = 0;
        if (!Tape.TryGetValue(t.K, out var r)) return false;
        dueFrame = t.ReqFrame + (r.Ret - r.Req);
        dueCall = r.Call;
        return Time.frameCount > dueFrame || (Time.frameCount == dueFrame && _callIdx >= dueCall);
    }

    // PathReturnQueue.ReturnPaths (main thread), before it drains: the due paths go into the queue.
    public static void OnReturnPaths(object owner, Queue<Path> queue, ref bool timeSlice)
    {
        if (!_active) return;
        if (Time.frameCount != _callFrame) { _callFrame = Time.frameCount; _callIdx = 0; }
        _callIdx++;
        if (!_replay) return;
        List<Track> due;
        lock (Tracks) due = Tracks.Where(t => !t.Released && !t.Done && Due(t, out _, out _)).ToList();
        if (due.Count == 0) return;
        var canWait = PathSync.WorkersCanRun();
        var sw = Stopwatch.StartNew();
        foreach (var t in due)
        {
            bool held;
            lock (Tracks) held = t.Held;
            if (held) continue;
            Waits++;
            while (true)
            {
                lock (Tracks) held = t.Held;
                if (held) break;
                if (!canWait || sw.ElapsedMilliseconds > 100)
                {
                    Problem($"pre-window path {t.K} due at frame {Time.frameCount} call {_callIdx} was not finished ({(canWait ? "timed out" : "workers blocked")})");
                    break;
                }
                System.Threading.Thread.Sleep(0);
            }
        }
        List<Track> release;
        lock (Tracks)
        {
            release = HeldOrder.FindAll(t => due.Contains(t));
            release.Sort((a, b) => Tape[a.K].Seq.CompareTo(Tape[b.K].Seq)); // the recording's order
            foreach (var t in release)
            {
                HeldOrder.Remove(t);
                t.Released = true;
            }
        }
        // Delivered by themselves before the native drain, which keeps its time slice for untracked paths.
        // The recording's order between tracked and untracked paths of one call isn't known: a call with both
        // can't follow it (critters' paths left out, as in the window).
        var queued = PathSync.QueuedUntracked(owner);
        if (release.Count > 0 && queued > 0)
            Problem($"pre-window path {release[0].K} due at frame {Time.frameCount} call {_callIdx} shares the call with {queued} untracked path(s) already queued (order not recorded)");
        foreach (var t in release)
        {
            var why = PathSync.DeliverNow(owner, t.P);
            if (why != null) Problem($"pre-window path {t.K}: {why}");
        }
    }

    // Path.ReturnPath (main thread): the delivery, noted; in a replay checked against its due frame and call.
    public static void OnReturn(Path p)
    {
        if (!_active) return;
        Track t;
        lock (Tracks)
        {
            t = Find(p);
            if (t == null) return;
            if (_replay && !t.Released) return; // not held (requested before the idle frame): native
            t.Done = true;
            Notes.Add((Time.frameCount, "ret", t.K, _callIdx, null));
        }
        if (_replay && Due(t, out var dueFrame, out var dueCall) && (Time.frameCount != dueFrame || _callIdx != dueCall))
            Problem($"pre-window path {t.K} delivered at frame {Time.frameCount} call {_callIdx}; due at frame {dueFrame} call {dueCall}");
    }
}

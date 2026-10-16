using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using Newtonsoft.Json.Linq;
using UnityEngine;

namespace WrathOracle;

// Scheduler boundaries and recorded inputs of a turn-based fixture window (WrathOracle 0.4).
//
// Turn-based play is gated by things outside the simulation tick: a combat-start window that
// waits four wall-clock seconds, an initiative tracker whose animations run on wall-clock
// deltas, camera scrolls, input timing. A replay can follow its recording only if each of
// those boundaries happens at the same tick. This class does three things for windows whose
// plan uses a turn-based step ("tbm:"):
//   * records every boundary as a "gate" (an "obs" record with g=true): WaitingForUI guard
//     changes with their callers, the realtime callbacks the game schedules (scheduled and
//     fired), turn status changes;
//   * in a replay, compares each gate with the recording's, in order and by tick, and ends the
//     fixture at the first difference (Invalidate), instead of letting a diverged replay run
//     on and report misleading draws much later;
//   * does the same for the window's inputs (the "input" records): a replayed step must be
//     the next recorded input, at the recorded tick.
// The clocks themselves are made deterministic by WallClock.cs; the gates are the check that
// this worked, tick by tick. Nothing here changes game state.
internal static class Sched
{
    // The window's plan uses turn-based steps (set by Fixture.Begin, cleared at its end).
    public static bool Window;

    // Incremented at every window start: things that outlive a window (delayed callbacks) are bound to it.
    public static int Generation;

    // Values the recording observed, read from its trace for a replay (WallClock reproduces them).
    public static int RecFrame;
    public static double RecTime0, RecUnscaled0;
    public static bool RecHeader;
    // The navigation refresh timer at the recording's window open (null: not recorded).
    public static float? RecNavTimer0;
    // The recording's physics settings at its window open (autoSyncTransforms, autoSimulation, fixedDeltaTime).
    public static (bool sync, bool sim, double step)? RecPhysics;
    // The recording's camera at its window open (InputPin.CameraState).
    public static double[] RecCamera;

    // Native identity hashes of units as the recording saw them, by unit id; loaded before the replay's
    // reload (Driver "prepare") and kept until its window closes, so a unit's value never changes.
    public static Dictionary<string, int> HashTape;

    public static void LoadHashTape(string path)
    {
        HashTape = null;
        if (!System.IO.File.Exists(path)) return;
        var d = new Dictionary<string, int>();
        using (var f = System.IO.File.OpenRead(path))
        using (var gz = new System.IO.Compression.GZipStream(f, System.IO.Compression.CompressionMode.Decompress))
        using (var r = new System.IO.StreamReader(gz, System.Text.Encoding.UTF8))
        {
            string line;
            while ((line = r.ReadLine()) != null)
            {
                if (line.IndexOf("\"ev\":\"hash\"", StringComparison.Ordinal) < 0) continue;
                var o = JObject.Parse(line);
                d[(string)o["u"]] = (int)o["h"];
            }
        }
        HashTape = d;
    }

    // A trace's header (its first line), or null.
    public static JObject ReadHeader(string path)
    {
        if (!System.IO.File.Exists(path)) return null;
        using (var f = System.IO.File.OpenRead(path))
        using (var gz = new System.IO.Compression.GZipStream(f, System.IO.Compression.CompressionMode.Decompress))
        using (var r = new System.IO.StreamReader(gz, System.Text.Encoding.UTF8))
        {
            var line = r.ReadLine();
            return line == null ? null : JObject.Parse(line);
        }
    }

    public static bool On => Window && Trace.Active && Trace.OnMainThread;

    private struct GateRec { public string Ev, D; public int T; }
    // A root rule of the recording (depth 0): its tick, class and initiator's id (null: none).
    private struct RootRec { public int T; public string Class, Ini; }
    private struct InputRec { public string Step; public int T; public JObject O; }

    private static List<GateRec> _gates;
    private static List<InputRec> _inputs;
    private static List<RootRec> _roots;
    private static int _gatePos, _inputPos, _rootPos;
    // Initiators with long ids (made in the window: run-specific, Rules.StableId) are bound recording-to-replay,
    // one to one, at their first root rule.
    private static readonly Dictionary<string, string> RootBind = new Dictionary<string, string>(), RootBindBack = new Dictionary<string, string>();
    public static long RootsChecked;

    // The units at the window's open (their ids are the save's, the same in every run of the fixture).
    public static readonly HashSet<string> StartUnits = new HashSet<string>();

    // Fixture.Begin, after the reload: every unit existing now.
    public static void CaptureStartUnits()
    {
        StartUnits.Clear();
        try { foreach (var u in StateDump.AllUnits(Kingmaker.Game.Instance)) if (u?.UniqueId != null) StartUnits.Add(u.UniqueId); }
        catch (Exception e) { Health.Failed("window's starting units", e); }
    }
    public static long GatesSeen, InputsChecked;

    // Animation events (a strike, the end of an attack animation) are placed by float arithmetic on
    // Unity's absolute clock (PlayableInfo.CreationTime is Time.time), whose rounding depends on
    // how long the process has been running, so an event can land a tick earlier or later in a
    // replay than in its recording without anything else being different. A replay therefore
    // follows its recording to within Tol ticks of the offset it has built up so far (the
    // difference at the last matching gate or input); the order and content of every gate and
    // input are exact. Tick-exact equality, where it happens, is reported by oracle-trace-diff.
    public const int Tol = 2;
    // Total departure from the recording that a replay may build up (ticks), reported as MaxOffset.
    public const int MaxDrift = 4;
    private static int _offset;
    public static int Offset => _offset;
    public static long MaxOffset, OffsetChanges;

    // First reason the replay left its recording (null while it follows it).
    public static string Invalid { get; private set; }

    public static bool Replay => _gates != null;

    public static void BeginWindow(bool turnBased)
    {
        Window = turnBased;
        Generation++;
        RecHeader = false;
        RecNavTimer0 = null;
        RecPhysics = null;
        RecCamera = null;
        Invalid = null;
        GatesSeen = InputsChecked = 0;
        _gates = null;
        _inputs = null;
        _roots = null;
        _gatePos = _inputPos = _rootPos = 0;
        RootBind.Clear(); RootBindBack.Clear();
        RootsChecked = 0;
        _offset = 0;
        MaxOffset = OffsetChanges = 0;
    }

    private static void Moved(int replayTick, int recordedTick)
    {
        var o = replayTick - recordedTick;
        if (Math.Abs(o) > MaxDrift) Invalidate($"replay is {o} ticks from its recording at tick {replayTick} (bound {MaxDrift})");
        if (o != _offset) OffsetChanges++;
        _offset = o;
        if (Math.Abs(o) > MaxOffset) MaxOffset = Math.Abs(o);
    }

    public static void EndWindow()
    {
        Window = false;
        _gates = null;
        _inputs = null;
        _roots = null;
        HashTape = null;
    }

    // Before the window closes: a replay that did not use up its recorded gates and inputs left its recording.
    public static void CheckComplete()
    {
        if (_gates == null || Invalid != null) return;
        PathSync.CheckComplete();
        ProjPin.CheckComplete();
        DeathFxPin.CheckComplete();
        CamPin.CheckComplete();
        WallClock.CheckComplete();
        if (Invalid != null) return;
        if (_gatePos < _gates.Count) Invalidate($"replay ended with {_gates.Count - _gatePos} recorded gates unused (next: {_gates[_gatePos].Ev} '{_gates[_gatePos].D}' at tick {_gates[_gatePos].T})");
        else if (_inputPos < _inputs.Count) Invalidate($"replay ended with {_inputs.Count - _inputPos} recorded inputs unused (next: '{_inputs[_inputPos].Step}' at tick {_inputs[_inputPos].T})");
        else if (_rootPos < _roots.Count) Invalidate($"replay ended with {_roots.Count - _rootPos} recorded root rules unused (next: {_roots[_rootPos].Class} of {_roots[_rootPos].Ini ?? "-"} at tick {_roots[_rootPos].T})");
    }

    public static void ClearInvalid() => Invalid = null;

    // ---- tape (read from the recorded trace by Rng.LoadTape) ----

    public static void StartTape()
    {
        WallClock.ResetNavTape();
        IdlePin.Reset();
        _gates = new List<GateRec>();
        _inputs = new List<InputRec>();
        _roots = new List<RootRec>();
        _gatePos = _inputPos = _rootPos = 0;
    }

    // A root rule record of the recording (Rng.LoadTape), validated.
    public static void TapeRoot(JObject o)
    {
        if (_roots == null) return;
        var ini = o["initiator"];
        string id = null;
        var ok = o["t"]?.Type == JTokenType.Integer && o["class"]?.Type == JTokenType.String;
        if (ini == null) ok = false; // the field is required (null: no initiator)
        else if (ini.Type == JTokenType.Null) { }
        else if (ini is JObject u && u["$unit"]?.Type == JTokenType.String) id = (string)u["$unit"];
        else ok = false;
        if (!ok) { Invalidate("recording's root rule record lacks or mistypes its tick, class or initiator"); return; }
        _roots.Add(new RootRec { T = (int)o["t"], Class = (string)o["class"], Ini = id });
    }

    // Every root rule a replay pushes (Rules.OnPush, depth 0): the recording's next, with its class and
    // initiator, on its tick plus the offset so far (exactly). A root rule is a cause the recording had or not:
    // the rules under it, their draws and mutations follow from it (poc5-tb-123: a dismember buff's rule the
    // replays didn't make went unnoticed, the tapes being complete without it).
    public static void CheckRoot(string cls, string ini)
    {
        if (_roots == null || !Trace.Active || Invalid != null) return;
        RootsChecked++;
        if (_rootPos >= _roots.Count)
        {
            Invalidate($"root rule {cls} of {ini ?? "-"} at tick {Trace.Tick}: the recording has no further root rule");
            return;
        }
        var r = _roots[_rootPos++];
        var same = r.Class == cls && Trace.Tick == r.T + _offset;
        if (same && !SameUnit(r.Ini, ini)) same = false;
        if (!same)
            Invalidate($"root rule #{_rootPos - 1}: replay {cls} of {ini ?? "-"} at tick {Trace.Tick}; recording {r.Class} of {r.Ini ?? "-"} at tick {r.T} (offset so far {_offset})");
    }

    public static void TapeRecord(string kind, JObject o)
    {
        if (_gates == null) return;
        if (kind == "obs")
        {
            if ((string)o["ev"] == "nav_clock") WallClock.AddNavTape((float)(double)o["v"]);
            if ((string)o["ev"] == "idle0")
                IdlePin.Add((string)o["u"], new IdlePin.State { Time = (float)(double)o["time"], Random = (float)(double)o["random"], Curve = (string)o["curve"] ?? "?", Playing = (bool?)o["playing"] ?? false });
            // Path keys are "<requester>#<n>" (recordings before that numbered all requests; they no longer replay).
            if ((string)o["ev"] == "path_ret") PathSync.AddTape((string)o["path"], (int)o["t"], (int)o["c"], (int?)o["req"] ?? -1);
            // Projectile launches and hits (ProjPin.cs), death effects (DeathFxPin.cs).
            ProjPin.TapeRecord(o);
            DeathFxPin.TapeRecord(o);
            CamPin.TapeRecord(o);
            if ((string)o["ev"] == "path_start")
                PathSync.AddStartTape((string)o["path"], o["e"] is JArray e && e.Count == 3 ? new UnityEngine.Vector3((float)e[0], (float)e[1], (float)e[2]) : (UnityEngine.Vector3?)null, (bool?)o["strict"] ?? true);
            // Gates carry "g": true; other records use "g" for other things (the memory observer: a group id).
            if (o["g"]?.Type == JTokenType.Boolean && (bool)o["g"]) _gates.Add(new GateRec { Ev = (string)o["ev"], D = (string)o["d"], T = (int)o["t"] });
        }
        else if (kind == "input")
        {
            _inputs.Add(new InputRec { Step = (string)o["step"], T = (int)o["t"], O = o });
        }
    }

    // ---- observation records (T1: left out of the T0 comparison) ----

    public static void Obs(string ev, Action<Json> body = null)
    {
        if (!On) return;
        try { Trace.Emit("obs", j => { j.Prop("ev", ev); body?.Invoke(j); }); }
        catch (Exception e) { Trace.HookFailed("obs " + ev, e); }
    }

    // A boundary the simulation depends on: recorded, and checked against the recording.
    public static void Gate(string ev, string detail)
    {
        if (!On) return;
        GatesSeen++;
        Obs(ev, j => { j.Prop("g", true); j.Prop("d", detail); });
        if (_gates == null || Invalid != null) return;
        if (_gatePos >= _gates.Count)
        {
            Invalidate($"gate {ev} '{detail}' at tick {Trace.Tick}: the recording has no further gate");
            return;
        }
        var g = _gates[_gatePos++];
        if (g.Ev != ev || g.D != detail || Math.Abs(Trace.Tick - (g.T + _offset)) > Tol)
            Invalidate($"gate #{_gatePos - 1}: replay {ev} '{detail}' at tick {Trace.Tick}; recording {g.Ev} '{g.D}' at tick {g.T} (offset so far {_offset}, tolerance {Tol})");
        else Moved(Trace.Tick, g.T);
    }

    // Whether the recording's unit id `rec` and the replay's `now` name the same unit: a unit existing at the
    // window's open (or a short id: the game's sequential ids) by its id; a long id the window made (a run-specific
    // id) by a one-to-one binding made at its first sight, identity bindings included, so two units can never
    // pass for one.
    public static bool SameUnit(string rec, string now)
    {
        if (rec == null || now == null) return rec == now;
        var recMade = rec.Length > 8 && !StartUnits.Contains(rec);
        var nowMade = now.Length > 8 && !StartUnits.Contains(now);
        if (!recMade || !nowMade) return rec == now && !recMade && !nowMade;
        if (RootBind.TryGetValue(rec, out var b)) return b == now;
        if (RootBindBack.ContainsKey(now)) return false;
        RootBind[rec] = now;
        RootBindBack[now] = rec;
        return true;
    }

    // ---- inputs ----

    // Every input record goes through here: a replay must make the recorded input, at the
    // recorded tick.
    public static void CheckInput(string step)
    {
        if (_inputs == null || !Trace.Active || Invalid != null) return;
        InputsChecked++;
        if (_inputPos >= _inputs.Count)
        {
            Invalidate($"input '{step}' at tick {Trace.Tick}: the recording has no further input");
            return;
        }
        var r = _inputs[_inputPos++];
        if (r.Step != step || Math.Abs(Trace.Tick - (r.T + _offset)) > Tol + 1)
            Invalidate($"input #{_inputPos - 1}: replay '{step}' at tick {Trace.Tick}; recording '{r.Step}' at tick {r.T} (offset so far {_offset}, tolerance {Tol + 1})");
        else Moved(Trace.Tick, r.T);
    }

    // The next recorded input a replay has not made yet (null in a recording or at the end).
    public static bool PeekInput(out string step, out int tick, out JObject record)
    {
        step = null; tick = 0; record = null;
        if (_inputs == null || _inputPos >= _inputs.Count) return false;
        var r = _inputs[_inputPos];
        step = r.Step; tick = r.T; record = r.O;
        return true;
    }

    // ---- failing fast ----

    public static void Invalidate(string reason)
    {
        if (Invalid != null) return;
        Invalid = reason;
        Main.Note("REPLAY LEFT ITS RECORDING: " + reason);
        try { Trace.Emit("obs", j => { j.Prop("ev", "invalid"); j.Prop("why", reason); }); }
        catch (Exception) { /* the note above stands */ }
    }
}

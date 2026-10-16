using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using System.Text;
using HarmonyLib;
using Kingmaker.AreaLogic.Cutscenes;
using Kingmaker.AreaLogic.Cutscenes.Commands;
using Kingmaker.Controllers.Optimization;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.View;
using Newtonsoft.Json.Linq;
using TurnBased.Controllers;
using UnityEngine;

namespace WrathOracle;

// The camera's progress where gameplay reads it, in turn-based windows: observed in a recording, pinned or
// checked in a replay (docs/poc5/tb-replay.md, "The camera's gates").
//
// [M] poc5-tb-130/tb-nearest: a turn scrolling to 523B was released (Prepare: commands interrupted, the AI
// forced, its attack) at t=2489 in the recording, 2488 in replay-1 and 2490 in replay-2; the root-rule check
// stopped both. [C] TurnController.Tick enters Prepare once !CameraRig.IsScrollActive(m_ScrollRoutine), and the
// scroll ScrollToTimed started ends after a duration computed from the rig's position, a camera-adjusted target
// (a physics ray), smoothing state and Time.time: presentation, pinned nowhere. -132 (same plan and build)
// replayed exactly: the scroll's end falls near a tick boundary only sometimes.
//
// The readers that gate gameplay on the camera, and what this does:
//  - TurnController.Tick's IsScrollActive (pinned): a recording notes each query ("cam_scroll": the site, its
//    ordinal, the turn's unit, whether the turn holds a routine, the result); a replay's query must be the
//    recording's next, on its tick plus the offset, of the same unit and routine state, and returns the recorded
//    result (the native one is noted, "nat"). A query the recording didn't make, one it made that the replay
//    didn't, or a mismatch invalidates.
//  - The cutscene commands whose completion reads the camera (pinned): CommandMoveCamera.IsFinished and
//    CommandControlCamera.IsFinished read the rig's distance to the target, IsScrollActive and
//    IsRotationActive; their result decides when a cutscene's track moves on. Each call is noted ("cam_cmd": the
//    command, its ordinal, the result) and a replay returns the recorded result in the same way.
//  - The entities in the camera's frustum (checked, not pinned): EntitiesInCameraFrustumController sets
//    IsInCameraFrustum from the rig's position, and SleepingUnitsController reads it for extra units (asleep out
//    of view) and dead ones (IsDeathRevealed). After each of its ticks the state of those units is noted when it
//    changes ("cam_frustum": the units in view, the units out of view) and a replay's must equal the recording's
//    on every tick.
// The camera's other readers are presentation (UI, overtips, IK, effects, sound).
internal static class CamPin
{
    public static bool Installed, RecordedOn;
    public static long Scrolls, Commands, FrustumChanges;

    private sealed class Query { public string Site, Key, Unit; public int T; public bool Routine, V, Used; }
    private sealed class Frustum { public int T; public string S; }

    private static readonly List<Query> ScrollTape = new List<Query>();
    private static readonly Dictionary<string, List<Query>> CmdTape = new Dictionary<string, List<Query>>();
    private static readonly List<Frustum> FrustumTape = new List<Frustum>();
    private static int _scrollPos, _frustumPos;
    private static readonly Dictionary<string, int> CmdPos = new Dictionary<string, int>();
    private static string _frustumLast;
    private static string _perturbNote;

    private static readonly AccessTools.FieldRef<TurnController, UnitReference> RiderRef =
        AccessTools.FieldRefAccess<TurnController, UnitReference>("m_Rider");

    public static void ResetWindow()
    {
        ScrollTape.Clear(); CmdTape.Clear(); FrustumTape.Clear(); CmdPos.Clear();
        _scrollPos = _frustumPos = 0;
        _frustumLast = null;
        _perturbNote = null;
        RecordedOn = false;
        Scrolls = Commands = FrustumChanges = 0;
    }

    private static bool On => Installed && Sched.On;

    // ---- the tape ----

    public static void TapeRecord(JObject o)
    {
        var ev = (string)o["ev"];
        if (ev != "cam_scroll" && ev != "cam_cmd" && ev != "cam_frustum") return;
        var t = o["t"]?.Type == JTokenType.Integer ? (int)o["t"] : -1;
        string S(string f) => o[f]?.Type == JTokenType.String ? (string)o[f] : null;
        bool? B(string f) => o[f]?.Type == JTokenType.Boolean ? (bool)o[f] : (bool?)null;
        int? I(string f) => o[f]?.Type == JTokenType.Integer ? (int)o[f] : (int?)null;
        if (ev == "cam_scroll")
        {
            var n = I("n");
            if (t < 0 || S("site") != "turn" || n != ScrollTape.Count || !(o["u"]?.Type == JTokenType.String || o["u"]?.Type == JTokenType.Null) ||
                B("routine") == null || B("v") == null)
            { Sched.Invalidate("recording's cam_scroll record lacks, mistypes or misnumbers a field"); return; }
            ScrollTape.Add(new Query { Site = "turn", T = t, Unit = S("u"), Routine = B("routine").Value, V = B("v").Value });
        }
        else if (ev == "cam_cmd")
        {
            var key = S("cmd");
            var n = I("n");
            if (key == null || t < 0 || n == null || B("v") == null) { Sched.Invalidate("recording's cam_cmd record lacks or mistypes a field"); return; }
            if (!CmdTape.TryGetValue(key, out var list)) CmdTape[key] = list = new List<Query>();
            if (n != list.Count) { Sched.Invalidate($"recording's cam_cmd records of {key} misnumbered"); return; }
            list.Add(new Query { Site = "cmd", Key = key, T = t, V = B("v").Value });
        }
        else
        {
            var s = S("s");
            if (t < 0 || s == null || (FrustumTape.Count > 0 && FrustumTape[FrustumTape.Count - 1].T >= t))
            { Sched.Invalidate("recording's cam_frustum record lacks a field or is out of order"); return; }
            FrustumTape.Add(new Frustum { T = t, S = s });
        }
    }

    // A damaged-tape test (plan step replay:NAME:scroll#K:flip): the K-th recorded scroll result (1-based) flipped.
    // The replay then follows a result the game didn't give; it must end invalid.
    public static bool Perturb(string perturb)
    {
        if (perturb == null || !perturb.StartsWith("scroll#", StringComparison.Ordinal)) return false;
        var parts = perturb.Split(':');
        if (parts.Length == 2 && parts[1] == "flip" && int.TryParse(parts[0].Substring(7), out var k) && k >= 1 && k <= ScrollTape.Count)
        {
            ScrollTape[k - 1].V = !ScrollTape[k - 1].V;
            _perturbNote = $"scroll query {k} of {ScrollTape.Count} (tick {ScrollTape[k - 1].T}) flipped to {ScrollTape[k - 1].V}";
        }
        else _perturbNote = $"perturbation {perturb} not applicable ({ScrollTape.Count} scroll queries)";
        Rng.PerturbNote = _perturbNote;
        return true;
    }

    // ---- TurnController.Tick's IsScrollActive (a call-site tap) ----

    public static bool TapTurnScroll(CameraRig rig, Coroutine routine, TurnController turn)
    {
        var native = rig.IsScrollActive(routine);
        if (!On) return native;
        try
        {
            var u = turn == null ? null : RiderRef(turn).UniqueId;
            var has = routine != null;
            var n = (int)Scrolls++;
            var now = Trace.Tick;
            var v = native;
            if (Sched.Replay)
            {
                if (_scrollPos >= ScrollTape.Count)
                    Sched.Invalidate($"scroll query #{n} of {u ?? "-"} at tick {now}: the recording made no further query");
                else
                {
                    var rec = ScrollTape[_scrollPos++];
                    rec.Used = true;
                    if (now != rec.T + Sched.Offset || has != rec.Routine || !Sched.SameUnit(rec.Unit, u))
                        Sched.Invalidate($"scroll query #{n} of {u ?? "-"} (routine {has}) at tick {now}; the recording's of {rec.Unit ?? "-"} (routine {rec.Routine}) at tick {rec.T} (offset so far {Sched.Offset})");
                    else v = rec.V;
                }
            }
            var vv = v;
            Sched.Obs("cam_scroll", j =>
            {
                j.Prop("site", "turn"); j.Prop("n", n); j.Prop("u", u); j.Prop("routine", has); j.Prop("v", vv);
                if (Sched.Replay) j.Prop("nat", native);
            });
            return v;
        }
        catch (Exception e) { Trace.HookFailed("camera scroll query", e); return native; }
    }

    public static IEnumerable<CodeInstruction> TurnTickTranspiler(IEnumerable<CodeInstruction> instructions)
    {
        var target = AccessTools.Method(typeof(CameraRig), nameof(CameraRig.IsScrollActive));
        var tap = AccessTools.Method(typeof(CamPin), nameof(TapTurnScroll));
        var n = 0;
        foreach (var i in instructions)
        {
            if ((i.opcode == OpCodes.Callvirt || i.opcode == OpCodes.Call) && Equals(i.operand, target))
            {
                n++;
                // The turn itself (this), after the rig and the routine.
                yield return new CodeInstruction(OpCodes.Ldarg_0).MoveLabelsFrom(i);
                yield return new CodeInstruction(OpCodes.Call, tap);
                continue;
            }
            yield return i;
        }
        _turnTapped = n;
    }

    private static int _turnTapped;

    // ---- cutscene camera commands' IsFinished ----

    // The command's key: its type and blueprint id.
    private static string CmdKey(object cmd) => cmd.GetType().Name + ":" + (cmd is Kingmaker.Blueprints.SimpleBlueprint b ? b.AssetGuid.ToString() : "?");

    public static bool CmdPrefix(object __instance, ref bool __result, out (string key, int n, bool pinned) __state)
    {
        __state = (null, -1, false);
        if (!On || __instance == null) return true;
        try
        {
            var key = CmdKey(__instance);
            CmdPos.TryGetValue(key, out var n);
            CmdPos[key] = n + 1;
            Commands++;
            __state = (key, n, false);
            if (!Sched.Replay || Sched.Invalid != null) return true;
            if (!CmdTape.TryGetValue(key, out var list) || n >= list.Count)
            {
                Sched.Invalidate($"camera command {key} completion query #{n} at tick {Trace.Tick}: the recording made no such query");
                return true;
            }
            var rec = list[n];
            rec.Used = true;
            if (Trace.Tick != rec.T + Sched.Offset)
            {
                Sched.Invalidate($"camera command {key} completion query #{n} at tick {Trace.Tick}; the recording's at tick {rec.T} (offset so far {Sched.Offset})");
                return true;
            }
            __result = rec.V;
            __state = (key, n, true);
            return false;
        }
        catch (Exception e) { Trace.HookFailed("camera command query", e); return true; }
    }

    public static void CmdPostfix(bool __result, (string key, int n, bool pinned) __state)
    {
        if (__state.key == null || !On) return;
        try
        {
            var (key, n, pinned) = __state;
            var v = __result;
            Sched.Obs("cam_cmd", j => { j.Prop("cmd", key); j.Prop("n", n); j.Prop("v", v); if (pinned) j.Prop("pin", true); });
        }
        catch (Exception e) { Trace.HookFailed("camera command query end", e); }
    }

    // ---- the frustum, for the units it can put to sleep or reveal dead ----

    public static void FrustumPostfix()
    {
        if (!On) return;
        try
        {
            var inView = new List<string>();
            var outView = new List<string>();
            foreach (var u in StateDump.AllUnits(Kingmaker.Game.Instance))
            {
                if (u == null || !(u.IsExtra || u.Descriptor.State.IsFinallyDead)) continue;
                (u.IsInCameraFrustum ? inView : outView).Add(Rules.StableId(u));
            }
            inView.Sort(StringComparer.Ordinal);
            outView.Sort(StringComparer.Ordinal);
            var s = new StringBuilder().Append(string.Join(",", inView)).Append('|').Append(string.Join(",", outView)).ToString();
            var now = Trace.Tick;
            if (s != _frustumLast)
            {
                _frustumLast = s;
                FrustumChanges++;
                Sched.Obs("cam_frustum", j => j.Prop("s", s));
            }
            if (!Sched.Replay || Sched.Invalid != null) return;
            // The recording's state on this tick: its last change at or before it.
            while (_frustumPos + 1 < FrustumTape.Count && FrustumTape[_frustumPos + 1].T + Sched.Offset <= now) _frustumPos++;
            var want = FrustumTape.Count > 0 && FrustumTape[_frustumPos].T + Sched.Offset <= now ? FrustumTape[_frustumPos].S : null;
            if (want == null) Sched.Invalidate($"units in the camera's view at tick {now}: the recording has no state for them yet");
            else if (want != s) Sched.Invalidate($"units in the camera's view at tick {now} are {s}; the recording's {want}");
        }
        catch (Exception e) { Trace.HookFailed("camera frustum", e); }
    }

    // Before the window closes (Sched.CheckComplete): every recorded query made.
    public static void CheckComplete()
    {
        if (!Installed || !Sched.Replay || Sched.Invalid != null) return;
        if (_scrollPos < ScrollTape.Count)
        {
            var r = ScrollTape[_scrollPos];
            Sched.Invalidate($"the recording's scroll query #{_scrollPos} of {r.Unit ?? "-"} at tick {r.T} wasn't made in the replay");
            return;
        }
        foreach (var kv in CmdTape)
        {
            CmdPos.TryGetValue(kv.Key, out var made);
            if (made < kv.Value.Count)
            {
                Sched.Invalidate($"the recording's camera command {kv.Key} completion query #{made} at tick {kv.Value[made].T} wasn't made in the replay");
                return;
            }
        }
        if (FrustumTape.Count > 0 && FrustumTape[FrustumTape.Count - 1].T + Sched.Offset > Trace.Tick)
            Sched.Invalidate($"the recording's view change at tick {FrustumTape[FrustumTape.Count - 1].T} came after the replay's window");
    }

    public static void Install(Harmony h)
    {
        try
        {
            MethodBase M(Type t, string name) => AccessTools.Method(t, name) ?? throw new MissingMethodException(t.Name + "." + name);
            if (RiderRef == null) throw new MissingFieldException("TurnController.m_Rider");
            _turnTapped = 0;
            h.Patch(M(typeof(TurnController), "Tick"), transpiler: new HarmonyMethod(typeof(CamPin), nameof(TurnTickTranspiler)));
            if (_turnTapped != 1) throw new Exception($"TurnController.Tick: {_turnTapped} IsScrollActive calls tapped, not 1");
            foreach (var t in new[] { typeof(CommandMoveCamera), typeof(CommandControlCamera) })
                h.Patch(AccessTools.Method(t, nameof(CommandBase.IsFinished), new[] { typeof(CutscenePlayerData) }) ?? throw new MissingMethodException(t.Name + ".IsFinished"),
                    prefix: new HarmonyMethod(typeof(CamPin), nameof(CmdPrefix)), postfix: new HarmonyMethod(typeof(CamPin), nameof(CmdPostfix)));
            h.Patch(M(typeof(EntitiesInCameraFrustumController), nameof(EntitiesInCameraFrustumController.Tick)), postfix: new HarmonyMethod(typeof(CamPin), nameof(FrustumPostfix)));
            Installed = true;
        }
        catch (Exception e) { Health.Failed("patch the camera's gates", e); }
    }
}

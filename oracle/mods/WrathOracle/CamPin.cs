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
//    command, its ordinal, the cutscene player's id, the result) and a replay returns the recorded result in the
//    same way, the player bound as units are (Sched.SameUnit).
//  - The entities in the camera's frustum (pinned): EntitiesInCameraFrustumController sets IsInCameraFrustum
//    from the rig's position; SleepingUnitsController reads it for extra units (asleep out of view) and finally
//    dead ones (IsDeathRevealed), EntityVisibilityForPlayerController for awake extra units,
//    UnitEntityView.OnVisibilityChanged for dead ones, HandleDeath for the dying (pinned in DeathFxPin). The state
//    of extra and dead units (in view, out of view: their ids) is noted at the window's open and after each of the
//    controller's ticks when it changes ("cam_frustum"). A replay sets it to the recording's on every tick, on the
//    same units (Sched.MapUnitGroups), each change applied on its own tick; the game's own is noted where it differs
//    ("nat_in", "nat_out"). [M] poc5-tb-134/tb-lizard replay-2: the camera's drift kept the dead fly 529A in view
//    past tick 11800, where the recording had it leave.
//  - Alushenyrra's isles (checked): IslesController.TickCameraRotation turns the camera's yaw into a direction
//    whose changes drive isle states (their actions, navmesh cuts). Each change is noted ("cam_isle") and a
//    replay's must be the recording's, on its tick.
// The camera's other readers are presentation (UI, overtips, IK, effects, sound).
internal static class CamPin
{
    public static bool Installed, RecordedOn;
    public static long Scrolls, Commands, FrustumChanges, PinnedViews;

    private sealed class Query { public string Site, Key, Unit; public int T; public bool Routine, V, Used; }
    private sealed class Frustum { public int T; public List<string> In, Out; }
    private sealed class Isle { public int T; public string Dir; }

    private static readonly List<Query> ScrollTape = new List<Query>();
    private static readonly Dictionary<string, List<Query>> CmdTape = new Dictionary<string, List<Query>>();
    private static readonly List<Frustum> FrustumTape = new List<Frustum>();
    private static readonly List<Isle> IsleTape = new List<Isle>();
    private static int _islePos;
    private static int _scrollPos, _frustumPos, _frustumChecked = -1;
    private static readonly Dictionary<string, int> CmdPos = new Dictionary<string, int>();
    // The player each command was first queried for in this window: a command queried for two players (copies
    // of one cutscene, its own random ids) has no identity to follow and is refused (ambiguous).
    private static readonly Dictionary<string, string> CmdPlayer = new Dictionary<string, string>();
    private static string _frustumLast;
    private static string _perturbNote;

    private static readonly AccessTools.FieldRef<TurnController, UnitReference> RiderRef =
        AccessTools.FieldRefAccess<TurnController, UnitReference>("m_Rider");

    public static void ResetWindow()
    {
        ScrollTape.Clear(); CmdTape.Clear(); FrustumTape.Clear(); CmdPos.Clear(); IsleTape.Clear(); CmdPlayer.Clear();
        _scrollPos = _frustumPos = _islePos = 0;
        _frustumChecked = -1;
        _frustumLast = null;
        _perturbNote = null;
        RecordedOn = false;
        Scrolls = Commands = FrustumChanges = PinnedViews = 0;
    }

    private static bool On => Installed && Sched.On;

    // ---- the tape ----

    public static void TapeRecord(JObject o)
    {
        var ev = (string)o["ev"];
        if (ev != "cam_scroll" && ev != "cam_cmd" && ev != "cam_frustum" && ev != "cam_isle") return;
        if (o["t"]?.Type != JTokenType.Integer) { Sched.Invalidate($"recording's {ev} record without an integer tick"); return; }
        var t = (int)o["t"];
        string S(string f) => o[f]?.Type == JTokenType.String ? (string)o[f] : null;
        bool? B(string f) => o[f]?.Type == JTokenType.Boolean ? (bool)o[f] : (bool?)null;
        int? I(string f) => o[f]?.Type == JTokenType.Integer ? (int)o[f] : (int?)null;
        if (ev == "cam_scroll")
        {
            var n = I("n");
            if (t < 0 || S("site") != "turn" || n != ScrollTape.Count || !(o["u"]?.Type == JTokenType.String || o["u"]?.Type == JTokenType.Null) ||
                B("routine") == null || B("v") == null)
            { Sched.Invalidate("recording's cam_scroll record lacks, mistypes or misnumbers a field"); return; }
            if (ScrollTape.Count > 0 && ScrollTape[ScrollTape.Count - 1].T > t) { Sched.Invalidate("recording's cam_scroll records go back in time"); return; }
            ScrollTape.Add(new Query { Site = "turn", T = t, Unit = S("u"), Routine = B("routine").Value, V = B("v").Value });
        }
        else if (ev == "cam_cmd")
        {
            var key = S("cmd");
            var n = I("n");
            if (key == null || t < 0 || n == null || B("v") == null || S("player") == null) { Sched.Invalidate("recording's cam_cmd record lacks or mistypes a field"); return; }
            if (o["ambiguous"] != null) { Sched.Invalidate($"recording's camera command {key} was queried for two cutscene players (ambiguous)"); return; }
            if (!CmdTape.TryGetValue(key, out var list)) CmdTape[key] = list = new List<Query>();
            if (n != list.Count) { Sched.Invalidate($"recording's cam_cmd records of {key} misnumbered"); return; }
            if (list.Count > 0 && list[list.Count - 1].T > t) { Sched.Invalidate($"recording's cam_cmd records of {key} go back in time"); return; }
            list.Add(new Query { Site = "cmd", Key = key, Unit = S("player"), T = t, V = B("v").Value });
        }
        else if (ev == "cam_isle")
        {
            var dir = S("dir");
            if (t < 0 || dir == null) { Sched.Invalidate("recording's cam_isle record lacks a field"); return; }
            if (IsleTape.Count > 0 && IsleTape[IsleTape.Count - 1].T > t) { Sched.Invalidate("recording's cam_isle records go back in time"); return; }
            IsleTape.Add(new Isle { T = t, Dir = dir });
        }
        else
        {
            List<string> L(string f) => o[f] is JArray a && a.All(x => x.Type == JTokenType.String) ? a.Select(x => (string)x).ToList() : null;
            var inV = L("in");
            var outV = L("out");
            // The window's open is tick -1.
            if (t < -1 || inV == null || outV == null || (FrustumTape.Count > 0 && FrustumTape[FrustumTape.Count - 1].T >= t))
            { Sched.Invalidate("recording's cam_frustum record lacks a field or is out of order"); return; }
            // Each unit once (StateDump.AllUnits lists a unit once).
            if (inV.Concat(outV).Distinct(StringComparer.Ordinal).Count() != inV.Count + outV.Count)
            { Sched.Invalidate($"recording's cam_frustum record at tick {t} names a unit twice"); return; }
            FrustumTape.Add(new Frustum { T = t, In = inV, Out = outV });
        }
    }

    // A damaged-tape test (plan step replay:NAME:scroll#K:flip): the K-th recorded scroll result (1-based) flipped.
    // The replay then follows a result the game didn't give; it must end invalid.
    public static bool Perturb(string perturb)
    {
        if (perturb == null) return false;
        // view#K:flip: the K-th recorded view state (1-based, the window's open first; "last" for the last) with its
        // groups swapped. The replay then sets the units in view the recording had out of it and the other way round,
        // until the next recorded state; a finally dead unit's IsDeathRevealed (saved) follows its view on each tick.
        if (perturb.StartsWith("view#", StringComparison.Ordinal))
        {
            var vp = perturb.Split(':');
            var kv = vp[0] == "view#last" ? FrustumTape.Count : int.TryParse(vp[0].Substring(5), out var k1) ? k1 : 0;
            if (vp.Length == 2 && vp[1] == "flip" && kv >= 1 && kv <= FrustumTape.Count)
            {
                var f = FrustumTape[kv - 1];
                (f.In, f.Out) = (f.Out, f.In);
                _perturbNote = $"view state {kv} of {FrustumTape.Count} (tick {f.T}) flipped: in [{string.Join(",", f.In)}], out [{string.Join(",", f.Out)}]";
            }
            else _perturbNote = $"perturbation {perturb} not applicable ({FrustumTape.Count} view states)";
            Rng.PerturbNote = _perturbNote;
            return true;
        }
        if (!perturb.StartsWith("scroll#", StringComparison.Ordinal)) return false;
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

    public static bool CmdPrefix(object __instance, CutscenePlayerData player, ref bool __result, out (string key, int n, bool pinned, string player, bool ambiguous) __state)
    {
        __state = (null, -1, false, null, false);
        if (!On || __instance == null) return true;
        try
        {
            var key = CmdKey(__instance);
            var pid = player?.UniqueId ?? "-";
            CmdPos.TryGetValue(key, out var n);
            CmdPos[key] = n + 1;
            Commands++;
            if (!CmdPlayer.TryGetValue(key, out var first)) CmdPlayer[key] = first = pid;
            var ambiguous = first != pid;
            __state = (key, n, false, pid, ambiguous);
            if (!Sched.Replay || Sched.Invalid != null) return true;
            if (ambiguous)
            {
                Sched.Invalidate($"camera command {key} queried for cutscene player {pid} after player {first}: no identity to follow");
                return true;
            }
            if (!CmdTape.TryGetValue(key, out var list) || n >= list.Count)
            {
                Sched.Invalidate($"camera command {key} completion query #{n} at tick {Trace.Tick}: the recording made no such query");
                return true;
            }
            var rec = list[n];
            rec.Used = true;
            if (Trace.Tick != rec.T + Sched.Offset || !Sched.SameUnit(rec.Unit, pid))
            {
                Sched.Invalidate($"camera command {key} completion query #{n} of player {pid} at tick {Trace.Tick}; the recording's of player {rec.Unit} at tick {rec.T} (offset so far {Sched.Offset})");
                return true;
            }
            __result = rec.V;
            __state = (key, n, true, pid, false);
            return false;
        }
        catch (Exception e) { Trace.HookFailed("camera command query", e); return true; }
    }

    public static void CmdPostfix(bool __result, (string key, int n, bool pinned, string player, bool ambiguous) __state)
    {
        if (__state.key == null || !On) return;
        try
        {
            var (key, n, pinned, player, ambiguous) = __state;
            var v = __result;
            Sched.Obs("cam_cmd", j =>
            {
                j.Prop("cmd", key); j.Prop("n", n); j.Prop("player", player); j.Prop("v", v);
                if (pinned) j.Prop("pin", true);
                if (ambiguous) j.Prop("ambiguous", true);
            });
        }
        catch (Exception e) { Trace.HookFailed("camera command query end", e); }
    }

    // ---- the frustum, for the units it can put to sleep or reveal dead ----

    public static void FrustumPostfix()
    {
        if (On) View(Trace.Tick);
    }

    // Fixture.Begin, after the trace's begin: the state the window's first ticks read.
    public static void WindowOpen()
    {
        if (On) View(Trace.Tick);
    }

    // The extra and dead units in view and out of view (raw ids, sorted), noted when changed. A replay sets them to
    // the recording's state on this tick (the game's own noted as "nat_in"/"nat_out" where it differs).
    private static void View(int now)
    {
        try
        {
            var units = new List<UnitEntityData>();
            foreach (var u in StateDump.AllUnits(Kingmaker.Game.Instance))
            {
                if (u == null || u.UniqueId == null) continue;
                var st = u.Descriptor.State;
                if (u.IsExtra || st.IsDead || st.IsFinallyDead) units.Add(u);
            }
            List<string> Ids(bool inView) => units.Where(u => u.IsInCameraFrustum == inView).Select(u => u.UniqueId).OrderBy(x => x, StringComparer.Ordinal).ToList();
            var natIn = Ids(true);
            var natOut = Ids(false);
            var pinned = Sched.Replay && Sched.Invalid == null && Pin(now, units);
            var inView = pinned ? Ids(true) : natIn;
            var outView = pinned ? Ids(false) : natOut;
            var differs = pinned && !natIn.SequenceEqual(inView);
            if (differs) PinnedViews++;
            var s = string.Join(",", inView) + "|" + string.Join(",", outView) + (differs ? "|" + string.Join(",", natIn) : "");
            if (s == _frustumLast) return;
            _frustumLast = s;
            FrustumChanges++;
            Sched.Obs("cam_frustum", j =>
            {
                j.Arr("in"); foreach (var x in inView) j.W.WriteValue(x); j.EndArr();
                j.Arr("out"); foreach (var x in outView) j.W.WriteValue(x); j.EndArr();
                if (pinned) j.Prop("pin", true);
                if (differs)
                {
                    j.Arr("nat_in"); foreach (var x in natIn) j.W.WriteValue(x); j.EndArr();
                    j.Arr("nat_out"); foreach (var x in natOut) j.W.WriteValue(x); j.EndArr();
                }
            });
        }
        catch (Exception e) { Trace.HookFailed("camera frustum", e); }
    }

    // A replay's view (WindowOpen, or after the controller's tick): the recording's state on this tick, its last change
    // at or before it, set on the same units. Moving past a recorded state needs it applied first, and a change is
    // first applied on its own tick (plus the offset): a change the replay would make later (a controller tick it
    // skipped) was read late by the controllers before it. False (invalid) when it can't be applied.
    private static bool Pin(int now, List<UnitEntityData> units)
    {
        while (_frustumPos + 1 < FrustumTape.Count && FrustumTape[_frustumPos + 1].T + Sched.Offset <= now)
        {
            if (_frustumChecked < _frustumPos)
            {
                Sched.Invalidate($"the recording's view at tick {FrustumTape[_frustumPos].T} was never applied in the replay (passed at tick {now})");
                return false;
            }
            _frustumPos++;
        }
        var want = FrustumTape.Count > 0 && FrustumTape[_frustumPos].T + Sched.Offset <= now ? FrustumTape[_frustumPos] : null;
        if (want == null)
        {
            Sched.Invalidate($"units in the camera's view at tick {now}: the recording has no state for them yet");
            return false;
        }
        if (_frustumChecked < _frustumPos && want.T + Sched.Offset != now)
        {
            Sched.Invalidate($"the recording's view at tick {want.T} first applied in the replay at tick {now} (offset so far {Sched.Offset})");
            return false;
        }
        var map = Sched.MapUnitGroups(want.In, want.Out, units.Select(u => u.UniqueId).ToList());
        if (map == null)
        {
            Sched.Invalidate($"extra and dead units at tick {now}: [{string.Join(",", units.Select(u => u.UniqueId).OrderBy(x => x, StringComparer.Ordinal))}]; " +
                             $"the recording's in view [{string.Join(",", want.In)}], out [{string.Join(",", want.Out)}] (tick {want.T})");
            return false;
        }
        foreach (var u in units) u.IsInCameraFrustum = map[u.UniqueId];
        _frustumChecked = _frustumPos;
        return true;
    }

    // ---- Alushenyrra's isles: the camera direction's changes ----

    private static readonly AccessTools.FieldRef<Kingmaker.AreaLogic.AlushenyrraIsles.IslesController, Kingmaker.AreaLogic.AlushenyrraIsles.CameraDirection?> LastDir =
        AccessTools.FieldRefAccess<Kingmaker.AreaLogic.AlushenyrraIsles.IslesController, Kingmaker.AreaLogic.AlushenyrraIsles.CameraDirection?>("m_LastDir");

    public static void IslePrefix(Kingmaker.AreaLogic.AlushenyrraIsles.IslesController __instance, out Kingmaker.AreaLogic.AlushenyrraIsles.CameraDirection? __state)
    {
        __state = On && __instance != null ? LastDir(__instance) : null;
    }

    public static void IslePostfix(Kingmaker.AreaLogic.AlushenyrraIsles.IslesController __instance, Kingmaker.AreaLogic.AlushenyrraIsles.CameraDirection? __state)
    {
        if (!On || __instance == null) return;
        try
        {
            var now = LastDir(__instance);
            if (now == __state) return;
            var dir = now?.ToString() ?? "-";
            var t = Trace.Tick;
            Sched.Obs("cam_isle", j => j.Prop("dir", dir));
            if (!Sched.Replay || Sched.Invalid != null) return;
            if (_islePos >= IsleTape.Count) Sched.Invalidate($"camera direction {dir} at tick {t}: the recording had no further change");
            else
            {
                var r = IsleTape[_islePos++];
                if (r.Dir != dir || t != r.T + Sched.Offset)
                    Sched.Invalidate($"camera direction {dir} at tick {t}; the recording's {r.Dir} at tick {r.T} (offset so far {Sched.Offset})");
            }
        }
        catch (Exception e) { Trace.HookFailed("camera isle direction", e); }
    }

    // Each tick's start (Hooks.ModeTickPrefix): a recorded view change due on an earlier tick but never checked
    // (the frustum controller didn't run then) is caught even if the controller doesn't run now either.
    public static void TickStart()
    {
        if (!On || !Sched.Replay || Sched.Invalid != null || FrustumTape.Count == 0) return;
        var now = Trace.Tick;
        var i = _frustumChecked + 1;
        if (i < FrustumTape.Count && FrustumTape[i].T + Sched.Offset < now)
            Sched.Invalidate($"the recording's view at tick {FrustumTape[i].T} was never applied in the replay (tick {now})");
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
        if (_islePos < IsleTape.Count)
        {
            Sched.Invalidate($"the recording's camera direction {IsleTape[_islePos].Dir} at tick {IsleTape[_islePos].T} wasn't reached in the replay");
            return;
        }
        // Every recorded view must have been matched by a check (the controller may not tick on the last ticks).
        if (_frustumChecked < FrustumTape.Count - 1)
            Sched.Invalidate($"the recording's view at tick {FrustumTape[_frustumChecked + 1].T} was never applied in the replay");
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
            if (LastDir == null) throw new MissingFieldException("IslesController.m_LastDir");
            h.Patch(M(typeof(Kingmaker.AreaLogic.AlushenyrraIsles.IslesController), "TickCameraRotation"),
                prefix: new HarmonyMethod(typeof(CamPin), nameof(IslePrefix)), postfix: new HarmonyMethod(typeof(CamPin), nameof(IslePostfix)));
            Installed = true;
        }
        catch (Exception e) { Health.Failed("patch the camera's gates", e); }
    }
}

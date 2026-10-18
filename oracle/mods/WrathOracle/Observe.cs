using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Reflection;
using System.Text;
using HarmonyLib;
using Kingmaker;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.UnitLogic;
using Kingmaker.Utility;
using Kingmaker.View;
using Pathfinding;
using TurnBased.Controllers;
using UnityEngine;

namespace WrathOracle;

// Passive observations of a turn-based window, written as "obs" records (T1: the T0
// comparison leaves them out; tools/oracle-obs-diff lines two traces up by tick). They exist
// to find where a replay first leaves its recording before combat starts: path requests and
// completions, per-unit movement/animation/command state each tick, combat joins, memory
// additions, the UI guard and the turn status. Everything is read through plain getters and
// fields; nothing here triggers a rule, a draw or a state change.
internal static class Observe
{
    private static readonly Dictionary<string, string> Last = new Dictionary<string, string>();
    private static int _lastGuard = -1;
    private static string _lastStatus;
    private static readonly AccessTools.FieldRef<CountingGuard, int> GuardCount = AccessTools.FieldRefAccess<CountingGuard, int>("m_GuardCount");
    private static readonly AccessTools.FieldRef<UnitGroupMemory, string> GroupId = AccessTools.FieldRefAccess<UnitGroupMemory, string>("GroupId");

    // DOTween's wall-clock delta of the last frame (WallClock makes it the fixed frame delta in
    // a window; an observation records any frame where it is not).
    private static readonly FieldInfo DotweenDelta = AccessTools.Field(AccessTools.TypeByName("DG.Tweening.Core.DOTweenComponent"), "_unscaledDeltaTime");
    private static UnityEngine.Object _dotweenComponent;

    // Units that exist when the window opens (ResetWindow); units created later have run-specific
    // ids and are left out. Physics steps per tick (Time.fixedTime) are logged as a pseudo unit.
    private static HashSet<string> _known, _awake;
    private static bool _rowsOpened;
    private static float _lastFixed = -1f;

    private static float _losStamp = float.MinValue;
    private static readonly Dictionary<string, HashSet<string>> VisionLast = new Dictionary<string, HashSet<string>>();
    private static readonly Dictionary<string, List<string>> VisionOrderLast = new Dictionary<string, List<string>>();

    // Fixture.Begin: the units that exist when the window opens (sleeping ones too: a unit that wakes later has
    // the same id in every run; units created in the window have run-specific ids and get no rows).
    public static void ResetWindow()
    {
        _losStamp = float.MinValue;
        VisionLast.Clear();
        VisionOrderLast.Clear();
        _known = new HashSet<string>();
        _rowsOpened = false;
        try { var g = Game.Instance; if (g?.State != null) foreach (var u in StateDump.AllUnits(g)) if (u != null) _known.Add(u.UniqueId); }
        catch (Exception e) { Health.Log("observe known units", e); }
        _awake = null;
        _lastFixed = -1f;
        _dotweenComponent = null;
        Last.Clear();
        _lastGuard = -1;
        _lastStatus = null;
    }

    // Units that never move or act in these fixtures (placeholders, scenery) are not worth a
    // row each tick; the filter is by what a unit is doing, so it is the same in both runs.
    private static string Row(UnitEntityData u)
    {
        var sb = new StringBuilder();
        var p = u.Position;
        sb.Append(p.x.ToString("R", CultureInfo.InvariantCulture)).Append(',').Append(p.y.ToString("R", CultureInfo.InvariantCulture)).Append(',').Append(p.z.ToString("R", CultureInfo.InvariantCulture));
        var view = u.View;
        sb.Append("|mv=").Append(view != null && view.IsMoving() ? 1 : 0);
        sb.Append("|ic=").Append(u.IsInCombat ? 1 : 0);
        sb.Append("|cmd=");
        foreach (var c in u.Commands.Raw)
            if (c != null) sb.Append(c.GetType().Name).Append(c.IsStarted ? "*" : "").Append(c.IsFinished ? "!" : "").Append(';');
        sb.Append("|q=").Append(u.Commands.Queue.Count);
        var am = view?.AnimationManager;
        sb.Append("|ap=").Append(am != null && am.IsPreventingMovement ? 1 : 0);
        sb.Append("|cp=").Append(view != null && view.IsCommandsPreventMovement ? 1 : 0);
        // How many units the group remembers: the memory's own dictionary, through the cached m_Group (the Group
        // getter runs UpdateGroup, and Memory.Enemies fills its cache and asks other units' Group: a recording
        // only observes).
        var mem = CachedGroup(u)?.Memory;
        sb.Append("|mem=").Append(mem != null ? MemoryUnits(mem).Count : -1);
        sb.Append("|fog=").Append(u.IsInFogOfWar ? 1 : 0);
        return sb.ToString();
    }

    private static readonly AccessTools.FieldRef<UnitGroupMemory, Dictionary<string, UnitGroupMemory.UnitInfo>> MemoryUnits =
        AccessTools.FieldRefAccess<UnitGroupMemory, Dictionary<string, UnitGroupMemory.UnitInfo>>("m_Units");

    private static readonly AccessTools.FieldRef<UnitEntityData, Kingmaker.UnitLogic.Groups.UnitGroup> CachedGroup =
        AccessTools.FieldRefAccess<UnitEntityData, Kingmaker.UnitLogic.Groups.UnitGroup>("m_Group");


    // Called once per sim tick (GameMode.Tick prefix) inside a turn-based window.
    public static void Tick()
    {
        if (!Sched.On) return;
        try
        {
            var game = Game.Instance;
            var cc = game.TurnBasedCombatController;
            if (cc != null)
            {
                var g = GuardCount(cc.WaitingForUI);
                if (g != _lastGuard)
                {
                    if (_lastGuard >= 0) Sched.Gate("ui_guard_poll", $"{_lastGuard}->{g}");
                    _lastGuard = g;
                }
                var turn = cc.CurrentTurn;
                var status = turn == null ? "none" : turn.Rider?.UniqueId + " " + turn.Status;
                if (status != _lastStatus)
                {
                    Sched.Gate("turn_status", $"{_lastStatus ?? "-"}->{status}");
                    _lastStatus = status;
                }
            }
            if (DotweenDelta != null)
            {
                if (_dotweenComponent == null) _dotweenComponent = UnityEngine.Object.FindObjectOfType(DotweenDelta.DeclaringType);
                if (_dotweenComponent != null)
                {
                    var d = (float)DotweenDelta.GetValue(_dotweenComponent);
                    if (Math.Abs(d - Time.captureDeltaTime) > 1e-6f) Sched.Obs("dotween_dt", j => { j.Prop("v", d); j.Prop("frame", Time.frameCount); });
                }
            }
            {
                var fx = Time.fixedTime;
                var steps = _lastFixed < 0 ? -1 : (int)Math.Round((fx - _lastFixed) / Time.fixedDeltaTime);
                _lastFixed = fx;
                var row = $"steps={steps}|fixedDelta={Time.fixedDeltaTime:R}";
                if (!Last.TryGetValue("~fx", out var oldFx) || oldFx != row)
                {
                    Last["~fx"] = row;
                    Sched.Obs("u", j => { j.Prop("u", "~fx"); j.Prop("s", row); });
                }
            }
            if (!_rowsOpened)
            {
                // First tick: a row for every unit known at the window's start, awake or not.
                _rowsOpened = true;
                foreach (var u in StateDump.AllUnits(game))
                {
                    if (u == null || u.IsDisposed || !_known.Contains(u.UniqueId)) continue;
                    IdlePin.Apply(u); // a turn-based replay: the recording's idle tracker (IdlePin.cs)
                    var row0 = Row(u);
                    Last[u.UniqueId] = row0;
                    var id0 = u.UniqueId;
                    Sched.Obs("u", j => { j.Prop("u", id0); j.Prop("s", row0); });
                    // The idle-animation tracker a unit carries into the window (its time and remaining draw).
                    var tr = IdlePin.Read(u);
                    if (tr.HasValue)
                    {
                        var st = tr.Value;
                        Sched.Obs("idle0", j => { j.Prop("u", id0); j.Prop("time", st.Time); j.Prop("random", st.Random); j.Prop("curve", st.Curve); j.Prop("playing", st.Playing); });
                    }
                }
            }
            GroupObs.Tick();
            CommandObs.Tick();
            {
                // The fog-of-war line-of-sight geometry's updates (UnitSightCache clears its entries when it
                // changed; the stamp is native Time.time, so only the change is noted).
                var lg = Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.FogOfWar.LineOfSightGeometry.Instance;
                var stamp = lg != null ? lg.LastUpdateTime : -1f;
                if (stamp != _losStamp)
                {
                    if (_losStamp != float.MinValue) Sched.Obs("los_geometry");
                    _losStamp = stamp;
                }
            }
            // Which units are awake (State.AwakeUnits: what the controllers tick), as changes.
            var awake = new HashSet<string>();
            foreach (var u in game.State.AwakeUnits) if (u != null && !u.IsDisposed) awake.Add(u.UniqueId);
            if (_awake != null)
            {
                foreach (var id in awake) if (!_awake.Contains(id)) Sched.Obs("awake", j => { j.Prop("u", id); j.Prop("on", true); });
                foreach (var id in _awake) if (!awake.Contains(id)) Sched.Obs("awake", j => { j.Prop("u", id); j.Prop("on", false); });
            }
            else
            {
                Sched.Obs("awake_set", j => { j.Arr("units"); foreach (var id in awake.OrderBy(x => x, StringComparer.Ordinal)) j.W.WriteValue(id); j.EndArr(); });
            }
            _awake = awake;
            foreach (var u in game.State.AwakeUnits)
            {
                if (u == null || u.IsDisposed) continue;
                var id = u.UniqueId;
                if (!_known.Contains(id)) continue; // created during the window: run-specific id
                var row = Row(u);
                if (Last.TryGetValue(id, out var old) && old == row) continue;
                Last[id] = row;
                Sched.Obs("u", j => { j.Prop("u", id); j.Prop("s", row); });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe tick", e); }
    }

    // ---- hooks ----

    public static void PathToPrefix(UnitMovementAgent __instance, Vector3 destination, float approachRadius)
    {
        if (!Sched.On) return;
        var u = __instance.Unit?.EntityData;
        Sched.Obs("path_to", j =>
        {
            j.Prop("u", u?.UniqueId);
            j.Prop("dest", destination);
            j.Prop("radius", approachRadius);
            j.Prop("from", __instance.transform.position);

        });
    }

    public static void PathCompletePrefix(UnitMovementAgent __instance, Path p)
    {
        if (!Sched.On) return;
        var u = __instance.Unit?.EntityData;
        Sched.Obs("path_done", j =>
        {
            j.Prop("u", u?.UniqueId);
            j.Prop("error", p.error);
            j.Prop("nodes", p.vectorPath?.Count ?? -1);
            if (p.vectorPath != null && p.vectorPath.Count > 0) j.Prop("end", p.vectorPath[p.vectorPath.Count - 1]);
            j.Prop("length", p.GetTotalLength());
        });
    }

    public static void JoinCombatPrefix(UnitEntityData __instance, bool notSurprised)
    {
        if (!Sched.On) return;
        Sched.Obs("join_combat", j => { j.Prop("u", __instance.UniqueId); j.Prop("not_surprised", notSurprised); j.Prop("was_in_combat", __instance.IsInCombat); });
    }

    public static void MemoryAddPrefix(UnitGroupMemory __instance, UnitEntityData unit, out bool __state)
    {
        __state = Sched.On && __instance.Find(unit) == null;
    }

    public static void MemoryAddPostfix(UnitGroupMemory __instance, UnitEntityData unit, bool __state)
    {
        if (!__state) return;
        var by = Callers(3);
        Sched.Obs("mem_add", j => { j.Prop("group", GroupId(__instance)); j.Prop("u", unit.UniqueId); j.Prop("by", by); });
    }

    internal static string Callers(int n)
    {
        try
        {
            var frames = new System.Diagnostics.StackTrace(2, false).GetFrames();
            if (frames == null) return "?";
            return string.Join("<", frames.Select(Rng.FrameMethod)
                .Where(m => m != null && m.DeclaringType != null && !(m.DeclaringType.Namespace ?? "").StartsWith("WrathOracle", StringComparison.Ordinal) &&
                            !(m.DeclaringType.Namespace ?? "").StartsWith("HarmonyLib", StringComparison.Ordinal) && !m.Name.StartsWith("DMD<", StringComparison.Ordinal))
                .Take(n).Select(m => m.DeclaringType.Name + "." + m.Name));
        }
        catch (Exception) { return "?"; }
    }

    // UnitVisionRangeController.Tick rebuilt every awake unit's CanBeInVisionRange (an overlap query on
    // the entity-bounds physics scene): the changes, per unit.
    public static void VisionTickPostfix()
    {
        if (!Sched.On) return;
        try
        {
            foreach (var u in Game.Instance.State.AwakeUnits)
            {
                if (u == null || u.IsDisposed) continue;
                var now = new HashSet<string>();
                var order = new List<string>();
                foreach (var o in u.CanBeInVisionRange) if (o != null) { now.Add(o.UniqueId); order.Add(o.UniqueId); }
                // vis_order: the set's enumeration order (insertion order after Clear: the overlap's result
                // order), whenever it differs from the unit's last one.
                VisionOrderLast.TryGetValue(u.UniqueId, out var oldOrder);
                if (oldOrder == null || !oldOrder.SequenceEqual(order))
                {
                    VisionOrderLast[u.UniqueId] = order;
                    var oid = u.UniqueId;
                    Sched.Obs("vis_order", j => { j.Prop("u", oid); j.Arr("v"); foreach (var x in order) j.W.WriteValue(x); j.EndArr(); });
                }
                VisionLast.TryGetValue(u.UniqueId, out var old);
                if (old != null && old.SetEquals(now)) continue;
                var add = now.Where(x => old == null || !old.Contains(x)).OrderBy(x => x, StringComparer.Ordinal).ToList();
                var rem = old == null ? new List<string>() : old.Where(x => !now.Contains(x)).OrderBy(x => x, StringComparer.Ordinal).ToList();
                VisionLast[u.UniqueId] = now;
                var id = u.UniqueId;
                Sched.Obs("vis", j =>
                {
                    j.Prop("u", id);
                    j.Arr("add"); foreach (var x in add) j.W.WriteValue(x); j.EndArr();
                    j.Arr("rem"); foreach (var x in rem) j.W.WriteValue(x); j.EndArr();
                });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe vision", e); }
    }

    // UnitSightCache.GetLOS computes a fresh result only on a cache miss, through UnitEntityData.CheckLOS
    // (its only caller): the observer, the target's eye point, the observer's sampled position, the result.
    public static void CheckLosPostfix(UnitEntityData __instance, Vector3 point, Vector3 overridePosition, ref bool hasLOS)
    {
        if (!Sched.On) return;
        var r = hasLOS;
        Sched.Obs("los", j => { j.Prop("o", __instance.UniqueId); j.Prop("p", point); j.Prop("s", overridePosition); j.Prop("r", r); });
    }

    private static readonly AccessTools.FieldRef<UnitSightCache, UnitEntityData> SightOwner = AccessTools.FieldRefAccess<UnitSightCache, UnitEntityData>("m_Owner");

    public static void SightClearPrefix(UnitSightCache __instance)
    {
        if (!Sched.On) return;
        var o = SightOwner(__instance);
        Sched.Obs("los_clear", j => { j.Prop("o", o?.UniqueId); if (o != null) j.Prop("at", o.Position); });
    }

    public static void Install(Harmony h)
    {
        void P(MethodBase m, string prefix = null, string postfix = null)
        {
            if (m == null) { Health.Failed("observe hook target missing", new Exception(prefix ?? postfix)); return; }
            try
            {
                h.Patch(m, prefix: prefix == null ? null : new HarmonyMethod(typeof(Observe), prefix),
                    postfix: postfix == null ? null : new HarmonyMethod(typeof(Observe), postfix));
            }
            catch (Exception e)
            {
                Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name, e);
            }
        }

        P(AccessTools.Method(typeof(UnitMovementAgent), "PathTo"), prefix: nameof(PathToPrefix));
        P(AccessTools.Method(typeof(UnitMovementAgent), "OnPathComplete", new[] { typeof(Path), typeof(bool) }), prefix: nameof(PathCompletePrefix));
        P(AccessTools.Method(typeof(UnitEntityData), nameof(UnitEntityData.JoinCombat)), prefix: nameof(JoinCombatPrefix));
        try
        {
            h.Patch(AccessTools.Method(typeof(UnitGroupMemory), nameof(UnitGroupMemory.Add)),
                prefix: new HarmonyMethod(typeof(Observe), nameof(MemoryAddPrefix)), postfix: new HarmonyMethod(typeof(Observe), nameof(MemoryAddPostfix)));
        }
        catch (Exception e) { Health.Failed("patch UnitGroupMemory.Add", e); }
        P(AccessTools.Method(typeof(Kingmaker.Controllers.Optimization.UnitVisionRangeController), "Tick"), postfix: nameof(VisionTickPostfix));
        P(AccessTools.Method(typeof(UnitEntityData), nameof(UnitEntityData.CheckLOS)), postfix: nameof(CheckLosPostfix));
        P(AccessTools.Method(typeof(UnitSightCache), "Clear"), prefix: nameof(SightClearPrefix));
    }
}

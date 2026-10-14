using System;
using System.Collections.Generic;
using System.Linq;
using HarmonyLib;
using Kingmaker;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.Settings;
using Kingmaker.UnitLogic.Commands;
using Kingmaker.UnitLogic.Commands.Base;
using Kingmaker.Utility;
using TurnBased.Controllers;

namespace WrathOracle;

// Observations for the turn controller's status (T1 "obs" records, inside a turn-based window), so a
// replay computes when a turn ends (TurnController.Tick: Preparing -> Acting -> Ending -> Ended)
// instead of taking it from "turn_end". Each record holds what that step reads that the simulation
// doesn't compute yet (commands, the view, projectiles, the movement agent, the AI's choice), read
// where the game reads it, and the controller's counters, which the replay computes and checks:
//  - tt (TurnController.Tick's prefix): the status, the counters (TimeWaitedForIdleAI,
//    TimeWaitedToEndTurn, FramesWaitedForStuckAI, AIForcedTickCount), m_WaitForGetUp,
//    UnitCanGetUpOnCommand, the rider's IsDirectlyControllable and Commands.Empty (the forced AI
//    tick's gate), the frame's time;
//  - tt_sc (the tick's "StatusChange" section, at its ProfileScope): what the section reads: the
//    rider's and mount's Commands.Empty, HasRunningCommands, a launched projectile (after the forced
//    AI tick or a Prepare, which run before it in the tick);
//  - tt_able (in the status change): each IsAbleToAct the game evaluates there (a preparing turn's
//    rider, then its mount), with its result;
//  - tt_prep (TurnController.Prepare's postfix, from StartTurn or from the tick once the camera's
//    scroll is over): the status it set and what set it;
//  - tt_ca (ContinueActing, prefix and postfix): what it reads, its result, the counters around it;
//  - tt_cw (ContinueWaiting, prefix and postfix): the same;
//  - tt_end (TurnController.Tick's postfix): the status and counters after the tick.
// Everything is read through fields and plain getters: the rider and mount by their references' ids
// (TurnController.Rider resolves the reference), UnitEntityView.IsGetUp without
// CombatController.IsInTurnBasedCombat (which writes CanChangeWeaponSet), a launched projectile by its
// launcher reference's id (TargetWrapper.Unit resolves it). What the game computes through methods
// with side effects is taken from the game's own evaluation (postfixes): IsAbleToAct (it sets
// Prone.Active and restores it) in the status change and in ContinueActing, IsDirectlyControllable
// and HasExtraAction (it reaches PreparedSpellStrike, whose spellbook getter fills a cache) in
// ContinueActing; null where the game didn't evaluate them. IsDirectlyControllable is called at the
// tick's start only where the tick reads it next (an acting turn whose rider has no command), and
// after Prepare, which read it: the same getter on the same unit, in the same state. Every hook body
// catches its own exceptions.
internal static class TurnTickObs
{
    private static readonly AccessTools.FieldRef<TurnController, UnitReference> RiderRef =
        AccessTools.FieldRefAccess<TurnController, UnitReference>("m_Rider");
    private static readonly AccessTools.FieldRef<TurnController, UnitReference> MountRef =
        AccessTools.FieldRefAccess<TurnController, UnitReference>("m_Mount");
    private static readonly AccessTools.FieldRef<TurnController, UnitCommands> MountCommands =
        AccessTools.FieldRefAccess<TurnController, UnitCommands>("m_MountCommands");
    private static readonly AccessTools.FieldRef<TurnController, List<UnitCommand>> Running =
        AccessTools.FieldRefAccess<TurnController, List<UnitCommand>>("m_RunningCommands");
    private static readonly AccessTools.FieldRef<TurnController, float> IdleWait =
        AccessTools.FieldRefAccess<TurnController, float>("TimeWaitedForIdleAI");
    private static readonly AccessTools.FieldRef<TurnController, int> StuckFrames =
        AccessTools.FieldRefAccess<TurnController, int>("FramesWaitedForStuckAI");
    private static readonly AccessTools.FieldRef<TurnController, int> AiForced =
        AccessTools.FieldRefAccess<TurnController, int>("AIForcedTickCount");
    private static readonly AccessTools.FieldRef<TurnController, bool> WaitForGetUp =
        AccessTools.FieldRefAccess<TurnController, bool>("m_WaitForGetUp");
    private static readonly AccessTools.FieldRef<TurnController, bool> SurpriseRound =
        AccessTools.FieldRefAccess<TurnController, bool>("m_ActingInSurpriseRound");
    private static readonly AccessTools.FieldRef<Kingmaker.View.UnitMovementAgent, bool> NoMoreMovement =
        AccessTools.FieldRefAccess<Kingmaker.View.UnitMovementAgent, bool>("m_NoMoreTurnBasedMovementForUinterruptableCommand");

    private static bool On => Sched.On && Game.Instance?.TurnBasedCombatController != null;

    // Inside TurnController.Tick (the StatusChange scope and Prepare's in_tick).
    private static bool _inTick;

    private static UnitEntityData Find(UnitReference r)
    {
        var id = r.UniqueId;
        if (id == null) return null;
        return StateDump.AllUnits(Game.Instance).FirstOrDefault(x => x != null && !x.IsDisposed && x.UniqueId == id);
    }

    // "TimeWaitedForIdleAI|TimeWaitedToEndTurn|FramesWaitedForStuckAI|AIForcedTickCount".
    private static string Counters(TurnController t) =>
        $"{IdleWait(t):R}|{t.TimeWaitedToEndTurn:R}|{StuckFrames(t)}|{AiForced(t)}";

    // "GameDeltaTime|DeltaTime|PlayerTimeScale|DebugTimeScale|TimeScaleAverage" (m_AnimationTimeDevider).
    private static string Time()
    {
        var tc = Game.Instance.TimeController;
        return $"{tc.GameDeltaTime:R}|{tc.DeltaTime:R}|{tc.PlayerTimeScale:R}|{tc.DebugTimeScale:R}|{Game.Instance.Player.UISettings.TimeScaleAverage:R}";
    }

    private static bool? Empty(UnitEntityData u) => u?.Commands.Empty;

    private static readonly AccessTools.FieldRef<Kingmaker.Controllers.Projectiles.ProjectileController, HashSet<Kingmaker.Controllers.Projectiles.Projectile>> Projectiles =
        AccessTools.FieldRefAccess<Kingmaker.Controllers.Projectiles.ProjectileController, HashSet<Kingmaker.Controllers.Projectiles.Projectile>>("m_Projectiles");

    // ProjectileController.HasLaunchedProjectile(rider, mount) without resolving the launchers'
    // references: an unhit projectile whose launcher reference names the rider or the mount.
    private static bool LaunchedPassive(string rider, string mount)
    {
        foreach (var p in Projectiles(Game.Instance.ProjectileController))
        {
            if (p == null || p.IsHit) continue;
            var id = p.Launcher?.UnitRef.Id;
            if (id != null && (id == rider || (mount != null && id == mount))) return true;
        }
        return false;
    }

    // ---- TurnController.Tick ----

    public static void TickPrefix(TurnController __instance)
    {
        _inTick = false;
        _inSc = false;
        if (!On) return;
        _inTick = true;
        try
        {
            var t = __instance;
            var rider = Find(RiderRef(t));
            var id = RiderRef(t).UniqueId;
            var mount = MountRef(t).UniqueId != null;
            var st = t.Status.ToString();
            var counters = Counters(t);
            var wfgu = WaitForGetUp(t);
            var ucgu = t.UnitCanGetUpOnCommand.Value;
            var empty = Empty(rider);
            // Read by the tick's start only for an acting turn whose rider has no command.
            bool? ctrl = t.Status == TurnController.TurnStatus.Acting && empty == true ? rider?.IsDirectlyControllable : null;
            var surprise = SurpriseRound(t);
            var time = Time();
            Sched.Obs("tt", j =>
            {
                j.Prop("u", id); j.Prop("mount", mount); j.Prop("st", st); j.Prop("w", counters);
                j.Prop("wfgu", wfgu); j.Prop("ucgu", ucgu); j.Prop("ctrl", ctrl); j.Prop("r_empty", empty);
                j.Prop("surprise", surprise); j.Prop("time", time);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe tt", e); }
    }

    public static void TickPostfix(TurnController __instance)
    {
        var was = _inTick;
        _inTick = false;
        _inSc = false;
        if (!On || !was) return;
        try
        {
            var t = __instance;
            var st = t.Status.ToString();
            var counters = Counters(t);
            var wfgu = WaitForGetUp(t);
            Sched.Obs("tt_end", j => { j.Prop("st", st); j.Prop("w", counters); j.Prop("wfgu", wfgu); });
        }
        catch (Exception e) { Trace.HookFailed("observe tt_end", e); }
    }

    // Inside the tick's StatusChange section (its IsAbleToAct calls are recorded as tt_able).
    private static bool _inSc;

    // ProfileScope.New(text, ctx): the tick's StatusChange section starts (tt_sc), or its EndWaiting
    // section (the status change is over).
    public static void ScopePrefix(string text)
    {
        if (!_inTick) return;
        if (text == "TurnController.Tick EndWaiting") { _inSc = false; return; }
        if (text != "TurnController.Tick StatusChange" || !On) return;
        _inSc = true;
        try
        {
            var t = Game.Instance.TurnBasedCombatController.CurrentTurn;
            if (t == null) { Sched.Obs("tt_sc", j => j.Prop("st", (string)null)); return; }
            var rider = Find(RiderRef(t));
            var st = t.Status.ToString();
            var rEmpty = Empty(rider);
            var mc = MountCommands(t);
            bool? mEmpty = mc?.Empty;
            var running = Running(t).Count;
            // Passive (the launchers' reference ids), so read whether or not the section asks.
            var proj = LaunchedPassive(RiderRef(t).UniqueId, MountRef(t).UniqueId);
            Sched.Obs("tt_sc", j =>
            {
                j.Prop("st", st); j.Prop("r_empty", rEmpty); j.Prop("m_empty", mEmpty);
                j.Prop("running", running); j.Prop("proj", proj);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe tt_sc", e); }
    }

    // ---- TurnController.Prepare ----

    public static void PreparePostfix(TurnController __instance)
    {
        if (!On) return;
        try
        {
            var t = __instance;
            var rider = Find(RiderRef(t));
            var mount = Find(MountRef(t));
            var id = RiderRef(t).UniqueId;
            var st = t.Status.ToString();
            var wfgu = WaitForGetUp(t);
            bool? ctrl = rider?.IsDirectlyControllable;
            bool? mctrl = mount?.IsDirectlyControllable;
            bool? prone = rider?.Descriptor.State.Prone.Active;
            var counters = Counters(t);
            var inTick = _inTick;
            Sched.Obs("tt_prep", j =>
            {
                j.Prop("u", id); j.Prop("st", st); j.Prop("wfgu", wfgu); j.Prop("ctrl", ctrl); j.Prop("mctrl", mctrl);
                j.Prop("prone", prone); j.Prop("in_tick", inTick); j.Prop("w", counters);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe tt_prep", e); }
    }

    // ---- ContinueActing and ContinueWaiting ----

    internal sealed class Before
    {
        public Action<Json> Write;
        public string Counters;
    }

    public static void ContinueActingPrefix(TurnController __instance, UnitEntityData unit, out Before __state)
    {
        __state = null;
        if (!On || unit == null) return;
        try
        {
            var t = __instance;
            var id = unit.UniqueId;
            var isRider = id == RiderRef(t).UniqueId;
            var run = unit.Commands.IsRunning();
            var getup = TurnBased.IsGetUpPassive(unit);
            var panicked = unit.Descriptor.State.IsPanicked;
            var inCombat = unit.IsInCombat;
            var ucgu = t.UnitCanGetUpOnCommand.Value;
            var surprise = SurpriseRound(t);
            var motion = unit.HasMotionThisTick;
            var agent = unit.View != null ? unit.View.AgentASP : null;
            bool? wants = agent != null ? agent.WantsToMove : (bool?)null;
            bool? nomore = agent != null ? NoMoreMovement(agent) : (bool?)null;
            var time = Time();
            _caUnit = unit; _caAble = null; _caCtrl = null; _caExtra = null;
            __state = new Before
            {
                Counters = Counters(t),
                Write = j =>
                {
                    j.Prop("u", id); j.Prop("rider", isRider); j.Prop("run", run); j.Prop("getup", getup);
                    j.Prop("panicked", panicked); j.Prop("in_combat", inCombat);
                    j.Prop("ucgu", ucgu); j.Prop("surprise", surprise); j.Prop("motion", motion);
                    j.Prop("wants", wants); j.Prop("nomore", nomore); j.Prop("time", time);
                },
            };
        }
        catch (Exception e) { Trace.HookFailed("observe tt_ca", e); }
    }

    public static void ContinueActingPostfix(TurnController __instance, bool __result, Before __state)
    {
        var (able, ctrl, extra) = (_caAble, _caCtrl, _caExtra);
        _caUnit = null;
        if (!On || __state == null) return;
        try
        {
            var s = __state;
            var after = Counters(__instance);
            Sched.Obs("tt_ca", j =>
            {
                s.Write(j); j.Prop("able", able); j.Prop("ctrl", ctrl); j.Prop("extra", extra);
                j.Prop("wb", s.Counters); j.Prop("res", __result); j.Prop("wa", after);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe tt_ca", e); }
    }

    // ContinueActing's unit, and the values the game computed for it during the call (the last read
    // of each; null when not evaluated).
    private static UnitEntityData _caUnit;
    private static bool? _caAble, _caCtrl, _caExtra;

    // IsAbleToAct as the game evaluates it: for ContinueActing's unit inside the call, or in the
    // status change (a preparing turn's rider, then its mount) as a tt_able record.
    public static void IsAbleToActPostfix(UnitEntityData __instance, bool __result)
    {
        if (_caUnit != null)
        {
            if (ReferenceEquals(__instance, _caUnit)) _caAble = __result;
            return;
        }
        if (!_inSc || !On) return;
        try
        {
            var id = __instance.UniqueId;
            Sched.Obs("tt_able", j => { j.Prop("u", id); j.Prop("res", __result); });
        }
        catch (Exception e) { Trace.HookFailed("observe tt_able", e); }
    }

    public static void ControllablePostfix(UnitEntityData __instance, bool __result)
    {
        if (_caUnit != null && ReferenceEquals(__instance, _caUnit)) _caCtrl = __result;
    }

    public static void ExtraActionPostfix(UnitEntityData unit, bool __result)
    {
        if (_caUnit != null && ReferenceEquals(unit, _caUnit)) _caExtra = __result;
    }

    public static void ContinueWaitingPrefix(TurnController __instance, out Before __state)
    {
        __state = null;
        if (!On) return;
        try
        {
            var t = __instance;
            var rider = Find(RiderRef(t));
            bool? rRun = rider?.Commands.IsRunning();
            var mc = MountCommands(t);
            bool? mRun = mc?.IsRunning();
            var getup = rider == null ? null : TurnBased.IsGetUpPassive(rider);
            var time = Time();
            __state = new Before
            {
                Counters = Counters(t),
                Write = j => { j.Prop("r_run", rRun); j.Prop("m_run", mRun); j.Prop("getup", getup); j.Prop("time", time); },
            };
        }
        catch (Exception e) { Trace.HookFailed("observe tt_cw", e); }
    }

    public static void ContinueWaitingPostfix(TurnController __instance, bool __result, Before __state)
    {
        if (!On || __state == null) return;
        try
        {
            var s = __state;
            var after = Counters(__instance);
            Sched.Obs("tt_cw", j => { s.Write(j); j.Prop("wb", s.Counters); j.Prop("res", __result); j.Prop("wa", after); });
        }
        catch (Exception e) { Trace.HookFailed("observe tt_cw", e); }
    }

    public static void Install(Harmony h)
    {
        void P(System.Reflection.MethodBase m, string prefix = null, string postfix = null)
        {
            if (m == null) { Health.Failed("turn tick observe hook target missing", new Exception(prefix ?? postfix)); return; }
            try
            {
                h.Patch(m, prefix: prefix == null ? null : new HarmonyMethod(typeof(TurnTickObs), prefix),
                    postfix: postfix == null ? null : new HarmonyMethod(typeof(TurnTickObs), postfix));
            }
            catch (Exception e) { Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name, e); }
        }

        var tc = typeof(TurnController);
        P(AccessTools.Method(tc, "Tick"), nameof(TickPrefix), nameof(TickPostfix));
        P(AccessTools.Method(typeof(ProfileScope), nameof(ProfileScope.New), new[] { typeof(string), typeof(UnityEngine.Object) }), nameof(ScopePrefix));
        P(AccessTools.Method(tc, nameof(TurnController.Prepare)), postfix: nameof(PreparePostfix));
        P(AccessTools.Method(tc, "ContinueActing"), nameof(ContinueActingPrefix), nameof(ContinueActingPostfix));
        P(AccessTools.Method(typeof(UnitEntityData), nameof(UnitEntityData.IsAbleToAct)), postfix: nameof(IsAbleToActPostfix));
        P(AccessTools.PropertyGetter(typeof(UnitEntityData), nameof(UnitEntityData.IsDirectlyControllable)), postfix: nameof(ControllablePostfix));
        P(AccessTools.Method(tc, nameof(TurnController.HasExtraAction)), postfix: nameof(ExtraActionPostfix));
        P(AccessTools.Method(tc, "ContinueWaiting"), nameof(ContinueWaitingPrefix), nameof(ContinueWaitingPostfix));
    }
}

using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Controllers.Clicks;
using Kingmaker.Controllers.Combat;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.Settings;
using Kingmaker.UnitLogic.Commands.Base;
using TurnBased.Controllers;

namespace WrathOracle;

// Observations for the units' cooldowns (T1 "obs" records, inside a turn-based window), so a replay
// computes them instead of taking GroupObs' "cooldowns" rows as inputs. Every writer of
// UnitCombatState.Cooldowns is hooked; each record holds the unit's row before and after (a replay
// checks both), plus what the writer reads that the simulation can't compute yet:
//  - cd_spend (UnitEntityData.SpendAction): a command acted (UnitCommandController), or an acting
//    command at the turn's start (TurnController.Prepare): type, full round, time since its start;
//  - cd_move (TurnController.TickMovement): the movement agent's delta in and out, force mode, the
//    turn's movement limit state (the UI's and the AI command's: m_ModifyMovementLimit's current and
//    requested values and the unit's View.IsMoving, which its getter reads), the agent's NearTheEnd, the unit's
//    memoized speed (null when it isn't up to date: reading it would compute it), panic;
//  - cd_force_end (TurnController.ForceToEnd): the end-turn button, an interrupted AI command, a
//    stuck AI or a delay;
//  - cd_cmd_end (TurnController.HandleUnitCommandDidEnd): the command's DoesCommandEmptiesMovement as the
//    handler evaluated it (null if it didn't), PointerController.SimulatingClick and the turn's status;
//  - cd_clear (UnitCombatState.Cooldowns.Clear): who clears (TurnController.Prepare, the prepare
//    controller's batch, UnitCombatState.Clear when leaving combat);
//  - cd_write: the writers not ported (getting up, delaying, mounting, a weapon-set change's
//    animation, the magus' spell combat, DeactivateImmediatelyIfNoAttacksThisRound, using an
//    attack of opportunity): every unit whose row changed.
// TurnController.End's writes follow "turn_end"; UnitCombatCooldownsController, the prepare
// controller and HandleUnitRollsInitiative are ported, and GroupObs' "cooldowns" rows check them
// ("cd_prefix" follows the rows of each prefix).
// A row is "Initiative|StandardAction|MoveAction|SwiftAction|AttackOfOpportunity"; movement stats
// "TimeMoved|TimeMovedInForceMode|TimeMovedByFiveFootStep|MetersMovedByFiveFootStep" (round-trip
// floats). Everything is read through fields and plain getters (TurnController.Rider and GetRider
// resolve references; CurrentSpeedMps computes when stale). Every hook body catches its own
// exceptions.
internal static class CooldownObs
{
    private static readonly AccessTools.FieldRef<TurnController, UnitReference> RiderRef =
        AccessTools.FieldRefAccess<TurnController, UnitReference>("m_Rider");
    private static readonly AccessTools.FieldRef<TurnController, UnitCombatState.Cooldowns> MountCooldown =
        AccessTools.FieldRefAccess<TurnController, UnitCombatState.Cooldowns>("m_MountCooldown");
    private static readonly AccessTools.FieldRef<TurnController, bool> AiUsedFiveFootStep =
        AccessTools.FieldRefAccess<TurnController, bool>("m_AIUsedFiveFootStep");
    private static readonly AccessTools.FieldRef<TurnController, bool> ModifyCurrent =
        AccessTools.FieldRefAccess<TurnController, bool>("m_ModifyMovementCurrentValue");
    private static readonly AccessTools.FieldRef<TurnController, bool> ModifyRequest =
        AccessTools.FieldRefAccess<TurnController, bool>("m_ModifyMovementRequestValue");
    private static readonly AccessTools.FieldRef<TurnController, bool> AutoStop =
        AccessTools.FieldRefAccess<TurnController, bool>("m_AutoStopAfterFirstMoveAction");
    private static readonly FieldInfo RiderStats = AccessTools.Field(typeof(TurnController), "m_RiderMovementStats");
    private static readonly Type StatsType = AccessTools.Inner(typeof(TurnController), "MovementStats");
    private static readonly FieldInfo[] StatsFields =
        new[] { "TimeMoved", "TimeMovedInForceMode", "TimeMovedByFiveFootStep", "MetersMovedByFiveFootStep" }
            .Select(n => AccessTools.Field(StatsType, n)).ToArray();
    private static readonly AccessTools.FieldRef<UnitEntityData, Kingmaker.Utility.PerFrameVar<float>> MemoizedSpeed =
        AccessTools.FieldRefAccess<UnitEntityData, Kingmaker.Utility.PerFrameVar<float>>("m_MemoizedSpeed");

    private static bool On => Sched.On && Game.Instance?.TurnBasedCombatController != null;

    internal static string Row(UnitCombatState.Cooldowns c) =>
        $"{c.Initiative:R}|{c.StandardAction:R}|{c.MoveAction:R}|{c.SwiftAction:R}|{c.AttackOfOpportunity:R}";

    private static string Stats(TurnController t)
    {
        var s = RiderStats.GetValue(t);
        return s == null ? null : string.Join("|", StatsFields.Select(f => ((float)f.GetValue(s)).ToString("R")));
    }

    // The game's methods calling the hooked one, innermost first (the hooked method itself first), at most four (as Rules' origin).
    private static string Caller()
    {
        try
        {
            var frames = new System.Diagnostics.StackTrace(1, false).GetFrames();
            if (frames == null) return "?";
            return string.Join(" < ", frames.Select(Rng.FrameMethod)
                .Where(m => m != null && m.DeclaringType != null && !m.Name.StartsWith("DMD<", StringComparison.Ordinal) &&
                            !(m.DeclaringType.Namespace ?? "").StartsWith("WrathOracle", StringComparison.Ordinal) &&
                            !(m.DeclaringType.Namespace ?? "").StartsWith("HarmonyLib", StringComparison.Ordinal))
                .Take(4).Select(m => m.DeclaringType.FullName + "." + m.Name));
        }
        catch (Exception) { return "?"; }
    }

    // ---- SpendAction ----

    public static void SpendPrefix(UnitEntityData __instance, out string __state)
    {
        __state = null;
        if (!On) return;
        try { __state = Row(__instance.CombatState.Cooldown); }
        catch (Exception e) { Trace.HookFailed("observe cd_spend", e); }
    }

    public static void SpendPostfix(UnitEntityData __instance, UnitCommand.CommandType type, bool isFullRound, float timeSinceCommandStart, string __state)
    {
        if (!On || __state == null) return;
        try
        {
            var after = Row(__instance.CombatState.Cooldown);
            var id = __instance.UniqueId;
            var caller = Caller();
            Sched.Obs("cd_spend", j =>
            {
                j.Prop("u", id); j.Prop("type", (int)type); j.Prop("full", isFullRound);
                j.Prop("tsc", timeSinceCommandStart.ToString("R")); j.Prop("caller", caller);
                j.Prop("b", __state); j.Prop("a", after);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe cd_spend", e); }
    }

    // ---- TickMovement ----

    internal sealed class MoveState
    {
        public string Id, Cd, Stats, Speed;
        public float Dt;
        public bool Force, Near, ModifyCurrent, ModifyRequest, Moving, Ai5, Auto, AutoSetting, Panicked, Mount;
        public int Limit;
    }

    public static void MovePrefix(TurnController __instance, UnitEntityData unit, ref float deltaTime, bool isInForceMode, out MoveState __state)
    {
        __state = null;
        if (!On || unit == null) return;
        try
        {
            var speed = MemoizedSpeed(unit);
            __state = new MoveState
            {
                Id = unit.UniqueId,
                Cd = Row(unit.CombatState.Cooldown),
                Stats = Stats(__instance),
                Speed = speed != null && speed.UpToDate ? speed.Value.ToString("R") : null,
                Dt = deltaTime,
                Force = isInForceMode,
                Near = unit.View != null && unit.View.AgentASP != null && unit.View.AgentASP.NearTheEnd,
                Limit = (int)__instance.CurrentMovementLimit,
                // m_ModifyMovementLimit's getter takes the requested value when the turn's units
                // aren't moving (IsMoving resolves Rider): its fields and the unit's own IsMoving.
                ModifyCurrent = ModifyCurrent(__instance),
                ModifyRequest = ModifyRequest(__instance),
                Moving = unit.View != null && unit.View.IsMoving(),
                Ai5 = AiUsedFiveFootStep(__instance),
                Auto = AutoStop(__instance),
                AutoSetting = SettingsRoot.Game.TurnBased.AutoStopAfterFirstMoveAction,
                Panicked = unit.Descriptor.State.IsPanicked,
                Mount = MountCooldown(__instance) != null,
            };
        }
        catch (Exception e) { Trace.HookFailed("observe cd_move", e); }
    }

    public static void MovePostfix(TurnController __instance, UnitEntityData unit, ref float deltaTime, MoveState __state)
    {
        if (!On || __state == null) return;
        try
        {
            var s = __state;
            var cd = Row(unit.CombatState.Cooldown);
            var stats = Stats(__instance);
            var dtOut = deltaTime;
            var limit = (int)__instance.CurrentMovementLimit;
            Sched.Obs("cd_move", j =>
            {
                j.Prop("u", s.Id); j.Prop("dt", s.Dt.ToString("R")); j.Prop("dt_out", dtOut.ToString("R"));
                j.Prop("force", s.Force); j.Prop("near", s.Near); j.Prop("limit", s.Limit); j.Prop("limit_out", limit);
                j.Prop("modify", s.ModifyCurrent); j.Prop("modify_req", s.ModifyRequest); j.Prop("moving", s.Moving); j.Prop("ai5", s.Ai5); j.Prop("auto", s.Auto); j.Prop("auto_setting", s.AutoSetting);
                j.Prop("speed", s.Speed); j.Prop("panicked", s.Panicked); j.Prop("mount", s.Mount);
                j.Prop("b", s.Cd); j.Prop("a", cd); j.Prop("sb", s.Stats); j.Prop("sa", stats);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe cd_move", e); }
    }

    // ---- ForceToEnd and HandleUnitCommandDidEnd: the rider's row and stats around the call ----

    internal sealed class TurnState
    {
        public string Id, Cd, Stats;
        public bool Mount;
    }

    public static void ForceEndPrefix(TurnController __instance, out TurnState __state)
    {
        __state = null;
        if (!On) return;
        try { __state = AroundPassive(__instance); }
        catch (Exception e) { Trace.HookFailed("observe cd_force_end", e); }
    }

    public static void ForceEndPostfix(TurnController __instance, bool setCooldowns, TurnState __state)
    {
        if (!On || __state == null) return;
        try
        {
            var s = __state;
            var a = AroundPassive(__instance);
            var caller = Caller();
            Sched.Obs("cd_force_end", j =>
            {
                j.Prop("u", s.Id); j.Prop("set", setCooldowns); j.Prop("mount", s.Mount); j.Prop("caller", caller);
                j.Prop("b", s.Cd); j.Prop("a", a.Cd); j.Prop("sb", s.Stats); j.Prop("sa", a.Stats);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe cd_force_end", e); }
    }

    // DoesCommandEmptiesMovement's result as HandleUnitCommandDidEnd evaluates it (calling it again would
    // run ShouldUnitApproach): null when the handler didn't evaluate it (or the call wasn't observed).
    private static bool _inCmdEnd;
    private static bool? _empties;

    public static void EmptiesPostfix(bool __result)
    {
        if (_inCmdEnd) _empties = __result;
    }

    public static void CmdEndPrefix(TurnController __instance, UnitCommand command, out TurnState __state)
    {
        __state = null;
        _inCmdEnd = false;
        _empties = null;
        if (!On || command?.Executor == null) return;
        try
        {
            // The handler acts only for the turn's rider or mount.
            if (command.Executor.UniqueId != RiderRef(__instance).UniqueId && MountCooldown(__instance) == null) return;
            __state = AroundPassive(__instance);
            _inCmdEnd = true;
        }
        catch (Exception e) { Trace.HookFailed("observe cd_cmd_end", e); }
    }

    public static void CmdEndPostfix(TurnController __instance, UnitCommand command, TurnState __state)
    {
        var empties = _empties;
        _inCmdEnd = false;
        _empties = null;
        if (!On || __state == null) return;
        try
        {
            var s = __state;
            var a = AroundPassive(__instance);
            var exec = command.Executor.UniqueId;
            var simulating = PointerController.SimulatingClick;
            var status = __instance.Status.ToString();
            Sched.Obs("cd_cmd_end", j =>
            {
                j.Prop("u", s.Id); j.Prop("exec", exec); j.Prop("class", command.GetType().Name);
                j.Key("empties"); if (empties.HasValue) j.W.WriteValue(empties.Value); else j.W.WriteNull(); j.Prop("simulating", simulating); j.Prop("status", status); j.Prop("mount", s.Mount);
                j.Prop("b", s.Cd); j.Prop("a", a.Cd); j.Prop("sb", s.Stats); j.Prop("sa", a.Stats);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe cd_cmd_end", e); }
    }

    // The rider's row read from its combat state without resolving m_Rider: the unit found by id in
    // the state's units (a disposed or missing rider has no row).
    private static TurnState AroundPassive(TurnController t)
    {
        var id = RiderRef(t).UniqueId;
        var u = StateDump.AllUnits(Game.Instance).FirstOrDefault(x => x != null && x.UniqueId == id);
        return new TurnState
        {
            Id = id,
            Cd = u == null || u.IsDisposed ? null : Row(u.CombatState.Cooldown),
            Stats = Stats(t),
            Mount = MountCooldown(t) != null,
        };
    }

    // ---- Cooldowns.Clear ----

    public static void ClearPrefix(UnitCombatState.Cooldowns __instance)
    {
        if (!On) return;
        try
        {
            var u = StateDump.AllUnits(Game.Instance).FirstOrDefault(x => x != null && !x.IsDisposed && ReferenceEquals(x.CombatState.Cooldown, __instance));
            var id = u?.UniqueId;
            var b = Row(__instance);
            var caller = Caller();
            Sched.Obs("cd_clear", j => { j.Prop("u", id); j.Prop("caller", caller); j.Prop("b", b); });
        }
        catch (Exception e) { Trace.HookFailed("observe cd_clear", e); }
    }

    // ---- the writers not ported ----

    private static Dictionary<string, string> Snapshot() =>
        StateDump.AllUnits(Game.Instance).Where(u => u != null && !u.IsDisposed)
            .GroupBy(u => u.UniqueId).ToDictionary(g => g.Key, g => Row(g.First().CombatState.Cooldown));

    public static void WritePrefix(MethodBase __originalMethod, out Dictionary<string, string> __state)
    {
        __state = null;
        if (!On) return;
        try { __state = Snapshot(); }
        catch (Exception e) { Trace.HookFailed("observe cd_write " + __originalMethod?.Name, e); }
    }

    public static void WritePostfix(MethodBase __originalMethod, Dictionary<string, string> __state)
    {
        if (!On || __state == null) return;
        try
        {
            var writer = __originalMethod.DeclaringType?.Name + "." + __originalMethod.Name;
            string caller = null;
            foreach (var kv in Snapshot())
            {
                if (!__state.TryGetValue(kv.Key, out var b) || b == kv.Value) continue;
                var (id, a) = (kv.Key, kv.Value);
                caller ??= Caller();
                var c = caller;
                Sched.Obs("cd_write", j => { j.Prop("u", id); j.Prop("writer", writer); j.Prop("caller", c); j.Prop("b", b); j.Prop("a", a); });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe cd_write " + __originalMethod?.Name, e); }
    }

    public static void Install(Harmony h)
    {
        void P(MethodBase m, string prefix, string postfix)
        {
            if (m == null) { Health.Failed("cooldown observe hook target missing", new Exception(prefix ?? postfix)); return; }
            try
            {
                h.Patch(m, prefix: prefix == null ? null : new HarmonyMethod(typeof(CooldownObs), prefix),
                    postfix: postfix == null ? null : new HarmonyMethod(typeof(CooldownObs), postfix));
            }
            catch (Exception e) { Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name, e); }
        }

        var tc = typeof(TurnController);
        P(AccessTools.Method(typeof(UnitEntityData), nameof(UnitEntityData.SpendAction)), nameof(SpendPrefix), nameof(SpendPostfix));
        P(AccessTools.Method(tc, "TickMovement"), nameof(MovePrefix), nameof(MovePostfix));
        P(AccessTools.Method(tc, nameof(TurnController.ForceToEnd)), nameof(ForceEndPrefix), nameof(ForceEndPostfix));
        P(AccessTools.Method(tc, nameof(TurnController.HandleUnitCommandDidEnd)), nameof(CmdEndPrefix), nameof(CmdEndPostfix));
        P(AccessTools.Method(typeof(UnitCommand), nameof(UnitCommand.DoesCommandEmptiesMovement)), null, nameof(EmptiesPostfix));
        P(AccessTools.Method(typeof(UnitCombatState.Cooldowns), "Clear"), nameof(ClearPrefix), null);
        foreach (var m in new MethodBase[]
                 {
                     AccessTools.Method(tc, nameof(TurnController.HandleUnitWillGetUp)),
                     AccessTools.Method(tc, nameof(TurnController.DelayInitiaive)),
                     AccessTools.Method(tc, nameof(TurnController.HandleUnitMount)),
                     AccessTools.EnumeratorMoveNext(AccessTools.Method(typeof(Kingmaker.View.Equipment.UnitViewHandsEquipment), "AnimateEquipmentChangeInCombat")),
                     AccessTools.Method(typeof(Kingmaker.Controllers.MagusController), nameof(Kingmaker.Controllers.MagusController.HandleUnitCommandDidAct)),
                     AccessTools.Method(typeof(Kingmaker.UnitLogic.ActivatableAbilities.DeactivateImmediatelyIfNoAttacksThisRound), nameof(Kingmaker.UnitLogic.ActivatableAbilities.DeactivateImmediatelyIfNoAttacksThisRound.OnAbilityTurnedOff)),
                     AccessTools.PropertySetter(typeof(UnitCombatState), nameof(UnitCombatState.AttackOfOpportunityCount)),
                 })
            P(m, nameof(WritePrefix), nameof(WritePostfix));
    }
}

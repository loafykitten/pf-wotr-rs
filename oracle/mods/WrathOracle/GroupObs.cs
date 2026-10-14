using System;
using System.Collections.Generic;
using System.Linq;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Controllers.Combat;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.UnitLogic;
using Kingmaker.UnitLogic.Commands.Base;
using Kingmaker.UnitLogic.Groups;
using TurnBased.Controllers;

namespace WrathOracle;

// Observations for UnitCombatLeaveController (T1 "obs" records, inside a turn-based window): the unit
// groups and their memory, which the sim can't compute (memory follows vision and line of sight), and
// the commands the controller reads (HasOffensiveCommand), at its prefix.
//  - group_state (first tick): every group of UnitGroupsController.Groups in order (its units, combat
//    count, leave timer, the units its memory holds), and each unit's cached m_Group, in State.Units order;
//  - group_new / group_add / group_remove / group_dispose: changes to the groups and their units;
//  - mem_remove / mem_clear: memory removals (mem_add is in Observe);
//  - turn_tick (CombatController.Tick's prefix, every call): WaitingForUI's count, m_IsUnitsChanged;
//  - cooldowns (UnitCombatPrepareController.Tick's prefix): units' cooldowns, as changes, then cd_prefix;
//  - leave_tick (UnitCombatLeaveController.Tick's prefix, every call): the turn-based gate's inputs, and each group's
//    combat count, leave timer and memory as the controller reads it (the dictionary, for Contains, and
//    the cached UnitsList with its validity: Cleanup removes from the dictionary without invalidating
//    the cached list), as changes;
//  - cmds (same prefix): a unit's commands (Raw, then Queue), as changes;
//  - choose_next (CombatController.ChooseNextUnit's prefix): the turn order's inputs it can't compute;
//  - leave_combat (UnitEntityData.LeaveCombat's prefix): who leaves, and from where.
// Everything is read through fields and plain getters: UnitEntityData.Group runs UpdateGroup,
// UnitGroupMemory.UnitsList/Enemies fill caches, a command's TargetUnit resolves an EntityRef (which can
// create a proxy) and CombatController.IsInTurnBasedCombat sets CanChangeWeaponSet, so none is used.
// Every hook body catches its own exceptions.
internal static class GroupObs
{
    private static readonly AccessTools.FieldRef<UnitGroup, List<UnitReference>> GroupUnits =
        AccessTools.FieldRefAccess<UnitGroup, List<UnitReference>>("m_Units");
    private static readonly AccessTools.FieldRef<UnitGroupMemory, Dictionary<string, UnitGroupMemory.UnitInfo>> MemoryUnits =
        AccessTools.FieldRefAccess<UnitGroupMemory, Dictionary<string, UnitGroupMemory.UnitInfo>>("m_Units");
    private static readonly AccessTools.FieldRef<UnitGroupMemory, List<UnitGroupMemory.UnitInfo>> MemoryList =
        AccessTools.FieldRefAccess<UnitGroupMemory, List<UnitGroupMemory.UnitInfo>>("m_UnitsList");
    private static readonly AccessTools.FieldRef<UnitGroupMemory, bool> MemoryListValid =
        AccessTools.FieldRefAccess<UnitGroupMemory, bool>("m_UnitsListValid");
    private static readonly AccessTools.FieldRef<UnitGroupMemory, string> MemoryGroupId =
        AccessTools.FieldRefAccess<UnitGroupMemory, string>("GroupId");
    private static readonly AccessTools.FieldRef<UnitEntityData, UnitGroup> CachedGroup =
        AccessTools.FieldRefAccess<UnitEntityData, UnitGroup>("m_Group");

    // `Target.Memory.Contains(initiator)` read without `UnitEntityData.Group`, whose `UpdateGroup` can
    // create or join a group: the cached group when `UpdateGroup` would keep it, else null (not
    // observed). It keeps a disposed unit's fake group, and a live unit's real undisposed group, or a
    // fake or disposed one when the unit isn't held in the game (`HoldingState == null || !IsInGame`).
    internal static bool? RemembersPassive(UnitEntityData t, UnitEntityData a)
    {
        var g = CachedGroup(t);
        if (g == null) return null;
        var kept = t.IsDisposed
            ? g.IsFake
            : !(g.IsFake || g.Disposed) || t.HoldingState == null || !t.IsInGame;
        return kept ? g.Memory.Contains(a) : (bool?)null;
    }
    private static readonly AccessTools.FieldRef<CombatController, UnitEntityData> NextUnit =
        AccessTools.FieldRefAccess<CombatController, UnitEntityData>("m_NextUnit");
    private static readonly AccessTools.FieldRef<TurnController, UnitReference> RiderRef =
        AccessTools.FieldRefAccess<TurnController, UnitReference>("m_Rider");
    private static readonly AccessTools.FieldRef<CombatController, List<CombatController.TBUnitInfo>> TbUnits =
        AccessTools.FieldRefAccess<CombatController, List<CombatController.TBUnitInfo>>("m_Units");
    private static readonly AccessTools.FieldRef<UnitGroup, Kingmaker.Utility.MultiSet<Kingmaker.Blueprints.BlueprintFaction>> GroupFactions =
        AccessTools.FieldRefAccess<UnitGroup, Kingmaker.Utility.MultiSet<Kingmaker.Blueprints.BlueprintFaction>>("m_Factions");
    private static readonly AccessTools.FieldRef<Kingmaker.Utility.MultiSet<Kingmaker.Blueprints.BlueprintFaction>, Dictionary<Kingmaker.Blueprints.BlueprintFaction, int>> MultiSetData =
        AccessTools.FieldRefAccess<Kingmaker.Utility.MultiSet<Kingmaker.Blueprints.BlueprintFaction>, Dictionary<Kingmaker.Blueprints.BlueprintFaction, int>>("m_Data");
    private static readonly AccessTools.FieldRef<UnitGroup, bool> EnemyForEveryone =
        AccessTools.FieldRefAccess<UnitGroup, bool>("m_IsEnemyForEveryone");

    private static readonly AccessTools.FieldRef<CombatController.TBUnitInfo, bool> Acting =
        AccessTools.FieldRefAccess<CombatController.TBUnitInfo, bool>("ActingInSurpriseRound");
    private static readonly AccessTools.FieldRef<CombatController.TBUnitInfo, bool> Marked =
        AccessTools.FieldRefAccess<CombatController.TBUnitInfo, bool>("MarkedAsLeave");

    // TurnController.Rider resolves its UnitReference (which can create a proxy): the field's id only.
    private static UnitReference Rider(TurnController t) => RiderRef(t);

    private static readonly AccessTools.FieldRef<CombatController, bool> HasEnemyField =
        AccessTools.FieldRefAccess<CombatController, bool>("m_HasEnemyInCombat");

    private static bool _opened;
    private static string _lastGate;
    private static readonly Dictionary<string, string> LastGroup = new Dictionary<string, string>();
    private static readonly Dictionary<string, string> LastCmds = new Dictionary<string, string>();

    public static void ResetWindow()
    {
        _opened = false;
        _lastGate = null;
        LastCooldowns.Clear();
        LastGroup.Clear();
        LastCmds.Clear();
    }

    private static string GroupKey(UnitGroup g) => g == null ? null : g.IsFake ? "~fake" : g.Disposed ? "~disposed:" + g.Id : g.Id;

    // Called from Observe.Tick (GameMode.Tick's prefix) inside a turn-based window.
    public static void Tick()
    {
        if (_opened) return;
        _opened = true;
        try
        {
            var game = Game.Instance;
            Sched.Obs("group_state", j =>
            {
                j.Arr("groups");
                foreach (var g in game.UnitGroups)
                {
                    j.Obj();
                    j.Prop("id", g.Id);
                    j.Arr("units"); foreach (var r in GroupUnits(g)) j.W.WriteValue(r.UniqueId); j.EndArr();
                    j.Prop("in_combat", g.IsInCombat.GuardCount);
                    j.Prop("timer", g.LeaveCombatTimer);
                    j.Arr("memory"); foreach (var id in MemoryUnits(g.Memory).Keys) j.W.WriteValue(id); j.EndArr();
                    j.End();
                }
                j.EndArr();
                j.Arr("units");
                foreach (var u in game.State.Units)
                {
                    if (u == null) continue;
                    j.Arr(); j.W.WriteValue(u.UniqueId); j.W.WriteValue(GroupKey(CachedGroup(u))); j.EndArr();
                }
                j.EndArr();
            });
        }
        catch (Exception e) { Trace.HookFailed("observe group_state", e); }
    }

    public static void GetGroupPrefix(out int __state)
    {
        __state = -1;
        try { if (Sched.On) __state = Game.Instance.UnitGroups.Count; }
        catch (Exception e) { Trace.HookFailed("observe group_new", e); }
    }

    public static void GetGroupPostfix(string id, int __state)
    {
        try
        {
            if (__state < 0 || id == null || Game.Instance.UnitGroups.Count == __state) return;
            Sched.Obs("group_new", j => j.Prop("group", id));
        }
        catch (Exception e) { Trace.HookFailed("observe group_new", e); }
    }

    public static void GroupAddPrefix(UnitGroup __instance, UnitEntityData unit, out bool __state)
    {
        __state = false;
        try { __state = Sched.On && unit != null && !__instance.IsFake && !__instance.Disposed && !GroupUnits(__instance).Any(r => r.UniqueId == unit.UniqueId); }
        catch (Exception e) { Trace.HookFailed("observe group_add", e); }
    }

    public static void GroupAddPostfix(UnitGroup __instance, UnitEntityData unit, bool __state)
    {
        if (!__state) return;
        try
        {
            var ic = unit.IsInCombat;
            var added = GroupUnits(__instance).Any(r => r.UniqueId == unit.UniqueId);
            Sched.Obs("group_add", j => { j.Prop("group", __instance.Id); j.Prop("u", unit.UniqueId); j.Prop("added", added); j.Prop("in_combat", ic); });
        }
        catch (Exception e) { Trace.HookFailed("observe group_add", e); }
    }

    public static void GroupRemovePrefix(UnitGroup __instance, UnitEntityData unit)
    {
        try
        {
            if (!Sched.On || __instance.IsFake || __instance.Disposed || unit == null) return;
            var ic = unit.IsInCombat;
            var had = GroupUnits(__instance).Any(r => r.UniqueId == unit.UniqueId);
            Sched.Obs("group_remove", j => { j.Prop("group", __instance.Id); j.Prop("u", unit.UniqueId); j.Prop("had", had); j.Prop("in_combat", ic); });
        }
        catch (Exception e) { Trace.HookFailed("observe group_remove", e); }
    }

    public static void GroupDisposePrefix(UnitGroup __instance)
    {
        try
        {
            if (!Sched.On || __instance.IsFake) return;
            Sched.Obs("group_dispose", j => j.Prop("group", __instance.Id));
        }
        catch (Exception e) { Trace.HookFailed("observe group_dispose", e); }
    }

    public static void MemoryRemovePrefix(UnitGroupMemory __instance, UnitEntityData unit)
    {
        try
        {
            if (!Sched.On || unit == null || !MemoryUnits(__instance).ContainsKey(unit.UniqueId)) return;
            var g = MemoryGroupId(__instance);
            Sched.Obs("mem_remove", j => { j.Prop("group", g); j.Prop("u", unit.UniqueId); j.Prop("by", "Remove"); });
        }
        catch (Exception e) { Trace.HookFailed("observe mem_remove", e); }
    }

    public static void MemoryCleanupPrefix(UnitGroupMemory __instance, out List<string> __state)
    {
        __state = null;
        try { if (Sched.On) __state = MemoryUnits(__instance).Keys.ToList(); }
        catch (Exception e) { Trace.HookFailed("observe mem_remove", e); }
    }

    public static void MemoryCleanupPostfix(UnitGroupMemory __instance, List<string> __state)
    {
        if (__state == null) return;
        try
        {
            var now = MemoryUnits(__instance);
            var g = MemoryGroupId(__instance);
            foreach (var id in __state.Where(x => !now.ContainsKey(x)).OrderBy(x => x, StringComparer.Ordinal))
                Sched.Obs("mem_remove", j => { j.Prop("group", g); j.Prop("u", id); j.Prop("by", "Cleanup"); });
        }
        catch (Exception e) { Trace.HookFailed("observe mem_remove", e); }
    }

    public static void MemoryClearPrefix(UnitGroupMemory __instance)
    {
        try
        {
            if (!Sched.On) return;
            var n = MemoryUnits(__instance).Count;
            if (n == 0) return;
            var g = MemoryGroupId(__instance);
            Sched.Obs("mem_clear", j => { j.Prop("group", g); j.Prop("n", n); });
        }
        catch (Exception e) { Trace.HookFailed("observe mem_clear", e); }
    }

    private static string CmdRow(UnitCommand c) =>
        c.GetType().Name + "|" + (c.Target?.UnitRef.Id ?? "") + "|" + (c.IsStarted ? 1 : 0) + (c.IsFinished ? 1 : 0);

    // The memory as UnitCombatLeaveController reads it: the dictionary's keys in its enumeration order
    // (Contains; UnitsList rebuilds from it in that order), and the cached UnitsList in order when it is
    // valid (ShouldEngageEnemy's loop), else "~invalid".
    private static string MemoryRow(UnitGroupMemory m) =>
        "dict=" + string.Join(",", MemoryUnits(m).Keys) +
        "|list=" + (MemoryListValid(m) ? string.Join(",", MemoryList(m).Select(i => i.UnitReference.UniqueId)) : "~invalid");

    // The caches UnitGroup.IsEnemy reads (UnitGroupsController defers attack-faction updates to its tick):
    // m_Factions (faction:count, in order), m_AttackFactions, m_IsEnemyForEveryone.
    private static string FactionRow(UnitGroup g) =>
        "f=" + string.Join(",", MultiSetData(GroupFactions(g)).Select(kv => Snap.Guid(kv.Key) + ":" + kv.Value)) +
        "|af=" + string.Join(",", g.AttackFactions.Select(f => Snap.Guid(f))) +
        "|efe=" + (EnemyForEveryone(g) ? 1 : 0);

    // UnitCombatLeaveController's inputs at its Tick's prefix, and at a direct TickGroups call
    // (DialogController.Start: ignoreTimer), as changes.
    private static void Snapshot(string via, bool ignoreTimer)
    {
        var game = Game.Instance;
        var cc = game.TurnBasedCombatController;
        // CombatController.IsInTurnBasedCombat() without its CanChangeWeaponSet side effect.
        var tb = game.Player.IsInCombat && game.Player.IsTurnBasedModeOn() && game.CurrentMode != Kingmaker.GameModes.GameModeType.Cutscene;
        var current = cc.CurrentTurn != null ? Rider(cc.CurrentTurn).UniqueId ?? "?" : null;
        var gate = $"tb={(tb ? 1 : 0)}|has_enemy={(HasEnemyField(cc) ? 1 : 0)}|had_enemy={(cc.HadEnemyAtSomePoint ? 1 : 0)}" +
                   $"|passing={(CombatController.IsPassing() ? 1 : 0)}|current={current}|next={NextUnit(cc)?.UniqueId}|player_in_combat={(game.Player.IsInCombat ? 1 : 0)}";
        var groups = new List<(string id, string row)>();
        foreach (var g in game.UnitGroups)
        {
            var row = $"{g.IsInCombat.GuardCount}|{g.LeaveCombatTimer:R}|{MemoryRow(g.Memory)}|{FactionRow(g)}";
            if (LastGroup.TryGetValue(g.Id, out var old) && old == row) continue;
            LastGroup[g.Id] = row;
            groups.Add((g.Id, row));
        }
        // Every call (it orders the frame's other records against the controller), the groups as
        // changes.
        {
            _lastGate = gate;
            Sched.Obs("leave_tick", j =>
            {
                j.Prop("via", via);
                if (via != "Tick") j.Prop("ignore_timer", ignoreTimer);
                j.Prop("gate", gate);
                j.Arr("groups"); foreach (var (id, row) in groups) { j.Arr(); j.W.WriteValue(id); j.W.WriteValue(row); j.EndArr(); } j.EndArr();
            });
        }
        // Every unit, suppressed ones too (a group's members are walked unfiltered).
        foreach (var u in StateDump.AllUnits(game))
        {
            if (u == null || u.IsDisposed) continue;
            var row = string.Join(";", u.Commands.Raw.Where(c => c != null).Select(CmdRow)) + "/" + string.Join(";", u.Commands.Queue.Where(c => c != null).Select(CmdRow));
            if (LastCmds.TryGetValue(u.UniqueId, out var old) ? old == row : row == "/") continue;
            LastCmds[u.UniqueId] = row;
            var id = u.UniqueId;
            Sched.Obs("cmds", j => { j.Prop("u", id); j.Prop("s", row); });
        }
    }

    private static bool _inTick;

    public static void LeaveTickPrefix()
    {
        _inTick = true;
        if (!Sched.On) return;
        try { Snapshot("Tick", false); }
        catch (Exception e) { Trace.HookFailed("observe leave_tick", e); }
    }

    public static void LeaveTickPostfix() => _inTick = false;

    public static void TickGroupsPrefix(bool ignoreTimer)
    {
        if (_inTick || !Sched.On) return;
        try { Snapshot("TickGroups", ignoreTimer); }
        catch (Exception e) { Trace.HookFailed("observe leave_tick", e); }
    }

    // CombatController.ChooseNextUnit's inputs: the current turn's rider and m_NextUnit, then each
    // TBUnitInfo of m_Units in order with the flags it reads (Surprised, ActingInSurpriseRound),
    // MarkedAsLeave, IsVisibleForPlayer (fog of war and the view: not computable) and whether the unit
    // can carry a rider (SaddledPart). Units are found by id among the state's units, without resolving
    // a reference.
    public static void ChooseNextPrefix(CombatController __instance)
    {
        if (!Sched.On) return;
        try
        {
            var byId = new Dictionary<string, UnitEntityData>();
            foreach (var u in StateDump.AllUnits(Game.Instance)) if (u != null) byId[u.UniqueId] = u;
            var current = __instance.CurrentTurn != null ? Rider(__instance.CurrentTurn).UniqueId ?? "?" : null;
            var next = NextUnit(__instance)?.UniqueId;
            var units = TbUnits(__instance).Select(i =>
            {
                var id = i.UnitRef.UniqueId;
                byId.TryGetValue(id ?? "", out var u);
                var vis = u != null && !u.IsInFogOfWar && u.View != null && u.View.IsVisible && u.IsInGame;
                return $"{id}|{(i.Surprised ? 1 : 0)}{(Acting(i) ? 1 : 0)}{(Marked(i) ? 1 : 0)}|vis={(u == null ? "?" : vis ? "1" : "0")}|saddled={(u?.SaddledPart != null ? 1 : 0)}";
            }).ToList();
            Sched.Obs("choose_next", j =>
            {
                j.Prop("current", current);
                j.Prop("next", next);
                j.Arr("units"); foreach (var r in units) j.W.WriteValue(r); j.EndArr();
            });
        }
        catch (Exception e) { Trace.HookFailed("observe choose_next", e); }
    }

    public static void LeaveCombatPrefix(UnitEntityData __instance)
    {
        try
        {
            if (!Sched.On) return;
            var ic = __instance.IsInCombat;
            var by = Observe.Callers(3);
            Sched.Obs("leave_combat", j => { j.Prop("u", __instance.UniqueId); j.Prop("in_combat", ic); j.Prop("by", by); });
        }
        catch (Exception e) { Trace.HookFailed("observe leave_combat", e); }
    }

    private static readonly AccessTools.FieldRef<CombatController, bool> UnitsChanged =
        AccessTools.FieldRefAccess<CombatController, bool>("m_IsUnitsChanged");
    private static readonly AccessTools.FieldRef<Kingmaker.Utility.CountingGuard, int> GuardCount =
        AccessTools.FieldRefAccess<Kingmaker.Utility.CountingGuard, int>("m_GuardCount");
    private static readonly Dictionary<string, string> LastCooldowns = new Dictionary<string, string>();

    // CombatController.Tick's prefix, every call: WaitingForUI's count and m_IsUnitsChanged (it orders
    // the UI guard's changes against the turn controller).
    public static void TurnTickPrefix(CombatController __instance)
    {
        if (!Sched.On) return;
        try
        {
            var row = $"guard={GuardCount(__instance.WaitingForUI)}|changed={(UnitsChanged(__instance) ? 1 : 0)}";
            Sched.Obs("turn_tick", j => j.Prop("s", row));
        }
        catch (Exception e) { Trace.HookFailed("observe turn_tick", e); }
    }

    // UnitCombatPrepareController.Tick's prefix: the cooldowns of the units in combat, as changes
    // (HandleUnitRollsInitiative reads them through GetTimeToNextTurn; the cooldown controller isn't
    // ported).
    public static void PrepareTickPrefix()
    {
        if (!Sched.On) return;
        try
        {
            foreach (var u in StateDump.AllUnits(Game.Instance))
            {
                if (u == null || u.IsDisposed || !u.IsInCombat) continue;
                var c = u.CombatState.Cooldown;
                var row = $"{c.Initiative:R}|{c.StandardAction:R}|{c.MoveAction:R}|{c.SwiftAction:R}|{c.AttackOfOpportunity:R}";
                if (LastCooldowns.TryGetValue(u.UniqueId, out var old) && old == row) continue;
                LastCooldowns[u.UniqueId] = row;
                var id = u.UniqueId;
                Sched.Obs("cooldowns", j => { j.Prop("u", id); j.Prop("s", row); });
            }
            // The rows of this prefix are complete (CooldownObs: a replay checks its own here).
            Sched.Obs("cd_prefix");
        }
        catch (Exception e) { Trace.HookFailed("observe cooldowns", e); }
    }

    public static void Install(Harmony h)
    {
        void P(System.Reflection.MethodBase m, string prefix = null, string postfix = null)
        {
            if (m == null) { Health.Failed("group observe hook target missing", new Exception(prefix ?? postfix)); return; }
            try
            {
                h.Patch(m, prefix: prefix == null ? null : new HarmonyMethod(typeof(GroupObs), prefix),
                    postfix: postfix == null ? null : new HarmonyMethod(typeof(GroupObs), postfix));
            }
            catch (Exception e) { Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name, e); }
        }

        P(AccessTools.Method(typeof(Kingmaker.Controllers.Units.UnitGroupsController), nameof(Kingmaker.Controllers.Units.UnitGroupsController.GetGroup)), nameof(GetGroupPrefix), nameof(GetGroupPostfix));
        P(AccessTools.Method(typeof(UnitGroup), nameof(UnitGroup.Add)), nameof(GroupAddPrefix), nameof(GroupAddPostfix));
        P(AccessTools.Method(typeof(UnitGroup), nameof(UnitGroup.Remove)), prefix: nameof(GroupRemovePrefix));
        P(AccessTools.Method(typeof(UnitGroup), nameof(UnitGroup.Dispose)), prefix: nameof(GroupDisposePrefix));
        P(AccessTools.Method(typeof(UnitGroupMemory), nameof(UnitGroupMemory.Remove)), prefix: nameof(MemoryRemovePrefix));
        P(AccessTools.Method(typeof(UnitGroupMemory), nameof(UnitGroupMemory.Cleanup)), nameof(MemoryCleanupPrefix), nameof(MemoryCleanupPostfix));
        P(AccessTools.Method(typeof(UnitGroupMemory), nameof(UnitGroupMemory.Clear)), prefix: nameof(MemoryClearPrefix));
        P(AccessTools.Method(typeof(UnitCombatLeaveController), nameof(UnitCombatLeaveController.Tick)), nameof(LeaveTickPrefix), nameof(LeaveTickPostfix));
        P(AccessTools.Method(typeof(UnitCombatLeaveController), nameof(UnitCombatLeaveController.TickGroups)), prefix: nameof(TickGroupsPrefix));
        P(AccessTools.Method(typeof(CombatController), "ChooseNextUnit"), prefix: nameof(ChooseNextPrefix));
        P(AccessTools.Method(typeof(CombatController), nameof(CombatController.Tick)), prefix: nameof(TurnTickPrefix));
        P(AccessTools.Method(typeof(UnitCombatPrepareController), nameof(UnitCombatPrepareController.Tick)), prefix: nameof(PrepareTickPrefix));
        P(AccessTools.Method(typeof(UnitEntityData), nameof(UnitEntityData.LeaveCombat)), prefix: nameof(LeaveCombatPrefix));
    }
}

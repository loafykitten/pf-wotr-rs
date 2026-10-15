using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using System.Text;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Blueprints;
using Kingmaker.Controllers;
using Kingmaker.Controllers.Units;
using Kingmaker.EntitySystem;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.UnitLogic;
using Kingmaker.UnitLogic.Groups;
using Kingmaker.Utility;

namespace WrathOracle;

// Observer "memory" (slice H1, docs/poc5/memory.md): UnitMemoryController's tick and the groups' memory, so a check
// (and later the driven replay) runs the controller and compares the memory it leaves:
//  - mem_init: at the window's first tick, every group of UnitGroupsController.Groups in order (then every unit's fake
//    group whose memory holds an entry, as "~fake:<unit>") with its memory (below); then mem_initq, the controller's
//    m_UnitsForAdd in order, and UnitGroupMemory.s_UnitsForRemove (pending: what a Cleanup that threw left there);
//  - mem_tick / mem_tick_end: UnitMemoryController.Tick, with GameTime, the frame UpdateVisibility reads
//    (Time.frameCount as WallClock pins it), the queue and pending; at its end GameTime again and whether it threw;
//  - mem_v: a read the controller's code makes outside itself, by a transpiler on the call (the game's own single
//    evaluation, then the record), in UnitMemoryController.Tick: apeace (the area's Settings.Peaceful), ready
//    (ReadyForCombatUnitGroups), near (the set ExtractNearUnits filled, in its enumeration order), peace (a near unit's
//    Faction.Peaceful, read from the faction the call returned), pass (its Passive), grp (UnitEntityData.Group), disp
//    (IsDisposed), stl (InStealthFor), getgrp (UnitGroupsController.GetGroup), wake (Wake, with AwakeTimer before and
//    after); in UpdateVisibility: stl, enemy (UnitGroup.IsEnemy), gdt (GameDeltaTime), los (UnitGroup.HasLOS), inv
//    (UnitState.HasCondition), seen (the invisibility loop's result, by a transpiler on its store into Visible); in
//    UnitGroupMemory.Cleanup: unit (UnitInfo.Unit), ingame (IsInGame);
//  - mem_s: a memory's state: after each ready group's Cleanup ("pass"), and at the tick's entry ("pre") and end
//    ("end") each group's of UnitGroupsController.Groups whose state differs from the one last recorded for its id (a
//    group never recorded is taken as a new memory: no slots, caches invalid);
//  - mem_q: each AddToMemory (observer, target, the target's Faction.Peaceful, whether it queued, whether it threw, the
//    caller chain);
//  - in_game: each change of a unit's IsInGame, with the caller chain, before its effects;
// and, while installed: group_remove's caller chain (by), Cleanup's mem_remove in removal order (GroupObs), and hash
// in replays too (WallClock: the value the game got, with whether it was the tape's).
// A memory is written as its dictionary's slots in array order (reference source's Dictionary: entries[0, count),
// a used slot as [key, LastDetectTime ticks, Visible], a freed one as [null, next]), freeList, freeCount, and both
// caches (m_UnitsList, m_EnemiesList: their ids while valid, else null). Everything is read through fields; every
// hook catches its own exceptions.
internal static partial class CommandObs
{
    internal static bool MemOn;

    // UnitMemoryController.Tick is running (its own code calls the tapped methods).
    private static int _memIn;

    // The near unit whose Faction the tick read last (its Passive comes next).
    private static string _memCur;

    // Each group's memory as last recorded, by group id.
    private static readonly Dictionary<string, string> LastMem = new Dictionary<string, string>();

    private static void ResetMemWindow()
    {
        LastMem.Clear();
        _memCur = null;
    }

    private static bool MemTapOn => On && MemOn && _memIn > 0;

    // ---- the memory's fields ----

    private static readonly AccessTools.FieldRef<UnitGroupMemory, Dictionary<string, UnitGroupMemory.UnitInfo>> MemUnits =
        AccessTools.FieldRefAccess<UnitGroupMemory, Dictionary<string, UnitGroupMemory.UnitInfo>>("m_Units");
    private static readonly AccessTools.FieldRef<UnitGroupMemory, List<UnitGroupMemory.UnitInfo>> MemList =
        AccessTools.FieldRefAccess<UnitGroupMemory, List<UnitGroupMemory.UnitInfo>>("m_UnitsList");
    private static readonly AccessTools.FieldRef<UnitGroupMemory, bool> MemListValid =
        AccessTools.FieldRefAccess<UnitGroupMemory, bool>("m_UnitsListValid");
    private static readonly AccessTools.FieldRef<UnitGroupMemory, List<UnitGroupMemory.UnitInfo>> MemEnemies =
        AccessTools.FieldRefAccess<UnitGroupMemory, List<UnitGroupMemory.UnitInfo>>("m_EnemiesList");
    private static readonly AccessTools.FieldRef<UnitGroupMemory, bool> MemEnemiesValid =
        AccessTools.FieldRefAccess<UnitGroupMemory, bool>("m_EnemiesListValid");
    private static readonly AccessTools.FieldRef<UnitGroupMemory, string> MemGroupId =
        AccessTools.FieldRefAccess<UnitGroupMemory, string>("GroupId");
    private static readonly AccessTools.FieldRef<UnitEntityData, UnitGroup> MemCachedGroup =
        AccessTools.FieldRefAccess<UnitEntityData, UnitGroup>("m_Group");

    // Dictionary<string, UnitInfo>'s arrays (the reference source's names).
    private static readonly Type MemDict = typeof(Dictionary<string, UnitGroupMemory.UnitInfo>);
    private static readonly FieldInfo MemDictEntries = AccessTools.Field(MemDict, "entries");
    private static readonly FieldInfo MemDictCount = AccessTools.Field(MemDict, "count");
    private static readonly FieldInfo MemDictFreeList = AccessTools.Field(MemDict, "freeList");
    private static readonly FieldInfo MemDictFreeCount = AccessTools.Field(MemDict, "freeCount");
    private static readonly Type MemEntry = MemDictEntries?.FieldType.GetElementType();
    private static readonly FieldInfo MemEntryHash = MemEntry == null ? null : AccessTools.Field(MemEntry, "hashCode");
    private static readonly FieldInfo MemEntryNext = MemEntry == null ? null : AccessTools.Field(MemEntry, "next");
    private static readonly FieldInfo MemEntryKey = MemEntry == null ? null : AccessTools.Field(MemEntry, "key");
    private static readonly FieldInfo MemEntryValue = MemEntry == null ? null : AccessTools.Field(MemEntry, "value");

    // UnitMemoryController.m_UnitsForAdd (a list of a private struct, UnitsPair) and the pair's fields.
    private static readonly FieldInfo MemForAdd = AccessTools.Field(typeof(UnitMemoryController), "m_UnitsForAdd");
    private static readonly Type MemPair = MemForAdd?.FieldType.IsGenericType == true ? MemForAdd.FieldType.GetGenericArguments()[0] : null;
    private static readonly FieldInfo MemPairObserver = MemPair == null ? null : AccessTools.Field(MemPair, "Observer");
    private static readonly FieldInfo MemPairTarget = MemPair == null ? null : AccessTools.Field(MemPair, "Target");

    // UnitGroupMemory.s_UnitsForRemove (static: what a Cleanup that threw left there is removed by the next one, first)
    // and UnitInfo.Visible.
    private static readonly FieldInfo MemForRemove = AccessTools.Field(typeof(UnitGroupMemory), "s_UnitsForRemove");
    private static readonly FieldInfo MemVisible = AccessTools.Field(typeof(UnitGroupMemory.UnitInfo), nameof(UnitGroupMemory.UnitInfo.Visible));

    private static List<string> MemPending() => InfoIds(MemForRemove.GetValue(null) as List<UnitGroupMemory.UnitInfo>);

    private struct MemSlot { public string Key; public long? Ticks; public bool? Vis; public int Next; }

    private sealed class MemSnap
    {
        public string G;
        public readonly List<MemSlot> Slots = new List<MemSlot>();
        public int Free, NFree;
        public List<string> List, Enemies;
        public string Sig;
    }

    private static List<string> InfoIds(List<UnitGroupMemory.UnitInfo> l) => l?.Select(i => i?.UnitReference.UniqueId).ToList();

    // A memory's state, read through its fields and its dictionary's arrays.
    private static MemSnap MemOf(UnitGroupMemory m, string g)
    {
        var s = new MemSnap { G = g };
        var d = MemUnits(m);
        var n = (int)MemDictCount.GetValue(d);
        var entries = (Array)MemDictEntries.GetValue(d);
        for (var i = 0; i < n; i++)
        {
            var e = entries.GetValue(i);
            if ((int)MemEntryHash.GetValue(e) >= 0)
            {
                var info = (UnitGroupMemory.UnitInfo)MemEntryValue.GetValue(e);
                s.Slots.Add(new MemSlot { Key = (string)MemEntryKey.GetValue(e), Ticks = info?.LastDetectTime.Ticks, Vis = info?.Visible });
            }
            else s.Slots.Add(new MemSlot { Next = (int)MemEntryNext.GetValue(e) });
        }
        s.Free = (int)MemDictFreeList.GetValue(d);
        s.NFree = (int)MemDictFreeCount.GetValue(d);
        s.List = MemListValid(m) ? InfoIds(MemList(m)) : null;
        s.Enemies = MemEnemiesValid(m) ? InfoIds(MemEnemies(m)) : null;
        s.Sig = MemSig(s.Slots, s.Free, s.NFree, s.List, s.Enemies);
        return s;
    }

    // The state as a string, for changes (freeList counts only while a slot is free: a dictionary that never held a
    // key has 0 there, a cleared one -1).
    private static string MemSig(List<MemSlot> slots, int free, int nfree, List<string> list, List<string> enemies)
    {
        var b = new StringBuilder();
        foreach (var x in slots)
        {
            if (x.Key == null) b.Append("~").Append(x.Next.ToString(CultureInfo.InvariantCulture));
            else
                b.Append(x.Key).Append(':').Append(x.Ticks.HasValue ? x.Ticks.Value.ToString(CultureInfo.InvariantCulture) : "null")
                    .Append(':').Append(x.Vis.HasValue ? (x.Vis.Value ? "1" : "0") : "null");
            b.Append(',');
        }
        b.Append('|').Append((nfree == 0 ? -1 : free).ToString(CultureInfo.InvariantCulture)).Append('|').Append(nfree.ToString(CultureInfo.InvariantCulture));
        b.Append("|L:").Append(list == null ? "~" : string.Join(",", list.Select(x => x ?? "null")));
        b.Append("|E:").Append(enemies == null ? "~" : string.Join(",", enemies.Select(x => x ?? "null")));
        return b.ToString();
    }

    private static readonly string MemFreshSig = MemSig(new List<MemSlot>(), -1, 0, null, null);

    private static void WriteIds(Json j, string k, List<string> ids)
    {
        j.Key(k);
        if (ids == null) { j.W.WriteNull(); return; }
        j.W.WriteStartArray();
        foreach (var x in ids) j.W.WriteValue(x);
        j.W.WriteEndArray();
    }

    private static void WriteMem(Json j, MemSnap s)
    {
        j.Prop("g", s.G);
        j.Arr("slots");
        foreach (var x in s.Slots)
        {
            j.Arr();
            if (x.Key == null) { j.W.WriteNull(); j.W.WriteValue(x.Next); }
            else
            {
                j.W.WriteValue(x.Key);
                if (x.Ticks.HasValue) j.W.WriteValue(x.Ticks.Value); else j.W.WriteNull();
                if (x.Vis.HasValue) j.W.WriteValue(x.Vis.Value); else j.W.WriteNull();
            }
            j.EndArr();
        }
        j.EndArr();
        j.Prop("free", s.Free);
        j.Prop("nfree", s.NFree);
        WriteIds(j, "list", s.List);
        WriteIds(j, "enemies", s.Enemies);
    }

    // A group as the leaves name it: its id, "~fake" or "~disposed:<id>".
    private static string MemKey(UnitGroup g)
    {
        try { return g == null ? null : g.IsFake ? "~fake" : g.Disposed ? "~disposed:" + g.Id : g.Id; }
        catch (Exception e) { Trace.HookFailed("observe memory group", e); return "?"; }
    }

    // The queue's pairs (observer, target), in order; null when it can't be read.
    private static List<(string o, string t)> MemQueue(UnitMemoryController c)
    {
        if (c == null || !(MemForAdd.GetValue(c) is IList l)) return null;
        var r = new List<(string, string)>();
        foreach (var p in l) r.Add((U(MemPairObserver.GetValue(p) as UnitEntityData), U(MemPairTarget.GetValue(p) as UnitEntityData)));
        return r;
    }

    private static void WritePairs(Json j, string k, List<(string o, string t)> q)
    {
        j.Key(k);
        if (q == null) { j.W.WriteNull(); return; }
        j.W.WriteStartArray();
        foreach (var (o, t) in q) { j.W.WriteStartArray(); j.W.WriteValue(o); j.W.WriteValue(t); j.W.WriteEndArray(); }
        j.W.WriteEndArray();
    }

    private static long? MemGameTime()
    {
        try { return Game.Instance.TimeController.GameTime.Ticks; }
        catch (Exception e) { Trace.HookFailed("observe memory game time", e); return null; }
    }

    private static void PropLong(Json j, string k, long? v)
    {
        j.Key(k);
        if (v.HasValue) j.W.WriteValue(v.Value); else j.W.WriteNull();
    }

    // ---- the initial state ----

    private static void MemInit()
    {
        try
        {
            var game = Game.Instance;
            foreach (var g in game.UnitGroups)
            {
                if (g == null) continue;
                var s = MemOf(g.Memory, g.Id);
                LastMem[g.Id ?? ""] = s.Sig;
                Sched.Obs("mem_init", j => WriteMem(j, s));
            }
            foreach (var u in StateDump.AllUnits(game))
            {
                var g = u == null ? null : MemCachedGroup(u);
                if (g == null || !g.IsFake || MemUnits(g.Memory).Count == 0) continue;
                var s = MemOf(g.Memory, "~fake:" + u.UniqueId);
                Sched.Obs("mem_init", j => WriteMem(j, s));
            }
            var q = MemQueue(game.UnitMemoryController);
            var pending = MemPending();
            Sched.Obs("mem_initq", j => { WritePairs(j, "q", q); WriteIds(j, "pending", pending); });
        }
        catch (Exception e) { Trace.HookFailed("observe mem_init", e); }
    }

    // Each group's memory whose state differs from the one last recorded for its id.
    private static void MemChanges(string at)
    {
        foreach (var g in Game.Instance.UnitGroups)
        {
            if (g?.Id == null) continue;
            var s = MemOf(g.Memory, g.Id);
            if (!LastMem.TryGetValue(g.Id, out var old)) old = MemFreshSig;
            if (old == s.Sig) continue;
            LastMem[g.Id] = s.Sig;
            Sched.Obs("mem_s", j => { j.Prop("at", at); WriteMem(j, s); });
        }
    }

    // ---- the tick ----

    public static void MemTickPrefix(UnitMemoryController __instance)
    {
        _memIn++;
        if (!On || !MemOn) return;
        try
        {
            var gt = MemGameTime();
            var frame = WallClock.PeekFrame();
            var q = MemQueue(__instance);
            var pending = MemPending();
            Sched.Obs("mem_tick", j => { PropLong(j, "gt", gt); j.Prop("frame", frame); WritePairs(j, "q", q); WriteIds(j, "pending", pending); });
            MemChanges("pre");
        }
        catch (Exception e) { Trace.HookFailed("observe mem_tick", e); }
    }

    public static void MemTickFinalizer(Exception __exception)
    {
        if (_memIn > 0) _memIn--;
        if (!On || !MemOn) return;
        try
        {
            MemChanges("end");
            var gt = MemGameTime();
            var exc = __exception != null;
            Sched.Obs("mem_tick_end", j => { PropLong(j, "gt", gt); j.Prop("exc", exc); });
        }
        catch (Exception e) { Trace.HookFailed("observe mem_tick_end", e); }
    }

    // UnitGroupMemory.Cleanup in the tick: the group's memory after its pass.
    public static void MemCleanupPostfix(UnitGroupMemory __instance)
    {
        try
        {
            if (!MemTapOn) return;
            var id = MemGroupId(__instance);
            var s = MemOf(__instance, id);
            if (id != null) LastMem[id] = s.Sig;
            Sched.Obs("mem_s", j => { j.Prop("at", "pass"); WriteMem(j, s); });
        }
        catch (Exception e) { Trace.HookFailed("observe mem_s pass", e); }
    }

    // ---- AddToMemory ----

    public static void MemQueuePrefix(UnitMemoryController __instance, out int __state)
    {
        __state = -1;
        try
        {
            if (On && MemOn && MemForAdd.GetValue(__instance) is IList l) __state = l.Count;
        }
        catch (Exception e) { Trace.HookFailed("observe mem_q", e); }
    }

    public static void MemQueueFinalizer(UnitMemoryController __instance, UnitEntityData observer, UnitEntityData target, int __state, Exception __exception)
    {
        if (__state < 0) return;
        try
        {
            var n = MemForAdd.GetValue(__instance) is IList l ? l.Count : -1;
            var queued = n > __state;
            var (o, t) = (U(observer), U(target));
            var f = target?.Descriptor?.Faction;
            bool? peaceful = f == null ? (bool?)null : f.Peaceful;
            var exc = __exception != null;
            var by = Observe.Callers(4);
            Sched.Obs("mem_q", j =>
            {
                j.Prop("o", o); j.Prop("t", t); j.Prop("peaceful", peaceful); j.Prop("queued", queued); j.Prop("exc", exc); j.Prop("by", by);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe mem_q", e); }
    }

    // ---- IsInGame ----

    public static void MemInGamePrefix(EntityDataBase __instance, bool value)
    {
        try
        {
            if (!On || !MemOn || !(__instance is UnitEntityData) || __instance.IsInGame == value) return;
            var id = __instance.UniqueId;
            var by = Observe.Callers(4);
            Sched.Obs("in_game", j => { j.Prop("u", id); j.Prop("v", value); j.Prop("by", by); });
        }
        catch (Exception e) { Trace.HookFailed("observe in_game", e); }
    }

    // ---- taps ----

    private static void MemV(string f, Action<Json> body)
    {
        try
        {
            if (!MemTapOn) return;
            Sched.Obs("mem_v", j => { j.Prop("f", f); body(j); });
        }
        catch (Exception e) { Trace.HookFailed("observe mem_v " + f, e); }
    }

    public static bool TapMemAreaPeaceful(CountableFlag flag)
    {
        var v = (bool)flag;
        MemV("apeace", j => j.Prop("v", v));
        return v;
    }

    public static List<UnitGroup> TapMemReady(Game game)
    {
        var v = game.ReadyForCombatUnitGroups;
        if (MemTapOn)
        {
            List<string> ids = null;
            try { ids = v?.Select(MemKey).ToList(); }
            catch (Exception e) { Trace.HookFailed("observe mem_v ready", e); }
            MemV("ready", j => WriteIds(j, "v", ids));
        }
        return v;
    }

    public static void TapMemNear(UnitGroup group, HashSet<UnitEntityData> result)
    {
        group.ExtractNearUnits(result);
        if (MemTapOn)
        {
            string g = null;
            List<string> ids = null;
            try { g = MemKey(group); ids = result?.Select(u => u?.UniqueId).ToList(); }
            catch (Exception e) { Trace.HookFailed("observe mem_v near", e); }
            MemV("near", j => { j.Prop("g", g); WriteIds(j, "v", ids); });
        }
    }

    public static BlueprintFaction TapMemFaction(UnitEntityData unit)
    {
        var v = unit.Faction;
        if (MemTapOn)
        {
            var u = U(unit);
            _memCur = u;
            bool? p = v == null ? (bool?)null : v.Peaceful;
            MemV("peace", j => { j.Prop("u", u); j.Prop("v", p); });
        }
        return v;
    }

    public static bool TapMemPassive(ObservableCountFlag flag)
    {
        var v = (bool)flag;
        var u = _memCur;
        MemV("pass", j => { j.Prop("u", u); j.Prop("v", v); });
        return v;
    }

    public static UnitGroup TapMemGroup(UnitEntityData unit)
    {
        var v = unit.Group;
        if (MemTapOn)
        {
            var (u, g) = (U(unit), MemKey(v));
            MemV("grp", j => { j.Prop("u", u); j.Prop("v", g); });
        }
        return v;
    }

    public static bool TapMemDisposed(EntityDataBase entity)
    {
        var v = entity.IsDisposed;
        if (MemTapOn)
        {
            var u = U(entity);
            MemV("disp", j => { j.Prop("u", u); j.Prop("v", v); });
        }
        return v;
    }

    public static bool TapMemStealth(UnitEntityData unit, UnitGroup observer)
    {
        var v = unit.InStealthFor(observer);
        if (MemTapOn)
        {
            var (u, g) = (U(unit), MemKey(observer));
            MemV("stl", j => { j.Prop("u", u); j.Prop("g", g); j.Prop("v", v); });
        }
        return v;
    }

    public static UnitGroup TapMemGetGroup(UnitGroupsController controller, string id)
    {
        var v = controller.GetGroup(id);
        if (MemTapOn)
        {
            var g = MemKey(v);
            MemV("getgrp", j => { j.Prop("id", id); j.Prop("v", g); });
        }
        return v;
    }

    public static void TapMemWake(UnitEntityData unit, float time, bool isForFade)
    {
        float? before = null;
        try { if (unit != null) before = isForFade ? unit.FadeAwakeTimer : unit.AwakeTimer; }
        catch (Exception e) { Trace.HookFailed("observe mem_v wake", e); }
        unit.Wake(time, isForFade);
        if (MemTapOn)
        {
            var u = U(unit);
            var after = isForFade ? unit.FadeAwakeTimer : unit.AwakeTimer;
            MemV("wake", j =>
            {
                j.Prop("u", u); j.Prop("time", F(time)); j.Prop("fade", isForFade);
                j.Prop("b", before.HasValue ? F(before.Value) : null); j.Prop("a", F(after));
            });
        }
    }

    public static bool TapMemEnemy(UnitGroup group, UnitEntityData unit)
    {
        var v = group.IsEnemy(unit);
        if (MemTapOn)
        {
            var (g, u) = (MemKey(group), U(unit));
            MemV("enemy", j => { j.Prop("g", g); j.Prop("u", u); j.Prop("v", v); });
        }
        return v;
    }

    public static float TapMemDelta(TimeController time)
    {
        var v = time.GameDeltaTime;
        MemV("gdt", j => j.Prop("v", F(v)));
        return v;
    }

    public static bool TapMemLos(UnitGroup group, UnitEntityData unit)
    {
        var v = group.HasLOS(unit);
        if (MemTapOn)
        {
            var (g, u) = (MemKey(group), U(unit));
            MemV("los", j => { j.Prop("g", g); j.Prop("u", u); j.Prop("v", v); });
        }
        return v;
    }

    public static bool TapMemCondition(UnitState state, UnitCondition condition)
    {
        var v = state.HasCondition(condition);
        if (MemTapOn)
        {
            string u = null;
            try { u = state.Owner?.Unit?.UniqueId; }
            catch (Exception e) { Trace.HookFailed("observe mem_v inv", e); }
            var c = (int)condition;
            MemV("inv", j => { j.Prop("u", u); j.Prop("c", c); j.Prop("v", v); });
        }
        return v;
    }

    // `unitInfo.Visible = flag2`: the store itself, then the loop's result.
    public static void TapMemSeen(UnitGroupMemory.UnitInfo info, bool value)
    {
        info.Visible = value;
        if (MemTapOn)
        {
            string k = null;
            try { k = info.UnitReference.UniqueId; }
            catch (Exception e) { Trace.HookFailed("observe mem_v seen", e); }
            MemV("seen", j => { j.Prop("u", k); j.Prop("v", value); });
        }
    }

    public static UnitEntityData TapMemUnit(UnitGroupMemory.UnitInfo info)
    {
        var v = info.Unit;
        if (MemTapOn)
        {
            string k = null;
            try { k = info.UnitReference.UniqueId; }
            catch (Exception e) { Trace.HookFailed("observe mem_v unit", e); }
            var r = U(v);
            MemV("unit", j => { j.Prop("u", k); j.Prop("v", r); });
        }
        return v;
    }

    public static bool TapMemInGame(EntityDataBase entity)
    {
        var v = entity.IsInGame;
        if (MemTapOn)
        {
            var u = U(entity);
            MemV("ingame", j => { j.Prop("u", u); j.Prop("v", v); });
        }
        return v;
    }

    private static readonly List<AiSite> MemSites = new List<AiSite>();

    // UpdateVisibility's `unitInfo.Visible = flag2` (the invisibility loop's result): the one store of UnitInfo.Visible
    // whose value isn't a constant (the others store true and false).
    private const string MemSeenKey = "mem:UnitMemoryController.UpdateVisibility:TapMemSeen";

    public static IEnumerable<CodeInstruction> MemTapTranspiler(IEnumerable<CodeInstruction> instructions, MethodBase original)
    {
        CodeInstruction prev = null;
        var inVis = original.DeclaringType == typeof(UnitMemoryController) && original.Name == "UpdateVisibility";
        foreach (var i in instructions)
        {
            if (inVis && i.opcode == OpCodes.Stfld && i.operand is FieldInfo fi && fi == MemVisible && prev != null &&
                !prev.opcode.Name.StartsWith("ldc.i4", StringComparison.Ordinal))
            {
                i.opcode = OpCodes.Call;
                i.operand = AccessTools.Method(typeof(CommandObs), nameof(TapMemSeen));
                Tapped[MemSeenKey] = Tapped.TryGetValue(MemSeenKey, out var c) ? c + 1 : 1;
            }
            else if ((i.opcode == OpCodes.Call || i.opcode == OpCodes.Callvirt) && i.operand is MethodBase mb)
            {
                foreach (var s in MemSites)
                {
                    if (s.Owner != original.DeclaringType || s.Method != original.Name || mb.Name != s.Name || mb.DeclaringType != s.Type ||
                        !mb.GetParameters().Select(p => p.ParameterType).SequenceEqual(s.Args)) continue;
                    i.opcode = OpCodes.Call;
                    i.operand = AccessTools.Method(typeof(CommandObs), s.Tap);
                    var k = "mem:" + s.Owner.Name + "." + s.Method + ":" + s.Tap;
                    Tapped[k] = Tapped.TryGetValue(k, out var n) ? n + 1 : 1;
                    break;
                }
            }
            prev = i;
            yield return i;
        }
    }

    // ---- install ----

    private static void InstallMemory(Harmony h, Action<MethodBase, string, string, string> patch)
    {
        void Fail(string what) => Health.Failed("memory observe: " + what, new Exception("memory"));
        // Declared only when every check and patch below succeeded.
        var failed = Health.FailedCount;
        if (!ImpOn) { Fail("the impatience observer isn't installed"); return; }
        if (MemDictEntries == null || MemDictCount == null || MemDictFreeList == null || MemDictFreeCount == null || MemEntryHash == null ||
            MemEntryNext == null || MemEntryKey == null || MemEntryValue == null)
        {
            Fail("Dictionary's arrays (entries, count, freeList, freeCount; hashCode, next, key, value) are missing");
            return;
        }
        if (MemForAdd == null || MemPairObserver == null || MemPairTarget == null) { Fail("UnitMemoryController.m_UnitsForAdd is missing"); return; }
        if (MemForRemove == null || MemVisible == null) { Fail("UnitGroupMemory.s_UnitsForRemove or UnitInfo.Visible is missing"); return; }
        var all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        MethodBase M(Type t, string name, params Type[] args)
        {
            var ms = t.GetMethods(all).Where(m => m.Name == name && (args.Length == 0 || m.GetParameters().Select(p => p.ParameterType).SequenceEqual(args))).ToList();
            if (ms.Count != 1) { Fail(t.Name + "." + name + ": " + ms.Count + " methods"); return null; }
            return ms[0];
        }
        var ctl = typeof(UnitMemoryController);
        var ue = typeof(UnitEntityData);
        var tick = M(ctl, nameof(UnitMemoryController.Tick));
        var vis = M(ctl, "UpdateVisibility");
        var cleanup = M(typeof(UnitGroupMemory), nameof(UnitGroupMemory.Cleanup));
        var add = M(ctl, nameof(UnitMemoryController.AddToMemory));
        var inGame = AccessTools.PropertySetter(typeof(EntityDataBase), nameof(EntityDataBase.IsInGame));
        if (tick == null || vis == null || cleanup == null || add == null || inGame == null) { Fail("a method is missing"); return; }

        void S(MethodBase owner, Type type, string name, string tap, int want, params Type[] args) =>
            MemSites.Add(new AiSite { Owner = owner.DeclaringType, Method = owner.Name, Type = type, Name = name, Args = args, Tap = tap, Want = want });
        S(tick, typeof(CountableFlag), "op_Implicit", nameof(TapMemAreaPeaceful), 1, typeof(CountableFlag));
        S(tick, typeof(Game), "get_ReadyForCombatUnitGroups", nameof(TapMemReady), 1);
        S(tick, typeof(UnitGroup), nameof(UnitGroup.ExtractNearUnits), nameof(TapMemNear), 1, typeof(HashSet<UnitEntityData>));
        S(tick, ue, "get_Faction", nameof(TapMemFaction), 1);
        S(tick, typeof(ObservableCountFlag), "op_Implicit", nameof(TapMemPassive), 1, typeof(ObservableCountFlag));
        S(tick, ue, "get_Group", nameof(TapMemGroup), 4);
        S(tick, typeof(EntityDataBase), "get_IsDisposed", nameof(TapMemDisposed), 2);
        S(tick, typeof(UnitStealthExtension), nameof(UnitStealthExtension.InStealthFor), nameof(TapMemStealth), 1, ue, typeof(UnitGroup));
        S(tick, typeof(UnitGroupsController), nameof(UnitGroupsController.GetGroup), nameof(TapMemGetGroup), 1, typeof(string));
        S(tick, ue, nameof(UnitEntityData.Wake), nameof(TapMemWake), 1, typeof(float), typeof(bool));
        S(vis, typeof(UnitStealthExtension), nameof(UnitStealthExtension.InStealthFor), nameof(TapMemStealth), 1, ue, typeof(UnitGroup));
        S(vis, typeof(UnitGroup), nameof(UnitGroup.IsEnemy), nameof(TapMemEnemy), 1, ue);
        S(vis, typeof(TimeController), "get_GameDeltaTime", nameof(TapMemDelta), 1);
        S(vis, typeof(UnitGroup), nameof(UnitGroup.HasLOS), nameof(TapMemLos), 1, ue);
        S(vis, typeof(UnitState), nameof(UnitState.HasCondition), nameof(TapMemCondition), 1, typeof(UnitCondition));
        S(cleanup, typeof(UnitGroupMemory.UnitInfo), "get_Unit", nameof(TapMemUnit), 2);
        S(cleanup, typeof(EntityDataBase), "get_IsInGame", nameof(TapMemInGame), 1);
        var tr = new HarmonyMethod(typeof(CommandObs), nameof(MemTapTranspiler));
        foreach (var m in new[] { tick, vis, cleanup })
        {
            try { h.Patch(m, transpiler: tr); }
            catch (Exception e) { Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name + " (memory taps)", e); }
        }
        foreach (var s in MemSites.GroupBy(s => "mem:" + s.Owner.Name + "." + s.Method + ":" + s.Tap))
        {
            var want = s.Sum(x => x.Want);
            if (!Tapped.TryGetValue(s.Key, out var n) || n != want)
                Fail("tap " + s.Key + " replaced " + (Tapped.TryGetValue(s.Key, out var x) ? x : 0) + " calls, not " + want);
        }
        if (!Tapped.TryGetValue(MemSeenKey, out var seen) || seen != 1)
            Fail("tap " + MemSeenKey + " replaced " + (Tapped.TryGetValue(MemSeenKey, out var y) ? y : 0) + " stores, not 1");

        patch(tick, nameof(MemTickPrefix), null, nameof(MemTickFinalizer));
        patch(cleanup, null, nameof(MemCleanupPostfix), null);
        patch(add, nameof(MemQueuePrefix), null, nameof(MemQueueFinalizer));
        patch(inGame, nameof(MemInGamePrefix), null, null);
        MemOn = Health.FailedCount == failed;
    }
}

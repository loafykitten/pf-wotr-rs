using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Controllers.Combat;
using Kingmaker.EntitySystem;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.Enums;
using Kingmaker.Items;
using Kingmaker.Items.Slots;
using Kingmaker.UnitLogic;
using Kingmaker.UnitLogic.Parts;
using Kingmaker.Utility;
using Kingmaker.View;
using TurnBased.Controllers;

namespace WrathOracle;

// Observer "engage" (slice E, docs/poc5/commands.md): the engagement controller's code is base lifecycle code, so a
// replay runs UnitCombatEngagementController (Tick, TickUnit), UnitEngagementExtension.IsEngage,
// UnitHelper.IsAttackOfOpportunityReach and UnitCombatState's engagement (IsEngage, Engage, Disengage,
// ShouldAttackOnDisengage, AttackOfOpportunity and the count), and takes as inputs only what that code reads outside
// itself:
//  - entries: uc_eng (UnitCombatEngagementController.Tick, with its caller) and uc_eng_end;
//  - leaves (uc_v), each read where the engagement's code makes it, by a transpiler on the call (the record made when
//    the innermost entry is the engagement's and every frame up to it base code): threat, trange, enemies, corp, sqd,
//    los, conc, tact, last, manual, immune, motion, tbon, prevent, fmove, memc, passive, canatk (the existing leaves
//    canact, caic and untgt are recorded inside it as everywhere else);
//  - checks: eng_tu (TickUnit: its unit and m_AttackOfOpportunityCount), eng_e (Engage and Disengage: the unit's
//    m_EngagedUnits and the target's m_EngagedBy after it, in their enumeration order), eng_aoo (AttackOfOpportunity:
//    its arguments, result and the count after it), eng_prov (ProvokeAttackOfOpportunity,
//    ProvokeAttackOfOpportunityAfterDefensivelyFail and ForceAttackOfOpportunity, from anywhere, with the caller);
//  - eng_init: at the window's first tick, every unit's engagement state (with each dictionary's used slots: freed
//    slots keep their place, so only a dictionary that never held an entry, or was cleared, is known fresh).
// Dictionaries are read through their Keys (no lookups), the count through its field.
internal static partial class CommandObs
{
    // Whether the engagement's code is lifecycle code (InstallEngage sets it; the trace's begin lists the observer).
    internal static bool EngOn;

    private static bool InEng => Entries.Count > 0 && Entries[Entries.Count - 1].Eng && Entries[Entries.Count - 1].Rec;

    private static bool EngChain(MethodBase m) => On && EngOn && InEng && Chain(m) == 1;

    private static readonly AccessTools.FieldRef<UnitCombatState, Dictionary<UnitEntityData, TimeSpan>> EngagedUnitsField =
        AccessTools.FieldRefAccess<UnitCombatState, Dictionary<UnitEntityData, TimeSpan>>("m_EngagedUnits");
    private static readonly AccessTools.FieldRef<UnitCombatState, Dictionary<UnitEntityData, TimeSpan>> EngagedByField =
        AccessTools.FieldRefAccess<UnitCombatState, Dictionary<UnitEntityData, TimeSpan>>("m_EngagedBy");
    private static readonly AccessTools.FieldRef<UnitCombatState, List<UnitEntityData>> AttackedByAooField =
        AccessTools.FieldRefAccess<UnitCombatState, List<UnitEntityData>>("m_AttackedByAooUnits");
    private static readonly AccessTools.FieldRef<UnitCombatState, int> AooCountField =
        AccessTools.FieldRefAccess<UnitCombatState, int>("m_AttackOfOpportunityCount");

    // Dictionary<,>'s count of entry slots used (freed ones included: a freed slot keeps its place
    // and is reused first), so a dictionary with no keys is known fresh only when it's 0.
    private static readonly FieldInfo DictSlots =
        AccessTools.Field(typeof(Dictionary<UnitEntityData, TimeSpan>), "count") ??
        AccessTools.Field(typeof(Dictionary<UnitEntityData, TimeSpan>), "_count");

    private static int? Slots(Dictionary<UnitEntityData, TimeSpan> d)
    {
        try { return d == null || DictSlots == null ? (int?)null : (int)DictSlots.GetValue(d); }
        catch (Exception e) { Trace.HookFailed("observe engage slots", e); return null; }
    }

    private static void WriteUnits(Json j, string k, IEnumerable<UnitEntityData> us)
    {
        j.Key(k);
        if (us == null) { j.W.WriteNull(); return; }
        j.W.WriteStartArray();
        foreach (var u in us) j.W.WriteValue(u?.UniqueId);
        j.W.WriteEndArray();
    }

    private static string CsUnit(UnitCombatState cs)
    {
        try { return cs?.Unit?.UniqueId; }
        catch (Exception e) { Trace.HookFailed("observe engage unit", e); return null; }
    }

    // ---- the initial state ----

    // Every unit's engagement state at the window's first tick.
    private static void EngInit()
    {
        try
        {
            foreach (var u in StateDump.AllUnits(Game.Instance))
            {
                var cs = u?.CombatState;
                if (cs == null) continue;
                var id = u.UniqueId;
                var cnt = AooCountField(cs);
                var eu = EngagedUnitsField(cs)?.Keys.ToList();
                var eb = EngagedByField(cs)?.Keys.ToList();
                var euSlots = Slots(EngagedUnitsField(cs));
                var ebSlots = Slots(EngagedByField(cs));
                var dat = cs.DisengageAttackTargets?.ToList();
                var abu = AttackedByAooField(cs)?.ToList();
                var prev = cs.PreventAttacksOfOpporunityNextFrame;
                Sched.Obs("eng_init", j =>
                {
                    j.Prop("u", id); j.Prop("cnt", cnt); WriteUnits(j, "eu", eu); WriteUnits(j, "eb", eb);
                    j.Key("eu_slots"); if (euSlots.HasValue) j.W.WriteValue(euSlots.Value); else j.W.WriteNull();
                    j.Key("eb_slots"); if (ebSlots.HasValue) j.W.WriteValue(ebSlots.Value); else j.W.WriteNull();
                    WriteUnits(j, "dat", dat); WriteUnits(j, "abu", abu); j.Prop("prevent", prev);
                });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe eng_init", e); }
    }

    // ---- the entry ----

    public static void EngEntryPrefix(MethodBase __originalMethod)
    {
        if (!Enter(__originalMethod, out var nested, eng: true)) return;
        try
        {
            var origin = Origin();
            Sched.Obs("uc_eng", j => { j.Prop("nested", nested); j.Prop("origin", origin); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_eng", e); }
    }

    public static void EngEntryFinalizer(Exception __exception)
    {
        if (!Pop() || !On) return;
        try
        {
            var exc = __exception != null;
            Sched.Obs("uc_eng_end", j => j.Prop("exc", exc));
        }
        catch (Exception e) { Trace.HookFailed("observe uc_eng_end", e); }
    }

    // ---- checks ----

    public static void TickUnitPrefix(UnitEntityData unit, MethodBase __originalMethod)
    {
        try
        {
            if (!EngChain(__originalMethod)) return;
            var u = unit?.UniqueId;
            var cnt = unit == null ? 0 : AooCountField(unit.CombatState);
            Sched.Obs("eng_tu", j => { j.Prop("u", u); j.Prop("cnt", cnt); });
        }
        catch (Exception e) { Trace.HookFailed("observe eng_tu", e); }
    }

    // Engage(target) and Disengage(target): the unit's engaged units and the target's engaged-by after it.
    public static void EngageFinalizer(UnitCombatState __instance, UnitEntityData target, Exception __exception, MethodBase __originalMethod)
    {
        try
        {
            if (!EngChain(__originalMethod)) return;
            var fn = __originalMethod.Name;
            var u = CsUnit(__instance);
            var t = target?.UniqueId;
            var eu = EngagedUnitsField(__instance)?.Keys.ToList();
            var eb = target == null ? null : EngagedByField(target.CombatState)?.Keys.ToList();
            var dat = __instance.DisengageAttackTargets?.ToList();
            var exc = __exception != null;
            Sched.Obs("eng_e", j =>
            {
                j.Prop("fn", fn); j.Prop("u", u); j.Prop("tgt", t); WriteUnits(j, "eu", eu); WriteUnits(j, "eb", eb);
                WriteUnits(j, "dat", dat); j.Prop("exc", exc);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe eng_e", e); }
    }

    public static void AooFinalizer(UnitCombatState __instance, UnitEntityData target, bool tricksterAttack, bool simulate, bool disengaging,
        bool afterDefensivelyFail, bool __result, Exception __exception, MethodBase __originalMethod)
    {
        try
        {
            if (!EngChain(__originalMethod)) return;
            var u = CsUnit(__instance);
            var t = target?.UniqueId;
            var cnt = AooCountField(__instance);
            var abu = AttackedByAooField(__instance)?.ToList();
            var exc = __exception != null;
            Sched.Obs("eng_aoo", j =>
            {
                j.Prop("u", u); j.Prop("tgt", t); j.Prop("trick", tricksterAttack); j.Prop("sim", simulate); j.Prop("dis", disengaging);
                j.Prop("adf", afterDefensivelyFail); j.Prop("ret", __result); j.Prop("cnt", cnt); WriteUnits(j, "abu", abu); j.Prop("exc", exc);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe eng_aoo", e); }
    }

    // ProvokeAttackOfOpportunity(unit), ProvokeAttackOfOpportunityAfterDefensivelyFail(unit) and
    // ForceAttackOfOpportunity(attacker, target, tricksterAttack), from anywhere.
    public static void ProvokePrefix(object[] __args, MethodBase __originalMethod)
    {
        try
        {
            if (!On || !EngOn) return;
            var fn = __originalMethod.Name;
            var a = (__args.Length > 0 ? __args[0] as UnitEntityData : null)?.UniqueId;
            var t = (__args.Length > 1 ? __args[1] as UnitEntityData : null)?.UniqueId;
            var trick = __args.Length > 2 && __args[2] is bool b && b;
            var origin = Origin();
            Sched.Obs("eng_prov", j =>
            {
                j.Prop("fn", fn); j.Prop("u", a); j.Prop("tgt", t); j.Prop("trick", trick); j.Prop("origin", origin);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe eng_prov", e); }
    }

    // ---- taps ----

    private static void EngTap(string f, Action<Json> body)
    {
        try
        {
            if (!EngChain(null)) return;
            Emit(f, body);
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v " + f, e); }
    }

    public static WeaponSlot TapEngThreat(UnitEntityData u)
    {
        var v = u.GetThreatHand();
        try
        {
            if (EngChain(null))
            {
                var id = U(u);
                var s = v == null ? null : SlotStr(v, u.Body);
                Emit("threat", j => { j.Prop("u", id); j.Prop("v", s); });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v threat", e); }
        return v;
    }

    public static float? TapThreatRange(UnitEntityData u, WeaponSlot hand)
    {
        var v = u.GetThreatRange(hand);
        var id = U(u);
        EngTap("trange", j => { j.Prop("u", id); j.Prop("v", v.HasValue ? F(v.Value) : null); });
        return v;
    }

    public static List<UnitGroupMemory.UnitInfo> TapEngEnemies(UnitGroupMemory m)
    {
        var v = m.Enemies;
        EngTap("enemies", j =>
        {
            j.Key("v");
            if (v == null) { j.W.WriteNull(); return; }
            j.W.WriteStartArray();
            foreach (var x in v) { if (x == null) j.W.WriteNull(); else j.W.WriteValue(x.UnitReference.UniqueId ?? ""); }
            j.W.WriteEndArray();
        });
        return v;
    }

    public static float TapEngCorp(UnitEntityView view)
    {
        var v = view.Corpulence;
        var u = U(Safe(() => view.EntityData));
        EngTap("corp", j => { j.Prop("u", u); j.Prop("v", F(v)); });
        return v;
    }

    public static float TapSqd(UnitEntityData a, UnitEntityData b)
    {
        var v = a.SqrDistanceTo(b);
        var ua = U(a);
        var ub = U(b);
        EngTap("sqd", j => { j.Prop("u", ua); j.Prop("to", ub); j.Prop("v", F(v)); });
        return v;
    }

    public static bool TapLos(UnitEntityData a, UnitEntityData b)
    {
        var v = a.HasLOS(b);
        var ua = U(a);
        var ub = U(b);
        EngTap("los", j => { j.Prop("u", ua); j.Prop("to", ub); j.Prop("v", v); });
        return v;
    }

    public static Concealment TapConc(UnitEntityData a, UnitEntityData b, bool attack)
    {
        var v = UnitPartConcealment.Calculate(a, b, attack);
        var ua = U(a);
        var ub = U(b);
        var n = (int)v;
        EngTap("conc", j => { j.Prop("u", ua); j.Prop("to", ub); j.Prop("attack", attack); j.Prop("v", n); });
        return v;
    }

    public static bool TapEngTact()
    {
        var v = Kingmaker.Armies.TacticalCombat.TacticalCombatHelper.IsActive;
        EngTap("tact", j => j.Prop("v", v));
        return v;
    }

    public static UnitEntityData TapEngLast(UnitCombatState cs)
    {
        var v = cs.LastTarget;
        var u = CsUnit(cs);
        var id = U(v);
        EngTap("last", j => { j.Prop("u", u); j.Prop("v", id); });
        return v;
    }

    public static UnitEntityData TapEngManual(UnitCombatState cs)
    {
        var v = cs.ManualTarget;
        var u = CsUnit(cs);
        var id = U(v);
        EngTap("manual", j => { j.Prop("u", u); j.Prop("v", id); });
        return v;
    }

    public static bool TapImmune(TurnController tc)
    {
        var v = tc.ImmuneAttackOfOpportunityOnDisengage;
        EngTap("immune", j => j.Prop("v", v));
        return v;
    }

    public static bool TapMotion(UnitEntityData u)
    {
        var v = u.HasMotionThisTick;
        var id = U(u);
        EngTap("motion", j => { j.Prop("u", id); j.Prop("v", v); });
        return v;
    }

    public static bool TapTbOn(Player p)
    {
        var v = p.IsTurnBasedModeOn();
        EngTap("tbon", j => j.Prop("v", v));
        return v;
    }

    public static bool TapPrevent(UnitCombatState cs)
    {
        var v = cs.PreventAttacksOfOpporunityNextFrame;
        var u = CsUnit(cs);
        EngTap("prevent", j => { j.Prop("u", u); j.Prop("v", v); });
        return v;
    }

    public static UnitPartForceMove TapForceMove(EntityDataBase e)
    {
        var v = e.Get<UnitPartForceMove>();
        var b = !ReferenceEquals(v, null);
        var id = U(e);
        EngTap("fmove", j => { j.Prop("u", id); j.Prop("v", b); });
        return v;
    }

    public static bool TapMemContains(UnitGroupMemory m, UnitEntityData u)
    {
        var v = m.Contains(u);
        var id = U(u);
        EngTap("memc", j => { j.Prop("of", id); j.Prop("v", v); });
        return v;
    }

    public static bool TapEngPassive(ObservableCountFlag f)
    {
        var v = (bool)f;
        EngTap("passive", j => j.Prop("v", v));
        return v;
    }

    public static bool TapCanAttack(UnitEntityData u, Func<UnitEntityData, ItemEntityWeapon> weaponGetter)
    {
        var v = u.CanAttack(weaponGetter);
        var id = U(u);
        EngTap("canatk", j => { j.Prop("u", id); j.Prop("v", v); });
        return v;
    }

    // ---- install ----

    private static readonly List<AiSite> EngSites = new List<AiSite>();

    public static IEnumerable<CodeInstruction> EngTapTranspiler(IEnumerable<CodeInstruction> instructions, MethodBase original)
    {
        foreach (var i in instructions)
        {
            if ((i.opcode == OpCodes.Call || i.opcode == OpCodes.Callvirt) && i.operand is MethodInfo mb)
            {
                foreach (var s in EngSites)
                {
                    if (s.Owner != original.DeclaringType || s.Method != original.Name || mb.Name != s.Name) continue;
                    if (s.Generic != null)
                    {
                        if (!mb.IsGenericMethod || mb.DeclaringType != s.Type || mb.GetGenericArguments()[0] != s.Generic) continue;
                    }
                    else if (mb.DeclaringType != s.Type || !mb.GetParameters().Select(p => p.ParameterType).SequenceEqual(s.Args)) continue;
                    i.opcode = OpCodes.Call;
                    i.operand = AccessTools.Method(typeof(CommandObs), s.Tap);
                    var k = "eng:" + s.Owner.Name + "." + s.Method + ":" + s.Tap;
                    Tapped[k] = Tapped.TryGetValue(k, out var n) ? n + 1 : 1;
                    break;
                }
            }
            yield return i;
        }
    }

    private static void InstallEngage(Harmony h, Action<MethodBase, string, string, string> patch)
    {
        var all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        IntPtr H(MethodBase m) => m?.MethodHandle.Value ?? IntPtr.Zero;
        void Fail(string what) => Health.Failed("engage observe: " + what, new Exception("engage"));
        MethodBase M(Type t, string name)
        {
            var ms = t?.GetMethods(all).Where(m => m.Name == name).ToList();
            if (ms == null || ms.Count != 1) { Fail(t?.Name + "." + name + ": " + (ms?.Count ?? 0) + " methods"); return null; }
            return ms[0];
        }
        MethodBase MA(Type t, string name, params Type[] args)
        {
            var m = t?.GetMethod(name, all, null, args, null);
            if (m == null) Fail(t?.Name + "." + name + " is missing");
            return m;
        }
        if (DictSlots == null) Fail("Dictionary's slot count field (count) is missing");
        var ctl = typeof(UnitCombatEngagementController);
        var cs = typeof(UnitCombatState);
        var ue = typeof(UnitEntityData);
        var helper = typeof(UnitHelper);
        var ext = typeof(UnitEngagementExtension);

        // The entry.
        var tick = M(ctl, nameof(UnitCombatEngagementController.Tick));
        if (tick == null) return;
        EntrySet.Add(H(tick));

        // Base code: the controller's methods and its nested types' (its lambdas), the extension's IsEngage, the reach,
        // and UnitCombatState's engagement.
        var types = new List<Type> { ctl };
        for (var k = 0; k < types.Count; k++) types.AddRange(types[k].GetNestedTypes(BindingFlags.Public | BindingFlags.NonPublic));
        var n0 = BaseSet.Count;
        foreach (var t in types)
            foreach (var m in t.GetMethods(all).Cast<MethodBase>().Concat(t.GetConstructors(all)))
                if (!m.IsAbstract && m != tick) BaseSet.Add(H(m));
        if (BaseSet.Count - n0 < 10) Fail("the controller's methods not found (" + (BaseSet.Count - n0) + ")");
        var reach = MA(helper, nameof(UnitHelper.IsAttackOfOpportunityReach), ue, ue, typeof(float));
        var isEngage = MA(ext, nameof(UnitEngagementExtension.IsEngage), ue, ue, typeof(bool), typeof(float));
        var engage = M(cs, nameof(UnitCombatState.Engage));
        var disengage = M(cs, nameof(UnitCombatState.Disengage));
        var aoo = M(cs, nameof(UnitCombatState.AttackOfOpportunity));
        var sad = M(cs, nameof(UnitCombatState.ShouldAttackOnDisengage));
        // (A local function capturing the target: compiled onto a closure class nested in UnitCombatState.)
        var csTypes = new List<Type> { cs };
        for (var k = 0; k < csTypes.Count; k++) csTypes.AddRange(csTypes[k].GetNestedTypes(BindingFlags.Public | BindingFlags.NonPublic));
        var checkMobility = csTypes.SelectMany(t => t.GetMethods(all))
            .Where(m => m.Name.StartsWith("<AttackOfOpportunity>g__CheckMobility", StringComparison.Ordinal)).ToList();
        if (checkMobility.Count != 1) Fail("AttackOfOpportunity's CheckMobility: " + checkMobility.Count + " methods");
        var baseMethods = new List<MethodBase>
        {
            reach, isEngage, engage, disengage, aoo, sad, M(cs, nameof(UnitCombatState.IsEngage)),
            M(cs, nameof(UnitCombatState.ClearAttackOfOpportunityTickData)),
            AccessTools.PropertyGetter(cs, nameof(UnitCombatState.AttackOfOpportunityCount)),
            AccessTools.PropertySetter(cs, nameof(UnitCombatState.AttackOfOpportunityCount)),
            AccessTools.PropertyGetter(cs, nameof(UnitCombatState.LastTarget)),
            AccessTools.PropertySetter(cs, nameof(UnitCombatState.LastTarget)),
            AccessTools.PropertyGetter(cs, nameof(UnitCombatState.ManualTarget)),
            AccessTools.PropertyGetter(cs, nameof(UnitCombatState.EngagedUnits)),
            AccessTools.PropertyGetter(cs, nameof(UnitCombatState.EngagedBy)),
            AccessTools.PropertyGetter(cs, nameof(UnitCombatState.CanAttackOfOpportunity)),
            AccessTools.PropertyGetter(cs, nameof(UnitCombatState.AttackOfOpportunityPerRound)),
            AccessTools.PropertyGetter(cs, nameof(UnitCombatState.PreventAttacksOfOpporunityNextFrame)),
            AccessTools.PropertySetter(cs, nameof(UnitCombatState.PreventAttacksOfOpporunityNextFrame)),
            AccessTools.PropertyGetter(cs, nameof(UnitCombatState.IsInCombat)),
        };
        baseMethods.AddRange(checkMobility);
        foreach (var m in baseMethods)
        {
            if (m == null) { Fail("a base method is missing"); continue; }
            BaseSet.Add(H(m));
        }
        // Kingmaker.Utility.LinqExtensions.ForEach(IEnumerable<T>, Action<T>): Tick's EngagedBy.ForEach.
        var forEach = typeof(LinqExtensions).GetMethods(BindingFlags.Static | BindingFlags.Public)
            .Where(m => m.Name == "ForEach" && m.IsGenericMethodDefinition && m.GetGenericArguments().Length == 1
                        && m.GetParameters()[0].ParameterType.IsGenericType
                        && m.GetParameters()[0].ParameterType.GetGenericTypeDefinition() == typeof(IEnumerable<>)).ToList();
        if (forEach.Count != 1) Fail("LinqExtensions.ForEach(IEnumerable): " + forEach.Count + " methods");
        else TransparentGeneric.Add(H(forEach[0]));

        // Taps.
        void S(MethodBase owner, Type type, string name, string tap, int want, params Type[] args)
        {
            if (owner == null) return;
            EngSites.Add(new AiSite { Owner = owner.DeclaringType, Method = owner.Name, Type = type, Name = name, Args = args, Tap = tap, Want = want });
        }
        void G(MethodBase owner, Type generic, string tap, int want)
        {
            if (owner == null) return;
            EngSites.Add(new AiSite { Owner = owner.DeclaringType, Method = owner.Name, Type = typeof(EntityDataBase), Name = "Get", Generic = generic, Tap = tap, Want = want });
        }
        var tickUnit = M(ctl, "TickUnit");
        var canAoo = AccessTools.PropertyGetter(cs, nameof(UnitCombatState.CanAttackOfOpportunity));
        S(tickUnit, helper, nameof(UnitHelper.GetThreatHand), nameof(TapEngThreat), 1, ue);
        S(tickUnit, helper, nameof(UnitHelper.GetThreatRange), nameof(TapThreatRange), 1, ue, typeof(WeaponSlot));
        S(tickUnit, typeof(UnitGroupMemory), "get_Enemies", nameof(TapEngEnemies), 1);
        S(reach, typeof(UnitEntityView), "get_Corpulence", nameof(TapEngCorp), 2);
        S(reach, ue, nameof(UnitEntityData.SqrDistanceTo), nameof(TapSqd), 1, ue);
        S(reach, ue, nameof(UnitEntityData.HasLOS), nameof(TapLos), 1, ue);
        S(isEngage, typeof(UnitPartConcealment), nameof(UnitPartConcealment.Calculate), nameof(TapConc), 1, ue, ue, typeof(bool));
        S(disengage, typeof(Kingmaker.Armies.TacticalCombat.TacticalCombatHelper), "get_IsActive", nameof(TapEngTact), 1);
        S(disengage, cs, "get_LastTarget", nameof(TapEngLast), 1);
        S(disengage, cs, "get_ManualTarget", nameof(TapEngManual), 1);
        S(sad, cs, "get_LastTarget", nameof(TapEngLast), 2);
        S(sad, typeof(TurnController), "get_ImmuneAttackOfOpportunityOnDisengage", nameof(TapImmune), 1);
        S(sad, ue, "get_HasMotionThisTick", nameof(TapMotion), 1);
        S(sad, typeof(UnitPartConcealment), nameof(UnitPartConcealment.Calculate), nameof(TapConc), 1, ue, ue, typeof(bool));
        S(sad, typeof(Player), nameof(Player.IsTurnBasedModeOn), nameof(TapTbOn), 1);
        S(aoo, cs, "get_PreventAttacksOfOpporunityNextFrame", nameof(TapPrevent), 2);
        G(aoo, typeof(UnitPartForceMove), nameof(TapForceMove), 2);
        S(aoo, ue, "get_HasMotionThisTick", nameof(TapMotion), 1);
        S(aoo, helper, nameof(UnitHelper.GetThreatHand), nameof(TapEngThreat), 1, ue);
        S(aoo, typeof(UnitGroupMemory), nameof(UnitGroupMemory.Contains), nameof(TapMemContains), 1, ue);
        S(canAoo, typeof(ObservableCountFlag), "op_Implicit", nameof(TapEngPassive), 1, typeof(ObservableCountFlag));
        S(canAoo, helper, nameof(UnitHelper.CanAttack), nameof(TapCanAttack), 1, ue, typeof(Func<UnitEntityData, ItemEntityWeapon>));

        var tr = new HarmonyMethod(typeof(CommandObs), nameof(EngTapTranspiler));
        foreach (var m in new[] { tickUnit, reach, isEngage, disengage, sad, aoo, canAoo })
        {
            if (m == null) continue;
            try { h.Patch(m, transpiler: tr); }
            catch (Exception e) { Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name + " (engage taps)", e); }
        }
        foreach (var s in EngSites.GroupBy(s => "eng:" + s.Owner.Name + "." + s.Method + ":" + s.Tap))
        {
            var want = s.Sum(x => x.Want);
            if (!Tapped.TryGetValue(s.Key, out var n) || n != want)
                Fail("tap " + s.Key + " replaced " + (Tapped.TryGetValue(s.Key, out var x) ? x : 0) + " calls, not " + want);
        }

        patch(tick, nameof(EngEntryPrefix), null, nameof(EngEntryFinalizer));
        patch(tickUnit, nameof(TickUnitPrefix), null, null);
        patch(engage, null, null, nameof(EngageFinalizer));
        patch(disengage, null, null, nameof(EngageFinalizer));
        patch(aoo, null, null, nameof(AooFinalizer));
        foreach (var name in new[]
                 {
                     nameof(UnitCombatEngagementController.ProvokeAttackOfOpportunity),
                     nameof(UnitCombatEngagementController.ProvokeAttackOfOpportunityAfterDefensivelyFail),
                     nameof(UnitCombatEngagementController.ForceAttackOfOpportunity),
                 })
            patch(M(ctl, name), nameof(ProvokePrefix), null, null);
        EngOn = true;
    }
}

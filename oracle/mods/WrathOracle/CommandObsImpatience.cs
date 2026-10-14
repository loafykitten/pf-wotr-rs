using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Controllers;
using Kingmaker.Controllers.Combat;
using Kingmaker.Controllers.Projectiles;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.RuleSystem.Rules;
using Kingmaker.RuleSystem.Rules.Damage;
using Kingmaker.UnitLogic;
using Kingmaker.UnitLogic.Commands;
using Kingmaker.UnitLogic.Commands.Base;

namespace WrathOracle;

// Observer "impatience" (slice G, docs/poc5/commands.md): UnitImpatienceController's state (CombatAiData's
// HardTargets, DislikedTargets, BotherUnits, ConsequentAttacksWithoutEffect, LastOffensiveStandardActionTarget,
// ImpatiencePriorityTarget, ConsequentInefficientAttacksCount), so a replay runs the controller and checks what the AI
// reads of it (ipt, disliked):
//  - imp_init: at the window's first tick, every unit's state (each HashSet in its enumeration order, with its used
//    slots: freed slots are reused first, so only a set that never held an entry is known fresh);
//  - entries: uc_imp (UnitImpatienceController.Tick, with the awake units in their order; UpdatePriorityTarget, with
//    its unit, from the tick or TurnController.Prepare) and uc_imp_end;
//  - leaves (uc_v) where the controller's code reads outside itself: ipic (Player.IsInCombat), ivis
//    (UnitGroupMemory.ContainsVisible), idist (DistanceTo), iotw (a UnitAttack's projectiles' OnTheWay), ilvl
//    (UnitState.Impatience);
//  - imp_s: a unit's state after each step: UpdatePriorityTarget and SetupPriorityTarget (in an entry), and the
//    handlers, wherever they run: HandleUnitCommandDidStart (start), OnEventDidTrigger(RuleDealDamage) (dmg, with the
//    rule's id), TryAddBotherUnit (bother, with its visibility read and whether it added), HandleUnitDeath and
//    HandleUnitDestroyed (death: each awake unit's), the heals (heal) and RuleSummonUnit (summon).
// Sets and lists are read through their own enumerators and fields, never through the controller.
internal static partial class CommandObs
{
    internal static bool ImpOn;

    private static bool InImp => Entries.Count > 0 && Entries[Entries.Count - 1].Imp && Entries[Entries.Count - 1].Rec;

    private static bool ImpChain() => On && ImpOn && InImp && Chain(null) == 1;

    // HashSet<UnitEntityData>'s count of slots used (freed ones included).
    private static readonly FieldInfo SetSlots =
        AccessTools.Field(typeof(HashSet<UnitEntityData>), "m_lastIndex") ??
        AccessTools.Field(typeof(HashSet<UnitEntityData>), "_lastIndex");

    // The visibility TryAddBotherUnit read last (its imp_s carries it).
    [ThreadStatic] private static bool? _botherVis;

    private static int? SetSlotCount(HashSet<UnitEntityData> s)
    {
        try { return s == null || SetSlots == null ? (int?)null : (int)SetSlots.GetValue(s); }
        catch (Exception e) { Trace.HookFailed("observe impatience slots", e); return null; }
    }

    private static void WriteSet(Json j, string k, HashSet<UnitEntityData> s)
    {
        WriteUnits(j, k, s?.ToList());
        var n = SetSlotCount(s);
        j.Key(k + "_slots"); if (n.HasValue) j.W.WriteValue(n.Value); else j.W.WriteNull();
    }

    // A unit's state, as one record body (command ids taken before the record: Id may write uc_new).
    private static Action<Json> ImpState(UnitEntityData u)
    {
        var d = u?.CombatState?.AIData;
        if (d == null) return j => j.Prop("u", U(u));
        var id = U(u);
        var hard = d.HardTargets.ToList();
        var dis = d.DislikedTargets.ToList();
        var bother = d.BotherUnits.ToList();
        var (hs, ds, bs) = (SetSlotCount(d.HardTargets), SetSlotCount(d.DislikedTargets), SetSlotCount(d.BotherUnits));
        var cawe = d.ConsequentAttacksWithoutEffect.Select(c => Id(c)).ToList();
        var lost = U(d.LastOffensiveStandardActionTarget);
        var ipt = U(d.ImpatiencePriorityTarget);
        var cnt = d.ConsequentInefficientAttacksCount;
        void Slots(Json j, string k, int? n) { j.Key(k); if (n.HasValue) j.W.WriteValue(n.Value); else j.W.WriteNull(); }
        return j =>
        {
            j.Prop("u", id);
            WriteUnits(j, "hard", hard); Slots(j, "hard_slots", hs);
            WriteUnits(j, "dis", dis); Slots(j, "dis_slots", ds);
            WriteUnits(j, "bother", bother); Slots(j, "bother_slots", bs);
            j.Key("cawe"); j.W.WriteStartArray(); foreach (var c in cawe) j.W.WriteValue(c); j.W.WriteEndArray();
            j.Prop("lost", lost); j.Prop("ipt", ipt); j.Prop("cnt", cnt);
        };
    }

    private static void ImpS(string at, UnitEntityData u, Action<Json> extra = null)
    {
        try
        {
            if (!On || !ImpOn || u == null) return;
            var body = ImpState(u);
            Sched.Obs("imp_s", j => { j.Prop("at", at); body(j); extra?.Invoke(j); });
        }
        catch (Exception e) { Trace.HookFailed("observe imp_s", e); }
    }

    // ---- the initial state ----

    private static void ImpInit()
    {
        try
        {
            foreach (var u in StateDump.AllUnits(Game.Instance))
            {
                if (u?.CombatState?.AIData == null) continue;
                var body = ImpState(u);
                Sched.Obs("imp_init", body);
            }
        }
        catch (Exception e) { Trace.HookFailed("observe imp_init", e); }
    }

    // ---- entries ----

    public static void ImpEntryPrefix(object[] __args, MethodBase __originalMethod)
    {
        if (!Enter(__originalMethod, out var nested, imp: true)) return;
        try
        {
            var fn = __originalMethod.Name;
            var origin = Origin();
            List<UnitEntityData> awake = fn == "Tick" ? Game.Instance.State.AwakeUnits.ToList() : null;
            var u = __args.Length > 0 ? U(__args[0] as UnitEntityData) : null;
            Sched.Obs("uc_imp", j =>
            {
                j.Prop("fn", fn); j.Prop("nested", nested); j.Prop("origin", origin); j.Prop("u", u);
                WriteUnits(j, "awake", awake);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_imp", e); }
    }

    public static void ImpEntryFinalizer(MethodBase __originalMethod, Exception __exception)
    {
        if (!Pop() || !On) return;
        try
        {
            var fn = __originalMethod.Name;
            var exc = __exception != null;
            Sched.Obs("uc_imp_end", j => { j.Prop("fn", fn); j.Prop("exc", exc); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_imp_end", e); }
    }

    // ---- the state after each step ----

    public static void UpdatePriorityFinalizer(UnitEntityData unit)
    {
        if (ImpChainOrEntry()) ImpS("upd", unit);
    }

    public static void SetupPriorityFinalizer(UnitEntityData unit)
    {
        if (ImpChainOrEntry()) ImpS("setup", unit);
    }

    // In an impatience entry (its own code down to it).
    private static bool ImpChainOrEntry()
    {
        try { return On && ImpOn && InImp; }
        catch (Exception e) { Trace.HookFailed("observe impatience chain", e); return false; }
    }

    public static void ImpStartPostfix(UnitCommand command)
    {
        try
        {
            if (!On || !ImpOn || command == null) return;
            var c = Id(command);
            ImpS("start", command.Executor, j => WriteId(j, "c", c));
        }
        catch (Exception e) { Trace.HookFailed("observe imp_s start", e); }
    }

    public static void ImpDamagePostfix(RuleDealDamage evt)
    {
        try
        {
            if (!On || !ImpOn || evt == null) return;
            var rule = Rules.IdOf(evt);
            ImpS("dmg", evt.Initiator, j => j.Prop("rule", rule));
        }
        catch (Exception e) { Trace.HookFailed("observe imp_s dmg", e); }
    }

    public static void BotherPrefix(UnitEntityData unit, out int __state)
    {
        __state = unit?.CombatState?.AIData?.BotherUnits.Count ?? -1;
        _botherVis = null;
    }

    public static void BotherPostfix(UnitEntityData unit, UnitEntityData botherUnit, int __state)
    {
        try
        {
            if (!On || !ImpOn || unit == null) return;
            var added = (unit.CombatState?.AIData?.BotherUnits.Count ?? -1) > __state;
            var vis = _botherVis;
            var b = U(botherUnit);
            ImpS("bother", unit, j =>
            {
                j.Prop("b", b); j.Prop("added", added);
                j.Key("vis"); if (vis.HasValue) j.W.WriteValue(vis.Value); else j.W.WriteNull();
            });
        }
        catch (Exception e) { Trace.HookFailed("observe imp_s bother", e); }
    }

    public static void ImpDeathPostfix(UnitEntityData unit, MethodBase __originalMethod)
    {
        try
        {
            if (!On || !ImpOn) return;
            var fn = __originalMethod.Name;
            var dead = U(unit);
            foreach (var a in Game.Instance.State.AwakeUnits.ToList())
                ImpS("death", a, j => { j.Prop("fn", fn); j.Prop("dead", dead); });
        }
        catch (Exception e) { Trace.HookFailed("observe imp_s death", e); }
    }

    public static void ImpOtherPostfix(object[] __args, MethodBase __originalMethod)
    {
        try
        {
            if (!On || !ImpOn) return;
            var fn = __originalMethod.DeclaringType?.Name + "." + __originalMethod.Name + "(" +
                     string.Join(",", __originalMethod.GetParameters().Select(p => p.ParameterType.Name)) + ")";
            Sched.Obs("imp_other", j => j.Prop("fn", fn));
        }
        catch (Exception e) { Trace.HookFailed("observe imp_other", e); }
    }

    // ---- taps ----

    private static void ImpTap(string f, Action<Json> body)
    {
        try
        {
            if (!ImpChain()) return;
            Emit(f, body);
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v " + f, e); }
    }

    public static bool TapImpInCombat(Player p)
    {
        var v = p.IsInCombat;
        ImpTap("ipic", j => j.Prop("v", v));
        return v;
    }

    public static bool TapImpVisible(UnitGroupMemory m, UnitEntityData u)
    {
        var v = m.ContainsVisible(u);
        _botherVis = v;
        var o = U(u);
        ImpTap("ivis", j => { j.Prop("o", o); j.Prop("v", v); });
        return v;
    }

    public static float TapImpDistance(UnitEntityData a, UnitEntityData b)
    {
        var v = a.DistanceTo(b);
        var (ia, ib) = (U(a), U(b));
        ImpTap("idist", j => { j.Prop("u", ia); j.Prop("o", ib); j.Prop("v", F(v)); });
        return v;
    }

    public static List<Projectile> TapImpProjectiles(UnitAttack a)
    {
        var v = a.LaunchedProjectiles;
        try
        {
            if (ImpChain())
            {
                var c = Id(a);
                var otw = v?.Select(p => p != null && p.OnTheWay).ToList();
                Emit("iotw", j =>
                {
                    WriteId(j, "c", c);
                    j.Key("v");
                    if (otw == null) j.W.WriteNull();
                    else { j.W.WriteStartArray(); foreach (var x in otw) j.W.WriteValue(x); j.W.WriteEndArray(); }
                });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v otw", e); }
        return v;
    }

    public static ImpatienceLevel TapImpLevel(UnitState s)
    {
        var v = s.Impatience;
        var u = U(s.Owner?.Unit);
        ImpTap("ilvl", j => { j.Prop("u", u); j.Prop("v", (int)v); });
        return v;
    }

    private static readonly List<AiSite> ImpSites = new List<AiSite>();

    public static IEnumerable<CodeInstruction> ImpTapTranspiler(IEnumerable<CodeInstruction> instructions, MethodBase original)
    {
        foreach (var i in instructions)
        {
            if ((i.opcode == OpCodes.Call || i.opcode == OpCodes.Callvirt) && i.operand is MethodBase mb)
            {
                foreach (var s in ImpSites)
                {
                    if (s.Owner != original.DeclaringType || s.Method != original.Name || mb.Name != s.Name || mb.DeclaringType != s.Type ||
                        !mb.GetParameters().Select(p => p.ParameterType).SequenceEqual(s.Args)) continue;
                    i.opcode = OpCodes.Call;
                    i.operand = AccessTools.Method(typeof(CommandObs), s.Tap);
                    var k = "imp:" + s.Owner.Name + "." + s.Method + ":" + s.Tap;
                    Tapped[k] = Tapped.TryGetValue(k, out var n) ? n + 1 : 1;
                    break;
                }
            }
            yield return i;
        }
    }

    // ---- install ----

    private static void InstallImpatience(Harmony h, Action<MethodBase, string, string, string> patch)
    {
        void Fail(string what) => Health.Failed("impatience observe: " + what, new Exception("impatience"));
        if (!TgtOn) { Fail("the targets observer isn't installed"); return; }
        if (SetSlots == null) Fail("HashSet's slot count field (m_lastIndex) is missing");
        var all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        var ctl = typeof(UnitImpatienceController);
        MethodBase M(string name, params Type[] args)
        {
            var ms = ctl.GetMethods(all).Where(m => m.Name == name && (args.Length == 0 || m.GetParameters().Select(p => p.ParameterType).SequenceEqual(args))).ToList();
            if (ms.Count != 1) { Fail(name + ": " + ms.Count + " methods"); return null; }
            return ms[0];
        }
        IntPtr H(MethodBase m) => m?.MethodHandle.Value ?? IntPtr.Zero;
        var ue = typeof(UnitEntityData);
        var tick = M(nameof(UnitImpatienceController.Tick));
        var upd = M(nameof(UnitImpatienceController.UpdatePriorityTarget));
        var setup = M("SetupPriorityTarget");
        var count = M("GetConsequentInefficientAttacksCount");
        var bother = M("TryAddBotherUnit");
        var inCombat = AccessTools.PropertyGetter(ctl, "IsInCombat");
        if (tick == null || upd == null || setup == null || count == null || bother == null || inCombat == null) return;
        EntrySet.Add(H(tick));
        EntrySet.Add(H(upd));
        // Base code: the controller's methods and its nested types' (its lambdas).
        var types = new List<Type> { ctl };
        for (var k = 0; k < types.Count; k++) types.AddRange(types[k].GetNestedTypes(BindingFlags.Public | BindingFlags.NonPublic));
        foreach (var t in types)
            foreach (var m in t.GetMethods(all).Cast<MethodBase>().Concat(t.GetConstructors(all)))
                if (!m.IsAbstract && m != tick && m != upd) BaseSet.Add(H(m));

        void S(MethodBase owner, Type type, string name, string tap, int want, params Type[] args)
        {
            if (owner == null) return;
            ImpSites.Add(new AiSite { Owner = owner.DeclaringType, Method = owner.Name, Type = type, Name = name, Args = args, Tap = tap, Want = want });
        }
        var mem = typeof(UnitGroupMemory);
        S(inCombat, typeof(Player), "get_IsInCombat", nameof(TapImpInCombat), 1);
        S(tick, mem, nameof(UnitGroupMemory.ContainsVisible), nameof(TapImpVisible), 1, ue);
        S(setup, mem, nameof(UnitGroupMemory.ContainsVisible), nameof(TapImpVisible), 1, ue);
        S(bother, mem, nameof(UnitGroupMemory.ContainsVisible), nameof(TapImpVisible), 1, ue);
        S(setup, ue, nameof(UnitEntityData.DistanceTo), nameof(TapImpDistance), 1, ue);
        S(count, typeof(UnitAttack), "get_LaunchedProjectiles", nameof(TapImpProjectiles), 1);
        S(upd, typeof(UnitState), "get_Impatience", nameof(TapImpLevel), 2);
        var tr = new HarmonyMethod(typeof(CommandObs), nameof(ImpTapTranspiler));
        foreach (var m in new[] { inCombat, tick, setup, bother, count, upd })
        {
            try { h.Patch(m, transpiler: tr); }
            catch (Exception e) { Health.Failed("patch UnitImpatienceController." + m.Name + " (impatience taps)", e); }
        }
        foreach (var s in ImpSites.GroupBy(s => "imp:" + s.Owner.Name + "." + s.Method + ":" + s.Tap))
        {
            var want = s.Sum(x => x.Want);
            if (!Tapped.TryGetValue(s.Key, out var n) || n != want)
                Fail("tap " + s.Key + " replaced " + (Tapped.TryGetValue(s.Key, out var x) ? x : 0) + " calls, not " + want);
        }

        patch(tick, nameof(ImpEntryPrefix), null, nameof(ImpEntryFinalizer));
        patch(upd, nameof(ImpEntryPrefix), null, nameof(ImpEntryFinalizer));
        patch(upd, null, nameof(UpdatePriorityFinalizer), null);
        patch(setup, null, nameof(SetupPriorityFinalizer), null);
        patch(M(nameof(UnitImpatienceController.HandleUnitCommandDidStart)), null, nameof(ImpStartPostfix), null);
        // (Before WrathOracle's handler postfix, so its record is inside the handler's h..h_end.)
        var dmg = M("OnEventDidTrigger", typeof(RuleDealDamage));
        if (dmg != null)
        {
            try { h.Patch(dmg, postfix: new HarmonyMethod(typeof(CommandObs), nameof(ImpDamagePostfix)) { priority = Priority.First }); }
            catch (Exception e) { Health.Failed("patch UnitImpatienceController.OnEventDidTrigger(RuleDealDamage)", e); }
        }
        patch(bother, nameof(BotherPrefix), nameof(BotherPostfix), null);
        patch(M(nameof(UnitImpatienceController.HandleUnitDeath)), null, nameof(ImpDeathPostfix), null);
        patch(M(nameof(UnitImpatienceController.HandleUnitDestroyed)), null, nameof(ImpDeathPostfix), null);
        foreach (var t in new[] { typeof(RuleHealDamage), typeof(RuleHealStatDamage), typeof(RuleHealEnergyDrain), typeof(RuleSummonUnit) })
            patch(M("OnEventDidTrigger", t), null, nameof(ImpOtherPostfix), null);
        ImpOn = true;
    }
}

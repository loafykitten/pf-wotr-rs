using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Controllers.Combat;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.UnitLogic.Commands;
using Kingmaker.UnitLogic.Commands.Base;

namespace WrathOracle;

// Observer "targets" (slice F, docs/poc5/commands.md): UnitCombatState's LastTarget and ManualTarget, so a replay
// keeps both and checks what reads them:
//  - tgt_init: at the window's first tick, every unit's LastTarget and ManualTarget (their fields);
//  - tgt_w: every write that changes either field (a prefix on each setter, comparing references as the setters do):
//    the unit, the field (last or manual), the new value and its writer (origin: the call chain, innermost first; the
//    driver's own write, which stands in for ClickUnitHandler.OnClick's, is "WrathOracle.TurnBased.IssueHovered");
//  - UnitCommands.UpdateCombatTarget is base lifecycle code: its reads of TacticalCombatHelper.IsActive (tact, with the
//    command) and of the executor's IsAIEnabled (aien, with the unit) are leaves (uc_v), by a transpiler on the call.
internal static partial class CommandObs
{
    // Whether the targets are observed (InstallTargets sets it; the trace's begin lists the observer).
    internal static bool TgtOn;

    // The driver's write in progress (its frames are WrathOracle's, which a call chain skips).
    [ThreadStatic] private static bool _tgtDriver;

    // UpdateCombatTarget's command, innermost last.
    private static readonly List<UnitCommand> Uct = new List<UnitCommand>();

    private static readonly AccessTools.FieldRef<UnitCombatState, UnitEntityData> LastTargetField =
        AccessTools.FieldRefAccess<UnitCombatState, UnitEntityData>("m_LastTarget");
    private static readonly AccessTools.FieldRef<UnitCombatState, UnitEntityData> ManualTargetField =
        AccessTools.FieldRefAccess<UnitCombatState, UnitEntityData>("m_ManualTarget");

    // ---- the initial state ----

    private static void TgtInit()
    {
        try
        {
            foreach (var u in StateDump.AllUnits(Game.Instance))
            {
                var cs = u?.CombatState;
                if (cs == null) continue;
                var id = u.UniqueId;
                var last = U(LastTargetField(cs));
                var manual = U(ManualTargetField(cs));
                Sched.Obs("tgt_init", j => { j.Prop("u", id); j.Prop("last", last); j.Prop("manual", manual); });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe tgt_init", e); }
    }

    // ---- the writes ----

    public static void SetLastTargetPrefix(UnitCombatState __instance, UnitEntityData value) =>
        TgtWrite(__instance, "last", LastTargetField(__instance), value);

    public static void SetManualTargetPrefix(UnitCombatState __instance, UnitEntityData value) =>
        TgtWrite(__instance, "manual", ManualTargetField(__instance), value);

    private static void TgtWrite(UnitCombatState cs, string f, UnitEntityData old, UnitEntityData value)
    {
        try
        {
            if (!On || !TgtOn || ReferenceEquals(old, value)) return;
            var u = CsUnit(cs);
            var v = U(value);
            var origin = _tgtDriver ? "WrathOracle.TurnBased.IssueHovered" : Origin();
            Sched.Obs("tgt_w", j => { j.Prop("u", u); j.Prop("f", f); j.Prop("v", v); j.Prop("origin", origin); });
        }
        catch (Exception e) { Trace.HookFailed("observe tgt_w", e); }
    }

    // The driver's `unit.CombatState.ManualTarget = target` (ClickUnitHandler.OnClick's, after its Run).
    internal static void DriverSetManualTarget(UnitEntityData unit, UnitEntityData target)
    {
        _tgtDriver = true;
        try { unit.CombatState.ManualTarget = target; }
        finally { _tgtDriver = false; }
    }

    // ---- UpdateCombatTarget ----

    public static void UctPrefix(UnitCommand cmd) => Uct.Add(cmd);

    public static void UctFinalizer()
    {
        if (Uct.Count > 0) Uct.RemoveAt(Uct.Count - 1);
    }

    private static bool UctLeaf() => On && TgtOn && InRecordedEntry && Uct.Count > 0 && Chain(null) == 1;

    public static bool TapUctTactical()
    {
        var v = Kingmaker.Armies.TacticalCombat.TacticalCombatHelper.IsActive;
        try
        {
            if (UctLeaf())
            {
                var c = Id(Uct[Uct.Count - 1]);
                Emit("tact", j => { WriteId(j, "c", c); j.Prop("v", v); });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v tact", e); }
        return v;
    }

    public static bool TapUctAiEnabled(UnitEntityData u)
    {
        var v = u.IsAIEnabled;
        try
        {
            if (UctLeaf())
            {
                var id = U(u);
                Emit("aien", j => { j.Prop("u", id); j.Prop("v", v); });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v aien", e); }
        return v;
    }

    public static IEnumerable<CodeInstruction> UctTapTranspiler(IEnumerable<CodeInstruction> instructions)
    {
        foreach (var i in instructions)
        {
            if ((i.opcode == OpCodes.Call || i.opcode == OpCodes.Callvirt) && i.operand is MethodInfo mb)
            {
                string tap = null;
                if (mb.DeclaringType == typeof(Kingmaker.Armies.TacticalCombat.TacticalCombatHelper) && mb.Name == "get_IsActive")
                    tap = nameof(TapUctTactical);
                else if (mb.DeclaringType == typeof(UnitEntityData) && mb.Name == "get_IsAIEnabled")
                    tap = nameof(TapUctAiEnabled);
                if (tap != null)
                {
                    i.opcode = OpCodes.Call;
                    i.operand = AccessTools.Method(typeof(CommandObs), tap);
                    var k = "tgt:UpdateCombatTarget:" + tap;
                    Tapped[k] = Tapped.TryGetValue(k, out var n) ? n + 1 : 1;
                }
            }
            yield return i;
        }
    }

    // ---- install ----

    private static void InstallTargets(Harmony h, Action<MethodBase, string, string, string> patch)
    {
        void Fail(string what) => Health.Failed("targets observe: " + what, new Exception("targets"));
        if (!EngOn) { Fail("the engage observer isn't installed"); return; }
        var all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        var cs = typeof(UnitCombatState);
        var setLast = AccessTools.PropertySetter(cs, nameof(UnitCombatState.LastTarget));
        var setManual = AccessTools.PropertySetter(cs, nameof(UnitCombatState.ManualTarget));
        var ms = typeof(UnitCommands).GetMethods(all).Where(m => m.Name == "UpdateCombatTarget").ToList();
        if (setLast == null || setManual == null || ms.Count != 1) { Fail("a setter or UnitCommands.UpdateCombatTarget is missing"); return; }
        var uct = ms[0];

        BaseSet.Add(uct.MethodHandle.Value);
        try { h.Patch(uct, transpiler: new HarmonyMethod(typeof(CommandObs), nameof(UctTapTranspiler))); }
        catch (Exception e) { Health.Failed("patch UnitCommands.UpdateCombatTarget (targets taps)", e); }
        foreach (var (tap, want) in new[] { (nameof(TapUctTactical), 1), (nameof(TapUctAiEnabled), 1) })
        {
            var k = "tgt:UpdateCombatTarget:" + tap;
            if (!Tapped.TryGetValue(k, out var n) || n != want)
                Fail("tap " + k + " replaced " + (Tapped.TryGetValue(k, out var x) ? x : 0) + " calls, not " + want);
        }
        patch(uct, nameof(UctPrefix), null, nameof(UctFinalizer));
        patch(setLast, nameof(SetLastTargetPrefix), null, null);
        patch(setManual, nameof(SetManualTargetPrefix), null, null);
        TgtOn = true;
    }
}

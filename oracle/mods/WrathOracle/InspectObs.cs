using System;
using System.Collections.Generic;
using System.Linq;
using HarmonyLib;
using Kingmaker.Controllers;
using Kingmaker.EntitySystem;
using Kingmaker.EntitySystem.Entities;

namespace WrathOracle;

// InspectUnitsController.HandleEntityRevealed (IEntityRevealedHandler: EntityDataBase.IsRevealed set,
// by fog of war or a view becoming visible): one "obs" record "insp_reveal" per call, after it, with
// the entity's id, whether it is a unit, whether the call added it to the controller's m_Units
// (the controller's next Tick makes its knowledge check) and the caller chain (the game's frames,
// innermost first, joined by " < ", deep enough to reach GameMode.Tick; a longer stack ends in
// "..."). The set is read through a field reference before and after; nothing the game evaluates is
// called.
internal static class InspectObs
{
    private static readonly AccessTools.FieldRef<InspectUnitsController, HashSet<UnitEntityData>> Units =
        AccessTools.FieldRefAccess<InspectUnitsController, HashSet<UnitEntityData>>("m_Units");

    internal static bool Installed;

    public static void Install(Harmony h)
    {
        try
        {
            var m = AccessTools.Method(typeof(InspectUnitsController), nameof(InspectUnitsController.HandleEntityRevealed))
                    ?? throw new MissingMethodException("InspectUnitsController.HandleEntityRevealed");
            h.Patch(m, prefix: new HarmonyMethod(typeof(InspectObs), nameof(RevealedPrefix)),
                postfix: new HarmonyMethod(typeof(InspectObs), nameof(RevealedPostfix)));
            Installed = true;
        }
        catch (Exception e)
        {
            Health.Failed("patch InspectUnitsController.HandleEntityRevealed (inspect obs)", e);
        }
    }

    // Whether the unit was in the set before the call (null: not a unit, or the set unreadable).
    public static void RevealedPrefix(InspectUnitsController __instance, EntityDataBase entity, out bool? __state)
    {
        __state = null;
        try
        {
            var u = entity as UnitEntityData;
            var set = Units(__instance);
            if (u != null && set != null) __state = set.Contains(u);
        }
        catch (Exception e) { Trace.HookFailed("inspect obs (prefix)", e); }
    }

    public static void RevealedPostfix(InspectUnitsController __instance, EntityDataBase entity, bool? __state)
    {
        if (!Sched.On) return;
        try
        {
            var u = entity as UnitEntityData;
            var set = Units(__instance);
            bool? added = null;
            if (u != null && set != null && __state.HasValue) added = set.Contains(u) && !__state.Value;
            var id = entity?.UniqueId;
            var unit = u != null;
            var origin = Origin();
            Sched.Obs("insp_reveal", j =>
            {
                j.Prop("e", id);
                j.Prop("unit", unit);
                j.Key("added"); if (added.HasValue) j.W.WriteValue(added.Value); else j.W.WriteNull();
                j.Prop("origin", origin);
            });
        }
        catch (Exception e) { Trace.HookFailed("inspect obs", e); }
    }

    private static string Origin()
    {
        try
        {
            var frames = new System.Diagnostics.StackTrace(1, false).GetFrames();
            if (frames == null) return "?";
            var ms = frames.Select(Rng.FrameMethod)
                .Where(m => m != null && m.DeclaringType != null && !m.Name.StartsWith("DMD<", StringComparison.Ordinal) &&
                            !(m.DeclaringType.Namespace ?? "").StartsWith("WrathOracle", StringComparison.Ordinal) &&
                            !(m.DeclaringType.Namespace ?? "").StartsWith("HarmonyLib", StringComparison.Ordinal))
                .ToList();
            var s = string.Join(" < ", ms.Take(64).Select(m => m.DeclaringType.FullName + "." + m.Name));
            return ms.Count > 64 ? s + " < ..." : s;
        }
        catch (Exception) { return "?"; }
    }
}

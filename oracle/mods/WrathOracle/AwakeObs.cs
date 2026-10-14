using System;
using System.Collections.Generic;
using System.Linq;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Controllers.Units;

namespace WrathOracle;

// State.AwakeUnits as UnitActivatableAbilitiesController reads it (the units it ticks), outside
// turn-based windows: one "obs" record "aa_awake" per run of that controller, at its Tick prefix
// (BaseUnitController.Tick, filtered to it), so it is exactly the set the controller iterates:
// SleepingUnitsController set it earlier in the tick, and later controllers (entity
// destruction) can change it before the next tick. The first run writes the whole set ("set"),
// later ones the changes ("on", "off"). A unit that didn't exist when the window opened has a
// run-specific id: its first appearance also lists it under "new" with its blueprint, so a
// replay can match it. Read only.
internal static class AwakeObs
{
    private static HashSet<string> _known, _awake, _announced;

    // Fixture.Begin.
    public static void ResetWindow()
    {
        _awake = null;
        _announced = new HashSet<string>();
        _known = new HashSet<string>();
        try { var g = Game.Instance; if (g?.State != null) foreach (var u in StateDump.AllUnits(g)) if (u != null) _known.Add(u.UniqueId); }
        catch (Exception e) { Health.Log("awake obs known units", e); }
    }

    public static void Install(Harmony h)
    {
        try
        {
            var m = AccessTools.Method(typeof(BaseUnitController), nameof(BaseUnitController.Tick)) ?? throw new MissingMethodException("BaseUnitController.Tick");
            h.Patch(m, prefix: new HarmonyMethod(typeof(AwakeObs), nameof(TickPrefix)));
        }
        catch (Exception e)
        {
            Health.Failed("patch BaseUnitController.Tick (awake obs)", e);
        }
    }

    public static void TickPrefix(BaseUnitController __instance)
    {
        if (!(__instance is UnitActivatableAbilitiesController)) return;
        if (!Trace.Active || !Trace.OnMainThread || Sched.Window || _known == null) return;
        try
        {
            var awake = new HashSet<string>();
            var fresh = new List<KeyValuePair<string, string>>();
            foreach (var u in Game.Instance.State.AwakeUnits)
            {
                if (u == null) continue;
                awake.Add(u.UniqueId);
                if (!_known.Contains(u.UniqueId) && _announced.Add(u.UniqueId))
                    fresh.Add(new KeyValuePair<string, string>(u.UniqueId, u.Blueprint == null ? null : u.Blueprint.AssetGuid.ToString()));
            }
            var first = _awake == null;
            var on = first ? null : awake.Where(id => !_awake.Contains(id)).OrderBy(x => x, StringComparer.Ordinal).ToList();
            var off = first ? null : _awake.Where(id => !awake.Contains(id)).OrderBy(x => x, StringComparer.Ordinal).ToList();
            Trace.Emit("obs", j =>
            {
                j.Prop("ev", "aa_awake");
                if (first)
                {
                    j.Arr("set"); foreach (var id in awake.OrderBy(x => x, StringComparer.Ordinal)) j.W.WriteValue(id); j.EndArr();
                }
                else
                {
                    j.Arr("on"); foreach (var id in on) j.W.WriteValue(id); j.EndArr();
                    j.Arr("off"); foreach (var id in off) j.W.WriteValue(id); j.EndArr();
                }
                j.Arr("new");
                foreach (var kv in fresh.OrderBy(x => x.Key, StringComparer.Ordinal))
                {
                    j.W.WriteStartArray(); j.W.WriteValue(kv.Key); j.W.WriteValue(kv.Value); j.W.WriteEndArray();
                }
                j.EndArr();
            });
            _awake = awake;
        }
        catch (Exception e) { Trace.HookFailed("awake obs", e); }
    }
}

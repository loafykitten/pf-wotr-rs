using System;
using HarmonyLib;
using Kingmaker.AreaLogic.Cutscenes;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.UnitLogic.Parts;
using Kingmaker.View.Roaming;
using UnityEngine;

namespace WrathOracle;

// Observer "roam_init" (declared with the agent; docs/poc5/orientation.md, "Roaming"): at the window's first tick
// (beside ag_init), one roam_init per unit of StateDump.AllUnits with a UnitPartRoaming, in that order, then
// roam_init_end with the count, so a replay starts the roaming controller from the game's state instead of the save's:
//  - u: the unit; disabled: m_Disabled; follow: whether the unit has a UnitPartFollowUnit (Disabled's other operand);
//    parts are found in EntityPartsManager.m_Parts as Get<T> would, without touching its cache;
//  - next: m_NextPoint (read as a field): its kind (null, "RoamingPoint", "RoamingWaypointData" or another type's
//    name), position, and for a RoamingPoint its IdleTime and IdleCutscene (blueprint guid), for a waypoint its
//    UniqueId; next_id: m_PersistentNextPointId;
//  - orig, prev, reverse: OriginalPoint, PreviousPosition, ReverseDirection;
//  - idle_cs: IdleCutscene (the player's UniqueId and its cutscene's blueprint guid); idle_end: IdleEndTime's ticks;
//  - settings: null, or Radius, MinIdleTime, MaxIdleTime, MovementType, Sleepless and the idle cutscenes' guids.
// Everything is read as fields or auto-properties; nothing here calls into the game's logic or fills its caches.
internal static partial class CommandObs
{
    private static readonly AccessTools.FieldRef<UnitPartRoaming, IRoamingPoint> RoamNextField =
        AccessTools.FieldRefAccess<UnitPartRoaming, IRoamingPoint>("m_NextPoint");
    private static readonly AccessTools.FieldRef<UnitPartRoaming, string> RoamNextIdField =
        AccessTools.FieldRefAccess<UnitPartRoaming, string>("m_PersistentNextPointId");
    private static readonly AccessTools.FieldRef<UnitPartRoaming, bool> RoamDisabledField =
        AccessTools.FieldRefAccess<UnitPartRoaming, bool>("m_Disabled");
    // EntityPartsManager.m_Parts, read directly: Get<TPart> fills the manager's cache (misses included).
    private static readonly AccessTools.FieldRef<Kingmaker.EntitySystem.EntityPartsManager, System.Collections.Generic.List<Kingmaker.EntitySystem.EntityPart>> PartsField =
        AccessTools.FieldRefAccess<Kingmaker.EntitySystem.EntityPartsManager, System.Collections.Generic.List<Kingmaker.EntitySystem.EntityPart>>("m_Parts");

    // The unit's first part of type T, as EntityPartsManager.Get<T> finds it, without its cache.
    private static T PartOf<T>(UnitEntityData unit) where T : class
    {
        var parts = unit?.Parts != null ? PartsField(unit.Parts) : null;
        if (parts == null) return null;
        foreach (var p in parts) if (p is T t) return t;
        return null;
    }

    private static readonly AccessTools.FieldRef<RoamingUnitSettings, Kingmaker.Blueprints.CutsceneReference[]> RoamIdleField =
        AccessTools.FieldRefAccess<RoamingUnitSettings, Kingmaker.Blueprints.CutsceneReference[]>("m_IdleCutscenes");

    private static void RoamInit()
    {
        try
        {
            var n = 0;
            foreach (var unit in StateDump.AllUnits(Kingmaker.Game.Instance))
            {
                var r = PartOf<UnitPartRoaming>(unit);
                if (r == null) continue;
                n++;
                RoamInitOne(unit, r);
            }
            Sched.Obs("roam_init_end", j => j.Prop("n", n));
        }
        catch (Exception e) { Trace.HookFailed("observe roam_init", e); }
    }

    private static void RoamInitOne(UnitEntityData unit, UnitPartRoaming r)
    {
        var next = RoamNextField(r);
        var nextId = RoamNextIdField(r);
        var disabled = RoamDisabledField(r);
        var follow = PartOf<UnitPartFollowUnit>(unit) != null;
        var cs = r.IdleCutscene;
        var settings = r.Settings;
        Sched.Obs("roam_init", j =>
        {
            j.Prop("u", unit.UniqueId); j.Prop("disabled", disabled); j.Prop("follow", follow);
            j.Key("next");
            if (next == null) j.W.WriteNull();
            else
            {
                j.W.WriteStartObject();
                switch (next)
                {
                    case RoamingPoint p:
                        j.Prop("kind", "RoamingPoint"); j.Prop("pos", p.Position); j.Prop("idle", p.IdleTime);
                        j.Prop("cs", p.IdleCutscene != null ? p.IdleCutscene.AssetGuid.ToString() : null);
                        break;
                    case RoamingWaypointData w:
                        j.Prop("kind", "RoamingWaypointData"); j.Prop("id", w.UniqueId);
                        break;
                    default:
                        j.Prop("kind", next.GetType().Name);
                        break;
                }
                j.W.WriteEndObject();
            }
            j.Prop("next_id", nextId);
            j.Prop("orig", r.OriginalPoint); j.Prop("prev", r.PreviousPosition); j.Prop("reverse", r.ReverseDirection);
            j.Key("idle_cs");
            if (cs == null) j.W.WriteNull();
            else
            {
                j.W.WriteStartObject();
                j.Prop("id", cs.UniqueId); j.Prop("cs", cs.Cutscene != null ? cs.Cutscene.AssetGuid.ToString() : null);
                j.W.WriteEndObject();
            }
            j.Prop("idle_end", r.IdleEndTime.Ticks);
            j.Key("settings");
            if (settings == null) j.W.WriteNull();
            else
            {
                j.W.WriteStartObject();
                j.Prop("radius", settings.Radius); j.Prop("min_idle", settings.MinIdleTime); j.Prop("max_idle", settings.MaxIdleTime);
                j.Prop("move", (int)settings.MovementType); j.Prop("sleepless", settings.Sleepless);
                j.Arr("idle");
                foreach (var c in RoamIdleField(settings) ?? Array.Empty<Kingmaker.Blueprints.CutsceneReference>())
                    j.W.WriteValue(c == null ? null : c.Guid.ToString());
                j.EndArr();
                j.W.WriteEndObject();
            }
        });
    }
}

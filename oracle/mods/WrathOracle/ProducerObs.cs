using System;
using System.Collections.Generic;
using System.Linq;
using HarmonyLib;
using Kingmaker;
using Kingmaker.AreaLogic.Cutscenes;
using Kingmaker.Blueprints;
using Kingmaker.Controllers.Units;
using Kingmaker.UnitLogic.Parts;
using Kingmaker.View.Roaming;

namespace WrathOracle;

// Who makes a global draw the sim doesn't (Wrath-Rust POC-4, Horgus): each of these methods draws
// from the global RNG to choose something whose effect the sim argues is presentation, and the
// draw's site key names only the method. So each call writes an "obs" record just before its
// draw, naming the producer and what the choice can start (the cutscenes among the candidates):
//   gate_track  CutscenePlayerGateData.SelectTrackToStart: the gate and its player's cutscene
//   roam_point  RoamingWaypointData.SelectNextPoint/SelectPrevPoint/SelectCutscene: the point and
//               its idle cutscenes
//   roam_roll   UnitRoamingController.RollNextPoint: the unit and its roaming settings' idle cutscenes
// Fields only (the references' GUIDs, not their blueprints); nothing here draws or changes state.
internal static class ProducerObs
{
    private static readonly AccessTools.FieldRef<RoamingUnitSettings, CutsceneReference[]> SettingsIdle =
        AccessTools.FieldRefAccess<RoamingUnitSettings, CutsceneReference[]>("m_IdleCutscenes");

    public static void Install(Harmony h)
    {
        Patch(h, typeof(CutscenePlayerGateData), "SelectTrackToStart", nameof(GatePrefix));
        Patch(h, typeof(RoamingWaypointData), nameof(RoamingWaypointData.SelectNextPoint), nameof(PointNextPrefix));
        Patch(h, typeof(RoamingWaypointData), nameof(RoamingWaypointData.SelectPrevPoint), nameof(PointPrevPrefix));
        Patch(h, typeof(RoamingWaypointData), nameof(RoamingWaypointData.SelectCutscene), nameof(PointCutscenePrefix));
        Patch(h, typeof(UnitRoamingController), "RollNextPoint", nameof(RollPrefix));
    }

    private static void Patch(Harmony h, Type t, string method, string prefix)
    {
        try
        {
            var m = AccessTools.Method(t, method) ?? throw new MissingMethodException(t.FullName, method);
            h.Patch(m, prefix: new HarmonyMethod(typeof(ProducerObs), prefix));
        }
        catch (Exception e)
        {
            Health.Failed("patch " + t.Name + "." + method, e);
        }
    }

    private static void Guids(Json j, string name, IEnumerable<CutsceneReference> refs)
    {
        j.Arr(name);
        foreach (var r in refs ?? Enumerable.Empty<CutsceneReference>())
            j.W.WriteValue(r == null ? null : r.Guid.ToString());
        j.EndArr();
    }

    public static void GatePrefix(CutscenePlayerGateData __instance)
    {
        if (!Trace.Active || !Trace.OnMainThread) return;
        try
        {
            var gate = __instance.Gate;
            var cutscene = __instance.Player?.Cutscene;
            var started = __instance.StartedTracks?.Count ?? -1;
            Trace.Emit("obs", j =>
            {
                j.Prop("ev", "gate_track");
                j.Prop("gate", gate == null ? null : gate.AssetGuid.ToString());
                j.Prop("cutscene", cutscene == null ? null : cutscene.AssetGuid.ToString());
                j.Prop("started", started);
            });
        }
        catch (Exception e) { Trace.HookFailed("gate_track", e); }
    }

    private static void Point(RoamingWaypointData p, string m)
    {
        if (!Trace.Active || !Trace.OnMainThread) return;
        try
        {
            var id = p?.UniqueId;
            var idle = p?.WaypointView?.IdleCutscenes;
            Trace.Emit("obs", j =>
            {
                j.Prop("ev", "roam_point");
                j.Prop("m", m);
                j.Prop("point", id);
                Guids(j, "idle", idle);
            });
        }
        catch (Exception e) { Trace.HookFailed("roam_point", e); }
    }

    public static void PointNextPrefix(RoamingWaypointData __instance) => Point(__instance, "SelectNextPoint");
    public static void PointPrevPrefix(RoamingWaypointData __instance) => Point(__instance, "SelectPrevPoint");
    public static void PointCutscenePrefix(RoamingWaypointData __instance) => Point(__instance, "SelectCutscene");

    public static void RollPrefix(UnitPartRoaming roaming)
    {
        if (!Trace.Active || !Trace.OnMainThread) return;
        try
        {
            var unit = roaming?.Owner?.UniqueId;
            var settings = roaming?.Settings;
            var idle = settings == null ? null : SettingsIdle(settings);
            Trace.Emit("obs", j =>
            {
                j.Prop("ev", "roam_roll");
                j.Prop("unit", unit);
                j.Prop("settings", settings != null);
                Guids(j, "idle", idle);
            });
        }
        catch (Exception e) { Trace.HookFailed("roam_roll", e); }
    }
}

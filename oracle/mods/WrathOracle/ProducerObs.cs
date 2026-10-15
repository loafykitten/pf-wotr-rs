using System;
using System.Collections.Generic;
using System.Linq;
using HarmonyLib;
using Kingmaker;
using Kingmaker.AreaLogic.Cutscenes;
using Kingmaker.AreaLogic.Cutscenes.Commands;
using Kingmaker.Blueprints;
using Kingmaker.Controllers;
using Kingmaker.Controllers.Units;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.UnitLogic.Parts;
using Kingmaker.View.MapObjects.SriptZones;
using Kingmaker.View.Roaming;

namespace WrathOracle;

// Who makes a global draw the sim doesn't (Wrath-Rust POC-4, Horgus): each of these methods draws
// from the global RNG to choose something whose effect the sim argues is presentation, and the
// draw's site key names only the method. So each call writes an "obs" record just before its
// draw, naming the producer and what the choice can start (the cutscenes among the candidates):
//   gate_track  CutscenePlayerGateData.SelectTrackToStart: the gate and its player's cutscene
//   roam_point  RoamingWaypointData.SelectNextPoint/SelectPrevPoint/SelectCutscene: the point, its
//               idle cutscenes, and (InstallRoaming) the candidates' count (n: the next waypoints, the
//               previous ones or the idle cutscenes) and who asked (caller: "controller" inside
//               UnitRoamingController.TickOnUnit, "advance" inside UnitPartRoaming.Advance, the
//               innermost; null elsewhere) for which roaming unit (unit)
//   roam_roll   UnitRoamingController.RollNextPoint: the unit and its roaming settings' idle cutscenes
// And when a cutscene's CommandAction runs (cs_action: the command and its player's cutscene), whose
// timing the sim can't compute (delays, animations): a replay runs a cutscene's T0 actions then.
// And (InstallLocks) when a CommandLockControls starts or stops (cs_lock), which ticks or postpones
// every unit's buffs (UnitBuffsController.TickAll/PostponeAll) and locks the game into Cutscene
// mode, with where it ran: inside a player's Start (start: that player's cutscene; StopCopies stops
// a copy there) and whether inside CutsceneController.Tick (controller).
// And (InstallZones) each ScriptZone.TryAddUnit (zone_enter: the zone's blueprint, the unit, whether
// it is the main character), whose tick follows from movement (T1): a replay enters zones then.
// And (InstallCommands) when a cutscene's CommandStartDialog runs (cs_dialog: the command, its
// player's cutscene, where it ran as cs_lock), which starts its dialogue at a time the sim can't
// compute, and each entry condition a track checks before starting a command (cs_entry: the command,
// its player's cutscene, whether it held, the command's OnFail, where it ran as cs_lock), whose
// conditions can read positions (UnitInScriptZone): a replay starts the dialogue and skips or runs
// the command then.
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
        Patch(h, typeof(CommandAction), "OnRun", nameof(ActionPrefix), new[] { typeof(CutscenePlayerData), typeof(bool) });
        try
        {
            var start = AccessTools.Method(typeof(CutscenePlayerData), nameof(CutscenePlayerData.Start)) ?? throw new MissingMethodException("CutscenePlayerData.Start");
            h.Patch(start, prefix: new HarmonyMethod(typeof(ProducerObs), nameof(StartPrefix)), finalizer: new HarmonyMethod(typeof(ProducerObs), nameof(StartFinalizer)));
        }
        catch (Exception e)
        {
            Health.Failed("patch CutscenePlayerData.Start", e);
        }
    }

    public static void InstallLocks(Harmony h)
    {
        Patch(h, typeof(CommandLockControls), "OnRun", nameof(LockRunPrefix), new[] { typeof(CutscenePlayerData), typeof(bool) });
        Patch(h, typeof(CommandLockControls), "OnStop", nameof(LockStopPrefix), new[] { typeof(CutscenePlayerData) });
        try
        {
            var tick = AccessTools.Method(typeof(CutsceneController), nameof(CutsceneController.Tick)) ?? throw new MissingMethodException("CutsceneController.Tick");
            h.Patch(tick, prefix: new HarmonyMethod(typeof(ProducerObs), nameof(ControllerPrefix)), finalizer: new HarmonyMethod(typeof(ProducerObs), nameof(ControllerFinalizer)));
        }
        catch (Exception e)
        {
            Health.Failed("patch CutsceneController.Tick", e);
        }
    }

    // Who is selecting a roaming point (roam_point's caller and unit): UnitRoamingController.TickOnUnit
    // (the controller moving its unit along the waypoints) or UnitPartRoaming.Advance (the AI's roam
    // and abilities that move along the path).
    public static void InstallRoaming(Harmony h)
    {
        void P(Type t, string method, string prefix)
        {
            try
            {
                var m = AccessTools.Method(t, method) ?? throw new MissingMethodException(t.FullName, method);
                h.Patch(m, prefix: new HarmonyMethod(typeof(ProducerObs), prefix), finalizer: new HarmonyMethod(typeof(ProducerObs), nameof(RoamFinalizer)));
            }
            catch (Exception e)
            {
                Health.Failed("patch " + t.Name + "." + method, e);
            }
        }
        P(typeof(UnitRoamingController), "TickOnUnit", nameof(RoamTickPrefix));
        P(typeof(UnitPartRoaming), nameof(UnitPartRoaming.Advance), nameof(RoamAdvancePrefix));
        _roamingContext = true;
    }

    private static bool _roamingContext;
    private static readonly List<KeyValuePair<string, string>> RoamCallers = new List<KeyValuePair<string, string>>();

    public static void RoamTickPrefix(UnitEntityData unit) =>
        RoamCallers.Add(new KeyValuePair<string, string>("controller", unit?.UniqueId));

    public static void RoamAdvancePrefix(UnitPartRoaming __instance) =>
        RoamCallers.Add(new KeyValuePair<string, string>("advance", __instance?.Owner?.UniqueId));

    public static Exception RoamFinalizer(Exception __exception)
    {
        if (RoamCallers.Count > 0) RoamCallers.RemoveAt(RoamCallers.Count - 1);
        return __exception;
    }

    public static void InstallZones(Harmony h)
    {
        Patch(h, typeof(ScriptZone), "TryAddUnit", nameof(ZonePrefix), new[] { typeof(UnitEntityData) });
    }

    // When a cutscene's CommandStartDialog runs (cs_dialog), and each entry condition a track checks
    // before it starts a command (cs_entry), both cutscene timing or positions the sim can't compute.
    public static void InstallCommands(Harmony h)
    {
        Patch(h, typeof(CommandStartDialog), "OnRun", nameof(DialogPrefix), new[] { typeof(CutscenePlayerData), typeof(bool) });
        Patch(h, typeof(CutscenePlayerTrackData), "StartCommand", nameof(StartCommandPrefix), new[] { typeof(bool) });
        try
        {
            var next = AccessTools.Method(typeof(CutscenePlayerTrackData), "StartNextCommand", new[] { typeof(bool) }) ?? throw new MissingMethodException("CutscenePlayerTrackData.StartNextCommand");
            h.Patch(next, prefix: new HarmonyMethod(typeof(ProducerObs), nameof(NextPrefix)), finalizer: new HarmonyMethod(typeof(ProducerObs), nameof(NextFinalizer)));
        }
        catch (Exception e)
        {
            Health.Failed("patch CutscenePlayerTrackData.StartNextCommand", e);
        }
    }

    // A CommandStartDialog's start, with where it ran (as cs_lock's start and controller).
    public static void DialogPrefix(CommandStartDialog __instance, CutscenePlayerData player, bool skipping)
    {
        if (!Trace.Active || !Trace.OnMainThread) return;
        try
        {
            var guid = __instance.AssetGuid.ToString();
            var cutscene = player?.Cutscene;
            var start = _starting == null ? null : _starting.Cutscene == null ? "" : _starting.Cutscene.AssetGuid.ToString();
            var controller = _inController > 0;
            Trace.Emit("obs", j =>
            {
                j.Prop("ev", "cs_dialog");
                j.Prop("command", guid);
                j.Prop("cutscene", cutscene == null ? null : cutscene.AssetGuid.ToString());
                j.Prop("start", start);
                j.Prop("controller", controller);
                j.Prop("skipping", skipping);
            });
        }
        catch (Exception e) { Trace.HookFailed("cs_dialog", e); }
    }

    // StartNextCommand's frames (innermost last): the track, the command it is about to check (taken
    // before the call: a failed FinishTrack check moves CommandIndex past the end), and whether the
    // frame's StartCommand ran, that is, whether the entry condition held. Written once per check:
    // when StartCommand starts (before the command's own records), or when the frame ends without
    // it (the condition failed).
    private static readonly List<CutscenePlayerTrackData> NextFrames = new List<CutscenePlayerTrackData>();
    private static readonly List<CommandBase> NextCommands = new List<CommandBase>();
    private static readonly List<bool> NextStarted = new List<bool>();

    public static void NextPrefix(CutscenePlayerTrackData __instance)
    {
        CommandBase next = null;
        try
        {
            // Fields and plain getters: Track reads the gate's started tracks.
            var commands = __instance.Track?.Commands;
            var i = __instance.CommandIndex + 1;
            if (commands != null && i >= 0 && i < commands.Count) next = commands[i];
        }
        catch (Exception e) { Trace.HookFailed("cs_entry", e); }
        NextFrames.Add(__instance);
        NextCommands.Add(next);
        NextStarted.Add(false);
    }

    public static void StartCommandPrefix(CutscenePlayerTrackData __instance)
    {
        var n = NextFrames.Count - 1;
        if (n < 0 || NextFrames[n] != __instance || NextStarted[n]) return;
        NextStarted[n] = true;
        Entry(__instance, NextCommands[n], true);
    }

    public static Exception NextFinalizer(Exception __exception, CutscenePlayerTrackData __instance)
    {
        var n = NextFrames.Count - 1;
        if (n >= 0)
        {
            var started = NextStarted[n];
            var command = NextCommands[n];
            NextFrames.RemoveAt(n);
            NextCommands.RemoveAt(n);
            NextStarted.RemoveAt(n);
            if (!started && __exception == null) Entry(__instance, command, false);
        }
        return __exception;
    }

    private static void Entry(CutscenePlayerTrackData track, CommandBase command, bool passed)
    {
        if (!Trace.Active || !Trace.OnMainThread) return;
        try
        {
            // Plain getters (HasConditions reads the checker's list).
            if (command == null || !command.HasConditions) return;
            var guid = command.AssetGuid.ToString();
            var onFail = (int)command.OnFail;
            var cutscene = track.StartGate?.Player?.Cutscene;
            var start = _starting == null ? null : _starting.Cutscene == null ? "" : _starting.Cutscene.AssetGuid.ToString();
            var controller = _inController > 0;
            Trace.Emit("obs", j =>
            {
                j.Prop("ev", "cs_entry");
                j.Prop("command", guid);
                j.Prop("cutscene", cutscene == null ? null : cutscene.AssetGuid.ToString());
                j.Prop("passed", passed);
                j.Prop("on_fail", onFail);
                j.Prop("start", start);
                j.Prop("controller", controller);
            });
        }
        catch (Exception e) { Trace.HookFailed("cs_entry", e); }
    }

    private static void Patch(Harmony h, Type t, string method, string prefix, Type[] args = null)
    {
        try
        {
            var m = AccessTools.Method(t, method, args) ?? throw new MissingMethodException(t.FullName, method);
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

    // The player whose Start is running: its root gate activates inside it (gate_track "start").
    private static CutscenePlayerData _starting;

    public static void StartPrefix(CutscenePlayerData __instance, out CutscenePlayerData __state)
    {
        __state = _starting;
        _starting = __instance;
    }

    public static Exception StartFinalizer(Exception __exception, CutscenePlayerData __state)
    {
        _starting = __state;
        return __exception;
    }

    public static void GatePrefix(CutscenePlayerGateData __instance)
    {
        if (!Trace.Active || !Trace.OnMainThread) return;
        try
        {
            var gate = __instance.Gate;
            var cutscene = __instance.Player?.Cutscene;
            var started = __instance.StartedTracks?.Count ?? -1;
            var start = _starting != null && __instance.Player == _starting;
            Trace.Emit("obs", j =>
            {
                j.Prop("ev", "gate_track");
                j.Prop("gate", gate == null ? null : gate.AssetGuid.ToString());
                j.Prop("cutscene", cutscene == null ? null : cutscene.AssetGuid.ToString());
                j.Prop("started", started);
                j.Prop("start", start);
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
            var view = p?.WaypointView;
            var idle = view?.IdleCutscenes;
            int? n = view == null ? (int?)null
                : m == "SelectNextPoint" ? view.NextWaypoints.Count
                : m == "SelectPrevPoint" ? view.PrevWaypoints.Count
                : view.IdleCutscenes.Count;
            var ctx = _roamingContext && RoamCallers.Count > 0 ? RoamCallers[RoamCallers.Count - 1] : default;
            Trace.Emit("obs", j =>
            {
                j.Prop("ev", "roam_point");
                j.Prop("m", m);
                j.Prop("point", id);
                Guids(j, "idle", idle);
                if (_roamingContext)
                {
                    j.Prop("n", n);
                    j.Prop("caller", ctx.Key);
                    j.Prop("unit", ctx.Value);
                }
            });
        }
        catch (Exception e) { Trace.HookFailed("roam_point", e); }
    }

    public static void PointNextPrefix(RoamingWaypointData __instance) => Point(__instance, "SelectNextPoint");
    public static void PointPrevPrefix(RoamingWaypointData __instance) => Point(__instance, "SelectPrevPoint");
    public static void PointCutscenePrefix(RoamingWaypointData __instance) => Point(__instance, "SelectCutscene");

    public static void ActionPrefix(CommandAction __instance, CutscenePlayerData player, bool skipping)
    {
        if (!Trace.Active || !Trace.OnMainThread) return;
        try
        {
            var command = __instance.AssetGuid.ToString();
            var cutscene = player?.Cutscene;
            Trace.Emit("obs", j =>
            {
                j.Prop("ev", "cs_action");
                j.Prop("command", command);
                j.Prop("cutscene", cutscene == null ? null : cutscene.AssetGuid.ToString());
                j.Prop("skipping", skipping);
            });
        }
        catch (Exception e) { Trace.HookFailed("cs_action", e); }
    }

    // CutsceneController.Tick's nesting (it doesn't nest; a counter in case).
    private static int _inController;

    public static void ControllerPrefix() => _inController++;

    public static Exception ControllerFinalizer(Exception __exception)
    {
        _inController--;
        return __exception;
    }

    private static void Lock(CommandLockControls command, CutscenePlayerData player, bool on, bool skipping)
    {
        if (!Trace.Active || !Trace.OnMainThread) return;
        try
        {
            var guid = command.AssetGuid.ToString();
            var cutscene = player?.Cutscene;
            var start = _starting == null ? null : _starting.Cutscene == null ? "" : _starting.Cutscene.AssetGuid.ToString();
            var controller = _inController > 0;
            Trace.Emit("obs", j =>
            {
                j.Prop("ev", "cs_lock");
                j.Prop("on", on);
                j.Prop("command", guid);
                j.Prop("cutscene", cutscene == null ? null : cutscene.AssetGuid.ToString());
                j.Prop("start", start);
                j.Prop("controller", controller);
                j.Prop("skipping", skipping);
            });
        }
        catch (Exception e) { Trace.HookFailed("cs_lock", e); }
    }

    public static void LockRunPrefix(CommandLockControls __instance, CutscenePlayerData player, bool skipping) => Lock(__instance, player, true, skipping);
    public static void LockStopPrefix(CommandLockControls __instance, CutscenePlayerData player) => Lock(__instance, player, false, false);

    private static readonly AccessTools.FieldRef<ScriptZone, BlueprintScriptZoneReference> ZoneBlueprint =
        AccessTools.FieldRefAccess<ScriptZone, BlueprintScriptZoneReference>("m_Blueprint");

    public static void ZonePrefix(ScriptZone __instance, UnitEntityData unit)
    {
        if (!Trace.Active || !Trace.OnMainThread) return;
        try
        {
            // The reference's GUID and the units' ids: fields, nothing resolved.
            var bp = ZoneBlueprint(__instance);
            var zone = bp == null || bp.Guid == BlueprintGuid.Empty ? null : bp.Guid.ToString();
            var id = unit?.UniqueId;
            var main = id != null && id == Game.Instance.Player?.MainCharacter.UniqueId;
            Trace.Emit("obs", j =>
            {
                j.Prop("ev", "zone_enter");
                j.Prop("zone", zone);
                j.Prop("unit", id);
                j.Prop("main", main);
            });
        }
        catch (Exception e) { Trace.HookFailed("zone_enter", e); }
    }

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

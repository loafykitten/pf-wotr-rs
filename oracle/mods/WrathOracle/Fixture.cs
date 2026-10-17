using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;
using UnityEngine;

namespace WrathOracle;

// Fixture windows: record a fixture, or replay it from its own recorded trace.
//
// Recording: Time.captureDeltaTime is fixed (Driver.FixedDelta, 1/60 s by default), so every
// frame advances Unity time (and therefore TimeController's raw delta, animation and
// physics) by the same amount whatever the real frame rate; the generator is seeded with
// Driver.Seed; the trace starts. The window normally spans a reload of the save, so the
// fixture starts from the save's state.
// Replaying: the same plan steps run again with the same fixed delta, but the generator is
// seeded differently and every hooked draw returns the recorded value (Rng tape, strict).
// The replay's trace must equal the recording's, apart from fields that are expected to
// differ (frame numbers, generator states, gaps, "forced"/"match"); tools/oracle-trace-diff
// compares them.
internal static class Fixture
{
    public static string Name, Mode, PathNow;
    private static string _saveFile; // the window's source save, for both its start and end dumps
    private static readonly Dictionary<string, int> Replays = new Dictionary<string, int>();
    public static readonly List<string> Results = new List<string>();
    public static readonly List<Dictionary<string, object>> HealthRecords = new List<Dictionary<string, object>>();

    public static string Dir(Plan plan, string name) => Path.Combine(plan.Out, "fixtures", name);

    public static void Begin(Plan plan, string name, bool replay, string perturb, string saveFile, List<string> stepsAhead)
    {
        if (Trace.Active) End();
        var dir = Dir(plan, name);
        Directory.CreateDirectory(dir);
        Name = name;
        Mode = replay ? "replay" : "record";
        _saveFile = saveFile;
        var record = Path.Combine(dir, "record.trace.jsonl.gz");
        if (replay)
        {
            Replays.TryGetValue(name, out var k);
            Replays[name] = ++k;
            var tag = string.IsNullOrEmpty(perturb) ? $"replay-{k}" : $"replay-{k}-perturbed";
            PathNow = Path.Combine(dir, tag + ".trace.jsonl.gz");
        }
        else PathNow = record;
        var prePath = PrePathSync.Stop(); // before the window's own path sync (held pre-window paths go back first)
        Rng.ResetWindow();
        Hooks.ResetWindow();
        // Turn-based windows (the plan has a "tbm:" step ahead) get deterministic clocks, boundary
        // gates and input checks (Sched, WallClock); other windows are unchanged.
        Sched.BeginWindow(stepsAhead.Any(x => x.StartsWith("tbm:", StringComparison.Ordinal)));
        Observe.ResetWindow();
        AwakeObs.ResetWindow();
        GroupObs.ResetWindow();
        CommandObs.ResetWindow();
        PathSync.ResetWindow();
        ProjPin.ResetWindow();
        DeathFxPin.ResetWindow();
        CamPin.ResetWindow();
        TieObs.ResetWindow();
        Sched.CaptureStartUnits();
        TurnBased.ResetWindow();
        LogObs.ResetWindow();
        if (replay)
        {
            if (!File.Exists(record)) throw new FileNotFoundException("no recording to replay", record);
            Rng.LoadTape(record, perturb);
        }
        PhysicsPhase.Unpinned = null;
        PhysicsPhase.WindowOpen(replay && Sched.Window);
        WallClock.BeginWindow(); // after the tape: a replay reads the recording's clocks from its header
        InputPin.Begin("fixture " + name); // normally on since the fixture's "prepare"
        if (Fidelity.Installed && !InputPin.ReloadedUnderPin)
            Health.Failed("input pin", new Exception($"fixture {name} opened without a reload under the input pin (\"prepare\" then \"reload\" before the window)"));
        InputPin.WindowOpen();
        var camera = InputPin.CameraState();
        OracleTime.FixtureMode = true;
        Time.captureDeltaTime = Driver.FixedDelta;
        // Record: the generator continues as it is (the "prepare" step seeded it before the
        // reload). Replay: reseeded, so code that draws without a hook sees another stream.
        if (replay) UnityEngine.Random.InitState(Driver.Seed ^ 0x5bd1e995);
        try { StateDump.Run(dir, "start-" + Path.GetFileName(PathNow).Replace(".trace.jsonl.gz", ""), saveFile); }
        catch (Exception e) { Health.Failed("fixture start state", e); }
        SeedInit.ResetWindow(stepsAhead);
        Trace.Begin(PathNow, name, Mode, j =>
        {
            j.Prop("fixed_delta", Driver.FixedDelta);
            j.Prop("seed", replay ? Driver.Seed ^ 0x5bd1e995 : Driver.Seed);
            j.Prop("random_state_at_begin", RandomState.Text());
            j.Prop("save_file", saveFile != null ? Path.GetFileName(saveFile) : null);
            j.Prop("save_sha256", saveFile != null ? Json.Sha256File(saveFile) : null);
            j.Prop("oracle_version", Main.Mod.Info.Version);
            j.Prop("game_version", Kingmaker.GameVersion.GetVersion());
            j.Prop("assembly_csharp_sha256", Fidelity.AssemblySha);
            j.Prop("tape", replay ? record : null);
            j.Prop("tape_draws", Rng.TapeLength);
            j.Prop("turn_based_window", Sched.Window);
            j.Prop("seedinit", SeedInit.Declared);
            // The observation families installed: a replay requires each one's records.
            j.Arr("observers"); foreach (var o in Fidelity.Observers) j.W.WriteValue(o); j.EndArr();
            // The dialogue controller's custom speaker fields, which live as long as the game process.
            if (TextObs.Installed) { j.Key("dialog_speaker"); TextObs.WriteSpeakerState(j); }
            // The combat log's dispatch tables, queue and context when the window opens (LogObs.cs).
            if (LogObs.Installed) { j.Key("game_log"); LogObs.WriteSetup(j); }
            j.Prop("input_pin", InputPin.Describe());
            j.Arr("camera"); if (camera != null) foreach (var v in camera) j.W.WriteValue(v); j.EndArr();
            // The physics clock at window open: whether colliders sync to queries without a step, and the
            // phase of the fixed-step clock against the frame clock (which frames run a step).
            j.Prop("physics_auto_sync", Physics.autoSyncTransforms);
            j.Prop("physics_auto_simulation", Physics.autoSimulation);
            j.Prop("fixed_delta_time", (double)Time.fixedDeltaTime);
            j.Prop("fixed_phase0", PhysicsPhase.Phase);
            j.Prop("frames_anchor_to_begin", PhysicsPhase.FramesSinceAnchor);
            j.Prop("fixed_phase_pin", PhysicsPhase.SteerNote);
            j.Prop("frame0", Time.frameCount);
            if (Sched.Window)
            {
                // What a replay must reproduce of this recording's native clocks (WallClock).
                j.Prop("time0", WallClock.Time0);
                j.Prop("unscaled0", WallClock.Unscaled0);
                if (WallClock.NavTimer0.HasValue) j.Prop("nav_timer0", (double)WallClock.NavTimer0.Value);
            }
            j.Prop("perturbation", Rng.PerturbNote);
            j.Arr("steps"); foreach (var s in stepsAhead) j.W.WriteValue(s); j.EndArr();
            j.Key("faults"); Fidelity.WriteFaults(j);
            j.Key("install"); Fidelity.WriteInstall(j);
            j.Key("rng_sites"); Rng.WriteSites(j, compact: true);
        });
        Rng.Checkpoint(); // the gap baseline: the window starts here
        // The colliders and a probe of them (observer phys_probe; reads only), where the window records
        // observations (Sched.On: turn-based windows), so no unrecorded query runs elsewhere.
        if (Fidelity.Observers.Contains("phys_probe") && Sched.On) PhysProbe.Run();
        // The line-of-sight geometry and the game's answers across it (observer los_geom; reads only).
        if (Fidelity.Observers.Contains("los_geom") && Sched.On) LosProbe.Run();
        CamPin.WindowOpen(); // the camera's view of extra and dead units at the window's open (reads only)
        Main.Note($"fixture {Mode} {name} -> {PathNow}");
        PrePathSync.Flush(Time.frameCount);
        if (prePath != null && Sched.Replay) Sched.Invalidate(prePath);
        if (PhysicsPhase.Unpinned != null && Sched.Replay) Sched.Invalidate(PhysicsPhase.Unpinned);
        // The camera is the player's input too (frustum: which extra units sleep); a turn-based replay must start
        // with the recording's.
        if (Sched.Replay)
        {
            // The pins need the recording's records: projectile arrivals, and critters' requests keyed apart.
            if (ProjPin.Installed && !ProjPin.RecordedOn)
                Sched.Invalidate("the recording has no projectile arrivals (observer proj_arrival): re-record");
            if (CamPin.Installed && !CamPin.RecordedOn)
                Sched.Invalidate("the recording has no camera gates (observer camera_gates): re-record");
            if (DeathFxPin.Installed && !DeathFxPin.RecordedOn)
                Sched.Invalidate("the recording has no death effects (observer death_effect): re-record");
            if (PathSync.CritterIdOk && !PathSync.RecordedCritters)
                Sched.Invalidate("the recording keys critters' path requests as unidentified (no observer critter_paths): re-record");
            var rec = Sched.RecCamera;
            if (rec == null || camera == null || rec.Length != camera.Length)
                Sched.Invalidate("camera at the window's start: the recording has none (re-record)");
            else
                for (var i = 0; i < rec.Length; i++)
                    if (Math.Abs(rec[i] - camera[i]) > 1e-3)
                    {
                        Sched.Invalidate($"camera at the window's start differs: recording [{string.Join(", ", rec)}], replay [{string.Join(", ", camera)}]");
                        break;
                    }
        }
    }

    // Unconditional cleanup: no fixture timing, tape or trace survives a plan's end or abort.
    public static void Cleanup()
    {
        try { if (Trace.Active) End(); }
        catch (Exception e) { Health.Failed("fixture cleanup", e); }
        finally
        {
            Rng.UnloadTape();
            PhysicsPhase.Stop();
            PrePathSync.Stop();
            PathSync.EndWindow();
            Sched.EndWindow();
            WallClock.Restore();
            InputPin.End();
            OracleTime.FixtureMode = false;
            Time.captureDeltaTime = 0f;
            TurnBased.RestoreSetting();
            IdTrace.Stop();
        }
    }

    public static string End()
    {
        if (!Trace.Active) return null;
        DeathFxPin.Flush(); // records still held go into the trace before it ends
        SeedInit.CloseWindow();
        Rng.Checkpoint(); // unrecorded steps since the last hooked draw count too
        var tapeLen = Rng.TapeLength;
        var tapePos = Rng.TapeLength - Rng.TapeLeftover;
        try { StateDump.Run(Path.GetDirectoryName(PathNow), "end-" + Path.GetFileName(PathNow).Replace(".trace.jsonl.gz", ""), _saveFile); }
        catch (Exception e) { Health.Failed("fixture end state", e); }
        var replay = Rng.ReplayActive;
        Sched.CheckComplete();
        PathSync.EndWindow(); // held paths go back to the game
        var inputReads = InputPin.Reads;
        var pointerPicks = InputPin.Picks;
        var focusHeld = InputPin.FocusHeld;
        var guiUsed = InputPin.GuiEventsUsed;
        WallClock.EndWindow(); // while the window's clock is still the one the game sees
        var invalid = Sched.Invalid;
        var gates = Sched.GatesSeen;
        var inputsChecked = Sched.InputsChecked;
        Trace.End(j =>
        {
            j.Prop("tick", Trace.Tick);
            j.Prop("rules", Rules.RulesSeen);
            j.Prop("handlers", Rules.HandlersSeen);
            j.Prop("draws", Rng.TotalDraws);
            j.Prop("dialog_transitions", Hooks.DialogTransitions);
            j.Prop("input_neutral_reads", inputReads);
            j.Prop("pointer_picks", pointerPicks);
            j.Prop("focus_changes_held", focusHeld);
            j.Prop("imgui_input_events_used", guiUsed);
            if (Sched.Window)
            {
                j.Prop("gates", gates);
                j.Prop("inputs_checked", inputsChecked);
                j.Prop("max_tick_offset", Sched.MaxOffset);
                j.Prop("tick_offset_changes", Sched.OffsetChanges);
                j.Prop("virtual_clock_reads", WallClock.VirtualReads);
                j.Prop("virtual_frame_reads", WallClock.FrameReads);
                j.Prop("paths_tracked", PathSync.Tracked);
                j.Prop("path_waits", PathSync.Waited);
                j.Prop("path_wait_timeouts", PathSync.Timeouts);
                j.Prop("paths_critter", PathSync.Critters);
                j.Prop("projectiles_tracked", ProjPin.Tracked);
                j.Prop("projectile_hits_held", ProjPin.HeldBack);
                j.Prop("projectile_hits_forced", ProjPin.Forced);
                j.Prop("deaths", DeathFxPin.Deaths);
                j.Prop("death_effect_pins", DeathFxPin.Pinned);
                j.Prop("dismember_effects", DeathFxPin.Inits);
                j.Prop("root_rules_checked", Sched.RootsChecked);
                j.Prop("camera_scroll_queries", CamPin.Scrolls);
                j.Prop("camera_command_queries", CamPin.Commands);
                j.Prop("camera_view_changes", CamPin.FrustumChanges);
                j.Prop("camera_view_pins", CamPin.PinnedViews);
                j.Prop("death_view_pins", DeathFxPin.ViewPins);
                j.Prop("init_tie_compares_turn", TieObs.TurnCompares);
                j.Prop("init_tie_compares_prepare", TieObs.PrepareCompares);
                j.Prop("init_ties_turn", TieObs.TurnTies);
                j.Prop("init_ties_prepare", TieObs.PrepareTies);
                j.Prop("seedinit_fired", SeedInit.Fired);
            }
            if (replay)
            {
                j.Prop("tape_draws", tapeLen);
                j.Prop("tape_leftover", Rng.TapeLeftover);
                j.Prop("replay_out_of_order", Rng.ReplayOutOfOrder);
                j.Prop("time_forced", OracleTime.Forced);
                j.Prop("time_tape_exhausted", OracleTime.Exhausted);
                j.Prop("replay_invalid", invalid);
                j.Arr("leftover_by_site");
                foreach (var kv in Rng.TapeLeftoverBySite()) { j.W.WriteStartArray(); j.W.WriteValue(kv.Key); j.W.WriteValue(kv.Value); j.W.WriteEndArray(); }
                j.EndArr();
            }
            j.Arr("site_draws");
            foreach (var s in Rng.Sites.Where(s => s.Draws > 0))
            {
                j.W.WriteStartArray(); j.W.WriteValue(s.Id); j.W.WriteValue(s.Draws); j.W.WriteEndArray();
            }
            foreach (var s in Rng.GlobalSiteList())
            {
                j.W.WriteStartArray(); j.W.WriteValue(s.Key); j.W.WriteValue(s.Draws); j.W.WriteEndArray();
            }
            j.EndArr();
        });
        Rng.UnloadTape();
        Sched.EndWindow();
        Sched.ClearInvalid();
        OracleTime.FixtureMode = false;
        Time.captureDeltaTime = 0f;
        TurnBased.RestoreSetting(); // every window starts with the player's own turn-based setting
        var summary = $"{Mode} {Name}: ticks {Trace.Tick}, records {Trace.Written}, failed {Trace.Failed}, dropped {Trace.Dropped}, structural {Trace.StructuralErrors}, rules {Rules.RulesSeen}, draws {Rng.TotalDraws}, unhooked steps {Rng.UnhookedSteps}/{Rng.UnhookedUnknown} unknown" +
                      (replay ? $", tape {tapePos}/{tapeLen} used, mismatches {Rng.ReplayMismatches}, exhausted {Rng.ReplayExhausted}, out of order {Rng.ReplayOutOfOrder}, gates {gates}, inputs checked {inputsChecked}" + (invalid != null ? ", INVALID: " + invalid : "") : "");
        Results.Add(summary);
        HealthRecords.Add(new Dictionary<string, object>
        {
            ["name"] = Name, ["mode"] = Mode, ["file"] = PathNow,
            ["records_failed"] = Trace.Failed, ["records_dropped"] = Trace.Dropped, ["structural_errors"] = Trace.StructuralErrors,
            ["hook_exceptions_caught"] = Trace.HookFailures, ["rng_unhooked_steps"] = Rng.UnhookedSteps, ["rng_unhooked_unknown"] = Rng.UnhookedUnknown,
            ["replay_mismatches"] = replay ? Rng.ReplayMismatches : 0, ["replay_tape_exhausted"] = replay ? Rng.ReplayExhausted : 0,
            ["replay_tape_leftover"] = replay ? tapeLen - tapePos : 0, ["time_tape_exhausted"] = replay ? OracleTime.Exhausted : 0,
            ["replay_invalid"] = invalid != null ? 1 : 0,
            ["snapshot_value_failures"] = Interlocked.Read(ref Snap.ValueFailures),
            ["install_incomplete"] = Fidelity.Installed ? Fidelity.InstallFailures : 1,
        });
        Main.Note(summary);
        Hooks.DialogTransitions = 0;
        Name = null;
        // Last: the input pin ends after the trace, the tape and the scheduler are closed, so the focus and
        // controller changes it held back reach the game outside the window.
        InputPin.End();
        return summary;
    }
}

// Fidelity instrumentation: installed only when the plan asks for it ("fidelity": true), so
// performance runs never carry it.
internal static class Fidelity
{
    public static bool Installed;
    public static string AssemblySha;
    public static double InstallSeconds;
    // Failures recorded while installing (a hook that didn't patch, a transpiler that failed,
    // or Install itself throwing): any of them makes every capture of the run uncertifiable.
    public static long InstallFailures;
    public static readonly List<string> Notes = new List<string>();

    public static void Install(Plan plan, HarmonyLib.Harmony h)
    {
        var failedBefore = Health.FailedCount;
        try { InstallInner(plan, h); }
        finally { InstallFailures = Health.FailedCount - failedBefore + Rng.TranspileFailures; }
    }

    private static void InstallInner(Plan plan, HarmonyLib.Harmony h)
    {
        var sw = System.Diagnostics.Stopwatch.StartNew();
        AssemblySha = Json.Sha256File(typeof(Kingmaker.Game).Assembly.Location);
        if (plan.Faults != null)
        {
            Rules.FaultThrowAboutAt = plan.Faults.ThrowAboutAt;
            Rules.FaultDropAboutAt = plan.Faults.DropAboutAt;
            Rng.DisableSiteContaining = plan.Faults.DisableSite;
        }
        Driver.FixedDelta = plan.FixedDelta;
        Driver.Seed = plan.Seed;
        Rng.SelfTest();
        Notes.Add("rng self-test: " + Rng.SelfTestNote);
        Rng.Scan();
        Notes.Add("rng scan: " + Rng.ScanNote);
        Rng.Install(h);
        Notes.Add($"rng: {Rng.MethodsTranspiled} methods transpiled ({Rng.Sites.Count(s => s.Transpiled)} sites), {Rng.TranspileFailures} failures, {Rng.PatchSeconds:0.00} s");
        var t0 = sw.Elapsed.TotalSeconds;
        Rules.Install(h);
        Notes.Add($"rules: {Rules.TriggersPatched} OnTrigger patched ({Rules.TriggersSkipped} skipped), {Rules.HandlersPatched} handler methods patched, {Rules.HandlersSkippedGeneric} generic handler methods not patchable, {sw.Elapsed.TotalSeconds - t0:0.00} s");
        Hooks.Install(h);
        TurnBased.Install(h);
        WallClock.Install(h);
        TieObs.Install(h);
        SeedInit.Install(h);
        PathSync.Install(h);
        ProjPin.Install(h);
        DeathFxPin.Install(h);
        CamPin.Install(h);
        InputPin.Install(h);
        Observe.Install(h);
        GroupObs.Install(h);
        CooldownObs.Install(h);
        TurnTickObs.Install(h);
        CommandObs.Install(h);
        if (Observers.Count == 0) Observers.AddRange(new[] { "groups", "cooldowns", "turn_ticks", "commands", "attacks", "approach", "ai", "engage", "targets", "impatience" });
        if (CommandObs.AgOn) Observers.Add("agent");
        if (CommandObs.AgSyncOn) Observers.Add("agent_sync");
        // The place-on-ground step's view position, reads and rays (ag_ground vafter/fly/prone/torso, ag_phys ctx ground).
        if (CommandObs.AgOn) Observers.Add("ground_view");
        // Each roaming part at the window's first tick (roam_init, roam_init_end; CommandObsRoam.cs).
        if (CommandObs.AgOn) Observers.Add("roam_init");
        // Every write of a unit's DesiredOrientation and m_Orientation outside TickUnit's own (ag_look; CommandObsLook.cs).
        if (CommandObs.LookOn) Observers.Add("look");
        if (CommandObs.MemOn) Observers.Add("memory");
        // The entity-bounds colliders at the window's first tick (bounds_init, bounds_init_end; CommandObsBounds.cs).
        if (CommandObs.MemOn && CommandObs.BoundsOn) Observers.Add("bounds_init");
        // Each EntityBoundsController.TickEntity: the colliders' radii and positions as set (bounds_tick).
        if (CommandObs.BoundsOn) Observers.Add("bounds_tick");
        // The collider writes outside TickEntity: enabled, position, destroy, relink (bounds_set).
        if (CommandObs.BoundsOn) Observers.Add("bounds_set");
        // EntityBoundsController's scene made anew (bounds_scene).
        if (CommandObs.BoundsOn) Observers.Add("bounds_scene");
        // Each awake unit's CanBeInVisionRange in enumeration order when it changes (vis_order; Observe.cs).
        Observers.Add("vis_order");
        if (PhysProbe.Installed) Observers.Add("phys_probe");
        if (LosProbe.Installed) Observers.Add("los_geom");
        if (CommandObs.PathsOn) Observers.Add("paths");
        // Projectile arrival pinned (proj_launch, proj_hit; ProjPin.cs), critters' path requests keyed apart
        // (path_critter; PathSync.cs).
        if (ProjPin.Installed) Observers.Add("proj_arrival");
        // Death effects' dismember decisions and DismemberUnitFX's buff (death_fx, dismember; DeathFxPin.cs).
        if (DeathFxPin.Installed) Observers.Add("death_effect");
        // The camera's gates on gameplay: the turn's scroll, cutscene camera commands' completion, the view (CamPin.cs).
        if (CamPin.Installed) Observers.Add("camera_gates");
        // The initiative comparers' identity-hash decisions (init_tie; TieObs.cs).
        if (TieObs.Installed) Observers.Add("init_ties");
        if (PathSync.CritterIdOk) Observers.Add("critter_paths");
        else Health.Failed("critter path identification", new MissingFieldException("CritterMoveAgent.m_Seeker or Seeker.tmpPathCallback"));
        // The cutscenes' progress (gate_track, cs_action, roaming's draw producers) and the awake
        // units outside turn-based windows (aa_awake): declared when their patches installed.
        var failed = Health.FailedCount;
        ProducerObs.Install(h);
        if (Health.FailedCount == failed) Observers.Add("cutscene_progress");
        failed = Health.FailedCount;
        ProducerObs.InstallLocks(h);
        if (Health.FailedCount == failed) Observers.Add("cutscene_lock");
        failed = Health.FailedCount;
        ProducerObs.InstallZones(h);
        if (Health.FailedCount == failed) Observers.Add("zone_enter");
        failed = Health.FailedCount;
        ProducerObs.InstallCommands(h);
        if (Health.FailedCount == failed) Observers.Add("cutscene_commands");
        failed = Health.FailedCount;
        AwakeObs.Install(h);
        if (Health.FailedCount == failed) Observers.Add("aa_awake");
        // Roaming's draw producers' caller and unit (roam_point), and the unit groups outside
        // turn-based windows (grp_tick).
        failed = Health.FailedCount;
        ProducerObs.InstallRoaming(h);
        if (Health.FailedCount == failed) Observers.Add("roam_producers");
        failed = Health.FailedCount;
        GroupObs.InstallTicks(h);
        if (Health.FailedCount == failed) Observers.Add("group_ticks");
        // The dialogue's history lines and display text (dlg_history, dlg_text).
        // Mutations in Pause mode carry their caller chain (Hooks.Mut).
        Observers.Add("paused_origins");
        try { TextObs.Install(h); Observers.Add("dialog_text"); }
        catch (Exception e) { Health.Failed("patch the dialogue text (dlg_history, dlg_text)", e); }
        // The combat log's events, ticks and messages (clog_q, clog_tick, clog_msg; LogObs.cs).
        failed = Health.FailedCount;
        try { LogObs.Install(h); } catch (Exception e) { Health.Failed("patch the combat log", e); }
        if (LogObs.Installed && Health.FailedCount == failed) Observers.Add("combat_log");
        // The inspection controller's reveals (insp_reveal; InspectObs.cs).
        failed = Health.FailedCount;
        InspectObs.Install(h);
        if (InspectObs.Installed && Health.FailedCount == failed) Observers.Add("inspect_reveal");
        OracleTime.Install(h);
        InstallSeconds = sw.Elapsed.TotalSeconds;
        Notes.Add($"install total {InstallSeconds:0.00} s");
        Installed = true;
        foreach (var n in Notes) Main.Note(n);
    }

    // The observation families Install set up (the trace's begin record lists them).
    internal static readonly List<string> Observers = new List<string>();

    public static void WriteInstall(Json j)
    {
        j.Obj();
        j.Prop("installed", Installed);
        j.Prop("install_failures", InstallFailures);
        j.Prop("rng_self_test_ok", Rng.SelfTestOk);
        j.Prop("rng_sites", Rng.Sites.Count);
        j.Prop("rng_sites_transpiled", Rng.Sites.Count(s => s.Transpiled));
        j.Prop("rng_sites_generic", Rng.Sites.Count(s => s.Generic));
        j.Prop("rules_on_trigger_patched", Rules.TriggersPatched);
        j.Prop("rules_handlers_patched", Rules.HandlersPatched);
        j.Prop("rules_handlers_generic_unpatched", Rules.HandlersSkippedGeneric);
        j.Arr("notes"); foreach (var n in Notes) j.W.WriteValue(n); j.EndArr();
        j.End();
    }

    public static void WriteFaults(Json j)
    {
        j.Obj();
        j.Prop("throw_about_at", Rules.FaultThrowAboutAt);
        j.Prop("drop_about_at", Rules.FaultDropAboutAt);
        j.Prop("disable_site", Rng.DisableSiteContaining);
        j.End();
    }

    public static void WriteSitesFile(string outDir)
    {
        using (var j = new Json(Path.Combine(outDir, "rng-sites.json"), indent: true))
        {
            j.Obj();
            j.Prop("schema", "wrath-oracle-rng-sites-v1");
            j.Key("install"); WriteInstall(j);
            j.Key("sites"); Rng.WriteSites(j, compact: false);
            j.Arr("global_sites");
            foreach (var s in Rng.GlobalSiteList()) j.Obj().Prop("key", s.Key).Prop("draws", s.Draws).End();
            j.EndArr();
            j.Arr("handlers_not_patchable"); foreach (var s in Rules.SkippedHandlers) j.W.WriteValue(s); j.EndArr();
            j.End();
        }
    }
}

using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Linq;
using Kingmaker;
using Kingmaker.EntitySystem.Persistence;
using Kingmaker.Utility;
using UnityEngine;

namespace WrathOracle;

// Runs the plan's steps on the main thread (a coroutine). run.json is rewritten after
// every step, so a crash keeps everything up to the last finished step.
internal sealed class Runner : MonoBehaviour
{
    public Plan Plan;
    public int StepIndex;
    private readonly List<string> _records = new List<string>();
    private readonly List<string> _errors = new List<string>();
    private string _saveFile;
    private SaveInfo _save;
    private bool _aborted;
    private int _routePass, _saveIndex;
    private readonly Stopwatch _runClock = Stopwatch.StartNew();

    private IEnumerator Start()
    {
        Directory.CreateDirectory(Plan.Out);
        while (Main.MainMenuInstance == null) yield return null;
        yield return new WaitForSecondsRealtime(Plan.MenuSettle);

        var dir = Path.Combine(ApplicationPaths.persistentDataPath, SaveManager.SaveFolderName);
        _saveFile = Directory.GetFiles(dir, Plan.Save + "*.zks").OrderBy(f => f, StringComparer.Ordinal).FirstOrDefault();
        if (_saveFile == null) Fail($"save '{Plan.Save}' not found in {dir}");
        else _save = Game.Instance.SaveManager.LoadZipSave(_saveFile);
        Main.Note($"run start: {Plan.Label} save={Path.GetFileName(_saveFile)} out={Plan.Out}");

        for (var i = 0; i < Plan.Steps.Count; i++)
        {
            StepIndex = i;
            var step = Plan.Steps[i];
            var name = step.Split(':')[0];
            var arg = step.Contains(":") ? step.Substring(step.IndexOf(':') + 1) : null;
            if (_aborted && name != "quit") continue;
            // A turn-based replay that left its recording is over: skip to the window's end.
            if (Sched.Invalid != null && name != "end" && name != "quit")
            {
                Main.Note($"step {i}: {step} skipped (replay left its recording)");
                _records.Add(Rec(i, step, j => j.Prop("skipped", "replay left its recording")));
                continue;
            }
            Main.Note($"step {i}: {step}");
            var t0 = _runClock.Elapsed.TotalSeconds;
            IEnumerator work = null;
            string record = null;
            try
            {
                switch (name)
                {
                    case "load": work = Load(i, fromMenu: true); break;
                    case "reload": work = Load(i, fromMenu: false); break;
                    case "settle": work = Settle(float.Parse(arg ?? "10", CultureInfo.InvariantCulture)); break;
                    case "mem": record = Rec(i, step, j => { j.Key("memory"); Memory.Write(j, arg ?? $"step{i}"); }); break;
                    case "identity":
                        using (var j = new Json(Path.Combine(Plan.Out, "identity.json"), indent: true))
                        {
                            j.Obj();
                            Identity.Write(j, Plan, _saveFile);
                            j.End();
                        }
                        break;
                    case "dump": SceneDump.Run(Plan.Out, Plan.Hierarchy, Plan.Inventory); break;
                    case "nav": NavDump.Run(Plan.Out, Plan.NavQueries); break;
                    case "route": work = CameraRoute.Run(Plan, ++_routePass, Plan.Out, Fail); break;
                    case "save": work = SaveStep(i, step); break;
                    case "quit": break;
                    case "fixture":
                    case "replay":
                        {
                            var parts = (arg ?? "").Split(new[] { ':' }, 2);
                            var ahead = Plan.Steps.Skip(i + 1).TakeWhile(x => x != "end").ToList();
                            Fixture.Begin(Plan, parts[0], name == "replay", parts.Length > 1 ? parts[1] : null, _saveFile, ahead);
                            break;
                        }
                    case "end":
                        record = Rec(i, step, j => j.Prop("fixture", Fixture.End()));
                        break;
                    case "state":
                        {
                            var path = StateDump.Run(Plan.Out, arg ?? $"step{i}", _saveFile);
                            record = Rec(i, step, j => j.Prop("file", path));
                            break;
                        }
                    case "blueprints":
                        {
                            var summary = BlueprintCanon.Run(Plan.Out, arg ?? "dump", Plan.BlueprintList, includeLoaded: true);
                            record = Rec(i, step, j => j.Prop("summary", summary));
                            break;
                        }
                    case "sites":
                        Fidelity.WriteSitesFile(Plan.Out);
                        break;
                    default:
                        if (name.EndsWith("?"))
                        {
                            // Optional step: if it can't be done, note it and carry on.
                            var opt = name.TrimEnd('?');
                            work = Driver.Step(opt, arg, this, m => Main.Note("optional step skipped: " + m), optional: true);
                            if (work == null) Fail($"unknown step '{step}'");
                            break;
                        }
                        work = Driver.Step(name, arg, this, Fail);
                        if (work == null) Fail($"unknown step '{step}'");
                        break;
                }
            }
            catch (Exception e)
            {
                Fail($"step {step}: {e}");
            }
            if (work != null) yield return Guarded(work, step);
            var elapsed = _runClock.Elapsed.TotalSeconds - t0;
            _records.Add(record ?? Rec(i, step, j => { }));
            _records[_records.Count - 1] = AddElapsed(_records[_records.Count - 1], elapsed);
            WriteRun();
            if (name == "quit")
            {
                Fixture.Cleanup();
                if (Fidelity.Installed) { try { Fidelity.WriteSitesFile(Plan.Out); } catch (Exception e) { Health.Failed("sites file", e); } }
                WriteRun(); // again: cleanup may have closed a fixture and added its health
                Main.Note("quit");
                yield return new WaitForSecondsRealtime(1f);
                Application.Quit();
                yield break;
            }
        }
        Fixture.Cleanup();
        if (Fidelity.Installed) { try { Fidelity.WriteSitesFile(Plan.Out); } catch (Exception e) { Health.Failed("sites file", e); } }
        Main.Note("plan finished (no quit step); saves are allowed again");
        SaveGuard.Active = false;
        WriteRun();
    }

    // Runs a step coroutine; an exception inside it fails the step instead of killing the run.
    private IEnumerator Guarded(IEnumerator work, string step)
    {
        while (true)
        {
            object current;
            if (Sched.Invalid != null) yield break; // the replay left its recording: stop the step
            try
            {
                if (!work.MoveNext()) yield break;
                current = work.Current;
            }
            catch (Exception e)
            {
                Fail($"step {step}: {e}");
                yield break;
            }
            yield return current is IEnumerator nested ? Guarded(nested, step) : current;
        }
    }

    private void OnDestroy() => Fixture.Cleanup();

    private void Fail(string message)
    {
        _aborted = true;
        _errors.Add(message);
        Main.Mod.Logger.Error(message);
        Main.Note("FAILED: " + message);
    }


    private static string Rec(int index, string step, Action<Json> body)
    {
        var sw = new StringWriter(CultureInfo.InvariantCulture);
        using (var j = new Json(sw))
        {
            j.Obj();
            j.Prop("index", index);
            j.Prop("step", step);
            body(j);
            j.End();
        }
        return sw.ToString();
    }

    private static string AddElapsed(string json, double elapsed) =>
        json.Substring(0, json.Length - 1) + ",\"elapsed_s\":" + elapsed.ToString("R", CultureInfo.InvariantCulture) + "}";

    private IEnumerator Settle(float seconds)
    {
        var until = Time.realtimeSinceStartup + seconds;
        while (Time.realtimeSinceStartup < until) yield return null;
    }

    // Load boundary: from the call (MainMenu.LoadGame or Game.LoadGame) to the first frame
    // in which LoadingProcess.IsLoadingInProcess is false again after it became true (the
    // same span WrathStats records as a load event). Then idle must hold for 1 s; if
    // loading resumes within that second, the wait continues and the chain is reported.
    private IEnumerator Load(int index, bool fromMenu)
    {
        if (_save == null) { Fail("no save to load"); yield break; }
        var clock = FrameClock.Instance;
        clock.Arm();
        var sw = Stopwatch.StartNew();
        var frame0 = Time.frameCount;
        // AbilityParams.Empty is process state (AddFacts writes its CasterLevel through param-less
        // contexts, and loading doesn't reset it): the value a load starts from, for the state dumps.
        StateDump.NoteAbilityParamsEmptyAtLoad();
        if (fromMenu)
        {
            if (Main.MainMenuInstance == null) { Fail("load: not at the main menu"); yield break; }
            Main.MainMenuInstance.LoadGame(_save);
        }
        else
        {
            Game.Instance.LoadGame(_save);
        }
        var lp = LoadingProcess.Instance;
        double busyAt = -1, idleAt = -1;
        while (sw.Elapsed.TotalSeconds < 10)
        {
            if (lp.IsLoadingInProcess) { busyAt = sw.Elapsed.TotalSeconds; break; }
            yield return null;
        }
        if (busyAt < 0) { clock.Disarm(); Fail("load never started"); yield break; }
        var restarts = 0;
        double idleSince = -1;
        int idleFrame = -1, idleClock = -1;
        var completed = false;
        while (sw.Elapsed.TotalSeconds < 300)
        {
            var now = sw.Elapsed.TotalSeconds;
            if (lp.IsLoadingInProcess)
            {
                if (idleSince >= 0) restarts++;
                idleSince = -1;
            }
            else if (idleSince < 0)
            {
                // The boundary: the first idle frame (again, after any restart).
                idleSince = idleAt = now;
                idleFrame = Time.frameCount;
                idleClock = clock.Count;
                PrePathSync.Start(); // pre-window path deliveries from here (a fixed number of frames before a window)
            }
            else if (Trace.Active || OracleTime.FixtureMode ? Time.frameCount - idleFrame >= 60 : now - idleSince >= 1.0)
            {
                completed = true;
                break;
            }
            yield return null;
        }
        clock.Disarm();
        if (!completed) { Fail("load did not finish within 300 s"); yield break; }
        PhysicsPhase.Anchor(this); // a turn-based replay reproduces its recording's physics phase from here
        InputPin.NoteReload();
        // Frame stats up to the boundary (the confirmation second excluded).
        var frames = idleFrame - frame0;
        double maxFrame = 0;
        for (var k = 1; k <= idleClock && k < clock.Count; k++) maxFrame = Math.Max(maxFrame, clock.IntervalMs(k));
        var area = Game.Instance.CurrentlyLoadedArea;
        _records.Add(Rec(index, fromMenu ? "load:detail" : "reload:detail", j =>
        {
            j.Obj("load");
            j.Prop("from_menu", fromMenu);
            j.Prop("call_to_busy_s", busyAt);
            j.Prop("busy_to_idle_s", idleAt - busyAt);
            j.Prop("call_to_idle_s", idleAt);
            j.Prop("idle_restarts", restarts);
            j.Prop("frames", frames);
            j.Prop("max_frame_ms", maxFrame);
            j.Prop("area", area != null ? area.name : null);
            j.Prop("area_guid", area != null ? area.AssetGuid.ToString() : null);
            j.Prop("game_mode", Game.Instance.CurrentMode.ToString());
            j.End();
        }));
        Main.Note($"{(fromMenu ? "load" : "reload")}: call->idle {idleAt:0.000} s (busy {idleAt - busyAt:0.000} s), {frames} frames, area {area?.name}");
        if (Plan.Area != "any" && (area == null || area.AssetGuid.ToString() != Plan.Area)) Fail($"loaded area is {area?.name}, expected {Plan.Area}");
    }

    private IEnumerator SaveStep(int index, string step)
    {
        var sw = new StringWriter(CultureInfo.InvariantCulture);
        var j = new Json(sw);
        j.Obj();
        j.Prop("index", index);
        j.Prop("step", step + ":detail");
        j.Key("save");
        var inner = SaveBench.Run(Plan, ++_saveIndex, j, Fail);
        while (true)
        {
            object cur;
            try
            {
                if (!inner.MoveNext()) break;
                cur = inner.Current;
            }
            catch (Exception e)
            {
                Fail($"save: {e}");
                break;
            }
            yield return cur;
        }
        try
        {
            if (j.W.WriteState == Newtonsoft.Json.WriteState.Property) j.W.WriteNull();
            j.End();
            j.Dispose();
            _records.Add(sw.ToString());
        }
        catch (Exception e)
        {
            Health.Failed("save record", e);
        }
    }

    private void WriteRun()
    {
        try
        {
            // Write a sibling file and swap it in, so a crash mid-write keeps the last good one.
            var path = Path.Combine(Plan.Out, "run.json");
            var tmp = path + ".tmp";
            using (var j = new Json(tmp, indent: true))
            {
            j.Obj();
            j.Prop("schema", "wrath-oracle-run-v1");
            j.Prop("label", Plan.Label);
            j.Prop("oracle_version", Main.Mod.Info.Version);
            j.Prop("save_file", _saveFile);
            j.Prop("plan", Plan.RawJson);
            j.Prop("aborted", _aborted);
            j.Arr("errors");
            foreach (var e in _errors) j.W.WriteValue(e);
            j.EndArr();
            j.Arr("steps");
            foreach (var r in _records) j.W.WriteRawValue(r);
            j.EndArr();
            Health.Write(j);
            if (Fidelity.Installed)
            {
                j.Key("fidelity"); Fidelity.WriteInstall(j);
                j.Arr("fixtures"); foreach (var r in Fixture.Results) j.W.WriteValue(r); j.EndArr();
                j.Arr("fixture_health");
                foreach (var h in Fixture.HealthRecords)
                {
                    j.Obj();
                    foreach (var kv in h) { j.Key(kv.Key); Snap.Value(j, kv.Value, Snap.Mode.Fields); }
                    j.End();
                }
                j.EndArr();
            }
            j.End();
            }
            if (File.Exists(path)) File.Replace(tmp, path, null);
            else File.Move(tmp, path);
        }
        catch (Exception e)
        {
            Main.Mod.Logger.Error($"could not write run.json: {e}");
        }
    }
}

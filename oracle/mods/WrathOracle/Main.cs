using System;
using System.IO;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Utility;
using Newtonsoft.Json.Linq;
using UnityEngine;
using UnityModManagerNet;

namespace WrathOracle;

// WrathOracle: the Wrath-Rust oracle (Wrath-Rust PLAN, "The oracle"). Like WrathBench it is
// idle unless <persistent data>/WrathOracle/plan.json exists. The plan is read and deleted
// at startup (one-shot), so the next normal launch is unaffected. With a plan it waits for
// the main menu, loads the named save and runs the plan's steps in order (Runner.cs):
//   load | reload | settle:N | mem:LABEL | identity | dump | nav | route | save | quit
// Outputs go to the plan's "out" folder. Version 0.1 (POC-0) only observes: scene/state
// dumps, navigation queries through the game's own A*, a camera route with resolved
// render-camera poses and screenshots or frame timings, and a timed manual save into a
// slot it creates and deletes again. It never writes to the save it loads.
public static class Main
{
    internal static UnityModManager.ModEntry Mod;
    internal static MainMenu MainMenuInstance; // set by the MainMenu.Start postfix

    public static bool Load(UnityModManager.ModEntry modEntry)
    {
        Mod = modEntry;
        var planPath = Path.Combine(ApplicationPaths.persistentDataPath ?? Application.persistentDataPath, "WrathOracle", "plan.json");
        if (!File.Exists(planPath))
        {
            modEntry.Logger.Log("idle (no plan.json)");
            return true;
        }

        // Consume the plan before doing anything with it: rename it away (atomic on the same
        // volume), so it can never trigger a second run. If that fails, don't run.
        var taken = planPath + ".taken";
        try
        {
            if (File.Exists(taken)) File.Delete(taken);
            File.Move(planPath, taken);
        }
        catch (Exception e)
        {
            modEntry.Logger.Error($"could not consume plan.json ({e.Message}); NOT running. Delete {planPath} by hand.");
            return true;
        }
        Plan plan;
        try
        {
            plan = Plan.Parse(JObject.Parse(File.ReadAllText(taken)));
        }
        catch (Exception e)
        {
            modEntry.Logger.Error($"bad plan.json, not running: {e.Message}");
            return true;
        }
        finally
        {
            try { File.Delete(taken); } catch (Exception) { /* inert name; harmless */ }
        }
        modEntry.Logger.Log($"plan: save={plan.Save} out={plan.Out} steps={string.Join(",", plan.Steps)}");

        SaveGuard.Active = true; // from here on, only the oracle's own bench slot may be saved
        var harmony = new Harmony(modEntry.Info.Id);
        harmony.PatchAll(typeof(Main).Assembly);
        if (plan.Fidelity)
        {
            try
            {
                Fidelity.Install(plan, harmony);
            }
            catch (Exception e)
            {
                modEntry.Logger.Error("fidelity install failed: " + e);
                Health.Failed("fidelity install", e);
                Fidelity.InstallFailures++;
            }
        }
        var host = new GameObject("WrathOracle");
        UnityEngine.Object.DontDestroyOnLoad(host);
        host.AddComponent<FrameClock>();
        host.AddComponent<Runner>().Plan = plan;
        return true;
    }

    // Any thread: UMM log plus, if WrathStats is installed, its session events, so the
    // frame data WrathStats records can be lined up with the oracle's steps.
    internal static void Note(string text)
    {
        Mod.Logger.Log(text);
        try
        {
            NoteToWrathStats?.Invoke(null, new object[] { "oracle: " + text });
        }
        catch (Exception)
        {
            // optional
        }
    }

    private static readonly System.Reflection.MethodInfo NoteToWrathStats =
        AccessTools.TypeByName("WrathStats.WrathStatsApi")?.GetMethod("Note", new[] { typeof(string) });
}

[HarmonyPatch(typeof(MainMenu), "Start")]
internal static class MainMenuStartPatch
{
    private static void Postfix(MainMenu __instance) => Main.MainMenuInstance = __instance;
}

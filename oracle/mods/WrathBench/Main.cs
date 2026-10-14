using System;
using System.Collections;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Threading;
using HarmonyLib;
using Kingmaker;
using Kingmaker.EntitySystem.Persistence;
using Kingmaker.Utility;
using Owlcat.Runtime.Visual.Dxt;
using UnityEngine;
using UnityModManagerNet;
using WrathStats;

namespace WrathBench;

// Dev-only benchmark driver for load-time experiments (PLAN Phase 8). Idle unless
// <persistent data>/WrathBench/plan.json exists; the plan is deleted when it's picked up,
// so a run happens once and the next normal launch is unaffected. With a plan it:
//   - waits for the main menu, loads the named save, then reloads it `reloads` times,
//     settling `settle` seconds after each load, and optionally quits;
//   - with `screenshots`, saves a screenshot at the end of each settle (WrathBench/shots/),
//     to compare portraits and equipment between configurations;
//   - times every DXT atlas compression call (DxtCompressorService.Request.Compress)
//     and reports totals per load as WrathStats notes.
// WrathStats records the loads themselves (load events with durations).
[Serializable]
public sealed class Plan
{
    public string save = "Manual_14_WS_Bench_Deskari";
    public int reloads = 3;
    public float settle = 10f;
    public bool quit = true;
    public bool screenshots;
    // Experiment: compress with one DxtContext initialised once (instead of per call).
    public bool dxtCachedContext;
}

public static class Main
{
    internal static UnityModManager.ModEntry.ModLogger Log;
    internal static MainMenu MainMenuInstance; // set when the main menu starts

    public static bool Load(UnityModManager.ModEntry modEntry)
    {
        Log = modEntry.Logger;
        var planPath = Path.Combine(ApplicationPaths.persistentDataPath ?? Application.persistentDataPath, "WrathBench", "plan.json");
        if (!File.Exists(planPath))
        {
            Log.Log("idle (no plan.json)");
            return true;
        }

        Plan plan;
        try
        {
            plan = JsonUtility.FromJson<Plan>(File.ReadAllText(planPath)) ?? new Plan();
        }
        finally
        {
            File.Delete(planPath); // one-shot
        }
        Log.Log($"plan: save={plan.save} reloads={plan.reloads} settle={plan.settle}s quit={plan.quit}");

        DxtCachedContext.Enabled = plan.dxtCachedContext;
        Log.Log($"experiment dxtCachedContext={plan.dxtCachedContext}");
        var harmony = new Harmony(modEntry.Info.Id);
        harmony.PatchAll(typeof(Main).Assembly);
        var host = new GameObject("WrathBench");
        UnityEngine.Object.DontDestroyOnLoad(host);
        host.AddComponent<Runner>().Plan = plan;
        return true;
    }
}

[HarmonyPatch(typeof(MainMenu), "Start")]
internal static class MainMenuStartPatch
{
    private static void Postfix(MainMenu __instance) => Main.MainMenuInstance = __instance;
}

// Times atlas compression, which runs on worker threads (Task).
[HarmonyPatch]
internal static class DxtProbe
{
    [ThreadStatic] private static long _start;
    private static long _ticks, _maxTicks, _calls;

    private static System.Reflection.MethodBase TargetMethod() =>
        AccessTools.Method(typeof(DxtCompressorService.Request), "Compress",
            new[] { typeof(int), typeof(int), typeof(byte*), typeof(byte*), typeof(bool) });

    [HarmonyPriority(Priority.First)]
    private static void Prefix() => _start = Stopwatch.GetTimestamp();

    private static void Postfix()
    {
        var t = Stopwatch.GetTimestamp() - _start;
        Interlocked.Add(ref _ticks, t);
        Interlocked.Increment(ref _calls);
        long max;
        while (t > (max = Interlocked.Read(ref _maxTicks)) && Interlocked.CompareExchange(ref _maxTicks, t, max) != max) { }
    }

    internal static string TakeReport()
    {
        var ticks = Interlocked.Exchange(ref _ticks, 0);
        var calls = Interlocked.Exchange(ref _calls, 0);
        var max = Interlocked.Exchange(ref _maxTicks, 0);
        double Ms(long x) => x * 1000.0 / Stopwatch.Frequency;
        return $"dxt compress: calls={calls} total={Ms(ticks):0} ms max={Ms(max):0} ms";
    }
}

// Experiment: same algorithm, but the lookup tables are built once. The original
// (Request.Compress$BurstManaged) runs DxtContext.Init() on every call.
[HarmonyPatch]
internal static class DxtCachedContext
{
    internal static bool Enabled;
    private static readonly StbDxtSharp.DxtContext Context = CreateContext();

    private static StbDxtSharp.DxtContext CreateContext()
    {
        var ctx = default(StbDxtSharp.DxtContext);
        ctx.Init();
        return ctx;
    }

    private static System.Reflection.MethodBase TargetMethod() =>
        AccessTools.Method(typeof(DxtCompressorService.Request), "Compress",
            new[] { typeof(int), typeof(int), typeof(byte*), typeof(byte*), typeof(bool) });

    private static unsafe bool Prefix(int width, int height, byte* inData, byte* outData, bool hasAlpha)
    {
        if (!Enabled) return true;
        if (hasAlpha) StbDxtSharp.StbDxt.CompressDxt5(Context, width, height, inData, outData);
        else StbDxtSharp.StbDxt.CompressDxt1(Context, width, height, inData, outData);
        return false;
    }
}

internal sealed class Runner : MonoBehaviour
{
    public Plan Plan;

    private IEnumerator Start()
    {
        while (Main.MainMenuInstance == null) yield return null;
        yield return new WaitForSecondsRealtime(Plan.settle);

        var dir = Path.Combine(ApplicationPaths.persistentDataPath, SaveManager.SaveFolderName);
        var file = Directory.GetFiles(dir, Plan.save + "*.zks").OrderBy(f => f).FirstOrDefault();
        if (file == null)
        {
            Main.Log.Error($"save '{Plan.save}' not found in {dir}");
            yield break;
        }
        var save = Game.Instance.SaveManager.LoadZipSave(file);
        Step($"bench start: {Path.GetFileName(file)}");
        DxtProbe.TakeReport(); // discard anything from startup

        // The main menu's own path: loading screen, dispose the menu, load the obligatory
        // scenes, then Game.LoadGameFromMainMenu. Calling the latter directly breaks loads.
        Main.MainMenuInstance.LoadGame(save);
        yield return WaitForLoad("load from main menu");

        for (var i = 1; i <= Plan.reloads; i++)
        {
            Game.Instance.LoadGame(save);
            yield return WaitForLoad($"reload {i}");
        }

        Step("bench done");
        if (Plan.quit)
        {
            yield return new WaitForSecondsRealtime(1f);
            Application.Quit();
        }
    }

    // Loading is a chain of LoadingProcess steps; done = idle for a full second.
    private IEnumerator WaitForLoad(string label)
    {
        var sw = Stopwatch.StartNew();
        while (!LoadingProcess.Instance.IsLoadingInProcess && sw.Elapsed.TotalSeconds < 5) yield return null;
        var idleSince = -1.0;
        while (true)
        {
            var now = sw.Elapsed.TotalSeconds;
            if (LoadingProcess.Instance.IsLoadingInProcess) idleSince = -1;
            else if (idleSince < 0) idleSince = now;
            else if (now - idleSince >= 1.0) break;
            yield return null;
        }
        var loaded = sw.Elapsed.TotalSeconds - 1.0;
        yield return new WaitForSecondsRealtime(Plan.settle);
        Step($"{label}: loaded in ~{loaded:0.00} s (driver view); {DxtProbe.TakeReport()}");
        if (Plan.screenshots)
        {
            var dir = Path.Combine(ApplicationPaths.persistentDataPath, "WrathBench", "shots");
            Directory.CreateDirectory(dir);
            // UMM opens its window at startup; it would cover the game.
            if (UnityModManager.UI.Instance != null && UnityModManager.UI.Instance.Opened)
            {
                UnityModManager.UI.Instance.ToggleWindow(false);
                yield return null;
            }
            ScreenCapture.CaptureScreenshot(Path.Combine(dir, $"{DateTime.Now:HHmmss}-{label.Replace(' ', '-')}.png"));
            yield return null; // written at the end of the frame
        }
    }

    private static void Step(string text)
    {
        Main.Log.Log(text);
        WrathStatsApi.Note("bench: " + text);
    }
}

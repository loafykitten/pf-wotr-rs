using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using Kingmaker.Utility;
using Newtonsoft.Json.Linq;

namespace WrathOracle;

// plan.json, e.g.
// {
//   "save": "Manual_13_Underground_Caves_of_Kenabres",
//   "out": "<repo>/fixtures/captures/poc0/oracle-1",
//   "steps": ["load", "settle:10", "identity", "dump", "nav", "route", "reload", "settle:10", "route", "quit"],
//   "route": { "mode": "shots", "settle": 5, "dwell": 37.5, "fow": "off", "hideUi": true },
//   "hierarchy": true
// }
internal sealed class Plan
{
    public string Save = "Manual_13_Underground_Caves_of_Kenabres";
    public string Out;
    public string Label = "";
    public float MenuSettle = 8f;
    public List<string> Steps = new List<string>();
    public RouteSettings Route = new RouteSettings();
    public bool Hierarchy = true;
    public string SaveSlotName = "WR_Bench";
    public string RawJson;
    // Optional inputs from the extraction tool (Wrath-Rust cache/import/<area>/): its
    // inventory.json (script-zone probes to evaluate) and nav.json (queries to run).
    public string Inventory, NavQueries;
    // 0.2: fidelity instrumentation (rules, draws, time, dialogue, mutations) and fixtures.
    public bool Fidelity;
    public float FixedDelta = 1f / 60f;
    public int Seed = 20261011;
    public string Area = "bad465e5404604346bdf71ef50b3299a"; // expected area after a load; "any" = don't check
    public string BlueprintList;   // file of GUIDs for the "blueprints" step
    public FaultSettings Faults;

    internal static Plan Parse(JObject o)
    {
        var p = new Plan { RawJson = o.ToString(Newtonsoft.Json.Formatting.None) };
        p.Save = (string)o["save"] ?? p.Save;
        p.Label = (string)o["label"] ?? "";
        p.MenuSettle = (float?)o["menuSettle"] ?? p.MenuSettle;
        p.Hierarchy = (bool?)o["hierarchy"] ?? p.Hierarchy;
        p.SaveSlotName = (string)o["saveSlot"] ?? p.SaveSlotName;
        p.Inventory = (string)o["inventory"];
        p.NavQueries = (string)o["navQueries"];
        p.Fidelity = (bool?)o["fidelity"] ?? false;
        p.FixedDelta = (float?)o["fixedDelta"] ?? p.FixedDelta;
        p.Seed = (int?)o["seed"] ?? p.Seed;
        p.Area = (string)o["area"] ?? p.Area;
        p.BlueprintList = (string)o["blueprintList"];
        if (o["faults"] is JObject f)
            p.Faults = new FaultSettings { ThrowAboutAt = (int?)f["throwAboutAt"] ?? -1, DropAboutAt = (int?)f["dropAboutAt"] ?? -1, DisableSite = (string)f["disableSite"] };
        if (!System.Text.RegularExpressions.Regex.IsMatch(p.SaveSlotName, "^WR_[A-Za-z0-9_]{1,40}$"))
            throw new ArgumentException("saveSlot must look like WR_Something (letters, digits, _)");
        var steps = o["steps"] as JArray ?? throw new ArgumentException("plan needs a steps array");
        foreach (var s in steps) p.Steps.Add((string)s);
        if (o["route"] is JObject r)
        {
            p.Route.Mode = (string)r["mode"] ?? p.Route.Mode;
            p.Route.Settle = (float?)r["settle"] ?? p.Route.Settle;
            p.Route.Dwell = (float?)r["dwell"] ?? p.Route.Dwell;
            p.Route.Fow = (string)r["fow"] ?? p.Route.Fow;
            p.Route.HideUi = (bool?)r["hideUi"] ?? p.Route.HideUi;
            p.Route.WarmupFrames = (int?)r["warmupFrames"] ?? p.Route.WarmupFrames;
        }
        if (p.Route.Mode != "shots" && p.Route.Mode != "bench") throw new ArgumentException("route.mode is shots or bench");
        if (p.Route.Fow != "off" && p.Route.Fow != "game") throw new ArgumentException("route.fow is off or game");
        var outDir = (string)o["out"];
        if (string.IsNullOrEmpty(outDir))
        {
            var stamp = DateTime.UtcNow.ToString("yyyyMMdd'T'HHmmss'Z'", CultureInfo.InvariantCulture);
            outDir = Path.Combine(ApplicationPaths.persistentDataPath, "WrathOracle", "runs", stamp);
        }
        if (!Path.IsPathRooted(outDir)) throw new ArgumentException("out must be an absolute path");
        p.Out = outDir;
        return p;
    }
}

internal sealed class RouteSettings
{
    // shots: settle at each point, then record the camera pose and a screenshot.
    // bench: dwell at each point recording every frame time (no screenshots).
    public string Mode = "shots";
    public float Settle = 5f;   // seconds after each move before the screenshot (shots)
    public float Dwell = 37.5f; // seconds at each point (bench); 8 x 37.5 s = 5 min
    public string Fow = "off";  // off: FogOfWarArea disabled during the route (as Arbiter does); game: as loaded
    public bool HideUi = true;
    public int WarmupFrames = 15; // frames after each move reported separately (bench)
}

internal sealed class FaultSettings
{
    public int ThrowAboutAt = -1, DropAboutAt = -1;
    public string DisableSite;
}

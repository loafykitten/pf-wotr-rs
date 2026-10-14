using System;
using System.Collections.Generic;
using System.IO;
using System.Reflection;
using Newtonsoft.Json.Linq;
using UnityEngine;

namespace WrathOracle;

// UnityEngine.Random's state, so a dump can show it consumed no draws (the oracle must
// not perturb the RNG the rules use).
internal static class RandomState
{
    private static readonly FieldInfo[] Fields =
    {
        typeof(UnityEngine.Random.State).GetField("s0", BindingFlags.Instance | BindingFlags.NonPublic),
        typeof(UnityEngine.Random.State).GetField("s1", BindingFlags.Instance | BindingFlags.NonPublic),
        typeof(UnityEngine.Random.State).GetField("s2", BindingFlags.Instance | BindingFlags.NonPublic),
        typeof(UnityEngine.Random.State).GetField("s3", BindingFlags.Instance | BindingFlags.NonPublic),
    };

    public static string Text()
    {
        object st = UnityEngine.Random.state;
        var parts = new List<string>();
        foreach (var f in Fields) parts.Add(f != null ? ((int)f.GetValue(st)).ToString("x8") : "?");
        return string.Join(":", parts);
    }

    public static void Write(Json j) => j.W.WriteValue(Text());
}

// The extraction tool's script-zone probes (Wrath-Rust cache/import/<area>/inventory.json,
// schema wr-import/area-inventory/0: zones[].unique_id, zones[].shapes[].probes[] with
// point, corpulence, inside), so the game evaluates exactly the same points.
internal sealed class ZoneProbes
{
    internal struct Probe
    {
        public Vector3 Point;
        public float Corpulence;
        public bool? Inside;
    }

    public string Source = "none";
    private readonly Dictionary<string, List<List<Probe>>> _byZone = new Dictionary<string, List<List<Probe>>>();

    public static ZoneProbes Load(string path)
    {
        var z = new ZoneProbes();
        if (string.IsNullOrEmpty(path)) return z;
        try
        {
            var inv = JObject.Parse(File.ReadAllText(path));
            foreach (var zone in inv["zones"] ?? new JArray())
            {
                var id = (string)zone["unique_id"];
                if (id == null) continue;
                var shapes = new List<List<Probe>>();
                foreach (var shape in zone["shapes"] ?? new JArray())
                {
                    var list = new List<Probe>();
                    foreach (var p in shape["probes"] ?? new JArray())
                    {
                        var pt = (JArray)p["point"];
                        list.Add(new Probe
                        {
                            Point = new Vector3((float)pt[0], (float)pt[1], (float)pt[2]),
                            Corpulence = (float?)p["corpulence"] ?? 0f,
                            Inside = (bool?)p["inside"],
                        });
                    }
                    shapes.Add(list);
                }
                _ = z._byZone[id] = shapes;
            }
            z.Source = $"{path} (sha256 {Json.Sha256File(path)})";
        }
        catch (Exception e)
        {
            Health.Failed("load inventory probes", e);
            z.Source = "failed: " + e.Message;
        }
        return z;
    }

    // Probes of a zone's shape by its index in GetComponentsInChildren(includeInactive)
    // order, the order the extractor lists shapes in (inactive ones included).
    public List<Probe> For(string zoneId, int shapeIndex)
    {
        if (zoneId == null || shapeIndex < 0 || !_byZone.TryGetValue(zoneId, out var shapes)) return null;
        return shapeIndex < shapes.Count ? shapes[shapeIndex] : null;
    }
}

// The extraction tool's queries (cache/import/<area>/nav.json: queries[].label, start, end).
internal static class ExtractorQueries
{
    public static List<(string label, Vector3 start, Vector3 end)> Load(string path, out string source)
    {
        var list = new List<(string, Vector3, Vector3)>();
        source = "none";
        if (string.IsNullOrEmpty(path)) return list;
        try
        {
            var nav = JObject.Parse(File.ReadAllText(path));
            foreach (var q in nav["queries"] ?? new JArray())
            {
                var s = (JArray)q["start"];
                var e = (JArray)q["end"];
                list.Add(((string)q["label"], new Vector3((float)s[0], (float)s[1], (float)s[2]), new Vector3((float)e[0], (float)e[1], (float)e[2])));
            }
            source = $"{path} (sha256 {Json.Sha256File(path)})";
        }
        catch (Exception e)
        {
            Health.Failed("load extractor nav queries", e);
            source = "failed: " + e.Message;
        }
        return list;
    }
}

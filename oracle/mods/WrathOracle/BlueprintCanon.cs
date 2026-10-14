using System;
using System.Collections;
using System.Collections.Generic;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Reflection;
using System.Text;
using Kingmaker.Blueprints;
using Kingmaker.Blueprints.JsonSystem;
using Kingmaker.ResourceLinks;
using Kingmaker.Utility;
using UnityEngine;
using UnityEngine.UI;

namespace WrathOracle;

// Loaded blueprints in the canonical form of Wrath-Rust docs/poc2/content.md ("Canonical
// form"), emitted from the live objects (as BlueprintsCache.Load returned them, after
// OnEnable). The recursion is ReflectionBasedSerializer.WriteField's, over the game's own
// BlueprintFieldsTraverser.GetUnitySerializedFields, so the field set and order are the
// game's. Unity objects are written as {"asset_index": i}, their index in the pack's
// referenced-asset list (UnityObjectConverter.AssetList), which is how wr-content's .bbp
// reader writes them; null objects as null.
//
// Output: blueprints-<label>.jsonl.gz, one compact canonical document per line, plus a
// summary in run.json. Floats are written "R" (round-trip); compare by bits.
internal static class BlueprintCanon
{
    public static int Written, Failed, Missing;

    public static string Run(string outDir, string label, string listFile, bool includeLoaded)
    {
        Written = Failed = Missing = 0;
        var guids = new List<string>();
        if (!string.IsNullOrEmpty(listFile) && File.Exists(listFile))
            guids.AddRange(File.ReadAllLines(listFile).Select(l => l.Trim()).Where(l => l.Length == 32));
        var loaded = new List<string>();
        if (includeLoaded)
        {
            ResourcesLibrary.BlueprintsCache.ForEachLoaded((g, bp) => { if (bp != null) loaded.Add(g.ToString()); });
        }
        var all = new SortedSet<string>(StringComparer.Ordinal);
        foreach (var g in guids) all.Add(g);
        foreach (var g in loaded) all.Add(g);
        var path = Path.Combine(outDir, $"blueprints-{label}.jsonl.gz");
        using (var f = File.Create(path))
        using (var gz = new GZipStream(f, CompressionMode.Compress))
        using (var w = new StreamWriter(gz, new UTF8Encoding(false)))
        {
            foreach (var g in all)
            {
                SimpleBlueprint bp;
                try { bp = ResourcesLibrary.TryGetBlueprint(BlueprintGuid.Parse(g)); }
                catch (Exception e) { Failed++; Health.Failed("load blueprint " + g, e); continue; }
                if (bp == null) { Missing++; continue; }
                try
                {
                    var sw = new StringWriter(System.Globalization.CultureInfo.InvariantCulture);
                    using (var j = new Json(sw))
                    {
                        j.Obj();
                        j.Prop("guid", bp.AssetGuid.ToString());
                        j.Prop("name", bp.name ?? "");
                        Body(j, bp, bp.GetType(), 0);
                        j.End();
                    }
                    w.Write(sw.ToString());
                    w.Write('\n');
                    Written++;
                }
                catch (Exception e)
                {
                    Failed++;
                    Health.Failed("canon " + g, e);
                }
            }
        }
        File.WriteAllLines(Path.Combine(outDir, $"blueprints-{label}.loaded.txt"), loaded.OrderBy(x => x, StringComparer.Ordinal));
        return $"{Written} written, {Missing} missing, {Failed} failed ({guids.Count} listed, {loaded.Count} loaded) -> {path}";
    }

    private static readonly Dictionary<Type, string> TypeIds = new Dictionary<Type, string>();

    private static string TypeId(Type t)
    {
        if (!TypeIds.TryGetValue(t, out var id))
        {
            var a = t.GetCustomAttribute<TypeIdAttribute>(false);
            id = a != null ? a.Guid.ToString("N") : null;
            TypeIds[t] = id;
        }
        return id;
    }

    private static void Body(Json j, object o, Type t, int depth)
    {
        if (depth > 64) throw new InvalidOperationException("nesting too deep");
        if (GuidClassBinder.IsIdentifiedType(t)) j.Prop("$type", TypeId(t));
        foreach (var m in BlueprintFieldsTraverser.GetUnitySerializedFields(t))
        {
            var field = (FieldInfo)m;
            j.Key(field.Name);
            if (IsPrototypeLink(field)) { j.W.WriteNull(); continue; }
            Value(j, field.FieldType, field.GetValue(o), depth);
        }
    }

    private static bool IsPrototypeLink(FieldInfo f) =>
        f.GetCustomAttributes(false).Any(a => a.GetType().Name == "JsonConverterAttribute" &&
            (a.GetType().GetProperty("ConverterType")?.GetValue(a) as Type)?.Name.Contains("PrototypeLinkConverter") == true);

    private static void Obj(Json j, object o, Type declared, int depth)
    {
        var identified = GuidClassBinder.IsIdentifiedType(o?.GetType() ?? declared);
        if (o == null)
        {
            if (identified) { j.W.WriteNull(); return; }
            o = Activator.CreateInstance(declared); // what the pack writes for a null plain object
        }
        if (o.GetType().FullName == "Kingmaker.Blueprints.OverridesManager") { j.W.WriteNull(); return; }
        if (o is WeakResourceLink link) { j.W.WriteValue(link.AssetId ?? ""); return; }
        j.W.WriteStartObject();
        Body(j, o, o.GetType(), depth + 1);
        j.W.WriteEndObject();
    }

    private static void Asset(Json j, UnityEngine.Object u)
    {
        if (!u) { j.W.WriteNull(); return; }
        j.W.WriteStartObject();
        j.W.WritePropertyName("asset_index");
        j.W.WriteValue(Kingmaker.Blueprints.JsonSystem.Converters.UnityObjectConverter.AssetList.IndexOf(u));
        j.W.WriteEndObject();
    }

    private static void Floats(Json j, params float[] f)
    {
        j.W.WriteStartArray();
        foreach (var x in f) j.W.WriteValue(x);
        j.W.WriteEndArray();
    }

    private static void Value(Json j, Type ft, object v, int depth)
    {
        var w = j.W;
        if (ft == typeof(int)) w.WriteValue((int)v);
        else if (ft == typeof(uint)) w.WriteValue((uint)v);
        else if (ft == typeof(float)) w.WriteValue((float)v);
        else if (ft == typeof(double)) w.WriteValue((double)v);
        else if (ft == typeof(long)) w.WriteValue((long)v);
        else if (ft == typeof(ulong)) w.WriteValue((ulong)v);
        else if (ft == typeof(string)) w.WriteValue((string)v ?? "");
        else if (ft == typeof(bool)) w.WriteValue((bool)v);
        else if (ft.IsEnum) w.WriteValue(Convert.ToInt32(v));
        else if (ft == typeof(Color)) { var c = (Color)v; Floats(j, c.r, c.g, c.b, c.a); }
        else if (ft == typeof(Color32)) { var c = (Color32)v; w.WriteStartArray(); w.WriteValue(c.r); w.WriteValue(c.g); w.WriteValue(c.b); w.WriteValue(c.a); w.WriteEndArray(); }
        else if (ft == typeof(Vector2)) { var x = (Vector2)v; Floats(j, x.x, x.y); }
        else if (ft == typeof(Vector3)) { var x = (Vector3)v; Floats(j, x.x, x.y, x.z); }
        else if (ft == typeof(Vector4)) { var x = (Vector4)v; Floats(j, x.x, x.y, x.z, x.w); }
        else if (ft == typeof(Bounds)) { var b = (Bounds)v; Floats(j, b.center.x, b.center.y, b.center.z, b.size.x, b.size.y, b.size.z); }
        else if (ft == typeof(Vector2Int)) { var x = (Vector2Int)v; w.WriteStartArray(); w.WriteValue(x.x); w.WriteValue(x.y); w.WriteEndArray(); }
        else if (ft == typeof(Gradient)) Gradient(j, (Gradient)v);
        else if (ft == typeof(AnimationCurve)) Curve(j, (AnimationCurve)v);
        else if (ft == typeof(ColorBlock))
        {
            var cb = (ColorBlock)v;
            w.WriteStartObject();
            foreach (var f in typeof(ColorBlock).GetFields(BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public))
            {
                w.WritePropertyName(f.Name);
                var fv = f.GetValue(cb);
                if (fv is Color c) Floats(j, c.r, c.g, c.b, c.a); else if (fv is float fl) w.WriteValue(fl); else w.WriteValue(fv?.ToString());
            }
            w.WriteEndObject();
        }
        else if (ft == typeof(BlueprintGuid)) { var g = (BlueprintGuid)v; w.WriteValue(g == BlueprintGuid.Empty ? null : g.ToString()); }
        else if (typeof(UnityEngine.Object).IsAssignableFrom(ft)) Asset(j, (UnityEngine.Object)v);
        else if (ft.IsArray || IsList(ft))
        {
            var elem = ft.IsArray ? ft.GetElementType() : ft.GetGenericArguments()[0];
            w.WriteStartArray();
            if (v is IEnumerable seq)
            {
                foreach (var e in seq)
                {
                    if (elem.IsEnum) w.WriteValue(Convert.ToInt32(e));
                    else if (typeof(BlueprintReferenceBase).IsAssignableFrom(elem)) Ref(j, e as BlueprintReferenceBase);
                    else if (typeof(UnityEngine.Object).IsAssignableFrom(elem)) Asset(j, e as UnityEngine.Object);
                    else if (elem.IsPrimitive || elem == typeof(string) || elem.IsValueType) Value(j, elem, e, depth);
                    else Obj(j, e, elem, depth);
                }
            }
            w.WriteEndArray();
        }
        else if (typeof(BlueprintReferenceBase).IsAssignableFrom(ft)) Ref(j, v as BlueprintReferenceBase);
        else if (GuidClassBinder.IsIdentifiedType(ft) || ft.GetCustomAttribute<SerializableAttribute>() != null || ft.IsSerializable) Obj(j, v, ft, depth);
        else w.WriteValue("$unrecognized:" + ft.Name);
    }

    private static bool IsList(Type t) => t.IsGenericType && t.GetGenericTypeDefinition() == typeof(List<>);

    private static void Ref(Json j, BlueprintReferenceBase r)
    {
        if (r == null || r.Guid == BlueprintGuid.Empty) j.W.WriteNull();
        else j.W.WriteValue(r.Guid.ToString());
    }

    private static void Curve(Json j, AnimationCurve c)
    {
        if (c == null) { j.W.WriteNull(); return; }
        j.W.WriteStartObject();
        j.W.WritePropertyName("keys");
        j.W.WriteStartArray();
        foreach (var k in c.keys.OrderBy(k => k.time))
        {
            j.W.WriteStartArray();
            j.W.WriteValue(k.time); j.W.WriteValue(k.value); j.W.WriteValue(k.inTangent); j.W.WriteValue(k.outTangent);
            j.W.WriteValue(k.inWeight); j.W.WriteValue(k.outWeight); j.W.WriteValue((int)k.weightedMode);
            j.W.WriteEndArray();
        }
        j.W.WriteEndArray();
        j.W.WriteEndObject();
    }

    private static void Gradient(Json j, Gradient g)
    {
        if (g == null) { j.W.WriteNull(); return; }
        j.W.WriteStartObject();
        j.W.WritePropertyName("colors");
        j.W.WriteStartArray();
        foreach (var k in g.colorKeys.OrderBy(k => k.time)) Floats(j, k.time, k.color.r, k.color.g, k.color.b);
        j.W.WriteEndArray();
        j.W.WritePropertyName("alphas");
        j.W.WriteStartArray();
        foreach (var k in g.alphaKeys.OrderBy(k => k.time)) Floats(j, k.time, k.alpha);
        j.W.WriteEndArray();
        j.W.WritePropertyName("mode");
        j.W.WriteValue((int)g.mode);
        j.W.WriteEndObject();
    }
}

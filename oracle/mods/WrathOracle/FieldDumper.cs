using System;
using System.Collections;
using System.Collections.Generic;
using System.Reflection;
using Kingmaker.Blueprints;
using UnityEngine;

namespace WrathOracle;

// Writes a component's Unity-serialized fields (what a scene bundle's type tree holds), by
// reflection, under the same field names, so a Unity-side extractor can be compared
// field by field. Rules follow Unity's serializer: public or [SerializeField] instance
// fields, not static/const/readonly/[NonSerialized]; [Serializable] nested types inline;
// UnityEngine.Object references as {"$ref": type, "path"/"name"}; blueprint references as
// {"$bp": guid}. Depth and list lengths are capped (marked with "$truncated").
internal static class FieldDumper
{
    private const int MaxDepth = 8;
    private const int MaxItems = 512;
    private static readonly Dictionary<Type, FieldInfo[]> Cache = new Dictionary<Type, FieldInfo[]>();

    public static void WriteFields(Json j, object obj)
    {
        j.W.WriteStartObject();
        WriteFieldsInner(j, obj, 0);
        j.W.WriteEndObject();
    }

    private static void WriteFieldsInner(Json j, object obj, int depth, Type layout = null)
    {
        foreach (var f in Fields(layout ?? obj.GetType()))
        {
            object v;
            try
            {
                v = f.GetValue(obj);
            }
            catch (Exception e)
            {
                j.W.WritePropertyName(f.Name);
                j.W.WriteValue("$error: " + e.Message);
                continue;
            }
            j.W.WritePropertyName(f.Name);
            WriteValue(j, v, f.FieldType, depth + 1, f.IsDefined(typeof(SerializeReference), true));
        }
    }

    private static FieldInfo[] Fields(Type t)
    {
        if (Cache.TryGetValue(t, out var cached)) return cached;
        var list = new List<FieldInfo>();
        var chain = new List<Type>();
        for (var c = t; c != null && c != typeof(object) && c != typeof(MonoBehaviour) && c != typeof(Behaviour)
                        && c != typeof(Component) && c != typeof(UnityEngine.Object) && c != typeof(ScriptableObject); c = c.BaseType)
            chain.Add(c);
        chain.Reverse(); // base fields first, as Unity orders them
        foreach (var c in chain)
        {
            foreach (var f in c.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly))
            {
                if (f.IsInitOnly || f.IsLiteral || f.IsNotSerialized) continue;
                var serializeField = f.IsDefined(typeof(SerializeField), true) || f.IsDefined(typeof(SerializeReference), true);
                if (!f.IsPublic && !serializeField) continue;
                if (f.Name.Contains("<")) continue; // compiler-generated backing fields
                if (!IsSerializableType(f.FieldType, f.IsDefined(typeof(SerializeReference), true))) continue;
                list.Add(f);
            }
        }
        var arr = list.ToArray();
        Cache[t] = arr;
        return arr;
    }

    private static bool IsSerializableType(Type t, bool serializeReference)
    {
        if (t.IsPrimitive || t.IsEnum || t == typeof(string)) return true;
        if (typeof(UnityEngine.Object).IsAssignableFrom(t)) return true;
        // One level of array/List<T> only: Unity doesn't serialize nested containers.
        if (t.IsArray) return t.GetArrayRank() == 1 && !IsContainer(t.GetElementType()) && IsSerializableType(t.GetElementType(), serializeReference);
        if (IsList(t)) return !IsContainer(t.GetGenericArguments()[0]) && IsSerializableType(t.GetGenericArguments()[0], serializeReference);
        if (t.Namespace == "UnityEngine") return t.IsValueType || t == typeof(AnimationCurve) || t == typeof(Gradient);
        if (serializeReference) return true;
        if (t.IsGenericType && !t.IsGenericTypeDefinition)
            // Unity 2020 serializes concrete generic [Serializable] user types, but not
            // framework collections such as Dictionary<,> or HashSet<>.
            return !(t.Namespace ?? "").StartsWith("System") && t.IsDefined(typeof(SerializableAttribute), false);
        if (t.IsAbstract || t.IsInterface) return false;
        if (typeof(Delegate).IsAssignableFrom(t)) return false;
        return t.IsDefined(typeof(SerializableAttribute), false);
    }

    private static bool IsList(Type t) => t.IsGenericType && t.GetGenericTypeDefinition() == typeof(List<>);
    private static bool IsContainer(Type t) => t.IsArray || IsList(t);

    private static void WriteValue(Json j, object v, Type declared, int depth, bool serializeReference = false)
    {
        var w = j.W;
        if (v == null) { w.WriteNull(); return; }
        if (v is UnityEngine.Object uo) { WriteRef(j, uo); return; }
        switch (v)
        {
            case string s: w.WriteValue(s); return;
            case bool b: w.WriteValue(b); return;
            case float f: w.WriteValue(f); return;
            case double d: w.WriteValue(d); return;
            case int i: w.WriteValue(i); return;
            case uint ui: w.WriteValue(ui); return;
            case long l: w.WriteValue(l); return;
            case ulong ul: w.WriteValue(ul); return;
            case short sh: w.WriteValue(sh); return;
            case ushort ush: w.WriteValue(ush); return;
            case byte by: w.WriteValue(by); return;
            case sbyte sb: w.WriteValue(sb); return;
            case char ch: w.WriteValue(ch.ToString()); return;
            case Vector3 v3: j.Vec(v3); return;
            case Vector2 v2: w.WriteStartArray(); w.WriteValue(v2.x); w.WriteValue(v2.y); w.WriteEndArray(); return;
            case Vector4 v4: w.WriteStartArray(); w.WriteValue(v4.x); w.WriteValue(v4.y); w.WriteValue(v4.z); w.WriteValue(v4.w); w.WriteEndArray(); return;
            case Vector2Int v2i: w.WriteStartArray(); w.WriteValue(v2i.x); w.WriteValue(v2i.y); w.WriteEndArray(); return;
            case Vector3Int v3i: w.WriteStartArray(); w.WriteValue(v3i.x); w.WriteValue(v3i.y); w.WriteValue(v3i.z); w.WriteEndArray(); return;
            case Quaternion q: j.Quat(q); return;
            case Color c: w.WriteStartArray(); w.WriteValue(c.r); w.WriteValue(c.g); w.WriteValue(c.b); w.WriteValue(c.a); w.WriteEndArray(); return;
            case Color32 c32: w.WriteStartArray(); w.WriteValue(c32.r); w.WriteValue(c32.g); w.WriteValue(c32.b); w.WriteValue(c32.a); w.WriteEndArray(); return;
            case Rect r: w.WriteStartArray(); w.WriteValue(r.x); w.WriteValue(r.y); w.WriteValue(r.width); w.WriteValue(r.height); w.WriteEndArray(); return;
            case Bounds bo: w.WriteStartObject(); j.Prop("center", bo.center); j.Prop("size", bo.size); w.WriteEndObject(); return;
            case LayerMask lm: w.WriteValue(lm.value); return;
            case Matrix4x4 m: j.Mat(m); return;
            case AnimationCurve ac:
                w.WriteStartObject();
                j.Prop("pre_wrap", ac.preWrapMode.ToString());
                j.Prop("post_wrap", ac.postWrapMode.ToString());
                w.WritePropertyName("keys"); // [time, value, inTangent, outTangent, inWeight, outWeight, weightedMode]
                w.WriteStartArray();
                foreach (var k in ac.keys)
                {
                    w.WriteStartArray();
                    w.WriteValue(k.time); w.WriteValue(k.value); w.WriteValue(k.inTangent); w.WriteValue(k.outTangent);
                    w.WriteValue(k.inWeight); w.WriteValue(k.outWeight); w.WriteValue(k.weightedMode.ToString());
                    w.WriteEndArray();
                }
                w.WriteEndArray();
                w.WriteEndObject();
                return;
            case Gradient g:
                w.WriteStartObject();
                j.Prop("mode", g.mode.ToString());
                w.WritePropertyName("colors"); // [time, r, g, b]
                w.WriteStartArray();
                foreach (var c in g.colorKeys) { w.WriteStartArray(); w.WriteValue(c.time); w.WriteValue(c.color.r); w.WriteValue(c.color.g); w.WriteValue(c.color.b); w.WriteEndArray(); }
                w.WriteEndArray();
                w.WritePropertyName("alphas"); // [time, alpha]
                w.WriteStartArray();
                foreach (var a in g.alphaKeys) { w.WriteStartArray(); w.WriteValue(a.time); w.WriteValue(a.alpha); w.WriteEndArray(); }
                w.WriteEndArray();
                w.WriteEndObject();
                return;
            case BlueprintReferenceBase br:
                w.WriteStartObject();
                j.Prop("$bp", br.Guid.ToString());
                w.WriteEndObject();
                return;
        }
        var t = v.GetType();
        if (t.IsEnum) { w.WriteValue(v.ToString()); return; }
        if (depth > MaxDepth) { w.WriteValue("$truncated: depth"); return; }
        if (v is IList list)
        {
            // Element layout from the declared field type (an ordinary Base[] field is
            // serialized as Base even if it holds a Derived[]).
            var layoutType = declared != null && IsContainer(declared) ? declared : t;
            var elemType = layoutType.IsArray ? layoutType.GetElementType() : layoutType.GetGenericArguments().Length > 0 ? layoutType.GetGenericArguments()[0] : typeof(object);
            if (elemType == typeof(byte) && list is byte[] bytes)
            {
                w.WriteStartObject();
                j.Prop("$bytes", bytes.Length);
                j.Prop("base64", Convert.ToBase64String(bytes, 0, Math.Min(bytes.Length, 4096)));
                w.WriteEndObject();
                return;
            }
            w.WriteStartArray();
            var n = Math.Min(list.Count, MaxItems);
            for (var i = 0; i < n; i++) WriteValue(j, list[i], elemType, depth + 1, serializeReference);
            if (list.Count > MaxItems) w.WriteValue($"$truncated: {list.Count} items");
            w.WriteEndArray();
            return;
        }
        // Inline [Serializable] values are laid out by their declared type (Unity ignores a
        // derived runtime type); [SerializeReference] values by their runtime type.
        w.WriteStartObject();
        if (serializeReference) j.Prop("$type", t.FullName);
        WriteFieldsInner(j, v, depth, serializeReference || declared == null || declared == typeof(object) ? t : declared);
        w.WriteEndObject();
    }

    public static void WriteRef(Json j, UnityEngine.Object o)
    {
        var w = j.W;
        if (o == null) { w.WriteNull(); return; } // Unity's "fake null" too
        w.WriteStartObject();
        j.Prop("$ref", o.GetType().Name);
        GameObject go = o as GameObject ?? (o as Component)?.gameObject;
        if (go != null && go.scene.IsValid())
        {
            j.Prop("scene", go.scene.name);
            j.Prop("path", Paths.Of(go.transform));
        }
        else
        {
            j.Prop("name", o.name);
        }
        w.WriteEndObject();
    }
}

internal static class Paths
{
    // "Root/Child/Name" through the transform hierarchy.
    public static string Of(Transform t)
    {
        var parts = new List<string>();
        for (var c = t; c != null; c = c.parent) parts.Add(c.name);
        parts.Reverse();
        return string.Join("/", parts);
    }

    // Sibling indices below the root (root excluded): with the root's name, a join key
    // that survives repeated names (the extraction tool's GameObjectRecord.sibling_path).
    public static void WriteSiblingPath(Json j, Transform t)
    {
        var idx = new List<int>();
        for (var c = t; c.parent != null; c = c.parent) idx.Add(c.GetSiblingIndex());
        idx.Reverse();
        j.W.WriteStartArray();
        foreach (var i in idx) j.W.WriteValue(i);
        j.W.WriteEndArray();
    }
}

using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using System.Runtime.CompilerServices;
using System.Threading;
using Kingmaker.Blueprints;
using Kingmaker.EntitySystem;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.EntitySystem.Persistence.JsonUtility;
using Kingmaker.EntitySystem.Stats;
using Kingmaker.Items;
using Kingmaker.RuleSystem;
using Kingmaker.UnitLogic.Mechanics;
using Newtonsoft.Json.Serialization;
using UnityEngine;

namespace WrathOracle;

// Value rendering shared by traces and state dumps (docs/oracle/state-dump-format.md,
// "Value conventions"). Two ways to descend into an ordinary object:
//   Persisted: the members the game's save serializer writes (its OptInContractResolver);
//   Fields:    every instance field plus pure getters (rule snapshots).
// Reading never calls anything that isn't a field read, a save-path getter or a getter that
// Purity proved free of side effects.
internal static class Snap
{
    public enum Mode { Persisted, Fields }

    public static long Truncated, Cycles, ValueFailures;
    public const int MaxDepthPersisted = 8;
    public const int MaxDepthFields = 6;
    public const int MaxElementsFields = 256;

    private static readonly HashSet<object> Stack = new HashSet<object>(RefEq.Instance);

    private static IContractResolver _resolver;
    public static IContractResolver Resolver
    {
        get
        {
            if (_resolver == null)
            {
                try { _resolver = DefaultJsonSettings.DefaultSettings?.ContractResolver; } catch (Exception) { }
                _resolver = _resolver ?? new OptInContractResolver();
            }
            return _resolver;
        }
    }

    public static string Guid(SimpleBlueprint bp) => bp == null ? null : bp.AssetGuid.ToString();

    public static string Hex(BlueprintGuid g) => g.ToString();

    public static void UnitRef(Json j, UnitEntityData u)
    {
        if (u == null) { j.W.WriteNull(); return; }
        j.W.WriteStartObject();
        j.W.WritePropertyName("$unit");
        j.W.WriteValue(u.UniqueId);
        j.W.WriteEndObject();
    }

    private static readonly FieldInfo CommandsOwner = HarmonyLib.AccessTools.Field(typeof(Kingmaker.UnitLogic.Commands.UnitCommands), "m_Owner");

    // An entity part, cut from rule snapshots: its type, the entity that owns it, and for a
    // spellbook its blueprint, so two parts of one unit (or the same part of two units) differ.
    // Only auto-property and field reads.
    public static void PartRef(Json j, object v)
    {
        string owner = null, blueprint = null;
        try
        {
            switch (v)
            {
                case Kingmaker.UnitLogic.Spellbook sb: owner = sb.Owner?.Unit?.UniqueId; blueprint = Guid(sb.Blueprint); break;
                case Kingmaker.EntitySystem.EntityPart ep: owner = ep.Owner?.UniqueId; break;
                case Kingmaker.Items.UnitBody ub: owner = ub.Owner?.Unit?.UniqueId; break;
                case CharacterStats cs: owner = cs.Owner?.Unit?.UniqueId; break;
                case Kingmaker.UnitLogic.UnitState us: owner = us.Owner?.Unit?.UniqueId; break;
                case Kingmaker.UnitLogic.UnitProgressionData pd: owner = pd.Owner?.Unit?.UniqueId; break;
                case EntityFactsManager fm: owner = fm.Owner?.UniqueId; break;
                case Kingmaker.UnitLogic.Commands.UnitCommands uc: owner = (CommandsOwner?.GetValue(uc) as UnitEntityData)?.UniqueId; break;
            }
        }
        catch (Exception e)
        {
            Interlocked.Increment(ref ValueFailures);
            Health.Log("part ref " + v.GetType().Name, e);
        }
        j.W.WriteStartObject();
        j.W.WritePropertyName("$part"); j.W.WriteValue(v.GetType().Name);
        j.W.WritePropertyName("owner"); j.W.WriteValue(owner);
        if (blueprint != null) { j.W.WritePropertyName("blueprint"); j.W.WriteValue(blueprint); }
        j.W.WriteEndObject();
    }

    public static void FactRef(Json j, EntityFact f)
    {
        if (f == null) { j.W.WriteNull(); return; }
        j.W.WriteStartObject();
        j.W.WritePropertyName("$fact");
        j.W.WriteValue(f.UniqueId);
        j.W.WritePropertyName("blueprint");
        j.W.WriteValue(SafeGuid(() => f.Blueprint));
        j.W.WriteEndObject();
    }

    public static string SafeGuid(Func<SimpleBlueprint> f)
    {
        try { return Guid(f()); } catch (Exception) { return null; }
    }

    // Writes one value. `depth` counts nested objects already open.
    public static void Value(Json j, object v, Mode mode, int depth = 0)
    {
        var w = j.W;
        {
            switch (v)
            {
                case null: w.WriteNull(); return;
                case string s: w.WriteValue(s); return;
                case bool b: w.WriteValue(b); return;
                case float f: w.WriteValue(f); return;
                case double d: w.WriteValue(d); return;
                case int i: w.WriteValue(i); return;
                case long l: w.WriteValue(l); return;
                case uint ui: w.WriteValue(ui); return;
                case ulong ul: w.WriteValue(ul); return;
                case short sh: w.WriteValue(sh); return;
                case ushort us: w.WriteValue(us); return;
                case byte by: w.WriteValue(by); return;
                case sbyte sb: w.WriteValue(sb); return;
                case char c: w.WriteValue((int)c); return;
                case decimal m: w.WriteValue(m); return;
                case Enum e: w.WriteValue(Convert.ToInt64(e, CultureInfo.InvariantCulture)); return;
                case TimeSpan ts: w.WriteValue(ts.Ticks); return;
                case DateTime dt: w.WriteValue(dt.Ticks); return;
                case Vector2 v2: w.WriteStartArray(); w.WriteValue(v2.x); w.WriteValue(v2.y); w.WriteEndArray(); return;
                case Vector3 v3: j.Vec(v3); return;
                case Vector4 v4: w.WriteStartArray(); w.WriteValue(v4.x); w.WriteValue(v4.y); w.WriteValue(v4.z); w.WriteValue(v4.w); w.WriteEndArray(); return;
                case Quaternion q: j.Quat(q); return;
                case Color col: w.WriteStartArray(); w.WriteValue(col.r); w.WriteValue(col.g); w.WriteValue(col.b); w.WriteValue(col.a); w.WriteEndArray(); return;
                case BlueprintGuid bg: w.WriteValue(bg == BlueprintGuid.Empty ? null : bg.ToString()); return;
                case System.Guid g: w.WriteValue(g.ToString("N")); return;
                case SimpleBlueprint bp: w.WriteValue(Guid(bp)); return;
                case BlueprintReferenceBase br: w.WriteValue(br.IsEmpty() ? null : br.Guid.ToString()); return;
                case UnitEntityData u: UnitRef(j, u); return;
                case EntityFact fact: FactRef(j, fact); return;
                case ItemEntity item:
                    w.WriteStartObject();
                    w.WritePropertyName("$item"); w.WriteValue(item.UniqueId);
                    w.WritePropertyName("blueprint"); w.WriteValue(SafeGuid(() => item.Blueprint));
                    w.WriteEndObject();
                    return;
                case EntityDataBase ent:
                    w.WriteStartObject();
                    w.WritePropertyName("$entity"); w.WriteValue(ent.UniqueId);
                    w.WritePropertyName("class"); w.WriteValue(ent.GetType().Name);
                    w.WriteEndObject();
                    return;
                case UnitReference ur:
                    if (string.IsNullOrEmpty(ur.UniqueId)) { w.WriteNull(); return; }
                    w.WriteStartObject(); w.WritePropertyName("$unit"); w.WriteValue(ur.UniqueId); w.WriteEndObject();
                    return;
                case ITypedEntityRef er:
                    {
                        var rt = er.GetType();
                        var id = rt.GetField("Id")?.GetValue(er) as string;
                        if (string.IsNullOrEmpty(id)) { w.WriteNull(); return; }
                        var target = rt.IsGenericType ? rt.GetGenericArguments()[0] : null;
                        w.WriteStartObject();
                        if (target != null && typeof(UnitEntityData).IsAssignableFrom(target))
                        {
                            w.WritePropertyName("$unit"); w.WriteValue(id);
                        }
                        else
                        {
                            w.WritePropertyName("$entity"); w.WriteValue(id);
                            w.WritePropertyName("class"); w.WriteValue(target?.Name ?? "EntityDataBase");
                        }
                        w.WriteEndObject();
                        return;
                    }
                case ModifiableValue mv: Stat(j, mv, mode, depth); return;
                case ModifiableValue.Modifier mod: Modifier(j, mod); return;
                case RulebookEvent rule:
                    w.WriteStartObject();
                    w.WritePropertyName("$rule"); w.WriteValue(Rules.IdOf(rule));
                    w.WritePropertyName("class"); w.WriteValue(rule.GetType().Name);
                    if (rule is Kingmaker.RuleSystem.Rules.RuleRollDice rd) { w.WritePropertyName("result"); w.WriteValue(rd.Result); }
                    w.WriteEndObject();
                    return;
                case UnityEngine.Object uo:
                    w.WriteStartObject();
                    w.WritePropertyName("$unity"); w.WriteValue(uo.GetType().Name);
                    if (uo == null) { w.WritePropertyName("destroyed"); w.WriteValue(true); }
                    else { w.WritePropertyName("name"); w.WriteValue(uo.name); }
                    w.WriteEndObject();
                    return;
                case Delegate _:
                case Type _:
                case MemberInfo _:
                case Thread _:
                    w.WriteStartObject(); w.WritePropertyName("$type"); w.WriteValue(v.GetType().Name); w.WriteEndObject();
                    return;
            }

            if (mode == Mode.Fields)
            {
                switch (v)
                {
                    case MechanicsContext mc:
                        w.WriteStartObject();
                        w.WritePropertyName("$context"); w.WriteValue(SafeGuid(() => mc.AssociatedBlueprint));
                        w.WritePropertyName("caster"); UnitRef(j, mc.MaybeCaster);
                        w.WriteEndObject();
                        return;
                    case Kingmaker.UnitLogic.Abilities.AbilityData ad:
                        w.WriteStartObject();
                        w.WritePropertyName("$ability"); w.WriteValue(SafeGuid(() => ad.Blueprint));
                        w.WritePropertyName("caster"); UnitRef(j, ad.Caster?.Unit);
                        w.WriteEndObject();
                        return;
                    case Kingmaker.UnitLogic.UnitDescriptor ud:
                        UnitRef(j, ud.Unit);
                        return;
                    case Kingmaker.EntitySystem.EntityPart _:
                    case Kingmaker.Items.UnitBody _:
                    case CharacterStats _:
                    case Kingmaker.UnitLogic.UnitState _:
                    case Kingmaker.UnitLogic.UnitProgressionData _:
                    case EntityFactsManager _:
                    case Kingmaker.UnitLogic.Commands.UnitCommands _:
                    case Kingmaker.UnitLogic.Spellbook _:
                        // Parts of an entity: a reference, never the whole unit's state.
                        PartRef(j, v);
                        return;
                    case RuleReason rr:
                        w.WriteStartObject();
                        w.WritePropertyName("$reason"); w.WriteValue(rr.Rule != null ? Rules.IdOf(rr.Rule) : 0);
                        w.WritePropertyName("fact"); FactRef(j, rr.Fact);
                        w.WriteEndObject();
                        return;
                }
            }
            var type = v.GetType();
            if (v is IDictionary dict)
            {
                w.WriteStartArray();
                var n = 0;
                foreach (DictionaryEntry e in dict)
                {
                    if (mode == Mode.Fields && n++ >= MaxElementsFields) { w.WriteValue("$more"); break; }
                    w.WriteStartArray();
                    Value(j, e.Key, mode, depth + 1);
                    Value(j, e.Value, mode, depth + 1);
                    w.WriteEndArray();
                }
                w.WriteEndArray();
                return;
            }
            if (v is IEnumerable seq && !(v is MechanicsContext))
            {
                w.WriteStartArray();
                var n = 0;
                foreach (var e in seq)
                {
                    if (mode == Mode.Fields && n++ >= MaxElementsFields) { w.WriteValue("$more"); break; }
                    Value(j, e, mode, depth + 1);
                }
                w.WriteEndArray();
                return;
            }
            if (type.IsPrimitive) { w.WriteValue(v.ToString()); return; }

            var max = mode == Mode.Persisted ? MaxDepthPersisted : MaxDepthFields;
            if (depth >= max)
            {
                Interlocked.Increment(ref Truncated);
                w.WriteStartObject(); w.WritePropertyName("$depth"); w.WriteValue(type.Name); w.WriteEndObject();
                return;
            }
            var isRef = !type.IsValueType;
            if (isRef && !Stack.Add(v))
            {
                Interlocked.Increment(ref Cycles);
                w.WriteStartObject(); w.WritePropertyName("$cycle"); w.WriteValue(type.Name); w.WriteEndObject();
                return;
            }
            try
            {
                if (mode == Mode.Persisted) Persisted(j, v, null, depth + 1);
                else Fields(j, v, depth + 1, null);
            }
            finally
            {
                if (isRef) Stack.Remove(v);
            }
        }
    }

    // Renders one value to JSON text; a failure becomes {"$failed": ...} (counted) instead of
    // aborting the surrounding record.
    public static string ValueText(object v, Mode mode, int depth)
    {
        try
        {
            var sw = new StringWriter(CultureInfo.InvariantCulture);
            using (var j = new Json(sw)) Value(j, v, mode, depth);
            return sw.ToString();
        }
        catch (Exception e)
        {
            Interlocked.Increment(ref ValueFailures);
            Health.Log("snap value", e);
            var inner = e is TargetInvocationException tie && tie.InnerException != null ? tie.InnerException : e;
            return "{\"$failed\":" + Newtonsoft.Json.JsonConvert.ToString(inner.GetType().Name + ": " + inner.Message) + "}";
        }
    }


    // A ModifiableValue: a unit's stat as a reference (Fields mode adds its value); a free
    // standing one (e.g. RuleSkillCheck.Bonus) in full.
    public static void Stat(Json j, ModifiableValue mv, Mode mode, int depth)
    {
        var w = j.W;
        var owner = mv.Owner?.Unit;
        w.WriteStartObject();
        if (owner != null)
        {
            w.WritePropertyName("$stat"); w.WriteValue((int)mv.Type);
            w.WritePropertyName("owner"); w.WriteValue(owner.UniqueId);
            if (mode == Mode.Fields) { w.WritePropertyName("modified"); w.WriteValue(mv.ModifiedValue); }
        }
        else
        {
            w.WritePropertyName("type"); w.WriteValue((int)mv.Type);
            w.WritePropertyName("base"); w.WriteValue(mv.BaseValue);
            w.WritePropertyName("modified"); w.WriteValue(mv.ModifiedValue);
            w.WritePropertyName("modifiers");
            Modifiers(j, mv);
        }
        w.WriteEndObject();
    }

    private static readonly FieldInfo ModifierListField = typeof(ModifiableValue).GetField("ModifierList", BindingFlags.Instance | BindingFlags.NonPublic);

    // Modifiers grouped by descriptor ascending; within a descriptor in list order.
    public static void Modifiers(Json j, ModifiableValue mv)
    {
        var w = j.W;
        w.WriteStartArray();
        if (ModifierListField?.GetValue(mv) is Dictionary<Kingmaker.Enums.ModifierDescriptor, List<ModifiableValue.Modifier>> lists)
        {
            foreach (var kv in lists.OrderBy(k => (int)k.Key))
                foreach (var m in kv.Value)
                    Modifier(j, m);
        }
        w.WriteEndArray();
    }

    public static void Modifier(Json j, ModifiableValue.Modifier m)
    {
        var w = j.W;
        if (m == null) { w.WriteNull(); return; }
        w.WriteStartObject();
        w.WritePropertyName("descriptor"); w.WriteValue((int)m.ModDescriptor);
        w.WritePropertyName("descriptor_name"); w.WriteValue(m.ModDescriptor.ToString());
        w.WritePropertyName("value"); w.WriteValue(m.ModValue);
        w.WritePropertyName("stack_mode"); w.WriteValue((int)m.StackMode);
        w.WritePropertyName("stacks"); w.WriteValue(m.Stacks);
        w.WritePropertyName("source"); FactRef(j, m.Source);
        w.WritePropertyName("source_component"); w.WriteValue(m.SourceComponent);
        w.WritePropertyName("item");
        if (m.ItemSource == null) w.WriteNull();
        else
        {
            w.WriteStartObject();
            w.WritePropertyName("$item"); w.WriteValue(m.ItemSource.UniqueId);
            w.WritePropertyName("blueprint"); w.WriteValue(SafeGuid(() => m.ItemSource.Blueprint));
            w.WriteEndObject();
        }
        w.WriteEndObject();
    }

    // ---- Persisted members (the save's contract) ----

    public static void Persisted(Json j, object o, ICollection<string> skip, int depth)
    {
        var w = j.W;
        w.WriteStartObject();
        if (Resolver.ResolveContract(o.GetType()) is JsonObjectContract c)
        {
            foreach (var p in c.Properties)
            {
                if (p.Ignored || !p.Readable || p.ValueProvider == null) continue;
                var name = p.PropertyName;
                if (skip != null && skip.Contains(name)) continue;
                object val;
                try
                {
                    val = p.ValueProvider.GetValue(o);
                }
                catch (Exception e)
                {
                    Interlocked.Increment(ref ValueFailures);
                    w.WritePropertyName(name);
                    w.WriteStartObject(); w.WritePropertyName("$failed"); w.WriteValue(e.GetType().Name + ": " + (e.InnerException ?? e).Message); w.WriteEndObject();
                    continue;
                }
                w.WritePropertyName(name);
                w.WriteRawValue(ValueText(val, Mode.Persisted, depth));
            }
        }
        else
        {
            w.WritePropertyName("$type"); w.WriteValue(o.GetType().Name);
        }
        w.WriteEndObject();
    }

    // ---- Fields mode (rules) ----

    private sealed class TypeShape
    {
        public FieldInfo[] Fields;
        public string[] FieldNames;
        public PropertyInfo[] Getters;
    }

    private static readonly Dictionary<Type, TypeShape> Shapes = new Dictionary<Type, TypeShape>();

    private static TypeShape ShapeOf(Type t)
    {
        if (Shapes.TryGetValue(t, out var s)) return s;
        var fields = new List<FieldInfo>();
        var names = new List<string>();
        var chain = new List<Type>();
        for (var x = t; x != null && x != typeof(object); x = x.BaseType) chain.Add(x);
        chain.Reverse();
        var seen = new HashSet<string>();
        foreach (var x in chain)
        {
            foreach (var f in x.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly))
            {
                if (typeof(Delegate).IsAssignableFrom(f.FieldType)) continue;
                var name = CleanName(f.Name);
                if (!seen.Add(name)) name = x.Name + "." + name;
                fields.Add(f);
                names.Add(name);
            }
        }
        var getters = new List<PropertyInfo>();
        var fieldNames = new HashSet<string>(names);
        foreach (var x in chain)
        {
            if (x.Assembly != typeof(RulebookEvent).Assembly) continue;
            foreach (var p in x.GetProperties(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly))
            {
                if (p.GetIndexParameters().Length != 0 || fieldNames.Contains(p.Name)) continue;
                var g = p.GetGetMethod(true);
                if (g == null) continue;
                // Use the most derived declaration for the runtime type (GetProperty by name
                // throws on properties hidden with "new", so walk the chain instead).
                PropertyInfo derived = null;
                for (var x2 = t; x2 != null && derived == null; x2 = x2.BaseType)
                    derived = x2.GetProperties(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly)
                        .FirstOrDefault(q => q.Name == p.Name && q.GetIndexParameters().Length == 0);
                derived = derived ?? p;
                var actual = derived.GetGetMethod(true) ?? g;
                if (!Purity.IsPure(actual)) continue;
                if (getters.Any(q => q.Name == p.Name)) continue;
                getters.Add(derived);
            }
        }
        s = new TypeShape { Fields = fields.ToArray(), FieldNames = names.ToArray(), Getters = getters.ToArray() };
        Shapes[t] = s;
        return s;
    }

    // Backing fields of auto-properties appear under the property name.
    public static string CleanName(string n)
    {
        if (n.Length > 2 && n[0] == '<')
        {
            var end = n.IndexOf('>');
            if (end > 1) return n.Substring(1, end - 1);
        }
        return n;
    }

    public static void Fields(Json j, object o, int depth, ICollection<string> skip)
    {
        var w = j.W;
        w.WriteStartObject();
        foreach (var kv in FieldPairs(o, depth, skip))
        {
            w.WritePropertyName(kv.Key);
            w.WriteRawValue(kv.Value);
        }
        w.WriteEndObject();
    }

    // name -> JSON text, fields then pure getters, for diffing.
    public static List<KeyValuePair<string, string>> FieldPairs(object o, int depth, ICollection<string> skip)
    {
        var shape = ShapeOf(o.GetType());
        var list = new List<KeyValuePair<string, string>>(shape.Fields.Length + shape.Getters.Length);
        for (var i = 0; i < shape.Fields.Length; i++)
        {
            var name = shape.FieldNames[i];
            if (skip != null && skip.Contains(name)) continue;
            list.Add(new KeyValuePair<string, string>(name, Render(() => shape.Fields[i].GetValue(o), depth)));
        }
        foreach (var p in shape.Getters)
        {
            if (skip != null && skip.Contains(p.Name)) continue;
            list.Add(new KeyValuePair<string, string>(p.Name, Render(() => p.GetValue(o, null), depth)));
        }
        return list;
    }

    private static string Render(Func<object> read, int depth)
    {
        object v;
        try
        {
            v = read();
        }
        catch (Exception e)
        {
            Interlocked.Increment(ref ValueFailures);
            var inner = e is TargetInvocationException tie && tie.InnerException != null ? tie.InnerException : e;
            return "{\"$failed\":" + Newtonsoft.Json.JsonConvert.ToString(inner.GetType().Name + ": " + inner.Message) + "}";
        }
        return ValueText(v, Mode.Fields, depth);
    }

    internal sealed class RefEq : IEqualityComparer<object>
    {
        public static readonly RefEq Instance = new RefEq();
        public new bool Equals(object a, object b) => ReferenceEquals(a, b);
        public int GetHashCode(object o) => RuntimeHelpers.GetHashCode(o);
    }
}

// A conservative heuristic (not a proof) for whether a getter is free of side effects: it may read fields, do arithmetic,
// branch, box, and call methods that are themselves pure (non-virtual or sealed, with a
// body), plus System.Math/Mathf/Nullable/String readers. Anything that stores to a field or
// array, allocates a reference object, calls through an interface or an overridable virtual,
// or calls native code is impure. Conservative: unknown means impure.
internal static class Purity
{
    private static readonly OpCode[] One = new OpCode[0x100];
    private static readonly OpCode[] Two = new OpCode[0x100];
    private static readonly Dictionary<MethodBase, bool> Memo = new Dictionary<MethodBase, bool>();
    private static readonly HashSet<MethodBase> InProgress = new HashSet<MethodBase>();

    static Purity()
    {
        foreach (var f in typeof(OpCodes).GetFields(BindingFlags.Public | BindingFlags.Static))
        {
            var op = (OpCode)f.GetValue(null);
            var v = (ushort)op.Value;
            if (v < 0x100) One[v] = op;
            else if ((v & 0xff00) == 0xfe00) Two[v & 0xff] = op;
        }
    }

    private static readonly HashSet<string> AllowedTypes = new HashSet<string>
    {
        "System.Math", "UnityEngine.Mathf", "System.String", "System.Nullable`1", "System.TimeSpan", "System.Int32", "System.Single",
        "System.Boolean", "System.Double", "System.Int64", "System.Enum",
    };

    private static readonly HashSet<string> AllowedCtorTypes = new HashSet<string>
    {
        "UnityEngine.Vector2", "UnityEngine.Vector3", "UnityEngine.Vector4", "UnityEngine.Quaternion", "UnityEngine.Color",
        "System.Nullable`1", "System.TimeSpan",
    };

    public static bool IsPure(MethodBase m, int depth = 0)
    {
        if (m == null) return false;
        if (Memo.TryGetValue(m, out var r)) return r;
        if (InProgress.Contains(m)) return true; // recursion: decided by the outer frame
        if (depth > 6) return false;
        InProgress.Add(m);
        try
        {
            r = Analyze(m, depth);
        }
        catch (Exception)
        {
            r = false;
        }
        finally
        {
            InProgress.Remove(m);
        }
        Memo[m] = r;
        return r;
    }

    private static bool Analyze(MethodBase m, int depth)
    {
        if (m.GetParameters().Any(p => p.ParameterType.IsByRef)) return false;
        if (m.DeclaringType != null && AllowedTypes.Contains(m.DeclaringType.IsGenericType ? m.DeclaringType.GetGenericTypeDefinition().FullName : m.DeclaringType.FullName))
            return !m.Name.StartsWith("set_", StringComparison.Ordinal);
        if (m.ContainsGenericParameters) return false;
        var body = m.GetMethodBody();
        if (body == null) return false;
        var il = body.GetILAsByteArray();
        if (il == null) return false;
        var module = m.Module;
        var typeArgs = m.DeclaringType != null && m.DeclaringType.IsGenericType ? m.DeclaringType.GetGenericArguments() : null;
        var methodArgs = m.IsGenericMethod ? m.GetGenericArguments() : null;
        var pos = 0;
        while (pos < il.Length)
        {
            OpCode op;
            var b = il[pos++];
            if (b == 0xfe) op = Two[il[pos++]];
            else op = One[b];
            var name = op.Name;
            if (name == null) return false;
            int operand = 0;
            switch (op.OperandType)
            {
                case OperandType.InlineNone: break;
                case OperandType.ShortInlineBrTarget:
                case OperandType.ShortInlineI:
                case OperandType.ShortInlineVar: pos += 1; break;
                case OperandType.InlineVar: pos += 2; break;
                case OperandType.InlineI8:
                case OperandType.InlineR: pos += 8; break;
                case OperandType.InlineSwitch:
                    {
                        var n = BitConverter.ToInt32(il, pos);
                        pos += 4 + 4 * n;
                        break;
                    }
                default:
                    operand = BitConverter.ToInt32(il, pos);
                    pos += 4;
                    break;
            }
            if (name.StartsWith("stfld") || name.StartsWith("stsfld") || name.StartsWith("stelem") || name.StartsWith("stind") ||
                name == "stobj" || name == "newarr" || name == "calli" || name == "cpblk" || name == "initblk" || name == "localloc" ||
                name == "ldftn" || name == "ldvirtftn" || name == "cpobj" || name == "mkrefany" || name == "initobj")
                return false;
            if (name == "newobj")
            {
                MethodBase ctor;
                try { ctor = module.ResolveMethod(operand, typeArgs, methodArgs); } catch (Exception) { return false; }
                // Only constructors of a few known plain value types.
                if (ctor?.DeclaringType == null || !AllowedCtorTypes.Contains(ctor.DeclaringType.IsGenericType ? ctor.DeclaringType.GetGenericTypeDefinition().FullName : ctor.DeclaringType.FullName)) return false;
                continue;
            }
            if (name == "call" || name == "callvirt")
            {
                MethodBase callee;
                try { callee = module.ResolveMethod(operand, typeArgs, methodArgs); } catch (Exception) { return false; }
                if (callee == null) return false;
                if (callee.DeclaringType != null && callee.DeclaringType.IsInterface) return false;
                if (name == "callvirt" && callee.IsVirtual && !callee.IsFinal && !(callee.DeclaringType?.IsSealed ?? false)) return false;
                if (!IsPure(callee, depth + 1)) return false;
            }
        }
        return true;
    }
}

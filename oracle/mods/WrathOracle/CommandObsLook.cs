using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using HarmonyLib;
using Kingmaker.Controllers.Units;
using Kingmaker.EntitySystem.Entities;
using UnityEngine;

namespace WrathOracle;

// Observer "look" (docs/poc5/orientation.md): every write of a unit's DesiredOrientation and m_Orientation the agent's
// records don't already show, so a replay computes the DesiredOrientation UnitMoveController.TickUnit reads (ag_rot's
// `desired`) instead of taking it:
//  - ag_look: one record per call of UnitEntityData.LookAt(point), ForceLookAt(point), the Orientation setter or the
//    DesiredOrientation setter, written once the call returns: fn ("LookAt", "ForceLookAt", "Orientation",
//    "DesiredOrientation"), the unit (u), the point (p: LookAt and ForceLookAt; else null) or the value (v: the setters;
//    else null), before the call the unit's m_Position (pos) and whether it has a RiderPart (rider: LookAt and
//    ForceLookAt then turn the saddled unit, which isn't recorded apart: resolving it would resolve an entity
//    reference; a replay reports such a call), m_Orientation and DesiredOrientation before (o0, d0) and after (o, d),
//    and the caller chain (origin). Where real time just ended, the agent's synchronization (ag_sync) comes before
//    the call.
// The calls are found by scanning the IL of the game's assemblies for call sites of those four members, and each site
// is transpiled into a tap that makes the same call and records it (the setters are small enough for Mono's JIT to
// inline into their callers, so hooking the members themselves could miss calls). Left out, because another record
// shows them: the members' calls inside the members (the Orientation setter's DesiredOrientation write, LookAt's
// DesiredOrientation write and its call on the saddled unit, ForceLookAt's Orientation write and its call on the
// saddled unit: their caller's record covers them), and UnitMoveController.TickUnit's Orientation write to its own unit
// (ag_rot's `o` and `desired` follow it; its write to another unit, the rider's copy, is recorded). The install checks
// that only the Orientation setter and UnitEntityData.UpdateSlowRotation store m_Orientation and only the
// DesiredOrientation setter its backing field (nothing else stores either or takes its address), and fails closed on
// a site it can't transpile (a generic method, a method group). Field reads only: m_Position, m_Orientation, the
// auto-properties DesiredOrientation and RiderPart.
internal static partial class CommandObs
{
    internal static bool LookOn;

    private static readonly Type Ued = typeof(UnitEntityData);

    // The four members, and the tap each site's call is replaced by.
    private static readonly (string name, Type arg, string tap)[] LookMembers =
    {
        ("set_DesiredOrientation", typeof(float), nameof(TapSetDesired)),
        ("set_Orientation", typeof(float), nameof(TapSetOrientation)),
        ("LookAt", typeof(Vector3), nameof(TapLookAt)),
        ("ForceLookAt", typeof(Vector3), nameof(TapForceLookAt)),
    };

    private static string LookMember(MethodBase m)
    {
        if (m == null || m.DeclaringType != Ued || m.IsStatic) return null;
        var ps = m.GetParameters();
        if (ps.Length != 1) return null;
        foreach (var x in LookMembers)
            if (m.Name == x.name && ps[0].ParameterType == x.arg) return x.name;
        return null;
    }

    // ---- the taps ----

    private struct LookBefore
    {
        public bool Rec, Rider;
        public string U;
        public Vector3 Pos;
        public float O, D;
    }

    private static bool LookObserved() => AgObserved() && LookOn;

    private static LookBefore LookPre(UnitEntityData u)
    {
        var b = new LookBefore();
        try
        {
            if (!LookObserved() || u == null) return b;
            // Where real time just ended, the synchronization's snapshot first: it shows the state before the write.
            AgCheckTb(false);
            b.U = u.UniqueId;
            b.Pos = PositionField(u);
            b.O = OrientationField(u);
            b.D = u.DesiredOrientation;
            b.Rider = u.RiderPart != null;
            b.Rec = true;
        }
        catch (Exception e)
        {
            b.Rec = false;
            Trace.HookFailed("observe ag_look", e);
        }
        return b;
    }

    private static void LookPost(UnitEntityData u, LookBefore b, string fn, Vector3? p, float? v)
    {
        try
        {
            if (!b.Rec || !LookObserved()) return;
            var o = OrientationField(u);
            var d = u.DesiredOrientation;
            var origin = Origin();
            AgObs("ag_look", j =>
            {
                j.Prop("fn", fn); j.Prop("u", b.U); NullableVec(j, "p", p); NullableFloat(j, "v", v); j.Prop("pos", b.Pos);
                j.Prop("rider", b.Rider); j.Prop("o0", b.O); j.Prop("d0", b.D); j.Prop("o", o); j.Prop("d", d);
                j.Prop("origin", origin);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe ag_look", e); }
    }

    public static void TapLookAt(UnitEntityData u, Vector3 point)
    {
        var b = LookPre(u);
        u.LookAt(point);
        LookPost(u, b, "LookAt", point, null);
    }

    public static void TapForceLookAt(UnitEntityData u, Vector3 point)
    {
        var b = LookPre(u);
        u.ForceLookAt(point);
        LookPost(u, b, "ForceLookAt", point, null);
    }

    public static void TapSetOrientation(UnitEntityData u, float value)
    {
        var b = LookPre(u);
        u.Orientation = value;
        LookPost(u, b, "Orientation", null, value);
    }

    // UnitMoveController.TickUnit's sites: its write to its own unit is shown by ag_rot; another unit's (the rider's
    // copy) is recorded.
    public static void TapSetOrientationCtl(UnitEntityData u, float value)
    {
        if (ReferenceEquals(u, _ctlUnit))
        {
            u.Orientation = value;
            return;
        }
        TapSetOrientation(u, value);
    }

    public static void TapSetDesired(UnitEntityData u, float value)
    {
        var b = LookPre(u);
        u.DesiredOrientation = value;
        LookPost(u, b, "DesiredOrientation", null, value);
    }

    // ---- finding the sites ----

    // The methods whose IL calls one of the members, with their number of such calls (each is transpiled).
    private static readonly Dictionary<MethodBase, int> LookSites = new Dictionary<MethodBase, int>();
    // The calls each transpiled method had replaced (its last transpilation), by method handle.
    private static readonly Dictionary<IntPtr, int> LookTapped = new Dictionary<IntPtr, int>();
    // Each transpiled method's sites, by method handle (checked at every transpilation once installed).
    private static readonly Dictionary<IntPtr, int> LookExpected = new Dictionary<IntPtr, int>();
    private static MethodBase _lookCtl;
    private static int _lookUnloaded, _lookUnresolved;

    // The members' own sites, left untapped (their caller's record covers them): method name, member name, count.
    private static readonly (string method, string member, int n)[] LookOwnSites =
    {
        ("set_Orientation", "set_DesiredOrientation", 1),
        ("LookAt", "set_DesiredOrientation", 1),
        ("LookAt", "LookAt", 1),
        ("ForceLookAt", "set_Orientation", 1),
        ("ForceLookAt", "ForceLookAt", 1),
    };

    // The methods allowed to store (stfld) the two fields; nothing may take their address (ldflda).
    private static readonly (string field, string method)[] LookFieldWriters =
    {
        ("m_Orientation", "set_Orientation"),
        ("m_Orientation", "UpdateSlowRotation"),
        ("<DesiredOrientation>k__BackingField", "set_DesiredOrientation"),
    };

    private static void LookScan(Action<string> fail)
    {
        Rng.BuildOpcodeTable();
        var fields = new Dictionary<FieldInfo, string>();
        foreach (var name in LookFieldWriters.Select(x => x.field).Distinct())
        {
            var f = Ued.GetField(name, BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.DeclaredOnly);
            if (f == null) { fail("UnitEntityData has no field " + name); return; }
            fields[f] = name;
        }
        var own = new Dictionary<(string, string), int>();
        var managed = System.IO.Path.GetDirectoryName(typeof(Kingmaker.Game).Assembly.Location);
        foreach (var a in AppDomain.CurrentDomain.GetAssemblies().Where(x => Rng.IsGameAssembly(x, managed)))
        {
            // A type that doesn't load can't run either (its methods are never compiled): skipped, counted.
            Type[] types;
            try { types = a.GetTypes(); }
            catch (ReflectionTypeLoadException e)
            {
                types = e.Types.Where(t => t != null).ToArray();
                _lookUnloaded += e.Types.Count(t => t == null);
            }
            catch (Exception e) { fail("the types of " + a.GetName().Name + ": " + e.Message); continue; }
            var module = a.ManifestModule;
            var methodCache = new Dictionary<int, string>();
            var fieldCache = new Dictionary<int, string>();
            foreach (var t in types)
            {
                const BindingFlags all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
                IEnumerable<MethodBase> methods;
                try { methods = t.GetMethods(all).Cast<MethodBase>().Concat(t.GetConstructors(all)).ToList(); }
                catch (Exception e) { fail("the methods of " + t.FullName + ": " + e.Message); continue; }
                foreach (var m in methods)
                {
                    byte[] il;
                    try { il = m.GetMethodBody()?.GetILAsByteArray(); }
                    catch (Exception e) { fail("the body of " + t.FullName + "." + m.Name + ": " + e.Message); continue; }
                    if (il == null) continue;
                    var calls = 0;
                    var pos = 0;
                    while (pos < il.Length)
                    {
                        OpCode op;
                        var b = il[pos++];
                        if (b == 0xfe) { if (pos >= il.Length) break; op = Rng.Two[il[pos++]]; }
                        else op = Rng.One[b];
                        if (op.Size == 0) { fail("an opcode it can't read in " + t.FullName + "." + m.Name); break; }
                        var operand = 0;
                        switch (op.OperandType)
                        {
                            case OperandType.InlineNone: break;
                            case OperandType.ShortInlineBrTarget:
                            case OperandType.ShortInlineI:
                            case OperandType.ShortInlineVar: pos += 1; break;
                            case OperandType.InlineVar: pos += 2; break;
                            case OperandType.InlineI8:
                            case OperandType.InlineR: pos += 8; break;
                            case OperandType.InlineSwitch: pos += 4 + 4 * BitConverter.ToInt32(il, pos); break;
                            default: operand = BitConverter.ToInt32(il, pos); pos += 4; break;
                        }
                        var table = (uint)operand >> 24;
                        var isCall = op.Value == OpCodes.Call.Value || op.Value == OpCodes.Callvirt.Value;
                        var isRef = op.Value == OpCodes.Ldftn.Value || op.Value == OpCodes.Ldvirtftn.Value;
                        var isStore = op.Value == OpCodes.Stfld.Value || op.Value == OpCodes.Ldflda.Value;
                        if ((isCall || isRef) && (table == 0x06 || table == 0x0A))
                        {
                            if (!methodCache.TryGetValue(operand, out var member))
                            {
                                // A method of this assembly always resolves; a reference that doesn't (its type or assembly
                                // doesn't load, a generic context) can't be one of UnitEntityData's: counted.
                                try { member = LookMember(module.ResolveMethod(operand)); }
                                catch (Exception e)
                                {
                                    member = null;
                                    if (table == 0x06) fail("a method token of " + t.FullName + "." + m.Name + ": " + e.Message);
                                    else _lookUnresolved++;
                                }
                                methodCache[operand] = member;
                            }
                            if (member == null) continue;
                            if (isRef) { fail("a method group of UnitEntityData." + member + " in " + t.FullName + "." + m.Name); continue; }
                            if (m.DeclaringType == Ued && LookOwnSites.Any(x => x.method == m.Name && x.member == member))
                            {
                                own[(m.Name, member)] = own.TryGetValue((m.Name, member), out var n) ? n + 1 : 1;
                                continue;
                            }
                            calls++;
                        }
                        else if (isStore && (table == 0x04 || table == 0x0A))
                        {
                            if (!fieldCache.TryGetValue(operand, out var field))
                            {
                                try
                                {
                                    var f = module.ResolveField(operand);
                                    field = f != null && fields.TryGetValue(f, out var nm) ? nm : null;
                                }
                                catch (Exception e)
                                {
                                    field = null;
                                    if (table == 0x04) fail("a field token of " + t.FullName + "." + m.Name + ": " + e.Message);
                                    else _lookUnresolved++;
                                }
                                fieldCache[operand] = field;
                            }
                            if (field == null) continue;
                            var allowed = op.Value == OpCodes.Stfld.Value && m.DeclaringType == Ued &&
                                          LookFieldWriters.Any(x => x.field == field && x.method == m.Name);
                            if (!allowed) fail((op.Value == OpCodes.Stfld.Value ? "a store to " : "the address of ") + field + " in " + t.FullName + "." + m.Name);
                        }
                    }
                    if (calls == 0) continue;
                    if (m.ContainsGenericParameters || (m.DeclaringType?.ContainsGenericParameters ?? false))
                    {
                        fail("a call site in a generic method: " + t.FullName + "." + m.Name);
                        continue;
                    }
                    LookSites[m] = calls;
                }
            }
        }
        foreach (var x in LookOwnSites)
        {
            own.TryGetValue((x.method, x.member), out var n);
            if (n != x.n) fail($"UnitEntityData.{x.method} calls {x.member} {n} times, not {x.n}");
        }
    }

    public static IEnumerable<CodeInstruction> LookTranspiler(IEnumerable<CodeInstruction> instructions, MethodBase __originalMethod)
    {
        var ctl = _lookCtl != null && __originalMethod.MethodHandle == _lookCtl.MethodHandle;
        var n = 0;
        foreach (var i in instructions)
        {
            if ((i.opcode == OpCodes.Call || i.opcode == OpCodes.Callvirt) && i.operand is MethodInfo mi)
            {
                var member = LookMember(mi);
                if (member != null)
                {
                    var tap = LookMembers.Single(x => x.name == member).tap;
                    if (ctl && tap == nameof(TapSetOrientation)) tap = nameof(TapSetOrientationCtl);
                    i.opcode = OpCodes.Call;
                    i.operand = AccessTools.Method(typeof(CommandObs), tap);
                    n++;
                }
            }
            yield return i;
        }
        var key = __originalMethod.MethodHandle.Value;
        LookTapped[key] = n;
        // Harmony transpiles a method again whenever a patch is added to it: each time, every site must be tapped.
        if (LookExpected.TryGetValue(key, out var want) && n != want && LookOn)
        {
            LookOn = false;
            Health.Failed("look observe: " + __originalMethod.DeclaringType?.FullName + "." + __originalMethod.Name + " transpiled again with " + n + " calls tapped, not " + want, new Exception("look"));
        }
    }

    // ---- install ----

    private static void InstallLook(Harmony h)
    {
        void Fail(string what) => Health.Failed("look observe: " + what, new Exception("look"));
        // The records go through the agent's (AgObs: a synchronization first where real time ends).
        if (!AgOn) return;
        var failed = Health.FailedCount;
        var sw = Stopwatch.StartNew();
        var ctls = typeof(UnitMoveController)
            .GetMethods(BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly)
            .Where(m => m.Name == "TickUnit").ToList();
        _lookCtl = ctls.Count == 1 ? ctls[0] : null;
        if (_lookCtl == null) Fail($"UnitMoveController.TickUnit matched {ctls.Count} methods");
        try { LookScan(Fail); }
        catch (Exception e) { Health.Failed("look observe: scan", e); return; }
        if (_lookCtl != null && !LookSites.Keys.Any(m => m.MethodHandle == _lookCtl.MethodHandle))
            Fail("UnitMoveController.TickUnit has no Orientation write");
        foreach (var kv in LookSites) LookExpected[kv.Key.MethodHandle.Value] = kv.Value;
        foreach (var kv in LookSites)
        {
            try { h.Patch(kv.Key, transpiler: new HarmonyMethod(typeof(CommandObs), nameof(LookTranspiler))); }
            catch (Exception e) { Health.Failed("patch " + kv.Key.DeclaringType?.FullName + "." + kv.Key.Name + " (look taps)", e); continue; }
            LookTapped.TryGetValue(kv.Key.MethodHandle.Value, out var n);
            if (n != kv.Value) Fail($"{kv.Key.DeclaringType?.FullName}.{kv.Key.Name}: {n} calls tapped, not {kv.Value}");
        }
        Fidelity.Notes.Add($"look: {LookSites.Values.Sum()} call sites in {LookSites.Count} methods transpiled ({_lookUnloaded} types that don't load and {_lookUnresolved} references that don't resolve skipped); {sw.Elapsed.TotalSeconds:0.00} s");
        LookOn = Health.FailedCount == failed;
    }
}

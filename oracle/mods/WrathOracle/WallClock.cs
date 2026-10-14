using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Utility;
using Owlcat.Runtime.UniRx;
using UnityEngine;

namespace WrathOracle;

// The wall clock inside turn-based fixture windows, and the replay's reproduction of what the
// recording observed. The governing rule (PLAN, the oracle): a recording observes the unmodified
// game; only a replay pins inputs, and only to the values its recording observed.
//
// Pinned in recordings and replays alike (an input definition: a game that never drops a frame),
// only inside a window whose plan has a "tbm:" step (Sched.Window), continuous at its edges:
//   * Time.unscaledDeltaTime reads (81 game methods transpiled; the getters are native calls Harmony
//     can't wrap) return the fixed frame delta; the real value stays in tick.wall_delta;
//   * DOTween's wall clock (it takes a delta from Time.realtimeSinceStartup differences) runs on a fixed
//     base plus the fixed deltas; its stored previous value is set at the window's start and put back on
//     the real clock at its end (WallClock owns that change and undoes it);
//   * DelayedInvoker's realtime waits count the fixed deltas of the window they were scheduled in
//     (Sched.Generation); once that window is gone they finish on the real clock.
//
// Pinned in replays only, to what the recording observed (nothing is changed in a recording):
//   * the identity hash of a unit, where the game uses it as a decision (friends' visibility phase,
//     the final initiative tie-break), is the recording's native value for that unit, for the whole
//     replay run from its "prepare", never changing at window open or close;
//   * the frame number the friends' visibility check reads is shifted by a constant so that the window
//     opens on the frame number the recording opened on;
//   * the camera rig's Time.time, and the absolute Time.unscaledTime the navigation refresh subtracts,
//     are offset by the difference between the recording's clocks and the replay's at window open, so
//     the float arithmetic (scroll end frames) rounds as the recording's did. The rig switches clock
//     only while none of its routines runs, so no retained deadline changes domain.
// Physics: the native 0.02 s step and transform-sync policy are untouched; the steps per frame are
// observed (Observe, "~fx" rows) and not pinned.
internal static class WallClock
{
    private static int _shift;
    private static double _time0, _unscaled0, _timeDelta;
    private static int _frame0;
    public static long VirtualReads, FrameReads;

    private static bool Active => Sched.Window && Trace.Active && Trace.OnMainThread && Time.captureDeltaTime > 0f;
    private static bool ReplayActive => Active && Sched.Replay;

    public static float RealUnscaledDeltaTime() => Time.unscaledDeltaTime;

    // Called by Fixture.Begin when the window starts (after the recording's header was read, for a replay).
    public static void BeginWindow()
    {
        _frame0 = Time.frameCount;
        _time0 = Time.timeAsDouble;
        _unscaled0 = Time.unscaledTimeAsDouble;
        VirtualReads = FrameReads = 0;
        _hashSeen.Clear();
        _shift = 0;
        _timeDelta = 0;
        if (Sched.Window && Sched.Replay && Sched.RecHeader)
        {
            _shift = (((Sched.RecFrame - _frame0) % 60) + 60) % 60;
            _timeDelta = Sched.RecTime0 - _time0;
        }
        // CombatController.m_NavmeshUpdateTimer survives from earlier windows of the process, so a replay
        // starts from the value the recording's window opened with.
        NavTimer0 = null;
        if (Sched.Window)
        {
            try
            {
                var cc = Game.Instance?.TurnBasedCombatController;
                if (cc != null && NavTimerField != null)
                {
                    NavTimer0 = (float)NavTimerField.GetValue(cc);
                    if (Sched.Replay && Sched.RecNavTimer0.HasValue) NavTimerField.SetValue(cc, Sched.RecNavTimer0.Value);
                }
            }
            catch (Exception e) { Health.Failed("navigation timer", e); }
        }
        if (Sched.Window) PinDotween();
    }

    private static readonly FieldInfo NavTimerField = AccessTools.Field(typeof(global::TurnBased.Controllers.CombatController), "m_NavmeshUpdateTimer");
    public static float? NavTimer0;

    // What the header of a recording says (read back by a replay).
    public static int Frame0 => _frame0;
    public static double Time0 => _time0;
    public static double Unscaled0 => _unscaled0;

    // Replacements for the getters (transpiled into the callers).
    public static float Unscaled()
    {
        if (!Active) return Time.unscaledDeltaTime;
        VirtualReads++;
        return Time.captureDeltaTime;
    }

    public static int Frame()
    {
        if (!ReplayActive) return Time.frameCount;
        FrameReads++;
        return Time.frameCount + _shift;
    }

    // DOTween measures its wall-clock delta as the difference of Time.realtimeSinceStartup between
    // two frames (two reads per Update, the second one stored for the next frame). In a window it is a
    // fixed small base plus the fixed frame deltas, so every run sees the same float sums (at the
    // magnitude of a real uptime each delta would round to a different float step).
    private const float DotweenBase = 16f;
    private static float _rtValue;
    private static int _rtFrame = -1;
    private static bool _rtVirtual, _dotweenOwned;
    private static readonly FieldInfo DotweenStored = AccessTools.Field(AccessTools.TypeByName("DG.Tweening.Core.DOTweenComponent"), "_unscaledTime");

    private static void SetDotweenStored(float v)
    {
        try
        {
            if (DotweenStored == null) return;
            foreach (var c in UnityEngine.Object.FindObjectsOfType(DotweenStored.DeclaringType)) DotweenStored.SetValue(c, v);
        }
        catch (Exception e) { Health.Failed("dotween clock", e); }
    }

    private static void PinDotween()
    {
        _rtVirtual = false;
        _rtFrame = -1;
        _dotweenOwned = true;
        SetDotweenStored(DotweenBase - Time.captureDeltaTime);
    }

    public static float Realtime()
    {
        if (!Active) return Time.realtimeSinceStartup;
        var frame = Time.frameCount;
        if (!_rtVirtual) { _rtVirtual = true; _rtValue = DotweenBase - Time.captureDeltaTime; _rtFrame = -1; }
        if (frame != _rtFrame)
        {
            _rtValue += (_rtFrame < 0 ? 1 : frame - _rtFrame) * Time.captureDeltaTime;
            _rtFrame = frame;
        }
        return _rtValue;
    }

    // Undoes what BeginWindow changed; idempotent, also called from cleanup. Only a change this class made
    // is undone (a real-time window never owns anything).
    public static void Restore()
    {
        if (_dotweenOwned)
        {
            _dotweenOwned = false;
            _rtVirtual = false;
            SetDotweenStored(Time.realtimeSinceStartup);
        }
        _shift = 0;
        _timeDelta = 0;
    }

    public static void EndWindow() => Restore();

    // ---- the camera rig's Time.time (replay only) ----
    //
    // The rig's scroll and rotate routines end at the first frame whose Time.time is past start + duration,
    // in float arithmetic at the magnitude of the process's game time, so with a duration of whole frames
    // the rounding of that sum decides n or n + 1 frames (and that moves a turn's start and the order of
    // other units' command events within a tick). The recording's native choice is the observation; a replay
    // that runs at another uptime has another rounding grid, so it reads the clock the recording had at the
    // same moment (its own double clock plus the difference at window open).
    private static bool _rigReplay;
    private static readonly FieldInfo RigScroll = AccessTools.Field(typeof(Kingmaker.View.CameraRig), "m_ScrollRoutine");
    private static readonly FieldInfo RigRotate = AccessTools.Field(typeof(Kingmaker.View.CameraRig), "m_RotateRoutine");

    private static bool RigIdle()
    {
        try
        {
            var rig = Game.Instance?.UI?.GetCameraRig();
            return rig == null || (RigScroll.GetValue(rig) == null && RigRotate.GetValue(rig) == null);
        }
        catch (Exception) { return false; }
    }

    // The offset a running scroll or rotation was started with stays until the rig is idle, even
    // past the window's end (Restore clears _timeDelta): only then does the clock switch.
    private static double _rigDelta;

    public static float RigTime()
    {
        if (RigIdle())
        {
            _rigReplay = ReplayActive;
            _rigDelta = _rigReplay ? _timeDelta : 0;
        }
        return _rigReplay ? (float)(Time.timeAsDouble + _rigDelta) : Time.time;
    }

    public static IEnumerable<CodeInstruction> RigTimeTranspiler(IEnumerable<CodeInstruction> ins) => Replace(ins, "time", nameof(RigTime));

    // The navigation refresh (CombatController.UpdateNavigationGrid) subtracts the absolute
    // Time.unscaledTime from its timer, so when it fires depends on the process's real uptime at each
    // read, which advances by wall time. A recording observes every read ("nav_clock"); a replay returns
    // the recorded values in order, and a read the recording didn't make, or one it made that the replay
    // didn't, invalidates it.
    private static readonly Queue<float> NavTape = new Queue<float>();

    public static void ResetNavTape() => NavTape.Clear();
    public static void AddNavTape(float v) => NavTape.Enqueue(v);

    public static float NavUnscaledTime()
    {
        var v = Time.unscaledTime;
        if (!Sched.On) return v;
        if (!ReplayActive)
        {
            Sched.Obs("nav_clock", j => j.Prop("v", (double)v));
            return v;
        }
        if (NavTape.Count == 0)
        {
            Sched.Invalidate($"navigation clock read at tick {Trace.Tick}: the recording made no further read");
            return v;
        }
        return NavTape.Dequeue();
    }

    public static void CheckComplete()
    {
        if (NavTape.Count > 0) Sched.Invalidate($"replay left {NavTape.Count} recorded navigation clock reads unused");
    }

    private static IEnumerable<CodeInstruction> Replace(IEnumerable<CodeInstruction> ins, string getter, string replacement)
    {
        var get = AccessTools.PropertyGetter(typeof(Time), getter);
        var repl = AccessTools.Method(typeof(WallClock), replacement);
        foreach (var i in ins)
        {
            if ((i.opcode == OpCodes.Call || i.opcode == OpCodes.Callvirt) && i.operand is MethodBase mb && mb.Module == get.Module && mb.MetadataToken == get.MetadataToken)
            {
                i.opcode = OpCodes.Call;
                i.operand = repl;
            }
            yield return i;
        }
    }

    // UnitMemoryController.UpdateVisibility checks friends' visibility only on frames where Time.frameCount % 5
    // equals (target.GetHashCode() >>> 2) % 5, and both initiative comparers end with the units' hashes.
    // UnitEntityData does not override GetHashCode, so that is the object's identity hash. A recording
    // returns the native value and records it (once per unit); a replay returns the recording's value for
    // that unit id (tape loaded at its "prepare"), else the native one. Only these calls are replaced.
    private static readonly HashSet<string> _hashSeen = new HashSet<string>();

    public static int UnitHash(object o)
    {
        var native = o.GetHashCode();
        if (!(o is Kingmaker.EntitySystem.Entities.UnitEntityData u)) return native;
        var tape = Sched.HashTape;
        if (tape != null && tape.TryGetValue(u.UniqueId, out var h)) return h;
        if (Sched.On && !Sched.Replay && _hashSeen.Add(u.UniqueId)) Sched.Obs("hash", j => { j.Prop("u", u.UniqueId); j.Prop("h", native); });
        return native;
    }

    public static IEnumerable<CodeInstruction> UnitHashTranspiler(IEnumerable<CodeInstruction> ins)
    {
        var repl = AccessTools.Method(typeof(WallClock), nameof(UnitHash));
        foreach (var i in Replace(ins, "frameCount", nameof(Frame)))
        {
            if ((i.opcode == OpCodes.Callvirt || i.opcode == OpCodes.Call) && i.operand is MethodBase mb && mb.Name == "GetHashCode" && mb.GetParameters().Length == 0)
            {
                i.opcode = OpCodes.Call;
                i.operand = repl;
            }
            yield return i;
        }
    }

    public static IEnumerable<CodeInstruction> UnscaledTranspiler(IEnumerable<CodeInstruction> ins) => Replace(ins, "unscaledDeltaTime", nameof(Unscaled));
    public static IEnumerable<CodeInstruction> FrameTranspiler(IEnumerable<CodeInstruction> ins) => Replace(ins, "frameCount", nameof(Frame));
    public static IEnumerable<CodeInstruction> RealtimeTranspiler(IEnumerable<CodeInstruction> ins) => Replace(ins, "realtimeSinceStartup", nameof(Realtime));
    public static IEnumerable<CodeInstruction> NavTranspiler(IEnumerable<CodeInstruction> ins) => Replace(ins, "unscaledTime", nameof(NavUnscaledTime));

    // ---- finding the callers of Time.unscaledDeltaTime ----

    public static string ScanNote;
    public static int PatchedCallers, FailedCallers;

    internal static bool WantAssembly(Assembly a, string managed)
    {
        if (Rng.IsGameAssembly(a, managed)) return true;
        string loc;
        if (a.IsDynamic) return false;
        try { loc = a.Location; } catch (Exception) { return false; }
        if (string.IsNullOrEmpty(loc)) return false;
        var n = a.GetName().Name;
        return System.IO.Path.GetDirectoryName(loc) == managed && (n == "UnityEngine.UI" || n.StartsWith("Unity.TextMeshPro", StringComparison.Ordinal));
    }

    // Every method of the game's assemblies whose IL calls the getter.
    private static List<MethodBase> FindCallers(MethodBase getter, Func<Assembly, string, bool> want) =>
        FindCallers(callee => callee.Name == getter.Name && callee.DeclaringType == getter.DeclaringType, want, ldftn: false)
            .Where(m => m.DeclaringType != typeof(Kingmaker.Controllers.TimeController)).ToList(); // OracleTime transpiles its reads

    // Every method of the wanted assemblies whose IL calls (call, callvirt) a method matching isTarget
    // through a MemberRef (a method of another assembly), or with ldftn also takes it as a delegate
    // (a method group such as Keys.Any(Input.GetKey)).
    internal static List<MethodBase> FindCallers(Func<MethodBase, bool> isTarget, Func<Assembly, string, bool> want, bool ldftn)
    {
        Rng.BuildOpcodeTable();
        var found = new List<MethodBase>();
        var managed = System.IO.Path.GetDirectoryName(typeof(Game).Assembly.Location);
        foreach (var a in AppDomain.CurrentDomain.GetAssemblies().Where(x => want(x, managed)))
        {
            Type[] types;
            try { types = a.GetTypes(); } catch (ReflectionTypeLoadException e) { types = e.Types.Where(t => t != null).ToArray(); }
            catch (Exception) { continue; }
            var module = a.ManifestModule;
            var cache = new Dictionary<int, bool>();
            foreach (var t in types)
            {
                const BindingFlags all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
                IEnumerable<MethodBase> methods;
                try { methods = t.GetMethods(all).Cast<MethodBase>().Concat(t.GetConstructors(all)); } catch (Exception) { continue; }
                foreach (var m in methods)
                {
                    byte[] il;
                    try { il = m.GetMethodBody()?.GetILAsByteArray(); } catch (Exception) { continue; }
                    if (il == null) continue;
                    var pos = 0;
                    var hit = false;
                    while (pos < il.Length && !hit)
                    {
                        OpCode op;
                        var b = il[pos++];
                        if (b == 0xfe) { if (pos >= il.Length) break; op = Rng.Two[il[pos++]]; }
                        else op = Rng.One[b];
                        if (op.Size == 0) break;
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
                            case OperandType.InlineSwitch: pos += 4 + 4 * BitConverter.ToInt32(il, pos); break;
                            default: operand = BitConverter.ToInt32(il, pos); pos += 4; break;
                        }
                        if (op.Value != OpCodes.Call.Value && op.Value != OpCodes.Callvirt.Value && !(ldftn && op.Value == OpCodes.Ldftn.Value)) continue;
                        if (((uint)operand >> 24) != 0x0A) continue; // a MemberRef (a method of another assembly)
                        if (!cache.TryGetValue(operand, out var isGetter))
                        {
                            try
                            {
                                var callee = module.ResolveMethod(operand);
                                isGetter = isTarget(callee);
                            }
                            catch (Exception) { isGetter = false; }
                            cache[operand] = isGetter;
                        }
                        hit = isGetter;
                    }
                    if (hit) found.Add(m);
                }
            }
        }
        return found;
    }

    // ---- realtime callbacks ----

    private sealed class Stopper : IDisposable
    {
        private readonly Coroutine _co;
        public Stopper(Coroutine co) { _co = co; }
        public void Dispose()
        {
            try { DelayedInvokerCoroutineRunner.Stop(_co); }
            catch (Exception) { /* the runner is gone with the scene */ }
        }
    }

    private static string Describe(Action a)
    {
        var m = a?.Method;
        return m == null ? "null" : (m.DeclaringType?.Name ?? "?") + "." + m.Name;
    }

    public static bool InvokeInTimePrefix(Action action, float time, bool realtime, ref IDisposable __result)
    {
        if (!realtime || !Active) return true;
        try
        {
            var name = Describe(action);
            Sched.Gate("delayed_schedule", $"{name} {time:R}");
            // Only callbacks scheduled inside the window are gates: the ones the game scheduled
            // before it (sound reminders, tooltips) expire at times that depend on how long the
            // load took.
            __result = new Stopper(DelayedInvokerCoroutineRunner.Start(Wait(action, time, name, Sched.On, Sched.Generation)));
            return false;
        }
        catch (Exception e)
        {
            Trace.HookFailed("InvokeInTime", e);
            return true;
        }
    }

    // The game waits `time` real seconds, then calls the action; here the seconds are the
    // fixed frame deltas of the frames that pass (the real ones if the window has ended).
    private static IEnumerator Wait(Action action, float time, string name, bool gated, int generation)
    {
        var elapsed = 0f;
        while (elapsed < time)
        {
            yield return null;
            // The fixed deltas count only in the window the wait was scheduled in; after it, real time.
            elapsed += Active && Sched.Generation == generation ? Time.captureDeltaTime : RealUnscaledDeltaTime();
        }
        if (gated && Sched.On && Sched.Generation == generation) Sched.Gate("delayed_fire", name);
        action?.Invoke();
    }

    // ---- the turn-based UI guard and turn status ----

    private static readonly AccessTools.FieldRef<CountingGuard, int> GuardCount = AccessTools.FieldRefAccess<CountingGuard, int>("m_GuardCount");

    private static string Callers()
    {
        try
        {
            var frames = new StackTrace(2, false).GetFrames();
            if (frames == null) return "?";
            var names = frames
                .Select(Rng.FrameMethod)
                .Where(m => m != null && m.DeclaringType != null && m.DeclaringType != typeof(CountingGuard) && !(m.DeclaringType.Namespace ?? "").StartsWith("WrathOracle", StringComparison.Ordinal) &&
                            !(m.DeclaringType.Namespace ?? "").StartsWith("HarmonyLib", StringComparison.Ordinal) && !m.Name.StartsWith("DMD<", StringComparison.Ordinal) && m.DeclaringType.Name != "DynamicMethodDefinition" &&
                            !m.Name.StartsWith("Patch", StringComparison.Ordinal))
                .Take(3)
                .Select(m => m.DeclaringType.Name + "." + m.Name);
            return string.Join("<", names);
        }
        catch (Exception) { return "?"; }
    }

    public static void GuardSetPostfix(CountingGuard __instance, bool value)
    {
        if (!Sched.On) return;
        try
        {
            var cc = Game.Instance?.TurnBasedCombatController;
            if (cc == null || !ReferenceEquals(cc.WaitingForUI, __instance)) return;
            Sched.Gate("ui_guard", $"{(value ? "+" : "-")} count={GuardCount(__instance)} by={Callers()}");
        }
        catch (Exception e) { Trace.HookFailed("ui guard", e); }
    }

    public static void Install(Harmony h)
    {
        void P(MethodBase m, string prefix = null, string postfix = null)
        {
            if (m == null) { Health.Failed("clock hook target missing", new Exception(prefix ?? postfix)); return; }
            try
            {
                h.Patch(m, prefix: prefix == null ? null : new HarmonyMethod(typeof(WallClock), prefix),
                    postfix: postfix == null ? null : new HarmonyMethod(typeof(WallClock), postfix));
            }
            catch (Exception e)
            {
                Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name, e);
            }
        }

        P(AccessTools.Method(typeof(DelayedInvoker), nameof(DelayedInvoker.InvokeInTime), new[] { typeof(Action), typeof(float), typeof(bool) }), prefix: nameof(InvokeInTimePrefix));
        P(AccessTools.PropertySetter(typeof(CountingGuard), nameof(CountingGuard.Value)), postfix: nameof(GuardSetPostfix));

        // The three time reads, transpiled into their callers. The broad one (the wall-clock
        // frame delta) is optional: a caller that can't be patched is counted and reported in
        // the notes, and the gates show whether anything depended on it.
        var sw = Stopwatch.StartNew();
        var getter = AccessTools.PropertyGetter(typeof(Time), nameof(Time.unscaledDeltaTime));
        var callers = FindCallers(getter, WantAssembly);
        var tr = new HarmonyMethod(typeof(WallClock), nameof(UnscaledTranspiler));
        var failed = new List<string>();
        foreach (var m in callers)
        {
            try { h.Patch(m, transpiler: tr); PatchedCallers++; }
            catch (Exception) { FailedCallers++; failed.Add(m.DeclaringType?.Name + "." + m.Name); }
        }
        // DOTween's own wall-clock delta comes from Time.realtimeSinceStartup differences.
        var rt = AccessTools.PropertyGetter(typeof(Time), nameof(Time.realtimeSinceStartup));
        var dotween = FindCallers(rt, (a, managed) => a.GetName().Name.StartsWith("DOTween", StringComparison.Ordinal) && Rng.IsGameAssembly(a, managed));
        var rtTr = new HarmonyMethod(typeof(WallClock), nameof(RealtimeTranspiler));
        var rtNames = new List<string>();
        foreach (var m in dotween)
        {
            try { h.Patch(m, transpiler: rtTr); rtNames.Add(m.DeclaringType?.Name + "." + m.Name); }
            catch (Exception) { failed.Add("rt " + m.DeclaringType?.Name + "." + m.Name); }
        }
        void One(string type, string method, string transpiler)
        {
            try
            {
                var t = AccessTools.TypeByName(type);
                h.Patch(AccessTools.Method(t, method), transpiler: new HarmonyMethod(typeof(WallClock), transpiler));
            }
            catch (Exception e) { Health.Failed($"patch {type}.{method}", e); }
        }
        One("Kingmaker.Controllers.Units.UnitMemoryController", "UpdateVisibility", nameof(UnitHashTranspiler));
        One("TurnBased.Controllers.CombatController", "UpdateNavigationGrid", nameof(NavTranspiler));
        // The last tie-break of the initiative order is the units' identity hashes.
        One("TurnBased.Controllers.CombatController+UnitsOrderComaprer", "Compare", nameof(UnitHashTranspiler));
        One("Kingmaker.Controllers.Combat.UnitCombatPrepareController+UnitEntityOrderComaprer", "Compare", nameof(UnitHashTranspiler));
        var rig = FindCallers(AccessTools.PropertyGetter(typeof(Time), nameof(Time.time)), (a, managed) => a.GetName().Name == "Assembly-CSharp")
            .Where(m => (m.DeclaringType?.FullName ?? "").StartsWith("Kingmaker.View.CameraRig", StringComparison.Ordinal)).ToList();
        var rigTr = new HarmonyMethod(typeof(WallClock), nameof(RigTimeTranspiler));
        foreach (var m in rig)
        {
            try { h.Patch(m, transpiler: rigTr); }
            catch (Exception) { failed.Add("rig " + m.DeclaringType?.Name + "." + m.Name); }
        }
        ScanNote = $"clock (camera rig time in {rig.Count} methods): {callers.Count} methods call Time.unscaledDeltaTime, {PatchedCallers} patched, {FailedCallers} failed ({string.Join(", ", failed.Take(8))}); DOTween realtime readers patched: {string.Join(", ", rtNames)}; {sw.Elapsed.TotalSeconds:0.00} s";
        Fidelity.Notes.Add(ScanNote);
    }
}

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using System.Text;
using System.Threading;
using HarmonyLib;
using Newtonsoft.Json.Linq;
using UnityEngine;
using URandom = UnityEngine.Random;

namespace WrathOracle;

// Every random draw, individually (Wrath-Rust PLAN, "The oracle"; contract item 3).
//
// 1. At install, every managed call site of UnityEngine.Random in the game's assemblies is
//    found by scanning method IL (call/callvirt to a UnityEngine.Random member). Each site
//    gets a stable key "Assembly|Type|Method(signature)|#ordinal@ILoffset|member" and an id
//    (its index in the key-sorted table, so the same build gives the same ids).
// 2. A transpiler replaces each site's call with "ldc.i4 <id>; call Rng.<wrapper>", whose
//    wrapper calls the same Random member with the same arguments (the same consumption) and
//    records site, member, arguments, value, the RNG state before, the steps consumed, and
//    the current tick, rule and handler.
// 3. Sites the transpiler can't reach (generic methods, static constructors) are covered by
//    a global prefix on the managed Random members (Range(int,int), the vector/rotation
//    getters, ColorHSV), which identifies the caller from the stack. Extern-only members
//    (value, Range(float,float)) called from such sites are not interceptable; they show up
//    in the gap count below.
// 4. Gap detection: UnityEngine.Random is xorshift128 (verified at install by a self-test
//    that saves and restores the state). Before each hooked draw the state is compared with
//    the state after the previous hooked draw (or tick checkpoint), and the number of
//    unrecorded steps in between is counted by stepping the generator forward. Any non-zero
//    gap is a draw some code made without going through a hook (native code, an unpatched
//    site); the total is in every capture's health.
// 5. Replay: with a tape (the draws of a recorded trace), each hooked draw returns the
//    recorded value after checking that the site, member and arguments match, and does NOT
//    call the real generator (strict mode), so code that draws without a hook sees a
//    different stream than in the recording; a replay that still matches proves those draws
//    don't matter for the trace.
internal static class Rng
{
    public sealed class Site
    {
        public int Id;
        public string Key, Assembly, Type, Method, Member;
        public int Ordinal, IlOffset;
        public bool Transpiled, Generic, StaticCtor, Disabled;
        public bool Presentation; // rendering/audio/UI-only consumer: kept out of the sim tape
        public string Note;
        public long Draws;
        public MethodBase Target;
    }

    public static readonly List<Site> Sites = new List<Site>();

    // Consumers that only feed presentation (rendering, particles, critters, weather, sound,
    // camera shake, UI, third-party visual plugins). They draw from the same global generator
    // as the dice, so they are recorded (and they perturb the dice stream in the original,
    // frame-rate dependently), but they are not part of the simulation's draw tape: replay
    // lets them use the real generator, and trace comparison leaves them out. Everything not
    // listed here is treated as simulation (conservative).
    private static readonly string[] PresentationPrefixes =
    {
        "Owlcat.Runtime.Visual", "Kingmaker.Visual.Critters", "Kingmaker.Visual.Particles", "Kingmaker.Visual.Sound",
        "Kingmaker.Visual.Trails", "Kingmaker.Visual.RandomRotationOnStart",
        "Kingmaker.Visual.RandomScaleOnStart", "Kingmaker.Visual.CameraShakeFx", "Kingmaker.Visual.CharactersRigidbody",
        "Kingmaker.Visual.HitSystem", "Kingmaker.View.ShakeSettings", "Kingmaker.UI.", "Kingmaker.Assets.Visual",
        "_2dxFX", "Dreamteck.", "RootMotion.", "DG.Tweening", "AkObstructionOcclusion", "AnimatorControllerSpeedMultiplyer",
        "LanternKingExpressionsRandom", "Kingmaker.Utility.ParticleSystemDestroyer", "Rewired.",
        // (Kingmaker.Visual.WeatherSystem and InclemencyController stay sim: WeatherMechanicController
        // picks units for buffs and spells, and inclemency is persistent weather state.)
    };

    // The weighted picker serves unit barks (UnitAsksComponent.Bark.Schedule), roaming waypoints and the
    // mercenary manager; only a call from the bark scheduler is presentation.
    private static bool IsBarkCall()
    {
        try
        {
            foreach (var f in new System.Diagnostics.StackTrace(2, false).GetFrames() ?? new System.Diagnostics.StackFrame[0])
            {
                var t = FrameMethod(f)?.DeclaringType;
                if (t != null && (t.FullName ?? "").Contains("UnitAsksComponent")) return true;
            }
        }
        catch (Exception) { /* not a bark */ }
        return false;
    }

    // The method a stack frame belongs to. A patched method (every method holding a site is transpiled,
    // Bark.Schedule among them) runs as a dynamic method whose frame has no declaring type; Harmony maps
    // it back to the original.
    internal static MethodBase FrameMethod(StackFrame f)
    {
        if (f == null) return null;
        try { return Harmony.GetOriginalMethodFromStackframe(f) ?? f.GetMethod(); }
        catch (Exception) { return f.GetMethod(); }
    }

    private const string BarkPicker = "Kingmaker.Utility.LinqExtensions|WeightedRandom";

    public static bool IsPresentation(string typeFullName) =>
        typeFullName != null && PresentationPrefixes.Any(p => typeFullName.StartsWith(p, StringComparison.Ordinal));
    private static readonly Dictionary<MethodBase, List<Site>> SitesByMethod = new Dictionary<MethodBase, List<Site>>();
    private static readonly Dictionary<string, Site> GlobalSites = new Dictionary<string, Site>();

    public static long UnhookedSteps, UnhookedUnknown, GlobalHookDraws, OffThreadDraws, ReplayMismatches, ReplayExhausted, TotalDraws;
    public static bool SelfTestOk;
    public static string SelfTestNote;
    public static string DisableSiteContaining; // fault injection: leave matching sites untranspiled

    [ThreadStatic] private static bool _inWrapper;

    // ---- state access and the generator model ----

    private delegate void StateReader(URandom.State s, uint[] dst);
    private static readonly StateReader ReadState = BuildReader();
    private static readonly uint[] Tmp = new uint[4];

    private static StateReader BuildReader()
    {
        var dm = new DynamicMethod("WrathOracle_ReadRandomState", typeof(void), new[] { typeof(URandom.State), typeof(uint[]) }, typeof(Rng).Module, true);
        var il = dm.GetILGenerator();
        var names = new[] { "s0", "s1", "s2", "s3" };
        for (var i = 0; i < 4; i++)
        {
            il.Emit(OpCodes.Ldarg_1);
            il.Emit(OpCodes.Ldc_I4, i);
            il.Emit(OpCodes.Ldarga_S, (byte)0);
            il.Emit(OpCodes.Ldfld, typeof(URandom.State).GetField(names[i], BindingFlags.Instance | BindingFlags.NonPublic));
            il.Emit(OpCodes.Stelem_I4);
        }
        il.Emit(OpCodes.Ret);
        return (StateReader)dm.CreateDelegate(typeof(StateReader));
    }

    public struct S4
    {
        public uint X, Y, Z, W;
        public bool Equals(S4 o) => X == o.X && Y == o.Y && Z == o.Z && W == o.W;
        public override string ToString() => $"{X:x8}:{Y:x8}:{Z:x8}:{W:x8}";

        public uint Next()
        {
            var t = X ^ (X << 11);
            X = Y; Y = Z; Z = W;
            W = W ^ (W >> 19) ^ t ^ (t >> 8);
            return W;
        }
    }

    public static S4 Now()
    {
        lock (Tmp)
        {
            ReadState(URandom.state, Tmp);
            return new S4 { X = Tmp[0], Y = Tmp[1], Z = Tmp[2], W = Tmp[3] };
        }
    }

    // Steps from a to b (0..max), or -1 if b isn't reachable within max steps.
    public static int StepsBetween(S4 a, S4 b, int max = 4096)
    {
        for (var i = 0; i <= max; i++)
        {
            if (a.Equals(b)) return i;
            a.Next();
        }
        return -1;
    }

    private static S4 _lastAfter;
    private static bool _haveLast;

    // Self-test of the generator model: one Range(int), one Range(float) and one value each
    // consume exactly one xorshift128 step, and Range(int) maps as min + next % (max - min).
    // The state is restored afterwards, so this consumes nothing.
    public static void SelfTest()
    {
        var saved = URandom.state;
        try
        {
            var s = Now();
            var v = URandom.Range(0, 1000);
            var a = Now();
            var m = s; var expect = (int)(m.Next() % 1000u);
            var ok1 = StepsBetween(s, a, 4) == 1 && expect == v;
            var b0 = Now(); URandom.Range(0f, 1f); var b1 = Now();
            var c0 = Now(); var _ = URandom.value; var c1 = Now();
            var ok2 = StepsBetween(b0, b1, 4) == 1 && StepsBetween(c0, c1, 4) == 1;
            SelfTestOk = ok1 && ok2;
            SelfTestNote = $"Range(int) one step and mapping {(ok1 ? "ok" : "MISMATCH")} (value {v}, model {expect}); Range(float)/value one step {(ok2 ? "ok" : "MISMATCH")}";
        }
        catch (Exception e)
        {
            SelfTestOk = false;
            SelfTestNote = "self-test failed: " + e.Message;
        }
        finally
        {
            URandom.state = saved;
        }
    }

    // Gap since the last hooked draw or checkpoint; adds to the health counters.
    private static int Gap(S4 before)
    {
        if (!_haveLast) return 0;
        var n = StepsBetween(_lastAfter, before);
        if (n < 0) Interlocked.Increment(ref UnhookedUnknown);
        else Interlocked.Add(ref UnhookedSteps, n);
        return n;
    }

    // Called at the start of every tick while recording: localizes unhooked draws in time.
    public static int Checkpoint()
    {
        var now = Now();
        var gap = Gap(now);
        _lastAfter = now;
        _haveLast = true;
        return gap;
    }

    public static void ResetWindow()
    {
        _haveLast = false;
        UnhookedSteps = UnhookedUnknown = GlobalHookDraws = OffThreadDraws = ReplayMismatches = ReplayExhausted = TotalDraws = 0;
        foreach (var s in Sites) s.Draws = 0;
        lock (GlobalSites) GlobalSites.Clear();
        _tapePos = 0;
    }

    // ---- replay tape ----

    private struct TapeEntry
    {
        public int Index; // position in the recorded sim-draw sequence
        public string Key, Member;
        public string Args; // JSON text
        public JToken Value;
        public string Ctx;  // turn-based windows: rule class, initiator, target and ordinal of a draw made inside a rule
        public int Tick;
    }

    // Sites that drew inside a rule in the recording (T0 sites); a tape problem there ends a
    // turn-based replay at once (Sched.Invalidate).
    private static HashSet<string> _t0Sites;

    private static List<TapeEntry> _tape;
    private static int _tapePos;
    public static bool ReplayActive => _tape != null;
    public static int TapeLength => _tape?.Count ?? 0;
    public static int TapePosition => _tapePos;
    public static string PerturbNote;

    // Loads the draws of a recorded trace; perturb = "N:+d" adds d to the N-th draw's value.
    public static void LoadTape(string tracePath, string perturb)
    {
        var tape = new List<TapeEntry>();
        var keys = new Dictionary<int, string>();
        var timeRaw = new List<float>();
        var tickUnscaled = new List<float>();
        var t0Sites = new HashSet<string>();
        if (Sched.Window) Sched.StartTape();
        using (var f = File.OpenRead(tracePath))
        using (var gz = new GZipStream(f, CompressionMode.Decompress))
        using (var r = new StreamReader(gz, Encoding.UTF8))
        {
            string line;
            while ((line = r.ReadLine()) != null)
            {
                if (line.Length == 0) continue;
                var k = line.IndexOf("\"k\":\"", StringComparison.Ordinal);
                if (k < 0) continue;
                if (string.CompareOrdinal(line, k + 5, "begin", 0, 5) == 0)
                {
                    var o = JObject.Parse(line);
                    foreach (var s in (JArray)o["rng_sites"] ?? new JArray()) keys[(int)s[0]] = (string)s[1];
                    // The observers the recording declared that its replay's pins need (Fixture.Begin checks them).
                    if (Sched.Window && o["observers"] is JArray obs)
                    {
                        ProjPin.RecordedOn = obs.Any(x => x.Type == JTokenType.String && (string)x == "proj_arrival");
                        PathSync.RecordedCritters = obs.Any(x => x.Type == JTokenType.String && (string)x == "critter_paths");
                        DeathFxPin.RecordedOn = obs.Any(x => x.Type == JTokenType.String && (string)x == "death_effect");
                        CamPin.RecordedOn = obs.Any(x => x.Type == JTokenType.String && (string)x == "camera_gates");
                    }
                    if (Sched.Window && o["time0"] != null)
                    {
                        Sched.RecFrame = (int)o["frame"];
                        Sched.RecTime0 = (double)o["time0"];
                        Sched.RecUnscaled0 = (double)o["unscaled0"];
                        Sched.RecHeader = true;
                        if (o["nav_timer0"] != null) Sched.RecNavTimer0 = (float)(double)o["nav_timer0"];
                        if (o["camera"] is JArray cam && cam.Count > 0) Sched.RecCamera = cam.Select(x => (double)x).ToArray();
                        if (o["physics_auto_sync"] != null && o["physics_auto_simulation"] != null && o["fixed_delta_time"] != null)
                            Sched.RecPhysics = ((bool)o["physics_auto_sync"], (bool)o["physics_auto_simulation"], (double)o["fixed_delta_time"]);
                    }
                }
                else if (string.CompareOrdinal(line, k + 5, "time\"", 0, 5) == 0)
                {
                    var o = JObject.Parse(line);
                    timeRaw.Add((float)o["raw"]);
                }
                else if (string.CompareOrdinal(line, k + 5, "tick\"", 0, 5) == 0)
                {
                    var o = JObject.Parse(line);
                    tickUnscaled.Add((float)o["unscaled_delta"]);
                }
                else if (string.CompareOrdinal(line, k + 5, "draw\"", 0, 5) == 0)
                {
                    var o = JObject.Parse(line);
                    if ((string)o["c"] == "pres") continue;
                    var site = (string)o["key"] ?? (keys.TryGetValue((int)o["site"], out var key) ? key : "?" + (int)o["site"]);
                    if ((int?)o["rule"] > 0) t0Sites.Add(site);
                    tape.Add(new TapeEntry { Index = tape.Count, Key = site, Member = (string)o["m"], Args = o["a"]?.ToString(Newtonsoft.Json.Formatting.None), Value = o["v"], Ctx = (string)o["ctx"], Tick = (int?)o["t"] ?? -1 });
                }
                else if (Sched.Window && string.CompareOrdinal(line, k + 5, "rule\"", 0, 5) == 0)
                {
                    // Root rules (depth 0) only: a replay must push the same ones (Sched.CheckRoot).
                    if (line.IndexOf("\"depth\":0,", StringComparison.Ordinal) < 0) continue;
                    var o = JObject.Parse(line);
                    if (o["depth"]?.Type == JTokenType.Integer && (int)o["depth"] == 0) Sched.TapeRoot(o);
                }
                else if (Sched.Window && string.CompareOrdinal(line, k + 5, "end\"", 0, 4) == 0)
                {
                    var o = JObject.Parse(line);
                    if (o["paths_tracked"] != null) PathSync.SetRecordedTracked((int)o["paths_tracked"]);
                }
                else if (Sched.Window && (string.CompareOrdinal(line, k + 5, "obs\"", 0, 4) == 0 || string.CompareOrdinal(line, k + 5, "input\"", 0, 6) == 0))
                {
                    // The kind comes from the matched prefix: a record's own fields may repeat "k", and
                    // JObject.Parse keeps the last duplicate (the path_ret records of poc5-tb-19 repeat it).
                    var o = JObject.Parse(line);
                    Sched.TapeRecord(line[k + 5] == 'o' ? "obs" : "input", o);
                }
            }
        }
        PerturbNote = null;
        // A camera-gate perturbation (scroll#K:flip) damages that tape instead of a draw (CamPin.Perturb).
        if (CamPin.Perturb(perturb)) { }
        else if (!string.IsNullOrEmpty(perturb))
        {
            var parts = perturb.Split(':');
            // N is a tape index, or "first-sim-d20": the first Range(1,21) draw (a d20).
            // or "d20#K": the K-th such draw (1-based).
            int n;
            if (parts[0] == "first-sim-d20") n = tape.FindIndex(t => t.Member == "Range_ii" && t.Args == "[1,21]");
            else if (parts[0].StartsWith("d20#", StringComparison.Ordinal))
            {
                var k = int.Parse(parts[0].Substring(4), CultureInfo.InvariantCulture);
                var d20s = tape.Where(t => t.Member == "Range_ii" && t.Args == "[1,21]").ToList();
                n = k >= 1 && k <= d20s.Count ? d20s[k - 1].Index : -1;
            }
            else n = int.Parse(parts[0], CultureInfo.InvariantCulture);
            var d = int.Parse(parts[1], CultureInfo.InvariantCulture);
            if (n >= 0 && n < tape.Count && tape[n].Value.Type == JTokenType.Integer)
            {
                var e = tape[n];
                var old = (int)e.Value;
                var args = JArray.Parse(e.Args ?? "[]");
                var nv = old + d;
                // Keep a perturbed Range(int) value inside its range (wrap), so it stays legal.
                if (e.Member == "Range_ii" && args.Count == 2)
                {
                    int lo = (int)args[0], hi = (int)args[1];
                    if (hi > lo) nv = lo + (((nv - lo) % (hi - lo)) + (hi - lo)) % (hi - lo);
                }
                e.Value = new JValue(nv);
                tape[n] = e;
                PerturbNote = $"draw {n} ({e.Key}) {old} -> {nv}";
            }
            else PerturbNote = $"perturbation {perturb} not applicable (tape has {tape.Count} draws)";
        }
        _tape = tape;
        _t0Sites = t0Sites;
        _tapePos = 0;
        _bySite = new Dictionary<string, Queue<TapeEntry>>();
        foreach (var e in tape)
        {
            if (!_bySite.TryGetValue(e.Key, out var q)) _bySite[e.Key] = q = new Queue<TapeEntry>();
            q.Enqueue(e);
        }
        ReplayOutOfOrder = 0;
        OracleTime.Load(timeRaw, tickUnscaled);
    }

    public static void UnloadTape()
    {
        _tape = null;
        _t0Sites = null;
        OracleTime.Unload();
    }

    // Each site replays its own recorded values in order (a per-site queue), so a draw that
    // happens a different number of times at one site (e.g. animation offsets drawn when unit
    // views activate) can't shift every other site's values. Whether the draws also come in
    // the recorded global order is counted separately (ReplayOutOfOrder) and checked by the
    // trace diff.
    private static Dictionary<string, Queue<TapeEntry>> _bySite;
    public static long ReplayOutOfOrder;

    public static int TapeLeftover => _bySite?.Values.Sum(q => q.Count) ?? 0;

    public static IEnumerable<KeyValuePair<string, int>> TapeLeftoverBySite() =>
        _bySite == null ? Enumerable.Empty<KeyValuePair<string, int>>() : _bySite.Where(kv => kv.Value.Count > 0).Select(kv => new KeyValuePair<string, int>(kv.Key, kv.Value.Count)).ToList();

    // ctx: the semantic context of a draw made inside a rule (turn-based windows), or null.
    private static bool TryTape(Site site, string member, string args, string ctx, out JToken value, out bool match)
    {
        value = null;
        match = false;
        var t0 = Sched.Window && (ctx != null || (_t0Sites != null && _t0Sites.Contains(site.Key)));
        if (!_bySite.TryGetValue(site.Key, out var q) || q.Count == 0)
        {
            Interlocked.Increment(ref ReplayExhausted);
            if (t0) Sched.Invalidate($"T0 draw with no recorded value: site {site.Key} {member} {args}{(ctx != null ? " in " + ctx : "")} at tick {Trace.Tick}");
            return false;
        }
        var e = q.Dequeue();
        if (e.Index != _tapePos) Interlocked.Increment(ref ReplayOutOfOrder);
        _tapePos = e.Index + 1;
        match = e.Key == site.Key && e.Member == member && e.Args == args;
        if (!match) Interlocked.Increment(ref ReplayMismatches);
        value = e.Value;
        if (t0 && Sched.Invalid == null)
        {
            if (!match) Sched.Invalidate($"T0 draw does not match the tape: site {site.Key} asked {member} {args}, recorded {e.Member} {e.Args} (recorded at tick {e.Tick}, now tick {Trace.Tick}){(ctx != null ? " in " + ctx : "")}");
            else if (e.Ctx != ctx) Sched.Invalidate($"T0 draw taken by another rule: site {site.Key} {member} {args} wanted by {ctx ?? "no rule"}, recorded for {e.Ctx ?? "no rule"} (recorded at tick {e.Tick}, now tick {Trace.Tick})");
        }
        return true;
    }

    // ---- the draw ----

    private static string ArgsText(params object[] a)
    {
        var sw = new StringWriter(CultureInfo.InvariantCulture);
        using (var j = new Json(sw))
        {
            j.W.WriteStartArray();
            foreach (var x in a) Snap.Value(j, x, Snap.Mode.Fields);
            j.W.WriteEndArray();
        }
        return sw.ToString();
    }

    private static T Draw<T>(int siteId, string member, Func<T> real, Func<JToken, T> fromTape, params object[] args)
    {
        var site = siteId >= 0 && siteId < Sites.Count ? Sites[siteId] : null;
        return DrawAt(site, member, real, fromTape, args);
    }

    private static T DrawAt<T>(Site site, string member, Func<T> real, Func<JToken, T> fromTape, params object[] args)
    {
        if (!Trace.Active && !ReplayActive) return Real(real);
        var main = Trace.OnMainThread;
        if (!main) Interlocked.Increment(ref OffThreadDraws);
        var before = Now();
        var gap = Gap(before);
        var argsText = ArgsText(args);
        T v;
        var forced = false;
        var match = true;
        // In turn-based windows a weighted pick made by the bark scheduler is presentation: the eligible set
        // follows the audio engine's state (a bark still playing is not eligible), a record's [0,1] being a
        // replay's [0,0] with nothing else different. Other callers of the picker stay on the tape.
        var pres = site != null && (site.Presentation || (Sched.Window && site.Key.Contains(BarkPicker) && IsBarkCall()));
        var fallback = false;
        // Turn-based windows: a draw inside a rule carries the rule's class, initiator, target
        // and the draw's ordinal within the rule, so the tape can tell whose dice these are.
        var ctx = Sched.Window && main && !pres ? Rules.DrawContext() : null;
        if (ReplayActive && site != null && !pres)
        {
            // Strict: a sim draw never uses the real generator in a replay. A missing or
            // mismatching tape entry gives a fixed fallback value (the range minimum, zero,
            // identity) and is counted; such a replay is invalid (health: replay_tape_exhausted,
            // replay_mismatches).
            if (TryTape(site, member, argsText, ctx, out var tv, out match) && match)
            {
                v = fromTape(tv);
                forced = true;
            }
            else
            {
                v = Fallback<T>(args);
                fallback = true;
            }
        }
        else
        {
            v = Real(real);
        }
        var after = Now();
        _lastAfter = after;
        _haveLast = true;
        Interlocked.Increment(ref TotalDraws);
        if (site != null) Interlocked.Increment(ref site.Draws);
        var steps = forced || fallback ? 0 : StepsBetween(before, after, 64);
        var vv = v;
        Trace.Emit("draw", j =>
        {
            j.Prop("site", site?.Id ?? -1);
            if (site != null && site.Id < 0) j.Prop("key", site.Key);
            j.Prop("c", pres ? "pres" : "sim");
            j.Prop("m", member);
            j.Key("a"); j.W.WriteRawValue(argsText);
            j.Key("v"); Snap.Value(j, vv, Snap.Mode.Fields);
            j.Prop("rule", main ? Rules.CurrentRuleId : 0);
            j.Prop("h", main ? Rules.CurrentHandlerId : 0);
            if (ctx != null) j.Prop("ctx", ctx);
            j.Prop("gap", gap);
            j.Prop("steps", steps);
            j.Prop("st", before.ToString());
            if (ReplayActive && !pres) { j.Prop("forced", forced); j.Prop("match", match); if (fallback) j.Prop("fallback", true); }
        });
        return v;
    }

    private static T Fallback<T>(object[] args)
    {
        object v;
        if (typeof(T) == typeof(int)) v = args.Length > 0 ? (int)args[0] : 0;
        else if (typeof(T) == typeof(float)) v = args.Length > 0 && args[0] is float f ? f : 0f;
        else if (typeof(T) == typeof(Quaternion)) v = Quaternion.identity;
        else v = default(T);
        return (T)v;
    }

    private static T Real<T>(Func<T> real)
    {
        var was = _inWrapper;
        _inWrapper = true;
        try { return real(); }
        finally { _inWrapper = was; }
    }

    // ---- wrappers called from transpiled sites (last parameter: site id) ----

    public static int W_Range_ii(int min, int max, int site) => !Trace.Active && !ReplayActive ? URandom.Range(min, max) : Draw(site, "Range_ii", () => URandom.Range(min, max), t => (int)t, min, max);
    public static float W_Range_ff(float min, float max, int site) => !Trace.Active && !ReplayActive ? URandom.Range(min, max) : Draw(site, "Range_ff", () => URandom.Range(min, max), t => (float)t, min, max);
    public static int W_RandomRange_ii(int min, int max, int site) => W_Range_ii(min, max, site);
    public static float W_RandomRange_ff(float min, float max, int site) => W_Range_ff(min, max, site);
    public static float W_value(int site) => !Trace.Active && !ReplayActive ? URandom.value : Draw(site, "value", () => URandom.value, t => (float)t);
    public static Vector3 W_insideUnitSphere(int site) => !Trace.Active && !ReplayActive ? URandom.insideUnitSphere : Draw(site, "insideUnitSphere", () => URandom.insideUnitSphere, V3);
    public static Vector3 W_onUnitSphere(int site) => !Trace.Active && !ReplayActive ? URandom.onUnitSphere : Draw(site, "onUnitSphere", () => URandom.onUnitSphere, V3);
    public static Vector2 W_insideUnitCircle(int site) => !Trace.Active && !ReplayActive ? URandom.insideUnitCircle : Draw(site, "insideUnitCircle", () => URandom.insideUnitCircle, V2);
    public static Quaternion W_rotation(int site) => !Trace.Active && !ReplayActive ? URandom.rotation : Draw(site, "rotation", () => URandom.rotation, Q);
    public static Quaternion W_rotationUniform(int site) => !Trace.Active && !ReplayActive ? URandom.rotationUniform : Draw(site, "rotationUniform", () => URandom.rotationUniform, Q);
    public static Color W_ColorHSV0(int site) => Draw(site, "ColorHSV", () => URandom.ColorHSV(), C);
    public static Color W_ColorHSV2(float a, float b, int site) => Draw(site, "ColorHSV", () => URandom.ColorHSV(a, b), C, a, b);
    public static Color W_ColorHSV4(float a, float b, float c, float d, int site) => Draw(site, "ColorHSV", () => URandom.ColorHSV(a, b, c, d), C, a, b, c, d);
    public static Color W_ColorHSV6(float a, float b, float c, float d, float e, float f, int site) => Draw(site, "ColorHSV", () => URandom.ColorHSV(a, b, c, d, e, f), C, a, b, c, d, e, f);
    public static Color W_ColorHSV8(float a, float b, float c, float d, float e, float f, float g, float h, int site) => Draw(site, "ColorHSV", () => URandom.ColorHSV(a, b, c, d, e, f, g, h), C, a, b, c, d, e, f, g, h);

    // State changes are recorded (not replayed): they reseed or restore the generator.
    public static void W_InitState(int seed, int site) { GapBeforeReseed(); Note(site, "InitState", seed.ToString(CultureInfo.InvariantCulture)); URandom.InitState(seed); _lastAfter = Now(); }
    public static URandom.State W_get_state(int site) => URandom.state;
    public static void W_set_state(URandom.State s, int site) { GapBeforeReseed(); URandom.state = s; Note(site, "set_state", Now().ToString()); _lastAfter = Now(); }
    public static int W_get_seed(int site) => URandom.seed;
    public static void W_set_seed(int s, int site) { GapBeforeReseed(); Note(site, "set_seed", s.ToString(CultureInfo.InvariantCulture)); URandom.seed = s; _lastAfter = Now(); }

    // An input (SeedInit): the generator's state set, noted like the game's own reseeds.
    public static void SetState(S4 s, string what)
    {
        GapBeforeReseed();
        object box = default(URandom.State);
        var names = new[] { "s0", "s1", "s2", "s3" };
        var v = new[] { s.X, s.Y, s.Z, s.W };
        for (var i = 0; i < 4; i++)
        {
            var f = typeof(URandom.State).GetField(names[i], BindingFlags.Instance | BindingFlags.NonPublic);
            f.SetValue(box, f.FieldType == typeof(uint) ? (object)v[i] : unchecked((int)v[i]));
        }
        URandom.state = (URandom.State)box;
        Note(-2, what, Now().ToString());
        _lastAfter = Now();
    }

    // Unrecorded consumption since the last hooked draw, accounted before game code reseeds or
    // restores the generator (draws whose state is later restored stay invisible to the gap).
    private static void GapBeforeReseed()
    {
        if (Trace.Active) Gap(Now());
    }

    private static void Note(int site, string what, string arg)
    {
        if (!Trace.Active) return;
        Trace.Emit("rng_state", j => { j.Prop("site", site); j.Prop("m", what); j.Prop("arg", arg); });
    }

    private static Vector3 V3(JToken t) => new Vector3((float)t[0], (float)t[1], (float)t[2]);
    private static Vector2 V2(JToken t) => new Vector2((float)t[0], (float)t[1]);
    private static Quaternion Q(JToken t) => new Quaternion((float)t[0], (float)t[1], (float)t[2], (float)t[3]);
    private static Color C(JToken t) => new Color((float)t[0], (float)t[1], (float)t[2], (float)t[3]);

    private static readonly Dictionary<MethodBase, MethodInfo> WrapperOf = new Dictionary<MethodBase, MethodInfo>();

    private static void MapWrappers()
    {
        var R = typeof(URandom);
        void Map(MethodBase original, string wrapper)
        {
            if (original == null) return;
            WrapperOf[original] = typeof(Rng).GetMethod(wrapper, BindingFlags.Static | BindingFlags.Public);
        }
        Map(R.GetMethod("Range", new[] { typeof(int), typeof(int) }), nameof(W_Range_ii));
        Map(R.GetMethod("Range", new[] { typeof(float), typeof(float) }), nameof(W_Range_ff));
        Map(R.GetMethod("RandomRange", new[] { typeof(int), typeof(int) }), nameof(W_RandomRange_ii));
        Map(R.GetMethod("RandomRange", new[] { typeof(float), typeof(float) }), nameof(W_RandomRange_ff));
        Map(R.GetProperty("value")?.GetGetMethod(), nameof(W_value));
        Map(R.GetProperty("insideUnitSphere")?.GetGetMethod(), nameof(W_insideUnitSphere));
        Map(R.GetProperty("onUnitSphere")?.GetGetMethod(), nameof(W_onUnitSphere));
        Map(R.GetProperty("insideUnitCircle")?.GetGetMethod(), nameof(W_insideUnitCircle));
        Map(R.GetProperty("rotation")?.GetGetMethod(), nameof(W_rotation));
        Map(R.GetProperty("rotationUniform")?.GetGetMethod(), nameof(W_rotationUniform));
        Map(R.GetMethod("ColorHSV", Type.EmptyTypes), nameof(W_ColorHSV0));
        Map(R.GetMethod("ColorHSV", Enumerable.Repeat(typeof(float), 2).ToArray()), nameof(W_ColorHSV2));
        Map(R.GetMethod("ColorHSV", Enumerable.Repeat(typeof(float), 4).ToArray()), nameof(W_ColorHSV4));
        Map(R.GetMethod("ColorHSV", Enumerable.Repeat(typeof(float), 6).ToArray()), nameof(W_ColorHSV6));
        Map(R.GetMethod("ColorHSV", Enumerable.Repeat(typeof(float), 8).ToArray()), nameof(W_ColorHSV8));
        Map(R.GetMethod("InitState", new[] { typeof(int) }), nameof(W_InitState));
        Map(R.GetProperty("state")?.GetGetMethod(), nameof(W_get_state));
        Map(R.GetProperty("state")?.GetSetMethod(), nameof(W_set_state));
        Map(R.GetProperty("seed")?.GetGetMethod(), nameof(W_get_seed));
        Map(R.GetProperty("seed")?.GetSetMethod(), nameof(W_set_seed));
    }

    // ---- scanning ----

    internal static readonly OpCode[] One = new OpCode[0x100];
    internal static readonly OpCode[] Two = new OpCode[0x100];

    internal static void BuildOpcodeTable()
    {
        foreach (var f in typeof(OpCodes).GetFields(BindingFlags.Public | BindingFlags.Static))
        {
            var op = (OpCode)f.GetValue(null);
            var v = (ushort)op.Value;
            if (v < 0x100) One[v] = op;
            else if ((v & 0xff00) == 0xfe00) Two[v & 0xff] = op;
        }
    }

    public static string ScanNote;
    public static double ScanSeconds, PatchSeconds;
    public static int MethodsScanned, AssembliesScanned;

    internal static bool IsGameAssembly(Assembly a, string managed)
    {
        string loc;
        if (a.IsDynamic) return false;
        try { loc = a.Location; } catch (Exception) { return false; }
        if (string.IsNullOrEmpty(loc) || Path.GetDirectoryName(loc) != managed) return false;
        var n = a.GetName().Name;
        return !(n.StartsWith("System") || n == "mscorlib" || n.StartsWith("Mono.") || n == "netstandard" || n.StartsWith("UnityEngine") ||
                 n == "0Harmony" || n.StartsWith("I18N") || n == "Newtonsoft.Json" || n.StartsWith("Microsoft."));
    }

    public static void Scan()
    {
        BuildOpcodeTable();
        MapWrappers();
        var sw = Stopwatch.StartNew();
        var managed = Path.GetDirectoryName(typeof(Kingmaker.Game).Assembly.Location);
        var found = new List<Site>();
        foreach (var a in AppDomain.CurrentDomain.GetAssemblies().Where(x => IsGameAssembly(x, managed)).OrderBy(x => x.GetName().Name, StringComparer.Ordinal))
        {
            AssembliesScanned++;
            Type[] types;
            try { types = a.GetTypes(); } catch (ReflectionTypeLoadException e) { types = e.Types.Where(t => t != null).ToArray(); }
            catch (Exception) { continue; }
            var tokenCache = new Dictionary<int, MethodBase>();
            var module = a.ManifestModule;
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
                    MethodsScanned++;
                    var ordinal = 0;
                    var pos = 0;
                    while (pos < il.Length)
                    {
                        var start = pos;
                        OpCode op;
                        var b = il[pos++];
                        if (b == 0xfe) { if (pos >= il.Length) break; op = Two[il[pos++]]; }
                        else op = One[b];
                        if (op.Size == 0) break; // unknown opcode: stop scanning this body
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
                        if (op.Value != OpCodes.Call.Value && op.Value != OpCodes.Callvirt.Value) continue;
                        var table = (uint)operand >> 24;
                        if (table != 0x0A) continue; // UnityEngine.Random members are MemberRefs from these assemblies
                        if (!tokenCache.TryGetValue(operand, out var callee))
                        {
                            try { callee = module.ResolveMethod(operand); } catch (Exception) { callee = null; }
                            tokenCache[operand] = callee;
                        }
                        if (callee == null || callee.DeclaringType != typeof(URandom)) continue;
                        var generic = m.ContainsGenericParameters || (m.DeclaringType?.ContainsGenericParameters ?? false);
                        var site = new Site
                        {
                            Assembly = a.GetName().Name,
                            Type = m.DeclaringType?.FullName,
                            Method = Sig(m),
                            Member = MemberName(callee),
                            Ordinal = ordinal++,
                            IlOffset = start,
                            Generic = generic,
                            StaticCtor = m.IsConstructor && m.IsStatic,
                            Target = m,
                            Presentation = IsPresentation(m.DeclaringType?.FullName),
                        };
                        site.Key = $"{site.Assembly}|{site.Type}|{site.Method}|#{site.Ordinal}@{site.IlOffset}|{site.Member}";
                        found.Add(site);
                    }
                }
            }
        }
        found.Sort((x, y) => string.CompareOrdinal(x.Key, y.Key));
        for (var i = 0; i < found.Count; i++) found[i].Id = i;
        Sites.AddRange(found);
        foreach (var s in found)
        {
            if (!SitesByMethod.TryGetValue(s.Target, out var list)) SitesByMethod[s.Target] = list = new List<Site>();
            list.Add(s);
        }
        // The transpiler indexes each method's sites by ordinal: numeric order, not key order.
        foreach (var list in SitesByMethod.Values) list.Sort((x, y) => x.Ordinal.CompareTo(y.Ordinal));
        ScanSeconds = sw.Elapsed.TotalSeconds;
        ScanNote = $"{found.Count} sites in {SitesByMethod.Count} methods; {MethodsScanned} methods in {AssembliesScanned} assemblies scanned in {ScanSeconds:0.00} s";
    }

    private static string Sig(MethodBase m)
    {
        var ps = m.GetParameters().Select(p => p.ParameterType.Name);
        var gen = m.IsGenericMethodDefinition ? "`" + m.GetGenericArguments().Length : "";
        return m.Name + gen + "(" + string.Join(",", ps) + ")";
    }

    private static string MemberName(MethodBase m)
    {
        var ps = m.GetParameters();
        if (m.Name == "Range" || m.Name == "RandomRange")
            return m.Name + (ps.Length == 2 && ps[0].ParameterType == typeof(int) ? "(int,int)" : "(float,float)");
        if (m.Name == "ColorHSV") return "ColorHSV/" + ps.Length;
        return m.Name;
    }

    // ---- patching ----

    public static int MethodsTranspiled, TranspileFailures;

    // Generic methods whose Random calls the global net can't see (Range(float,float) and value
    // are native, so they can't take a prefix), transpiled through one closed instantiation.
    // Mono compiles one shared body for all reference-type instantiations, so this covers every
    // reference-type caller (a value-type one needs its own entry). LinqExtensions.WeightedRandom<T> picks unit barks
    // (UnitAsksComponent.Bark.Schedule), roaming waypoints, mercenaries and camp banters.
    private static readonly (string Type, string Method, Type[] Args)[] ClosedGenerics =
    {
        ("Kingmaker.Utility.LinqExtensions", "WeightedRandom", new[] { typeof(Kingmaker.Visual.Sound.UnitAsksComponent.BarkEntry) }),
        // A value-type instantiation has a body of its own: UnitEntityView.GetDismemberType's weighted pick of the
        // dismember type (DeathFxPin.cs), a simulation draw (its result adds a buff or not).
        ("Kingmaker.Utility.RandomUtils", "GetRandomValue", new[] { typeof(Kingmaker.UnitLogic.UnitState.DismemberType) }),
    };

    private static MethodInfo ClosedFor(MethodBase m)
    {
        if (!(m is MethodInfo mi) || !mi.IsGenericMethodDefinition || (mi.DeclaringType?.ContainsGenericParameters ?? true)) return null;
        foreach (var (type, method, args) in ClosedGenerics)
            if (mi.DeclaringType.FullName == type && mi.Name == method && mi.GetGenericArguments().Length == args.Length)
                return mi.MakeGenericMethod(args);
        return null;
    }

    public static void Install(Harmony h)
    {
        var sw = Stopwatch.StartNew();
        var tr = new HarmonyMethod(typeof(Rng), nameof(Transpiler));
        foreach (var kv in SitesByMethod.ToList())
        {
            var m = kv.Key;
            var sites = kv.Value;
            if (sites[0].Generic)
            {
                MethodInfo closed = null;
                try { closed = ClosedFor(m); }
                catch (Exception e) { Health.Failed("close generic " + sites[0].Type + "." + sites[0].Method, e); }
                if (closed != null)
                {
                    // The transpiler finds the sites by the method it is given: the closed one.
                    SitesByMethod[closed] = sites;
                    try
                    {
                        h.Patch(closed, transpiler: tr);
                        MethodsTranspiled++;
                        foreach (var s in sites) s.Note = "generic method: transpiled via " + string.Join(",", closed.GetGenericArguments().Select(t => t.Name));
                    }
                    catch (Exception e)
                    {
                        TranspileFailures++;
                        foreach (var s in sites) s.Note = "transpile failed: " + e.Message;
                        Health.Failed("transpile " + sites[0].Type + "." + sites[0].Method, e);
                    }
                    continue;
                }
            }
            if (sites[0].Generic || sites[0].StaticCtor)
            {
                foreach (var s in sites) s.Note = s.Generic ? "generic method: global hook only" : "static constructor: not patched";
                continue;
            }
            if (DisableSiteContaining != null && sites.Any(s => s.Key.Contains(DisableSiteContaining)))
            {
                foreach (var s in sites) { s.Disabled = true; s.Note = "disabled by fault injection"; }
                continue;
            }
            try
            {
                h.Patch(m, transpiler: tr);
                MethodsTranspiled++;
            }
            catch (Exception e)
            {
                TranspileFailures++;
                foreach (var s in sites) s.Note = "transpile failed: " + e.Message;
                Health.Failed("transpile " + sites[0].Type + "." + sites[0].Method, e);
            }
        }
        // The global net for managed members (callers we didn't transpile).
        var R = typeof(URandom);
        h.Patch(R.GetMethod("Range", new[] { typeof(int), typeof(int) }), prefix: new HarmonyMethod(typeof(Rng), nameof(GlobalRangeInt)));
        foreach (var (prop, pre) in new[] { ("insideUnitSphere", nameof(GlobalV3a)), ("onUnitSphere", nameof(GlobalV3b)), ("insideUnitCircle", nameof(GlobalV2)), ("rotation", nameof(GlobalQa)), ("rotationUniform", nameof(GlobalQb)) })
        {
            try { h.Patch(R.GetProperty(prop).GetGetMethod(), prefix: new HarmonyMethod(typeof(Rng), pre)); }
            catch (Exception e) { Health.Failed("global hook " + prop, e); }
        }
        PatchSeconds = sw.Elapsed.TotalSeconds;
    }

    private static IEnumerable<CodeInstruction> Transpiler(IEnumerable<CodeInstruction> instructions, MethodBase original)
    {
        SitesByMethod.TryGetValue(original, out var sites);
        var list = instructions.ToList();
        var ordinal = 0;
        for (var i = 0; i < list.Count; i++)
        {
            var ins = list[i];
            if ((ins.opcode != OpCodes.Call && ins.opcode != OpCodes.Callvirt) || !(ins.operand is MethodBase mb) || mb.DeclaringType != typeof(URandom)) continue;
            var site = sites != null && ordinal < sites.Count ? sites[ordinal] : null;
            ordinal++;
            if (site != null && MemberName(mb) != site.Member)
            {
                site.Note = $"IL changed: expected {site.Member}, found {MemberName(mb)}; not replaced";
                continue;
            }
            if (site == null || !WrapperOf.TryGetValue(mb, out var wrapper) || wrapper == null)
            {
                if (site != null) site.Note = "no wrapper for " + mb.Name;
                continue;
            }
            // Replace "call Random.X" by "ldc.i4 id; call Rng.W_X": the id instruction takes over
            // the call's labels and exception blocks.
            // Block markers: beginnings go on the first replacement instruction, the end of a
            // block on the last (Harmony closes a block after the instruction carrying the end).
            var begins = ins.blocks.Where(b => b.blockType != ExceptionBlockType.EndExceptionBlock).ToList();
            var ends = ins.blocks.Where(b => b.blockType == ExceptionBlockType.EndExceptionBlock).ToList();
            var ld = new CodeInstruction(OpCodes.Ldc_I4, site.Id) { labels = ins.labels, blocks = begins };
            list[i] = ld;
            list.Insert(i + 1, new CodeInstruction(OpCodes.Call, wrapper) { blocks = ends });
            i++;
            site.Transpiled = true;
        }
        if (sites != null && ordinal != sites.Count)
        {
            Health.Failed("transpile " + original.DeclaringType?.FullName + "." + original.Name, new Exception($"found {ordinal} Random calls, scan found {sites.Count}"));
            foreach (var s in sites) s.Note = (s.Note ?? "") + $" (IL now has {ordinal} calls, scan found {sites.Count})";
        }
        return list;
    }

    // ---- the global net ----

    private static Site CallerSite(string member)
    {
        // Frames: 0 this, 1 the Global* prefix, 2 the patched Random member, 3+ the caller.
        string key;
        string callerType = null;
        try
        {
            var st = new StackTrace(2, false);
            MethodBase caller = null;
            string via = null;
            for (var i = 0; i < st.FrameCount && i < 12; i++)
            {
                var m = FrameMethod(st.GetFrame(i));
                if (m == null || m.DeclaringType == typeof(URandom) || m.DeclaringType == typeof(Rng)) continue;
                var ns = m.DeclaringType?.Namespace ?? "";
                if (ns.StartsWith("HarmonyLib") || ns.StartsWith("MonoMod") || m.Name.Contains("_Patch")) continue;
                // Generic helpers (LinqExtensions.Random, RandomUtils...): name the code that
                // called them, which says what the draw is for.
                var tn = m.DeclaringType?.FullName ?? "";
                if (tn == "Kingmaker.Utility.LinqExtensions" || tn == "Kingmaker.Utility.RandomUtils" || tn.StartsWith("Kingmaker.Utility.LinqExtensions+"))
                {
                    via = via ?? tn.Split('.').Last() + "." + m.Name;
                    continue;
                }
                caller = m;
                break;
            }
            var viaText = via != null ? "|via " + via : "";
            key = caller == null ? "global|?|" + member + viaText : $"global|{caller.DeclaringType?.Assembly.GetName().Name}|{caller.DeclaringType?.FullName}|{Sig(caller)}{viaText}|{member}";
            callerType = caller?.DeclaringType?.FullName;
        }
        catch (Exception)
        {
            key = "global|?|" + member;
        }
        lock (GlobalSites)
        {
            if (!GlobalSites.TryGetValue(key, out var s))
                GlobalSites[key] = s = new Site { Id = -1, Key = key, Member = member, Note = "global hook", Presentation = IsPresentation(callerType) };
            return s;
        }
    }

    public static IEnumerable<Site> GlobalSiteList()
    {
        lock (GlobalSites) return GlobalSites.Values.ToList();
    }

    private static bool Global<T>(string member, Func<T> real, Func<JToken, T> fromTape, ref T result, params object[] args)
    {
        if (_inWrapper || (!Trace.Active && !ReplayActive)) return true;
        Interlocked.Increment(ref GlobalHookDraws);
        result = DrawAt(CallerSite(member), member, real, fromTape, args);
        return false;
    }

    private static bool GlobalRangeInt(int minInclusive, int maxExclusive, ref int __result) =>
        Global("Range_ii", () => URandom.Range(minInclusive, maxExclusive), t => (int)t, ref __result, minInclusive, maxExclusive);
    private static bool GlobalV3a(ref Vector3 __result) => Global("insideUnitSphere", () => URandom.insideUnitSphere, V3, ref __result);
    private static bool GlobalV3b(ref Vector3 __result) => Global("onUnitSphere", () => URandom.onUnitSphere, V3, ref __result);
    private static bool GlobalV2(ref Vector2 __result) => Global("insideUnitCircle", () => URandom.insideUnitCircle, V2, ref __result);
    private static bool GlobalQa(ref Quaternion __result) => Global("rotation", () => URandom.rotation, Q, ref __result);
    private static bool GlobalQb(ref Quaternion __result) => Global("rotationUniform", () => URandom.rotationUniform, Q, ref __result);

    // The site table, for traces and rng-sites.json.
    public static void WriteSites(Json j, bool compact)
    {
        j.W.WriteStartArray();
        foreach (var s in Sites)
        {
            if (compact)
            {
                j.W.WriteStartArray(); j.W.WriteValue(s.Id); j.W.WriteValue(s.Key); j.W.WriteValue(s.Presentation ? "pres" : "sim"); j.W.WriteEndArray();
                continue;
            }
            j.Obj();
            j.Prop("id", s.Id);
            j.Prop("key", s.Key);
            j.Prop("assembly", s.Assembly);
            j.Prop("type", s.Type);
            j.Prop("method", s.Method);
            j.Prop("member", s.Member);
            j.Prop("il_offset", s.IlOffset);
            j.Prop("transpiled", s.Transpiled);
            j.Prop("generic", s.Generic);
            j.Prop("static_ctor", s.StaticCtor);
            j.Prop("disabled", s.Disabled);
            j.Prop("presentation", s.Presentation);
            j.Prop("note", s.Note);
            j.Prop("draws", s.Draws);
            j.End();
        }
        j.W.WriteEndArray();
    }
}

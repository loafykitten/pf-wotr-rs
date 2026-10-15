using System;
using System.Collections.Generic;
using System.Linq;
using HarmonyLib;
using Kingmaker.Controllers.Projectiles;
using Newtonsoft.Json.Linq;

namespace WrathOracle;

// Projectile arrival in turn-based windows: observed in a recording, pinned in a replay
// (docs/poc5/tb-replay.md, "Projectile arrival and critter paths").
//
// Projectile.Tick moves a projectile toward its target's bone (a live, animated transform) and sets IsHit
// once it is within 0.1 m; ProjectileHitController.Tick calls OnHit in the same game tick, and OnHit's
// trigger or callback makes the rules (RuleAttackWithWeaponResolve, the damage) [C]. Bone poses differ
// between runs by millimetres to centimetres (T1, Ember 2026-10-11), so the flight can take one step more or
// less: poc5-tb-119 tb-lizard, Anevia's arrow launched at tick 6918 in every run with the same snapshot
// hit at 6938 in the recording and 6937 in both replays [M], and the fight's end moved with it. Everything
// the game derives from a projectile's flight goes through IsHit (OnHit, HasLaunchedProjectile and
// HasActive, which hold turns and commands, OnTheWay, cleanup) [C], so the tick IsHit becomes true is what
// is pinned; the flight itself (position, view) stays native.
//
// Recording (nothing changes): every projectile added to the controller in the window that can matter is
// keyed "<launcher unit>#<n>" (n: that launcher's launches in the window; "pt" for a launcher that isn't a
// unit) and noted at launch ("proj_launch": the open rule, the attack roll's rule, the on-hit trigger's
// class, whether it has a callback, the target unit, the blueprint, the attack result) and at the tick its
// IsHit became true ("proj_hit"). A projectile the two decorative launchers make (ProjectileSpawner.Fire:
// a scene's ambient spawner, random targets, always a miss; FxProjectileLauncher.LaunchProjectile: a scene
// effect on a game-time or real-time delay) is presentation, identified by its launch inside them: noted with
// "pres", never pinned. Every other projectile is pinned, an ability's too (AbilityDeliverProjectile, Touch,
// Chain, Ricochet... poll IsHit, and set the attack roll and range after the launch).
// Replay: a launch must be the recording's (same key and contents, within the scheduler's tolerance of
// the recorded tick); its hit is due the recorded number of ticks after this replay's own launch. Before
// then a hit the native flight makes is taken back (IsHit, and IsHitWall if that tick set it, cleared
// right after Projectile.Tick, before anything reads them); on the due tick IsHit is set if the flight
// hasn't got there. A recorded projectile that never hit in its window doesn't hit in the replay's either.
// Any departure invalidates the replay: a launch the recording didn't make or made differently, a
// recorded launch that didn't happen, a due hit that didn't happen (the projectile gone or not ticked),
// OnHit on another tick than the hit, an unpinned projectile hitting inside the window.
internal static class ProjPin
{
    public static bool Installed;
    // The recording declared this observer (its trace's begin record), read with its tape.
    public static bool RecordedOn;
    public static long Tracked, HeldBack, Forced;

    private sealed class Track
    {
        public string K;
        public int Launch, Due = -1, Hit = -1, Held;
        public bool OnHit, IsForced;
    }

    private sealed class Rec { public int Launch, Hit = -1; public string Desc; public bool Matched, Wall; public float Passed; }

    private static readonly Dictionary<Projectile, Track> Tracks = new Dictionary<Projectile, Track>(new RefEq<Projectile>());
    private static readonly List<Track> Pending = new List<Track>(); // replay: a due hit not made yet
    private static readonly Dictionary<string, int> PerLauncher = new Dictionary<string, int>();
    private static readonly Dictionary<string, Rec> Tape = new Dictionary<string, Rec>();
    private static readonly HashSet<Projectile> Untracked = new HashSet<Projectile>(new RefEq<Projectile>());

    private static readonly AccessTools.FieldRef<Projectile, bool> IsHitRef = AccessTools.FieldRefAccess<Projectile, bool>("<IsHit>k__BackingField");
    private static readonly AccessTools.FieldRef<Projectile, bool> IsHitWallRef = AccessTools.FieldRefAccess<Projectile, bool>("<IsHitWall>k__BackingField");
    private static readonly AccessTools.FieldRef<Projectile, float> PassedDistanceRef = AccessTools.FieldRefAccess<Projectile, float>("PassedDistance");
    private static readonly AccessTools.FieldRef<Projectile, Kingmaker.RuleSystem.Rules.RuleAttackRoll> AttackRollRef =
        AccessTools.FieldRefAccess<Projectile, Kingmaker.RuleSystem.Rules.RuleAttackRoll>("m_AttackRoll");

    public static void ResetWindow()
    {
        Tracks.Clear();
        Pending.Clear();
        PerLauncher.Clear();
        Tape.Clear();
        Untracked.Clear();
        Pres.Clear();
        RecordedOn = false;
        Tracked = HeldBack = Forced = 0;
    }

    private static bool On => Installed && Sched.On;

    // ---- the tape (Sched.TapeRecord, from the recording's trace) ----

    // What a launch is, the same text from the projectile and from its record.
    private static string Describe(int rule, int roll, string trig, bool cb, string tgt, string bp, string res) =>
        $"rule {rule} roll {roll} trig {trig ?? "-"} cb {(cb ? 1 : 0)} tgt {tgt ?? "-"} bp {bp ?? "-"} res {res}";

    public static void TapeRecord(JObject o)
    {
        var ev = (string)o["ev"];
        if (ev != "proj_launch" && ev != "proj_hit") return;
        if (o["pres"]?.Type == JTokenType.Boolean && (bool)o["pres"]) return;
        var k = o["p"]?.Type == JTokenType.String ? (string)o["p"] : null;
        var t = o["t"]?.Type == JTokenType.Integer ? (int)o["t"] : (int?)null;
        if (k == null || t == null) { Sched.Invalidate($"recording's {ev} record without its projectile or tick"); return; }
        if (ev == "proj_launch")
        {
            int? I(string f) => o[f]?.Type == JTokenType.Integer ? (int)o[f] : (int?)null;
            string S(string f) => o[f]?.Type == JTokenType.String ? (string)o[f] : null;
            // A string, or an explicit null.
            bool StrOrNull(string f) => o[f]?.Type == JTokenType.String || o[f]?.Type == JTokenType.Null;
            var rule = I("rule");
            var roll = I("roll");
            var cb = o["cb"]?.Type == JTokenType.Boolean ? (bool)o["cb"] : (bool?)null;
            var res = S("res");
            if (rule == null || roll == null || cb == null || res == null || !StrOrNull("trig") || !StrOrNull("tgt") || !StrOrNull("bp"))
            {
                Sched.Invalidate($"recording's launch of projectile {k} lacks fields");
                return;
            }
            if (Tape.ContainsKey(k)) { Sched.Invalidate($"recording launches projectile {k} twice"); return; }
            Tape[k] = new Rec { Launch = t.Value, Desc = Describe(rule.Value, roll.Value, S("trig"), cb.Value, S("tgt"), S("bp"), res) };
        }
        else
        {
            if (!Tape.TryGetValue(k, out var r)) { Sched.Invalidate($"recording's projectile {k} hits without a launch"); return; }
            if (r.Hit >= 0) { Sched.Invalidate($"recording's projectile {k} hits twice"); return; }
            if (t.Value < r.Launch) { Sched.Invalidate($"recording's projectile {k} hits before its launch"); return; }
            var pd = o["pd"]?.Type == JTokenType.Float || o["pd"]?.Type == JTokenType.Integer ? (float)(double)o["pd"] : (float?)null;
            if (o["wall"]?.Type != JTokenType.Boolean || pd == null) { Sched.Invalidate($"recording's hit of projectile {k} lacks fields"); return; }
            r.Hit = t.Value;
            r.Wall = (bool)o["wall"];
            r.Passed = pd.Value;
        }
    }

    // ---- ProjectileController.AddProjectile (postfix): a launch ----

    // Inside a decorative launcher (ProjectileSpawner.Fire, FxProjectileLauncher.LaunchProjectile): its
    // projectiles are presentation (Pres), noted and never pinned.
    private static int _decorative;
    private static readonly HashSet<Projectile> Pres = new HashSet<Projectile>(new RefEq<Projectile>());
    public static void DecorativePrefix() => _decorative++;
    public static Exception DecorativeFinalizer(Exception __exception) { _decorative--; return __exception; }

    public static void AddPostfix(Projectile p)
    {
        if (!On || p == null) return;
        try
        {
            if (!p.View) return; // AddProjectile skipped it (no view): never ticked
            var bp = Snap.Guid(p.Blueprint);
            var tgt = p.Target?.UnitRef.Id;
            if (string.IsNullOrEmpty(tgt)) tgt = null;
            if (_decorative > 0)
            {
                Pres.Add(p);
                Sched.Obs("proj_launch", j => { j.Prop("pres", true); j.Prop("bp", bp); j.Prop("tgt", tgt); });
                return;
            }
            var launcher = p.Launcher?.UnitRef.Id;
            if (string.IsNullOrEmpty(launcher)) launcher = "pt";
            PerLauncher.TryGetValue(launcher, out var n);
            PerLauncher[launcher] = n + 1;
            var k = launcher + "#" + n;
            var rule = Rules.CurrentRuleId;
            var roll = Rules.IdOf(AttackRollRef(p));
            var trig = p.OnHitTrigger?.GetType().Name;
            var cb = p.Callback != null;
            var res = p.AttackResult.ToString();
            var now = Trace.Tick;
            var t = new Track { K = k, Launch = now };
            Tracks[p] = t;
            Tracked++;
            Sched.Obs("proj_launch", j =>
            {
                j.Prop("p", k); j.Prop("rule", rule); j.Prop("roll", roll); j.Prop("trig", trig); j.Prop("cb", cb);
                j.Prop("tgt", tgt); j.Prop("bp", bp); j.Prop("res", res);
            });
            if (!Sched.Replay) return;
            var desc = Describe(rule, roll, trig, cb, tgt, bp, res);
            if (!Tape.TryGetValue(k, out var rec))
                Sched.Invalidate($"projectile {k} launched at tick {now} ({desc}): the recording launched no such projectile");
            else if (rec.Matched)
                Sched.Invalidate($"projectile {k} launched twice at tick {now}");
            else if (rec.Desc != desc)
                Sched.Invalidate($"projectile {k} launched at tick {now} is {desc}; the recording's was {rec.Desc}");
            else if (Math.Abs(now - (rec.Launch + Sched.Offset)) > Sched.Tol)
                Sched.Invalidate($"projectile {k} launched at tick {now}; the recording's at tick {rec.Launch} (offset so far {Sched.Offset}, tolerance {Sched.Tol})");
            else
            {
                rec.Matched = true;
                if (rec.Hit >= 0)
                {
                    t.Due = now + (rec.Hit - rec.Launch);
                    Pending.Add(t);
                }
            }
        }
        catch (Exception e) { Trace.HookFailed("projectile launch", e); }
    }

    // ---- Projectile.Tick: the arrival ----

    // Before the tick: whether it was observed, IsHit and IsHitWall, PassedDistance.
    public struct TickState { public bool On, Hit, Wall; public float Passed; }

    public static void TickPrefix(Projectile __instance, out TickState __state)
    {
        __state = default;
        if (!On || __instance == null) return;
        __state = new TickState { On = true, Hit = __instance.IsHit, Wall = __instance.IsHitWall, Passed = PassedDistanceRef(__instance) };
    }

    public static void TickPostfix(Projectile __instance, TickState __state)
    {
        if (!__state.On || __state.Hit || !On) return;
        var p = __instance;
        try
        {
            var now = Trace.Tick;
            if (!Tracks.TryGetValue(p, out var t))
            {
                // Not launched in this window (or presentation): native. One that matters hitting here is unpinned.
                if (!p.IsHit || Pres.Contains(p) || !Untracked.Add(p)) return;
                Sched.Obs("proj_untracked", j => j.Prop("bp", Snap.Guid(p.Blueprint)));
                if (Sched.Replay) Sched.Invalidate($"a projectile launched before the window hit at tick {now} (its arrival isn't pinned)");
                return;
            }
            if (t.Hit >= 0) return;
            if (!Sched.Replay)
            {
                if (!p.IsHit) return;
                t.Hit = now;
                var wall = p.IsHitWall;
                var pd = PassedDistanceRef(p);
                Sched.Obs("proj_hit", j => { j.Prop("p", t.K); j.Prop("fl", now - t.Launch); j.Prop("wall", wall); j.Prop("pd", pd); });
                return;
            }
            if (Sched.Invalid != null) return; // left its recording: native from here
            if (t.Due < 0 || now < t.Due)
            {
                // Not due (or the recording's never hit in its window): the native hit is taken back, with what
                // the hit did to the distance flown (a line or cone ability's delivery reads it every tick:
                // AbilityDeliverProjectile.StepBasedDeliver), back to the unclamped sum Tick made before it.
                if (!p.IsHit) return;
                IsHitRef(p) = false;
                if (!__state.Wall) IsHitWallRef(p) = false;
                if (p.MaxRange > 0f) PassedDistanceRef(p) = __state.Passed + Kingmaker.Game.Instance.TimeController.DeltaTime * p.Blueprint.Speed;
                if (t.Held++ == 0) HeldBack++;
                return;
            }
            if (now > t.Due)
            {
                Sched.Invalidate($"projectile {t.K} was due to hit at tick {t.Due}; still in flight at tick {now}");
                return;
            }
            if (!p.IsHit)
            {
                // What Tick does when it arrives: IsHit, and the distance clamped to the range.
                IsHitRef(p) = true;
                if (p.MaxRange > 0f) PassedDistanceRef(p) = p.MaxRange;
                t.IsForced = true;
                Forced++;
            }
            t.Hit = now;
            Pending.Remove(t);
            // The arrival's kind (a terrain-following projectile stopped by an obstacle: IsHitWall, which
            // AbilityDeliverAttackWithWeapon reads) is the recording's; the distance flown must be too.
            var rec = Tape[t.K];
            if (p.IsHitWall != rec.Wall) IsHitWallRef(p) = rec.Wall;
            var pd2 = PassedDistanceRef(p);
            if (pd2 != rec.Passed)
            {
                Sched.Invalidate($"projectile {t.K} hit at tick {now} having flown {pd2:R}; the recording's {rec.Passed:R}");
                return;
            }
            var wall2 = p.IsHitWall;
            var held = t.Held;
            var forced = t.IsForced;
            Sched.Obs("proj_hit", j =>
            {
                j.Prop("p", t.K); j.Prop("fl", now - t.Launch); j.Prop("wall", wall2); j.Prop("pd", pd2);
                if (held > 0) j.Prop("held", held);
                if (forced) j.Prop("forced", true);
            });
        }
        catch (Exception e) { Trace.HookFailed("projectile arrival", e); }
    }

    // ---- Projectile.OnHit (prefix): on its hit's tick ----

    public static void OnHitPrefix(Projectile __instance)
    {
        if (!On || __instance == null || !Sched.Replay) return;
        try
        {
            if (!Tracks.TryGetValue(__instance, out var t)) return;
            t.OnHit = true;
            if (t.Hit != Trace.Tick) Sched.Invalidate($"projectile {t.K}: OnHit at tick {Trace.Tick}, its hit at tick {t.Hit}");
        }
        catch (Exception e) { Trace.HookFailed("projectile on-hit", e); }
    }

    // ---- ProjectileHitController.Tick (postfix): every due hit made, with its OnHit ----

    public static void HitTickPostfix()
    {
        if (!On || !Sched.Replay || Sched.Invalid != null) return;
        try
        {
            var now = Trace.Tick;
            foreach (var t in Pending)
                if (t.Hit < 0 && t.Due <= now)
                {
                    Sched.Invalidate($"projectile {t.K} was due to hit at tick {t.Due}; it didn't (gone or not ticked)");
                    return;
                }
            foreach (var t in Tracks.Values)
                if (t.Hit == now && !t.OnHit)
                {
                    Sched.Invalidate($"projectile {t.K} hit at tick {now} without its OnHit");
                    return;
                }
        }
        catch (Exception e) { Trace.HookFailed("projectile hits", e); }
    }

    // Before the window closes (Sched.CheckComplete): every recorded launch and due hit happened.
    public static void CheckComplete()
    {
        if (!Installed || !Sched.Replay || Sched.Invalid != null) return;
        var missing = Tape.Where(kv => !kv.Value.Matched).OrderBy(kv => kv.Value.Launch).FirstOrDefault();
        if (missing.Key != null)
        {
            Sched.Invalidate($"the recording launched projectile {missing.Key} at tick {missing.Value.Launch}; the replay didn't");
            return;
        }
        var owed = Pending.FirstOrDefault(t => t.Hit < 0);
        if (owed != null) Sched.Invalidate($"projectile {owed.K} was due to hit at tick {owed.Due}; the window closed first");
    }

    public static void Install(Harmony h)
    {
        try
        {
            if (IsHitRef == null || IsHitWallRef == null || AttackRollRef == null || PassedDistanceRef == null) throw new MissingFieldException("Projectile's IsHit, IsHitWall, PassedDistance or m_AttackRoll");
            h.Patch(AccessTools.Method(typeof(ProjectileController), "AddProjectile", new[] { typeof(Projectile) }) ?? throw new MissingMethodException("ProjectileController.AddProjectile"),
                postfix: new HarmonyMethod(typeof(ProjPin), nameof(AddPostfix)));
            h.Patch(AccessTools.Method(typeof(Projectile), nameof(Projectile.Tick)),
                prefix: new HarmonyMethod(typeof(ProjPin), nameof(TickPrefix)), postfix: new HarmonyMethod(typeof(ProjPin), nameof(TickPostfix)));
            h.Patch(AccessTools.Method(typeof(Projectile), nameof(Projectile.OnHit)), prefix: new HarmonyMethod(typeof(ProjPin), nameof(OnHitPrefix)));
            h.Patch(AccessTools.Method(typeof(ProjectileHitController), nameof(ProjectileHitController.Tick)), postfix: new HarmonyMethod(typeof(ProjPin), nameof(HitTickPostfix)));
            foreach (var (type, name) in new[] { (typeof(Kingmaker.View.Spawners.Projectile.ProjectileSpawner), "Fire"), (typeof(Kingmaker.View.FxProjectileLauncher), "LaunchProjectile") })
                h.Patch(AccessTools.Method(type, name) ?? throw new MissingMethodException(type.Name + "." + name),
                    prefix: new HarmonyMethod(typeof(ProjPin), nameof(DecorativePrefix)), finalizer: new HarmonyMethod(typeof(ProjPin), nameof(DecorativeFinalizer)));
            Installed = true;
        }
        catch (Exception e) { Health.Failed("patch projectile arrival", e); }
    }
}

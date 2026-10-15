using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using HarmonyLib;
using Kingmaker.UnitLogic;
using Kingmaker.View;
using Kingmaker.Visual.Particles;
using Newtonsoft.Json.Linq;

namespace WrathOracle;

// A unit's death effect in turn-based windows: observed in a recording, pinned in a replay
// (docs/poc5/tb-replay.md, "The death effect's dismemberment").
//
// [C] UnitLifeController calls UnitEntityView.HandleDeath; a visible unit with an active view starts the
// PlayDeathEffect coroutine, whose first step (run inside HandleDeath) decides the effect:
// ShouldDismember reads the player's settings, the unit's size and companion state, IsFinallyDead, the
// dismemberment manager's last impulse (presentation physics: whether the blueprint has a rip-limbs effect
// for its damage form) and the last damage against MaxHP and crits; GetDismemberType then picks InPower,
// LimbsApart or Normal, with CanDismemberLimbsApart reading the impulse's age on Unity's clock
// (Time.time) and RandomUtils' draw on the simulation tape. A blow-up dismember effect is spawned through
// FxSpawnController (NotImportant: at most 6 spawns a frame, dropped after 5 frames in the queue), and its
// DismemberUnitFX.Init adds SystemMechanics.DismemberedUnitBuff to the unit: a real buff, with its rule
// (RuleCanApplyBuff) and fact id, on a dead unit. [M] poc5-tb-123/tb-chain: the fly 527A, killed at t=5422,
// got the buff at 5423 in the recording (and lost another buff when it expired at 5432), in neither replay.
//
// Recording (nothing changes): every HandleDeath in the window is keyed "<unit>#<n>" (n: that unit's deaths
// in the window) and noted ("death_fx": whether PlayDeathEffect started, ShouldDismember's and
// CanDismemberLimbsApart's results inside it and GetDismemberType's, null where not reached), and every
// DismemberUnitFX.Init ("dismember": the unit, whether it is in game, so whether Init adds the buff).
// Replay: inside HandleDeath, ShouldDismember and CanDismemberLimbsApart return the recording's results (their
// presentation inputs are not the recording's; GetDismemberType then runs natively, its draw from the tape),
// and the rest of the death must be the recording's: the same death on the same tick, the coroutine started
// or not, the same type. Each DismemberUnitFX.Init must be the recording's next for its unit, on the recorded
// tick (plus the replay's offset): its tick follows the effect's spawn through FxSpawnController's queue,
// which isn't pinned, so a spawn on another tick (or dropped) invalidates the replay rather than being
// followed.
internal static class DeathFxPin
{
    public static bool Installed, RecordedOn;
    public static long Deaths, Pinned, Inits;

    private sealed class Death { public string K, Type; public int T; public bool Start; public bool? Dsm, Limbs; public bool Matched; }
    private sealed class Init { public string U; public int T; public bool InGame, Seen; }

    private static readonly Dictionary<string, Death> Tape = new Dictionary<string, Death>();
    private static readonly List<Init> InitTape = new List<Init>();
    private static readonly Dictionary<string, int> PerUnit = new Dictionary<string, int>();

    // The death being handled (HandleDeath's prefix to its postfix), and what its effect decided.
    private static UnitEntityView _view;
    private static Death _now, _rec;

    public static void ResetWindow()
    {
        Tape.Clear();
        InitTape.Clear();
        PerUnit.Clear();
        _view = null; _now = _rec = null;
        RecordedOn = false;
        Deaths = Pinned = Inits = 0;
    }

    private static bool On => Installed && Sched.On;

    // ---- the tape ----

    public static void TapeRecord(JObject o)
    {
        var ev = (string)o["ev"];
        if (ev != "death_fx" && ev != "dismember") return;
        var t = o["t"]?.Type == JTokenType.Integer ? (int)o["t"] : -1;
        bool? B(string f) => o[f]?.Type == JTokenType.Boolean ? (bool)o[f] : (bool?)null;
        bool BoolOrNull(string f) => o[f]?.Type == JTokenType.Boolean || o[f]?.Type == JTokenType.Null;
        if (ev == "death_fx")
        {
            var k = o["p"]?.Type == JTokenType.String ? (string)o["p"] : null;
            if (k == null || t < 0 || B("start") == null || !BoolOrNull("dsm") || !BoolOrNull("limbs") ||
                !(o["type"]?.Type == JTokenType.String || o["type"]?.Type == JTokenType.Null))
            { Sched.Invalidate("recording's death_fx record lacks or mistypes a field"); return; }
            if (Tape.ContainsKey(k)) { Sched.Invalidate($"recording's death {k} twice"); return; }
            Tape[k] = new Death { K = k, T = t, Start = B("start").Value, Dsm = B("dsm"), Limbs = B("limbs"), Type = o["type"]?.Type == JTokenType.String ? (string)o["type"] : null };
        }
        else
        {
            var u = o["u"]?.Type == JTokenType.String ? (string)o["u"] : null;
            if (u == null || t < 0 || B("ingame") == null) { Sched.Invalidate("recording's dismember record lacks or mistypes a field"); return; }
            InitTape.Add(new Init { U = u, T = t, InGame = B("ingame").Value });
        }
    }

    // ---- UnitEntityView.HandleDeath ----

    public static void HandleDeathPrefix(UnitEntityView __instance)
    {
        _view = null; _now = _rec = null;
        if (!On || __instance == null) return;
        try
        {
            var id = Rules.StableId(__instance.Data as Kingmaker.EntitySystem.Entities.UnitEntityData);
            PerUnit.TryGetValue(id, out var n);
            PerUnit[id] = n + 1;
            var k = id + "#" + n;
            _view = __instance;
            _now = new Death { K = k, T = Trace.Tick };
            Deaths++;
            if (!Sched.Replay) return;
            if (!Tape.TryGetValue(k, out var rec))
                Sched.Invalidate($"death {k} at tick {Trace.Tick}: the recording has no such death");
            else if (rec.Matched)
                Sched.Invalidate($"death {k} handled twice");
            else if (Trace.Tick != rec.T + Sched.Offset)
                Sched.Invalidate($"death {k} at tick {Trace.Tick}; the recording's at tick {rec.T} (offset so far {Sched.Offset})");
            else { rec.Matched = true; _rec = rec; }
        }
        catch (Exception e) { Trace.HookFailed("death effect", e); }
    }

    public static Exception HandleDeathFinalizer(UnitEntityView __instance, Exception __exception)
    {
        var now = _now;
        var rec = _rec;
        _view = null; _now = _rec = null;
        if (now == null || !On) return __exception;
        try
        {
            Sched.Obs("death_fx", j =>
            {
                j.Prop("p", now.K); j.Prop("start", now.Start); j.Prop("dsm", now.Dsm); j.Prop("limbs", now.Limbs); j.Prop("type", now.Type);
                if (__exception != null) j.Prop("exc", __exception.GetType().Name);
            });
            if (__exception != null)
            {
                if (Sched.Replay) Sched.Invalidate($"death {now.K}: HandleDeath threw {__exception.GetType().Name}");
                return __exception;
            }
            if (rec != null && (now.Start != rec.Start || now.Dsm != rec.Dsm || now.Limbs != rec.Limbs || now.Type != rec.Type))
                Sched.Invalidate($"death {now.K} at tick {now.T}: effect started {now.Start}, dismember {F(now.Dsm)}, limbs apart {F(now.Limbs)}, type {now.Type ?? "-"}; " +
                                 $"the recording's {rec.Start}, {F(rec.Dsm)}, {F(rec.Limbs)}, {rec.Type ?? "-"}");
        }
        catch (Exception e) { Trace.HookFailed("death effect end", e); }
        return __exception;
    }

    private static string F(bool? b) => b.HasValue ? (b.Value ? "true" : "false") : "-";

    // PlayDeathEffect (the iterator's factory: called by HandleDeath when it starts the coroutine).
    public static void PlayDeathEffectPrefix(UnitEntityView __instance)
    {
        if (_now != null && ReferenceEquals(__instance, _view)) _now.Start = true;
    }

    // ShouldDismember and CanDismemberLimbsApart inside a death being handled: noted; a replay takes the
    // recording's (both read presentation: the last impulse, its age on Unity's clock).
    public static bool ShouldDismemberPrefix(UnitEntityView __instance, ref bool __result) => Pin(__instance, ref __result, limbs: false);
    public static bool CanLimbsApartPrefix(UnitEntityView __instance, ref bool __result) => Pin(__instance, ref __result, limbs: true);

    private static bool Pin(UnitEntityView v, ref bool result, bool limbs)
    {
        if (_now == null || !ReferenceEquals(v, _view) || !Sched.Replay || _rec == null || Sched.Invalid != null) return true;
        var want = limbs ? _rec.Limbs : _rec.Dsm;
        if (!want.HasValue)
        {
            Sched.Invalidate($"death {_now.K}: {(limbs ? "CanDismemberLimbsApart" : "ShouldDismember")} asked; the recording's effect didn't ask it");
            return true;
        }
        result = want.Value;
        Pinned++;
        return false;
    }

    public static void ShouldDismemberPostfix(UnitEntityView __instance, bool __result)
    {
        if (_now != null && ReferenceEquals(__instance, _view)) _now.Dsm = __result;
    }

    public static void CanLimbsApartPostfix(UnitEntityView __instance, bool __result)
    {
        if (_now != null && ReferenceEquals(__instance, _view)) _now.Limbs = __result;
    }

    public static void DismemberTypePostfix(UnitEntityView __instance, UnitState.DismemberType __result)
    {
        if (_now != null && ReferenceEquals(__instance, _view)) _now.Type = __result.ToString();
    }

    // ---- DismemberUnitFX.Init (prefix): the buff it adds ----

    public static void InitPrefix(UnitEntityView unit)
    {
        if (!On || unit == null) return;
        try
        {
            var id = Rules.StableId(unit.Data as Kingmaker.EntitySystem.Entities.UnitEntityData);
            var ingame = unit.Data != null && unit.Data.IsInGame;
            var now = Trace.Tick;
            Inits++;
            Sched.Obs("dismember", j => { j.Prop("u", id); j.Prop("ingame", ingame); });
            if (!Sched.Replay) return;
            var rec = InitTape.FirstOrDefault(r => !r.Seen && r.U == id);
            if (rec == null) Sched.Invalidate($"dismember effect on {id} at tick {now}: the recording has no such effect");
            else if (now != rec.T + Sched.Offset || ingame != rec.InGame)
                Sched.Invalidate($"dismember effect on {id} at tick {now} (in game {ingame}); the recording's at tick {rec.T} (in game {rec.InGame}, offset so far {Sched.Offset})");
            else rec.Seen = true;
        }
        catch (Exception e) { Trace.HookFailed("dismember effect", e); }
    }

    // Every tick (FxSpawnController.Tick's finalizer) and at the window's close: a recorded effect whose tick
    // passed without it (before this tick: a spawn made outside the controller's tick, as when it is inactive,
    // can come later in its tick than this check).
    public static Exception FxTickFinalizer(Exception __exception)
    {
        if (On && Sched.Replay && Sched.Invalid == null) CheckDue(Trace.Tick);
        return __exception;
    }

    private static void CheckDue(int now)
    {
        var late = InitTape.FirstOrDefault(r => !r.Seen && r.T + Sched.Offset < now);
        if (late != null) Sched.Invalidate($"dismember effect on {late.U} due at tick {late.T + Sched.Offset}; it didn't happen (its spawn dropped or late)");
    }

    public static void CheckComplete()
    {
        if (!Installed || !Sched.Replay || Sched.Invalid != null) return;
        var missing = Tape.Values.Where(d => !d.Matched).OrderBy(d => d.T).FirstOrDefault();
        if (missing != null) { Sched.Invalidate($"the recording's death {missing.K} at tick {missing.T} didn't happen in the replay"); return; }
        var init = InitTape.FirstOrDefault(r => !r.Seen);
        if (init != null) Sched.Invalidate($"the recording's dismember effect on {init.U} at tick {init.T} didn't happen in the replay");
    }

    public static void Install(Harmony h)
    {
        try
        {
            var v = typeof(UnitEntityView);
            MethodBase M(Type t, string name) => AccessTools.Method(t, name) ?? throw new MissingMethodException(t.Name + "." + name);
            h.Patch(M(v, nameof(UnitEntityView.HandleDeath)), prefix: new HarmonyMethod(typeof(DeathFxPin), nameof(HandleDeathPrefix)),
                finalizer: new HarmonyMethod(typeof(DeathFxPin), nameof(HandleDeathFinalizer)));
            h.Patch(M(v, "PlayDeathEffect"), prefix: new HarmonyMethod(typeof(DeathFxPin), nameof(PlayDeathEffectPrefix)));
            h.Patch(M(v, "ShouldDismember"), prefix: new HarmonyMethod(typeof(DeathFxPin), nameof(ShouldDismemberPrefix)),
                postfix: new HarmonyMethod(typeof(DeathFxPin), nameof(ShouldDismemberPostfix)));
            h.Patch(M(v, "CanDismemberLimbsApart"), prefix: new HarmonyMethod(typeof(DeathFxPin), nameof(CanLimbsApartPrefix)),
                postfix: new HarmonyMethod(typeof(DeathFxPin), nameof(CanLimbsApartPostfix)));
            h.Patch(M(v, "GetDismemberType"), postfix: new HarmonyMethod(typeof(DeathFxPin), nameof(DismemberTypePostfix)));
            h.Patch(M(typeof(DismemberUnitFX), nameof(DismemberUnitFX.Init)), prefix: new HarmonyMethod(typeof(DeathFxPin), nameof(InitPrefix)));
            h.Patch(M(typeof(Kingmaker.Visual.Particles.FxSpawnSystem.FxSpawnController), "Tick"), finalizer: new HarmonyMethod(typeof(DeathFxPin), nameof(FxTickFinalizer)));
            Installed = true;
        }
        catch (Exception e) { Health.Failed("patch the death effect", e); }
    }
}

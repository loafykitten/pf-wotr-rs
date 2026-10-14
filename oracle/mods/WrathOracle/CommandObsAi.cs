using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using HarmonyLib;
using Kingmaker;
using Kingmaker.AI;
using Kingmaker.AI.Blueprints;
using Kingmaker.AI.Blueprints.Considerations;
using Kingmaker.Controllers.Combat;
using Kingmaker.EntitySystem;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.UnitLogic;
using Kingmaker.UnitLogic.Commands;
using Kingmaker.UnitLogic.Commands.Base;
using Kingmaker.UnitLogic.Groups;
using Kingmaker.UnitLogic.Parts;
using Kingmaker.Utility;
using Kingmaker.View;
using Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.FogOfWar;
using Pathfinding;
using TurnBased.Controllers;
using UnityEngine;

namespace WrathOracle;

// Observer "ai" (slice D, docs/poc5/commands.md): the AI's code is base lifecycle code, so a replay runs
// AiBrainController (ForceTick, TickBrain, SelectAction and the scoring), DecisionContext, TargetInfo, AiAction,
// AiActionSimple, BlueprintAiAction and BlueprintAiAttack and the ported considerations, and takes as inputs only
// what that code reads outside itself:
//  - entries: uc_ai (AiBrainController.Tick, every call; ForceTick, nested when Tick calls it) and uc_ai_end, with the
//    caller and AiBrainController.LimitedAI;
//  - leaves (uc_v), each read where the AI's code makes it, by a transpiler on the call (the same call, then the
//    record when the read is the AI's: the innermost entry an AI entry and every frame up to it base code): wfui,
//    groups, sleep, passive, nct, time, aici, conf, auto, ptf, disliked, ipt, pe, enemies, hpcm, manual, corp, dist,
//    los, rider, last, amf, unreach, speed, mrange, path (FixPathIfTooShort's result: an input, not base code), pclose,
//    mag, plen, r5, h5, hfra, msim, ablk, aien, diff, range, magus; aoo (EngagementSimulator.CheckForAttacksOfOpportunity, a postfix: the class is internal);
//  - ai_nct: every write of NextCommandTime, with its writer (SelectAction's are checks, the others inputs);
//  - checks: ai_sc (AiAction.ScoreActor, ScoreTarget and ScorePath: the score they leave), ai_best
//    (FindBestAction: the action, target, score, five-foot step and path) and ai_sel (SelectAction: its result and
//    the unit's NextCommandTime).
// The existing leaves (ctrl, untgt, saddled) are recorded inside the AI as everywhere else. Decimals are written
// with decimal.ToString in the invariant culture (the scale kept), floats round-trip.
internal static partial class CommandObs
{
    // Whether the AI's code is lifecycle code (Install sets it; the trace's begin lists the observer).
    internal static bool AiOn;

    private static bool InAi => Entries.Count > 0 && Entries[Entries.Count - 1].Ai && Entries[Entries.Count - 1].Rec;

    // A read the AI's code makes: `m` the hooked method (null for a tap, whose own frames are skipped).
    private static bool AiChain(MethodBase m) => On && AiOn && InAi && Chain(m) == 1;

    private static string D(decimal v) => v.ToString(CultureInfo.InvariantCulture);

    // The hooks' own reads (beyond the game's call a tap wraps) never throw into the game.
    private static string Guid(Kingmaker.Blueprints.SimpleBlueprint b)
    {
        try { return b == null ? null : b.AssetGuid.ToString(); }
        catch (Exception e) { Trace.HookFailed("observe ai guid", e); return null; }
    }

    private static void WritePoints(Json j, string k, Pathfinding.Path p)
    {
        j.Key(k);
        var pts = p?.vectorPath;
        if (p == null || pts == null) { j.W.WriteNull(); return; }
        j.W.WriteStartArray();
        foreach (var x in pts) j.Vec(x);
        j.W.WriteEndArray();
    }

    // ---- entries ----

    // AiBrainController.Tick and ForceTick.
    public static void AiEntryPrefix(MethodBase __originalMethod)
    {
        if (!Enter(__originalMethod, out var nested, ai: true)) return;
        try
        {
            var fn = __originalMethod.Name;
            var origin = Origin();
            var limited = AiBrainController.LimitedAI;
            Sched.Obs("uc_ai", j => { j.Prop("fn", fn); j.Prop("nested", nested); j.Prop("origin", origin); j.Prop("limited", limited); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_ai", e); }
    }

    public static void AiEntryFinalizer(MethodBase __originalMethod, Exception __exception)
    {
        if (!Pop() || !On) return;
        try
        {
            var fn = __originalMethod.Name;
            var exc = __exception != null;
            Sched.Obs("uc_ai_end", j => { j.Prop("fn", fn); j.Prop("exc", exc); });
        }
        catch (Exception e) { Trace.HookFailed("observe uc_ai_end", e); }
    }

    // ---- taps ----

    private static void AiTap(string f, Action<Json> body)
    {
        try
        {
            if (!AiChain(null)) return;
            Emit(f, body);
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v " + f, e); }
    }

    private static T Safe<T>(Func<T> f) where T : class
    {
        try { return f(); }
        catch (Exception e) { Trace.HookFailed("observe ai read", e); return null; }
    }

    private static string U(EntityDataBase e)
    {
        try { return e?.UniqueId; }
        catch (Exception ex) { Trace.HookFailed("observe ai unit", ex); return null; }
    }

    private static readonly AccessTools.FieldRef<UnitGroup, List<UnitReference>> GroupUnits =
        AccessTools.FieldRefAccess<UnitGroup, List<UnitReference>>("m_Units");

    public static CountingGuard TapWaitingForUI(CombatController c)
    {
        var v = c.WaitingForUI;
        var b = (bool)v;
        AiTap("wfui", j => j.Prop("v", b));
        return v;
    }

    // Game.ReadyForCombatUnitGroups: each group as [id (null for a fake one), [its units' reference ids], IsInCombat].
    public static List<UnitGroup> TapGroups(Game g)
    {
        var v = g.ReadyForCombatUnitGroups;
        AiTap("groups", j =>
        {
            j.Key("v");
            if (v == null) { j.W.WriteNull(); return; }
            j.W.WriteStartArray();
            foreach (var x in v)
            {
                if (x == null) { j.W.WriteNull(); continue; }
                j.W.WriteStartArray();
                j.W.WriteValue(x.Id);
                j.W.WriteStartArray();
                foreach (var r in GroupUnits(x)) j.W.WriteValue(r.UniqueId);
                j.W.WriteEndArray();
                j.W.WriteValue((bool)x.IsInCombat);
                j.W.WriteEndArray();
            }
            j.W.WriteEndArray();
        });
        return v;
    }

    public static bool TapSleeping(UnitEntityData u)
    {
        var v = u.IsSleeping;
        var id = U(u);
        AiTap("sleep", j => { j.Prop("u", id); j.Prop("v", v); });
        return v;
    }

    public static bool TapPassive(ObservableCountFlag f)
    {
        var v = (bool)f;
        AiTap("passive", j => j.Prop("v", v));
        return v;
    }

    // The unit a CombatAiData belongs to (its UnitCombatState's), found among the units once (passive reads of
    // auto-properties and readonly fields).
    private static System.Runtime.CompilerServices.ConditionalWeakTable<CombatAiData, string> _aiDataUnit =
        new System.Runtime.CompilerServices.ConditionalWeakTable<CombatAiData, string>();

    private static string AiDataUnit(CombatAiData d)
    {
        if (d == null || !On || !AiOn) return null;
        try
        {
            if (_aiDataUnit.TryGetValue(d, out var s)) return s;
            foreach (var u in StateDump.AllUnits(Game.Instance))
            {
                var cs = u?.CombatState;
                if (cs == null || !ReferenceEquals(cs.AIData, d)) continue;
                _aiDataUnit.Add(d, u.UniqueId);
                return u.UniqueId;
            }
        }
        catch (Exception e) { Trace.HookFailed("observe ai data's unit", e); }
        return null;
    }

    public static float TapNct(CombatAiData d)
    {
        var v = d.NextCommandTime;
        var u = AiDataUnit(d);
        AiTap("nct", j => { j.Prop("u", u); j.Prop("v", F(v)); });
        return v;
    }

    // Every write of NextCommandTime (ai_nct, with its writer): the AI's own (SelectAction) are checks, the others
    // (the engagement's, UnitCommands.UpdateCombatTarget's, the auto-use ability's setter) inputs.
    private static void SetNct(CombatAiData d, float v, string site)
    {
        d.NextCommandTime = v;
        try
        {
            if (!On || !AiOn) return;
            var u = AiDataUnit(d);
            Sched.Obs("ai_nct", j => { j.Prop("u", u); j.Prop("site", site); j.Prop("v", F(v)); });
        }
        catch (Exception e) { Trace.HookFailed("observe ai_nct", e); }
    }

    public static void TapSetNctSelect(CombatAiData d, float v) => SetNct(d, v, "SelectAction");
    public static void TapSetNctEngage(CombatAiData d, float v) => SetNct(d, v, "Engage");
    public static void TapSetNctDisengage(CombatAiData d, float v) => SetNct(d, v, "Disengage");
    public static void TapSetNctUpdate(CombatAiData d, float v) => SetNct(d, v, "UpdateCombatTarget");
    public static void TapSetNctAuto(CombatAiData d, float v) => SetNct(d, v, "set_AutoUseAbility");

    public static float TapTime()
    {
        var v = Time.time;
        AiTap("time", j => j.Prop("v", F(v)));
        return v;
    }

    public static bool TapAiCanInterrupt(UnitCommand c)
    {
        var v = c.AiCanInterrupt;
        try
        {
            if (AiChain(null))
            {
                var id = Id(c);
                Emit("aici", j => { WriteId(j, "c", id); j.Prop("v", v); });
            }
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v aici", e); }
        return v;
    }

    // `?.` after these parts: a reference test.
    public static UnitPartConfusion TapConfusion(EntityDataBase e)
    {
        var v = e.Get<UnitPartConfusion>();
        var b = !ReferenceEquals(v, null);
        var id = U(e);
        AiTap("conf", j => { j.Prop("u", id); j.Prop("v", b); });
        return v;
    }

    public static UnitPartPersonalEnemy TapPersonalEnemy(EntityDataBase e)
    {
        var v = e.Get<UnitPartPersonalEnemy>();
        var b = !ReferenceEquals(v, null);
        var id = U(e);
        AiTap("pe", j => { j.Prop("u", id); j.Prop("v", b); });
        return v;
    }

    public static UnitPartMagus TapMagus(EntityDataBase e)
    {
        var v = e.Get<UnitPartMagus>();
        var b = !ReferenceEquals(v, null);
        var id = U(e);
        AiTap("magus", j => { j.Prop("u", id); j.Prop("v", b); });
        return v;
    }

    // UnitBrain.GetAvailableAutoUseAbility: null, or the ability's blueprint.
    public static Kingmaker.UnitLogic.Abilities.AbilityData TapAuto(UnitBrain b)
    {
        var v = b.GetAvailableAutoUseAbility();
        var bp = ReferenceEquals(v, null) ? null : Guid(Safe(() => v.Blueprint));
        var id = U(Safe(() => b.Owner));
        AiTap("auto", j => { j.Prop("u", id); j.Prop("v", bp); });
        return v;
    }

    public static Kingmaker.Blueprints.Facts.BlueprintUnitFact TapPriorityFact(UnitBrain b)
    {
        var v = b.PriorityTargetFact;
        var bp = Guid(v);
        var id = U(Safe(() => b.Owner));
        AiTap("ptf", j => { j.Prop("u", id); j.Prop("v", bp); });
        return v;
    }

    public static bool TapDisliked(HashSet<UnitEntityData> s, UnitEntityData u)
    {
        var v = s.Contains(u);
        var id = U(u);
        AiTap("disliked", j => { j.Prop("u", id); j.Prop("v", v); });
        return v;
    }

    public static bool TapUnreachable(HashSet<UnitEntityData> s, UnitEntityData u)
    {
        var v = s.Contains(u);
        var id = U(u);
        AiTap("unreach", j => { j.Prop("u", id); j.Prop("v", v); });
        return v;
    }

    public static UnitEntityData TapImpatienceTarget(CombatAiData d)
    {
        var v = d.ImpatiencePriorityTarget;
        var id = U(v);
        AiTap("ipt", j => j.Prop("v", id));
        return v;
    }

    // Memory.Enemies: the units' ids (UnitReference.UniqueId, never Value, which resolves a proxy).
    public static List<UnitGroupMemory.UnitInfo> TapAiEnemies(UnitGroupMemory m)
    {
        var v = m.Enemies;
        AiTap("enemies", j =>
        {
            j.Key("v");
            if (v == null) { j.W.WriteNull(); return; }
            j.W.WriteStartArray();
            foreach (var x in v) { if (x == null) j.W.WriteNull(); else j.W.WriteValue(x.UnitReference.UniqueId ?? ""); }
            j.W.WriteEndArray();
        });
        return v;
    }

    public static bool TapHasPlayerCharacter(UnitGroupMemory m)
    {
        var v = m.HasPlayerCharacterInMemory();
        AiTap("hpcm", j => j.Prop("v", v));
        return v;
    }

    public static UnitEntityData TapManual(UnitCombatState cs)
    {
        var v = cs.ManualTarget;
        var u = U(Safe(() => cs.Unit));
        var id = U(v);
        AiTap("manual", j => { j.Prop("u", u); j.Prop("v", id); });
        return v;
    }

    public static UnitEntityData TapLastTarget(UnitCombatState cs)
    {
        var v = cs.LastTarget;
        var u = U(Safe(() => cs.Unit));
        var id = U(v);
        AiTap("last", j => { j.Prop("u", u); j.Prop("v", id); });
        return v;
    }

    public static float TapCorpulence(UnitEntityView view)
    {
        var v = view.Corpulence;
        var u = U(Safe(() => view.EntityData));
        AiTap("corp", j => { j.Prop("u", u); j.Prop("v", F(v)); });
        return v;
    }

    public static float TapDistance(UnitEntityData a, UnitEntityData b)
    {
        var v = a.DistanceTo(b);
        var ua = U(a);
        var ub = U(b);
        AiTap("dist", j => { j.Prop("u", ua); j.Prop("to", ub); j.Prop("v", F(v)); });
        return v;
    }

    public static bool TapObstacle(LineOfSightGeometry g, Vector3 from, Vector3 to, float fudgeRadius)
    {
        var v = g.HasObstacle(from, to, fudgeRadius);
        AiTap("los", j => j.Prop("v", v));
        return v;
    }

    public static UnitEntityData TapRider(UnitEntityData u)
    {
        var v = UnitHelper.GetRider(u);
        var id = U(u);
        var r = U(v);
        AiTap("rider", j => { j.Prop("u", id); j.Prop("v", r); });
        return v;
    }

    public static bool TapMovementForbidden(UnitEntityData u)
    {
        var v = u.AiMovementForbidden;
        var id = U(u);
        AiTap("amf", j => { j.Prop("u", id); j.Prop("v", v); });
        return v;
    }

    public static float TapSpeed(UnitEntityData u)
    {
        var v = u.CurrentSpeedMps;
        var id = U(u);
        AiTap("speed", j => { j.Prop("u", id); j.Prop("v", F(v)); });
        return v;
    }

    public static float TapMovementRange(TurnController t, UnitEntityData u, bool total, bool singleActionMove)
    {
        var v = t.GetRemainingMovementRange(u, total, singleActionMove);
        var id = U(u);
        AiTap("mrange", j => { j.Prop("u", id); j.Prop("total", total); j.Prop("single", singleActionMove); j.Prop("v", F(v)); });
        return v;
    }

    private static readonly Action<DecisionContext, Vector3> FixPath =
        AccessTools.MethodDelegate<Action<DecisionContext, Vector3>>(AccessTools.Method(typeof(AiAction), "FixPathIfTooShort"));

    // FixPathIfTooShort(context, destination): the path ScorePath goes on with (found, or replaced by a fake one).
    public static void TapFixPath(DecisionContext context, Vector3 destination)
    {
        var before = context.CurrentPath;
        FixPath(context, destination);
        var p = context.CurrentPath;
        var fake = !ReferenceEquals(before, p);
        var err = p != null && p.error;
        var st = p == null ? -1 : (int)p.CompleteState;
        AiTap("path", j => { j.Prop("fake", fake); j.Prop("err", err); j.Prop("st", st); WritePoints(j, "v", p); });
    }

    public static bool TapCloseEnough(Vector3 approachPoint, Vector3 position, Vector3 eyePosition, float approachRadius, bool needLOS, float ignoreBlockerRadius)
    {
        var v = UnitCommand.IsUnitCloseEnough(approachPoint, position, eyePosition, approachRadius, needLOS, ignoreBlockerRadius);
        AiTap("pclose", j => { j.Prop("r", F(approachRadius)); j.Prop("v", v); });
        return v;
    }

    public static float TapMagnitude(ref Vector3 v)
    {
        var m = v.magnitude;
        AiTap("mag", j => j.Prop("v", F(m)));
        return m;
    }

    public static float TapPathLength(Pathfinding.Path p)
    {
        var v = p.GetTotalLength();
        AiTap("plen", j => j.Prop("v", F(v)));
        return v;
    }

    public static float TapFiveFootRange(TurnController t, UnitEntityData u)
    {
        var v = t.GetRemainingFiveFootStepRange(u);
        var id = U(u);
        AiTap("r5", j => { j.Prop("u", id); j.Prop("v", F(v)); });
        return v;
    }

    public static bool TapHasFiveFoot(TurnController t, UnitEntityData u)
    {
        var v = t.HasFiveFootStep(u);
        var id = U(u);
        AiTap("h5", j => { j.Prop("u", id); j.Prop("v", v); });
        return v;
    }

    public static bool TapFullRound(UnitEntityData u)
    {
        var v = u.HasFullRoundAction();
        var id = U(u);
        AiTap("hfra", j => { j.Prop("u", id); j.Prop("v", v); });
        return v;
    }

    // One step of ScorePath's movement simulation: its result and the simulator after it.
    public static bool TapSimulateTick(UnitMovementAgent a, ref MovementSimulator sim, float deltaTime, float maxMoveLength)
    {
        var v = a.SimulateTickMovementForTurnBased(ref sim, deltaTime, maxMoveLength);
        var pos = sim.Position;
        var len = sim.LastMoveLength;
        var rmd = sim.ReachedMaxDistance;
        var wi = sim.WaypointIndex;
        AiTap("msim", j =>
        {
            j.Prop("max", F(maxMoveLength)); j.Prop("v", v); j.Key("p"); j.Vec(pos); j.Prop("len", F(len)); j.Prop("rmd", rmd);
            j.Prop("wi", wi);
        });
        return v;
    }

    // GridGraph.GetNearest(position): the node's AllyBlocked (null without a node).
    public static NNInfoInternal TapNearest(NavGraph g, Vector3 position)
    {
        var v = g.GetNearest(position);
        int? blocked = v.node == null ? (int?)null : v.node.AllyBlocked;
        AiTap("ablk", j => { j.Key("v"); if (blocked.HasValue) j.W.WriteValue(blocked.Value); else j.W.WriteNull(); });
        return v;
    }

    public static bool TapAiEnabled(UnitEntityData u)
    {
        var v = u.IsAIEnabled;
        var id = U(u);
        AiTap("aien", j => { j.Prop("u", id); j.Prop("v", v); });
        return v;
    }

    public static int TapDifficulty(Kingmaker.Settings.Difficulty.DifficultyPresetsController c, Kingmaker.Settings.GameDifficultyOption o)
    {
        var v = c.CurrentDifficultyStatAdjustmentCompareTo(o);
        AiTap("diff", j => { j.Prop("o", (int)o); j.Prop("v", v); });
        return v;
    }

    // An action's blueprint GetRange(context) (AiActionSimple.GetRange): the weapon's reach, or an ability's.
    public static float TapRange(BlueprintAiAction b, DecisionContext context)
    {
        var v = b.GetRange(context);
        var bp = Guid(b);
        AiTap("range", j => { j.Prop("a", bp); j.Prop("v", F(v)); });
        return v;
    }

    // EngagementSimulator.CheckForAttacksOfOpportunity(newPosition) (internal: a postfix).
    public static void AooPostfix(int __result, MethodBase __originalMethod)
    {
        try
        {
            if (!AiChain(__originalMethod)) return;
            Emit("aoo", j => j.Prop("v", __result));
        }
        catch (Exception e) { Trace.HookFailed("observe uc_v aoo", e); }
    }

    // ---- checks ----

    public static void ScorePostfix(DecisionContext context, MethodBase __originalMethod)
    {
        try
        {
            if (!AiChain(__originalMethod)) return;
            var fn = __originalMethod.Name;
            var a = Guid(context.Action?.Blueprint);
            var t = U(context.Target?.Unit);
            var s = D(context.CurrentScore);
            var b = D(context.BestScore);
            var path = fn == nameof(AiAction.ScorePath);
            var five = context.CurrentEnableFiveFootStep;
            var p = path ? context.CurrentPath : null;
            Sched.Obs("ai_sc", j =>
            {
                j.Prop("fn", fn); j.Prop("a", a); j.Prop("tgt", t); j.Prop("s", s); j.Prop("b", b);
                if (path) { j.Prop("five", five); WritePoints(j, "path", p); }
            });
        }
        catch (Exception e) { Trace.HookFailed("observe ai_sc", e); }
    }

    public static void BestPostfix(UnitEntityData unit, DecisionContext context, ref AiAction bestActionResult, ref UnitEntityData bestTargetResult,
        ref bool isAutoUseAbility, MethodBase __originalMethod)
    {
        try
        {
            if (!AiChain(__originalMethod)) return;
            var u = U(unit);
            var a = Guid(bestActionResult?.Blueprint);
            var t = U(bestTargetResult);
            var best = D(context.BestScore);
            var cur = D(context.CurrentScore);
            var five = context.BestEnableFiveFootStep;
            var p = context.BestPath;
            var auto = isAutoUseAbility;
            Sched.Obs("ai_best", j =>
            {
                j.Prop("u", u); j.Prop("a", a); j.Prop("tgt", t); j.Prop("best", best); j.Prop("cur", cur); j.Prop("auto", auto);
                j.Prop("five", five); WritePoints(j, "path", p);
            });
        }
        catch (Exception e) { Trace.HookFailed("observe ai_best", e); }
    }

    public static void SelectPostfix(UnitEntityData unit, bool __result, MethodBase __originalMethod)
    {
        try
        {
            if (!AiChain(__originalMethod)) return;
            var u = U(unit);
            var nct = unit.CombatState.AIData.NextCommandTime;
            Sched.Obs("ai_sel", j => { j.Prop("u", u); j.Prop("ret", __result); j.Prop("nct", F(nct)); });
        }
        catch (Exception e) { Trace.HookFailed("observe ai_sel", e); }
    }

    // ---- install ----

    // A call to tap: matched by its declaring type, name and parameter types (or, for a generic method, its type
    // argument), in the method `method` of `owner`.
    private sealed class AiSite
    {
        public Type Owner; public string Method; public Type Type; public string Name; public Type[] Args; public Type Generic; public string Tap;
        public int Want;
    }

    private static readonly List<AiSite> AiSites = new List<AiSite>();

    public static IEnumerable<CodeInstruction> AiTapTranspiler(IEnumerable<CodeInstruction> instructions, MethodBase original)
    {
        foreach (var i in instructions)
        {
            if ((i.opcode == OpCodes.Call || i.opcode == OpCodes.Callvirt) && i.operand is MethodInfo mb)
            {
                foreach (var s in AiSites)
                {
                    if (s.Owner != original.DeclaringType || s.Method != original.Name || mb.Name != s.Name) continue;
                    if (s.Generic != null)
                    {
                        if (!mb.IsGenericMethod || mb.DeclaringType != s.Type || mb.GetGenericArguments()[0] != s.Generic) continue;
                    }
                    else if (mb.DeclaringType != s.Type || !mb.GetParameters().Select(p => p.ParameterType).SequenceEqual(s.Args)) continue;
                    i.opcode = OpCodes.Call;
                    i.operand = AccessTools.Method(typeof(CommandObs), s.Tap);
                    var k = s.Owner.Name + "." + s.Method + ":" + s.Tap;
                    Tapped[k] = Tapped.TryGetValue(k, out var n) ? n + 1 : 1;
                    break;
                }
            }
            yield return i;
        }
    }

    private static void InstallAi(Harmony h, Action<MethodBase, string, string, string> patch)
    {
        var all = BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        IntPtr H(MethodBase m) => m?.MethodHandle.Value ?? IntPtr.Zero;
        // The method declared on `t` with that name (null, reported, when there are none or several).
        MethodBase M(Type t, string name)
        {
            var ms = t?.GetMethods(all).Where(m => m.Name == name).ToList();
            if (ms == null || ms.Count != 1) { Health.Failed("ai observe: " + t?.Name + "." + name + ": " + (ms?.Count ?? 0) + " methods", new Exception("ai")); return null; }
            return ms[0];
        }
        var ai = typeof(AiBrainController);
        var engagementSimulator = AccessTools.TypeByName("Kingmaker.Controllers.Combat.EngagementSimulator");

        // Entries.
        var tick = M(ai, nameof(AiBrainController.Tick));
        var forceTick = M(ai, nameof(AiBrainController.ForceTick));
        if (tick == null || forceTick == null) { Health.Failed("ai observe: AiBrainController's ticks are missing", new Exception("ai")); return; }
        EntrySet.Add(H(tick));
        EntrySet.Add(H(forceTick));

        // Base code: every method declared on the AI's types and their nested (compiler-generated) types, and
        // UnitAttack.CreateAttackCommand (BlueprintAiAttack.CreateCommand's).
        var types = new List<Type>
        {
            ai, typeof(DecisionContext), typeof(TargetInfo), typeof(AiAction), typeof(AiActionSimple), typeof(BlueprintAiAction),
            AccessTools.TypeByName("Kingmaker.AI.Blueprints.BlueprintAiAttack"), typeof(ComplexConsideration), typeof(InRangeConsideration),
            typeof(LifeStateConsideration), typeof(FactConsideration), typeof(NotImpatientConsideration),
        };
        if (types.Any(t => t == null) || engagementSimulator == null) { Health.Failed("ai observe: a type is missing", new Exception("ai")); return; }
        var attack = types[6];
        for (var k = 0; k < types.Count; k++) types.AddRange(types[k].GetNestedTypes(BindingFlags.Public | BindingFlags.NonPublic));
        var n0 = BaseSet.Count;
        foreach (var t in types)
            foreach (var m in t.GetMethods(all).Cast<MethodBase>().Concat(t.GetConstructors(all)))
                if (!m.IsAbstract) BaseSet.Add(H(m));
        if (BaseSet.Count - n0 < 60) Health.Failed("ai observe: the AI's methods not found", new Exception((BaseSet.Count - n0).ToString()));
        // FixPathIfTooShort is an input (its tap records the path ScorePath goes on with): the reads inside it (the
        // agent's position, ActionRangeWithCorpulence's corpulences) depend on the path it was given, which isn't recorded.
        var fixPath = M(typeof(AiAction), "FixPathIfTooShort");
        if (fixPath == null) Health.Failed("ai observe: AiAction.FixPathIfTooShort is missing", new Exception("ai"));
        else BaseSet.Remove(H(fixPath));
        var createAttack = M(typeof(UnitAttack), nameof(UnitAttack.CreateAttackCommand));
        if (createAttack == null) Health.Failed("ai observe: UnitAttack.CreateAttackCommand is missing", new Exception("ai"));
        else BaseSet.Add(H(createAttack));

        // Taps.
        void S(Type owner, string method, Type type, string name, string tap, int want, params Type[] args) =>
            AiSites.Add(new AiSite { Owner = owner, Method = method, Type = type, Name = name, Args = args, Tap = tap, Want = want });
        void G(Type owner, string method, Type generic, string tap, int want) =>
            AiSites.Add(new AiSite { Owner = owner, Method = method, Type = typeof(EntityDataBase), Name = "Get", Generic = generic, Tap = tap, Want = want });
        var ue = typeof(UnitEntityData);
        var cs0 = typeof(UnitCombatState);
        var hs = typeof(HashSet<UnitEntityData>);
        var cs = typeof(UnitCombatState);
        var tc = typeof(TurnController);
        S(ai, "Tick", typeof(CombatController), "get_WaitingForUI", nameof(TapWaitingForUI), 1);
        S(ai, "ForceTick", typeof(Game), "get_ReadyForCombatUnitGroups", nameof(TapGroups), 1);
        S(ai, "TickBrain", ue, "get_IsSleeping", nameof(TapSleeping), 1);
        S(ai, "TickBrain", typeof(ObservableCountFlag), "op_Implicit", nameof(TapPassive), 1, typeof(ObservableCountFlag));
        S(ai, "TickBrain", typeof(CombatAiData), "get_NextCommandTime", nameof(TapNct), 2);
        S(ai, "TickBrain", typeof(Time), "get_time", nameof(TapTime), 2);
        S(ai, "CanInterrupt", typeof(UnitCommand), "get_AiCanInterrupt", nameof(TapAiCanInterrupt), 1);
        S(ai, "SelectAction", typeof(Time), "get_time", nameof(TapTime), 1);
        var aid = typeof(CombatAiData);
        S(ai, "SelectAction", aid, "set_NextCommandTime", nameof(TapSetNctSelect), 2, typeof(float));
        S(cs0, "Engage", aid, "set_NextCommandTime", nameof(TapSetNctEngage), 2, typeof(float));
        S(cs0, "Disengage", aid, "set_NextCommandTime", nameof(TapSetNctDisengage), 1, typeof(float));
        S(typeof(UnitCommands), "UpdateCombatTarget", aid, "set_NextCommandTime", nameof(TapSetNctUpdate), 1, typeof(float));
        S(typeof(UnitBrain), "set_AutoUseAbility", aid, "set_NextCommandTime", nameof(TapSetNctAuto), 1, typeof(float));
        G(ai, "FindBestAction", typeof(UnitPartConfusion), nameof(TapConfusion), 1);
        S(ai, "FindBestAction", typeof(UnitBrain), "GetAvailableAutoUseAbility", nameof(TapAuto), 1);
        S(ai, "CalculateTargetsScore", typeof(UnitBrain), "get_PriorityTargetFact", nameof(TapPriorityFact), 1);
        S(ai, "CalculateTargetsScore", hs, "Contains", nameof(TapDisliked), 1, ue);
        S(ai, "CalculateTargetsScore", typeof(CombatAiData), "get_ImpatiencePriorityTarget", nameof(TapImpatienceTarget), 2);
        var dc = typeof(DecisionContext);
        G(dc, "CalculateEnemies", typeof(UnitPartPersonalEnemy), nameof(TapPersonalEnemy), 2);
        S(dc, "CalculateEnemies", typeof(UnitGroupMemory), "get_Enemies", nameof(TapAiEnemies), 1);
        S(dc, "CalculateEnemies", typeof(UnitGroupMemory), "HasPlayerCharacterInMemory", nameof(TapHasPlayerCharacter), 1);
        S(dc, "CalculateEnemies", cs, "get_ManualTarget", nameof(TapManual), 1);
        S(dc, "get_ActionRangeWithCorpulence", typeof(UnitEntityView), "get_Corpulence", nameof(TapCorpulence), 2);
        var ti = typeof(TargetInfo);
        S(ti, "UpdateDistance", ue, "DistanceTo", nameof(TapDistance), 1, ue);
        S(ti, "InLos", typeof(LineOfSightGeometry), "HasObstacle", nameof(TapObstacle), 1, typeof(Vector3), typeof(Vector3), typeof(float));
        S(ti, "FillWeight", cs, "get_ManualTarget", nameof(TapManual), 2);
        S(ti, "FillWeight", typeof(UnitHelper), "GetRider", nameof(TapRider), 1, ue);
        S(ti, "FillWeight", typeof(UnitEntityView), "get_Corpulence", nameof(TapCorpulence), 2);
        S(ti, "FillWeight", cs, "get_LastTarget", nameof(TapLastTarget), 1);
        var aa = typeof(AiAction);
        S(aa, "IsTargetValidInternal", ue, "get_AiMovementForbidden", nameof(TapMovementForbidden), 1);
        S(aa, "IsTargetValidInternal", hs, "Contains", nameof(TapUnreachable), 1, ue);
        S(aa, "ScorePath", ue, "get_CurrentSpeedMps", nameof(TapSpeed), 1);
        S(aa, "ScorePath", tc, "GetRemainingMovementRange", nameof(TapMovementRange), 1, ue, typeof(bool), typeof(bool));
        S(aa, "ScorePath", aa, "FixPathIfTooShort", nameof(TapFixPath), 1, dc, typeof(Vector3));
        S(aa, "ScorePath", typeof(UnitCommand), "IsUnitCloseEnough", nameof(TapCloseEnough), 1,
            typeof(Vector3), typeof(Vector3), typeof(Vector3), typeof(float), typeof(bool), typeof(float));
        S(aa, "ScorePath", typeof(Vector3), "get_magnitude", nameof(TapMagnitude), 1);
        S(aa, "ScorePath", typeof(Pathfinding.Path), "GetTotalLength", nameof(TapPathLength), 1);
        S(aa, "ScorePath", tc, "GetRemainingFiveFootStepRange", nameof(TapFiveFootRange), 1, ue);
        S(aa, "ScorePath", tc, "HasFiveFootStep", nameof(TapHasFiveFoot), 1, ue);
        S(aa, "ScorePath", ue, "HasFullRoundAction", nameof(TapFullRound), 1);
        S(aa, "ScorePath", typeof(UnitMovementAgent), "SimulateTickMovementForTurnBased", nameof(TapSimulateTick), 1,
            typeof(MovementSimulator).MakeByRefType(), typeof(float), typeof(float));
        S(aa, "ScorePath", typeof(NavGraph), "GetNearest", nameof(TapNearest), 2, typeof(Vector3));
        S(typeof(AiActionSimple), "GetRange", typeof(BlueprintAiAction), "GetRange", nameof(TapRange), 1, dc);
        var ba = typeof(BlueprintAiAction);
        S(ba, "IsActorValid", ue, "get_IsAIEnabled", nameof(TapAiEnabled), 1);
        S(ba, "IsActorValid", typeof(Kingmaker.Settings.Difficulty.DifficultyPresetsController), "CurrentDifficultyStatAdjustmentCompareTo",
            nameof(TapDifficulty), 2, typeof(Kingmaker.Settings.GameDifficultyOption));
        S(attack, "CreateCommand", typeof(UnitBrain), "GetAvailableAutoUseAbility", nameof(TapAuto), 1);
        S(attack, "CanKeepCommand", typeof(UnitBrain), "GetAvailableAutoUseAbility", nameof(TapAuto), 1);
        G(typeof(UnitAttack), "CreateAttackCommand", typeof(UnitPartMagus), nameof(TapMagus), 1);

        var tr = new HarmonyMethod(typeof(CommandObs), nameof(AiTapTranspiler));
        foreach (var (owner, method) in AiSites.Select(s => (s.Owner, s.Method)).Distinct().ToList())
        {
            var m = M(owner, method);
            if (m == null) { Health.Failed("ai observe: " + owner.Name + "." + method + " is missing", new Exception("ai")); continue; }
            try { h.Patch(m, transpiler: tr); }
            catch (Exception e) { Health.Failed("patch " + owner.Name + "." + method + " (ai taps)", e); }
        }
        foreach (var s in AiSites.GroupBy(s => s.Owner.Name + "." + s.Method + ":" + s.Tap))
        {
            var want = s.Sum(x => x.Want);
            if (!Tapped.TryGetValue(s.Key, out var n) || n != want)
                Health.Failed("ai observe: tap " + s.Key + " replaced " + (Tapped.TryGetValue(s.Key, out var x) ? x : 0) + " calls, not " + want, new Exception("ai"));
        }

        patch(tick, nameof(AiEntryPrefix), null, nameof(AiEntryFinalizer));
        patch(forceTick, nameof(AiEntryPrefix), null, nameof(AiEntryFinalizer));
        patch(M(engagementSimulator, "CheckForAttacksOfOpportunity"), null, nameof(AooPostfix), null);
        foreach (var name in new[] { nameof(AiAction.ScoreActor), nameof(AiAction.ScoreTarget), nameof(AiAction.ScorePath) })
            patch(M(aa, name), null, nameof(ScorePostfix), null);
        patch(M(ai, "FindBestAction"), null, nameof(BestPostfix), null);
        patch(M(ai, nameof(AiBrainController.SelectAction)), null, nameof(SelectPostfix), null);
        AiOn = true;
    }
}

using System;
using System.Collections.Generic;
using System.Reflection;
using System.Reflection.Emit;
using HarmonyLib;
using Kingmaker.EntitySystem.Entities;

namespace WrathOracle;

// The initiative comparers' identity-hash decisions (observer "init_ties").
//
// CombatController.UnitsOrderComaprer.Compare (SortUnits' OrderBy) and
// UnitCombatPrepareController.UnitEntityOrderComaprer.Compare (Tick's List.Sort) end, when everything
// else ties, with `unit.GetHashCode() >= unit2.GetHashCode() ? -1 : 1`. UnitEntityData.GetHashCode is
// base.GetHashCode(), which EntityDataBase doesn't override: the object's identity hash, which Mono
// computes from the object's address the first time it is asked for and then keeps. Asking for it
// earlier than the game would could change the value the game gets, so nothing here asks for it: the
// comparers' own two GetHashCode calls are replaced (transpiler) by TieHash, which makes exactly that
// call through WallClock.UnitHash (the native call, the recording's value in a replay: the hash tape,
// unchanged) and notes the value it returned and the unit it was called on (UniqueId, an
// auto-property). A postfix on Compare then knows whether the comparer reached its last branch (both
// calls made) and writes one "obs" record per decision, in the window (Sched.On):
//   init_tie: cmp ("turn": UnitsOrderComaprer, "prepare": UnitEntityOrderComaprer), i (the decision's
//   index within its sort: SortUnits' call, or Tick's call, each reset by a prefix), a and b (the
//   units hashed, in call order: the comparer's `unit` and `unit2`), ha and hb (the values the
//   comparer got), r (Compare's result).
// A Compare that made one call, or more than two, or whose result isn't the hashes' (ha >= hb ? -1 :
// 1), or that threw after a hash call, writes init_tie_bad (why) instead: the recording then gives the
// sim nothing to follow. Field reads only; nothing the game evaluates is called.
// The end record counts, per comparer, its Compare calls in the window and the decisions among them
// (init_tie_compares_turn/_prepare, init_ties_turn/_prepare), so a window without records shows
// whether the comparers ran at all.
internal static class TieObs
{
    internal static bool Installed;

    // The comparer currently running (null: none), its hash calls so far, and what they returned.
    private static string _cmp;
    private static int _n;
    private static string _a, _b;
    private static int _ha, _hb;
    // Decisions so far in the current sort call of each comparer.
    private static int _turnIndex, _prepareIndex;
    // GetHashCode calls the transpiler replaced (per patched method).
    private static int _replaced;
    // In the window: each comparer's Compare calls that returned, and the decisions among them.
    public static long TurnCompares, PrepareCompares, TurnTies, PrepareTies;

    public static void ResetWindow() => TurnCompares = PrepareCompares = TurnTies = PrepareTies = 0;

    public static void Install(Harmony h)
    {
        try
        {
            var turn = AccessTools.Method(AccessTools.TypeByName("TurnBased.Controllers.CombatController+UnitsOrderComaprer"), "Compare")
                       ?? throw new MissingMethodException("CombatController.UnitsOrderComaprer.Compare");
            var prep = AccessTools.Method(AccessTools.TypeByName("Kingmaker.Controllers.Combat.UnitCombatPrepareController+UnitEntityOrderComaprer"), "Compare")
                       ?? throw new MissingMethodException("UnitCombatPrepareController.UnitEntityOrderComaprer.Compare");
            var sort = AccessTools.Method(typeof(global::TurnBased.Controllers.CombatController), "SortUnits")
                       ?? throw new MissingMethodException("CombatController.SortUnits");
            var tick = AccessTools.Method(typeof(Kingmaker.Controllers.Combat.UnitCombatPrepareController), "Tick")
                       ?? throw new MissingMethodException("UnitCombatPrepareController.Tick");
            var tr = new HarmonyMethod(typeof(TieObs), nameof(Transpiler));
            var fin = new HarmonyMethod(typeof(TieObs), nameof(CompareFinalizer));
            _replaced = 0;
            h.Patch(turn, prefix: new HarmonyMethod(typeof(TieObs), nameof(TurnPrefix)), postfix: new HarmonyMethod(typeof(TieObs), nameof(ComparePostfix)), transpiler: tr, finalizer: fin);
            if (_replaced != 2) throw new InvalidOperationException($"UnitsOrderComaprer.Compare: {_replaced} GetHashCode calls replaced, expected 2");
            _replaced = 0;
            h.Patch(prep, prefix: new HarmonyMethod(typeof(TieObs), nameof(PreparePrefix)), postfix: new HarmonyMethod(typeof(TieObs), nameof(ComparePostfix)), transpiler: tr, finalizer: fin);
            if (_replaced != 2) throw new InvalidOperationException($"UnitEntityOrderComaprer.Compare: {_replaced} GetHashCode calls replaced, expected 2");
            h.Patch(sort, prefix: new HarmonyMethod(typeof(TieObs), nameof(SortUnitsPrefix)));
            h.Patch(tick, prefix: new HarmonyMethod(typeof(TieObs), nameof(PrepareTickPrefix)));
            Installed = true;
        }
        catch (Exception e) { Health.Failed("patch the initiative comparers' hash tie-break (init_ties)", e); }
    }

    // Each GetHashCode() call in the comparer becomes TieHash(object): same stack, the same native call.
    public static IEnumerable<CodeInstruction> Transpiler(IEnumerable<CodeInstruction> ins)
    {
        var repl = AccessTools.Method(typeof(TieObs), nameof(TieHash));
        foreach (var i in ins)
        {
            if ((i.opcode == OpCodes.Callvirt || i.opcode == OpCodes.Call) && i.operand is MethodBase mb && mb.Name == "GetHashCode" && mb.GetParameters().Length == 0)
            {
                i.opcode = OpCodes.Call;
                i.operand = repl;
                _replaced++;
            }
            yield return i;
        }
    }

    public static int TieHash(object o)
    {
        var h = WallClock.UnitHash(o);
        if (_cmp != null)
        {
            _n++;
            var id = (o as UnitEntityData)?.UniqueId;
            if (_n == 1) { _a = id; _ha = h; }
            else if (_n == 2) { _b = id; _hb = h; }
        }
        return h;
    }

    public static void TurnPrefix() => Begin("turn");
    public static void PreparePrefix() => Begin("prepare");
    public static void SortUnitsPrefix() => _turnIndex = 0;
    public static void PrepareTickPrefix() => _prepareIndex = 0;

    private static void Begin(string cmp)
    {
        _cmp = cmp;
        _n = 0;
        _a = _b = null;
        _ha = _hb = 0;
    }

    public static void ComparePostfix(int __result)
    {
        var cmp = _cmp;
        _cmp = null;
        if (cmp == null) return;
        var on = Sched.On;
        if (on)
        {
            if (cmp == "turn") TurnCompares++;
            else PrepareCompares++;
        }
        if (_n == 0) return;
        var i = cmp == "turn" ? _turnIndex++ : _prepareIndex++;
        if (!on) return;
        if (cmp == "turn") TurnTies++;
        else PrepareTies++;
        try
        {
            string why = null;
            if (_n != 2) why = $"{_n} hash calls";
            else if (__result != (_ha >= _hb ? -1 : 1)) why = $"result {__result} for hashes {_ha}, {_hb}";
            if (why != null)
            {
                Sched.Obs("init_tie_bad", j => { j.Prop("cmp", cmp); j.Prop("why", why); });
                return;
            }
            string a = _a, b = _b;
            int ha = _ha, hb = _hb, r = __result;
            Sched.Obs("init_tie", j =>
            {
                j.Prop("cmp", cmp);
                j.Prop("i", i);
                j.Key("a"); if (a != null) j.W.WriteValue(a); else j.W.WriteNull();
                j.Key("b"); if (b != null) j.W.WriteValue(b); else j.W.WriteNull();
                j.Prop("ha", ha);
                j.Prop("hb", hb);
                j.Prop("r", r);
            });
        }
        catch (Exception e) { Trace.HookFailed("init_tie", e); }
    }

    // A Compare that threw: the postfix didn't run.
    public static void CompareFinalizer(Exception __exception)
    {
        if (__exception == null) return;
        var cmp = _cmp;
        var n = _n;
        _cmp = null;
        if (cmp == null || !Sched.On) return;
        Sched.Obs("init_tie_bad", j => { j.Prop("cmp", cmp); j.Prop("why", $"Compare threw after {n} hash calls"); });
    }
}

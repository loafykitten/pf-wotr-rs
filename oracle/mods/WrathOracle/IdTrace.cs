using System;
using System.Collections;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.CompilerServices;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Blueprints.Facts;
using Kingmaker.EntitySystem;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.UnitLogic.ActivatableAbilities;

namespace WrathOracle;

// Which code allocates each game unique id (Player.GetNewUniqueId) - Wrath-Rust POC-5, for
// reproducing the ids that units created at load (placeholders' BecomeUnit) give their facts.
// With the ids, in one sequence ("seq"), two load events (Wrath-Rust POC-3, Manual_10: why the
// toggles of some companions are stopped during the load and restarted by the controller):
//   "aa_stop"  ActivatableAbility.Stop on a started toggle (the only call that stops it): the
//              owner, the toggle's blueprint and its m_IsOn when the call began;
//   "in_game"  a unit's IsInGame setter changing the value: the unit, from, to.
// Each with the calling methods, innermost first. Field reads only (no getter of the game's runs
// in the hooks); an event whose fields can't be read is not written and the run's health fails.
// Plan steps (fidelity plans; the hooks are installed with the fidelity instrumentation):
//   idtrace:on            start recording (e.g. before a reload)
//   idtrace:write:LABEL   write ids-LABEL.json ("ids" and "events", schema v2) and stop recording
internal static class IdTrace
{
    private sealed class Rec
    {
        public int Seq;
        public string Id;
        public string Kind;
        public string Unit;
        public string Bp;
        public bool On;
        public bool From;
        public bool To;
        public string[] Stack;
    }

    private static bool _on;
    private static int _seq;
    private static readonly List<Rec> Ids = new List<Rec>();
    private static readonly List<Rec> Events = new List<Rec>();

    private static AccessTools.FieldRef<ActivatableAbility, bool> _isOn;
    private static AccessTools.FieldRef<ActivatableAbility, bool> _isStarted;
    private static AccessTools.FieldRef<EntityFact, EntityFactsManager> _manager;
    private static AccessTools.FieldRef<EntityFact, BlueprintFact> _blueprint;
    private static AccessTools.FieldRef<EntityFactsManager, EntityDataBase> _owner;
    private static AccessTools.FieldRef<EntityDataBase, bool> _inGame;

    // Not inlined: its callers skip a fixed number of frames.
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static string[] Stack(int skip, int max)
    {
        var st = new StackTrace(skip, false);
        var n = Math.Min(st.FrameCount, max);
        var frames = new string[n];
        for (var i = 0; i < n; i++)
        {
            var m = Rng.FrameMethod(st.GetFrame(i));
            frames[i] = m == null ? "?" : (m.DeclaringType?.FullName ?? "?") + "." + m.Name;
        }
        return frames;
    }

    public static void Postfix(string __result)
    {
        if (!_on) return;
        try
        {
            Ids.Add(new Rec { Seq = _seq++, Id = __result, Stack = Stack(2, 24) });
        }
        catch (Exception e)
        {
            Health.Failed("idtrace", e);
        }
    }

    // ActivatableAbility.Stop(bool): only a started toggle is stopped.
    public static void StopPrefix(ActivatableAbility __instance)
    {
        if (!_on) return;
        try
        {
            if (!_isStarted(__instance)) return;
            var owner = _manager(__instance) is EntityFactsManager m ? _owner(m) : null;
            var bp = _blueprint(__instance);
            if (owner == null || bp == null) throw new InvalidOperationException("a started toggle without an owner or blueprint");
            Events.Add(new Rec
            {
                Seq = _seq++,
                Kind = "aa_stop",
                Unit = owner.UniqueId,
                Bp = bp.AssetGuid.ToString(),
                On = _isOn(__instance),
                Stack = Stack(2, 32),
            });
        }
        catch (Exception e)
        {
            Health.Failed("idtrace aa_stop", e);
        }
    }

    // EntityDataBase.IsInGame's setter, for units: a change only.
    public static void InGamePrefix(EntityDataBase __instance, bool value)
    {
        if (!_on || !(__instance is UnitEntityData)) return;
        try
        {
            var from = _inGame(__instance);
            if (from == value) return;
            Events.Add(new Rec
            {
                Seq = _seq++,
                Kind = "in_game",
                Unit = __instance.UniqueId,
                From = from,
                To = value,
                Stack = Stack(2, 32),
            });
        }
        catch (Exception e)
        {
            Health.Failed("idtrace in_game", e);
        }
    }

    // Plan end or abort: stop recording and drop what was not written.
    public static void Stop()
    {
        _on = false;
        Ids.Clear();
        Events.Clear();
    }

    public static void Install(Harmony h)
    {
        try
        {
            h.Patch(AccessTools.Method(typeof(Player), nameof(Player.GetNewUniqueId)), postfix: new HarmonyMethod(typeof(IdTrace), nameof(Postfix)));
        }
        catch (Exception e)
        {
            Health.Failed("patch GetNewUniqueId", e);
        }
        try
        {
            _isOn = AccessTools.FieldRefAccess<ActivatableAbility, bool>("m_IsOn");
            _isStarted = AccessTools.FieldRefAccess<ActivatableAbility, bool>("<IsStarted>k__BackingField");
            _manager = AccessTools.FieldRefAccess<EntityFact, EntityFactsManager>("<Manager>k__BackingField");
            _blueprint = AccessTools.FieldRefAccess<EntityFact, BlueprintFact>("<Blueprint>k__BackingField");
            _owner = AccessTools.FieldRefAccess<EntityFactsManager, EntityDataBase>("<Owner>k__BackingField");
            _inGame = AccessTools.FieldRefAccess<EntityDataBase, bool>("m_IsInGame");
            var stop = AccessTools.Method(typeof(ActivatableAbility), nameof(ActivatableAbility.Stop)) ?? throw new MissingMethodException("ActivatableAbility.Stop");
            var inGame = AccessTools.PropertySetter(typeof(EntityDataBase), nameof(EntityDataBase.IsInGame)) ?? throw new MissingMethodException("EntityDataBase.IsInGame setter");
            h.Patch(stop, prefix: new HarmonyMethod(typeof(IdTrace), nameof(StopPrefix)));
            h.Patch(inGame, prefix: new HarmonyMethod(typeof(IdTrace), nameof(InGamePrefix)));
        }
        catch (Exception e)
        {
            Health.Failed("patch idtrace events (ActivatableAbility.Stop, IsInGame)", e);
        }
    }

    public static IEnumerator Step(string arg, Runner runner)
    {
        if (arg == "on")
        {
            Ids.Clear();
            Events.Clear();
            _seq = 0;
            _on = true;
            yield break;
        }
        if (arg != null && arg.StartsWith("write:", StringComparison.Ordinal))
        {
            _on = false;
            var label = arg.Substring(6);
            using (var j = new Json(Path.Combine(runner.Plan.Out, "ids-" + label + ".json"), indent: true))
            {
                j.Obj();
                j.Prop("schema", "wrath-oracle-ids-v2");
                j.Arr("ids");
                foreach (var r in Ids)
                {
                    j.Obj();
                    j.Prop("seq", r.Seq);
                    j.Prop("id", r.Id);
                    j.Arr("stack");
                    foreach (var f in r.Stack) j.W.WriteValue(f);
                    j.EndArr();
                    j.End();
                }
                j.EndArr();
                j.Arr("events");
                foreach (var r in Events)
                {
                    j.Obj();
                    j.Prop("seq", r.Seq);
                    j.Prop("kind", r.Kind);
                    j.Prop("unit", r.Unit);
                    if (r.Kind == "aa_stop")
                    {
                        j.Prop("bp", r.Bp);
                        j.Prop("on", r.On);
                    }
                    else
                    {
                        j.Prop("from", r.From);
                        j.Prop("to", r.To);
                    }
                    j.Arr("stack");
                    foreach (var f in r.Stack) j.W.WriteValue(f);
                    j.EndArr();
                    j.End();
                }
                j.EndArr();
                j.End();
            }
            Ids.Clear();
            Events.Clear();
            yield break;
        }
        throw new ArgumentException("idtrace:on or idtrace:write:LABEL");
    }
}

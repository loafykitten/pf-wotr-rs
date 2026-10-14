using System;
using System.Collections.Generic;
using HarmonyLib;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.Visual.Animation.Kingmaker;
using Kingmaker.Visual.Animation.Kingmaker.Actions;

namespace WrathOracle;

// The idle-animation tracker each unit carries into a turn-based window, pinned in replays.
//
// UnitAnimationManager.m_RandomIdleTracker decides when a unit starts an idle variant or, in combat, a
// micro idle: it accumulates game time and fires once its curve has eaten a Random.value it drew when it
// was made. Units make theirs while the save loads, from a generator whose draws during loading depend on
// how long the load took, so the draw (and so the micro idles inside the window, whose own draws are T1)
// differs by run [M: poc5-tb-42, the same tracker times, other draws in one replay; a micro idle at tick
// 578 in the recording, none in that replay]. A recording observes each unit's tracker at the window's
// first tick ("idle0": its time, its draw, which curve it runs (the variant idle's or the micro idle's) and
// whether an idle animation is still playing, which holds the tracker back). A turn-based replay sets the same
// unit's tracker to the recorded time and draw at the same point, before its own observation, but only when
// the curve and the playing state are the recording's too; otherwise (or with a tracker on one side only)
// nothing is pinned and the difference is noted ("idle0_unpinned"). These draws are T1 either way.
internal static class IdlePin
{
    public struct State { public float Time, Random; public string Curve; public bool Playing; }

    private static readonly Dictionary<string, State> Tape = new Dictionary<string, State>();

    public static void Reset() => Tape.Clear();
    public static void Add(string unit, State st) => Tape[unit] = st;

    private static readonly AccessTools.FieldRef<UnitAnimationManager, TimedProbabilityCurve.Tracker> Tracker =
        AccessTools.FieldRefAccess<UnitAnimationManager, TimedProbabilityCurve.Tracker>("m_RandomIdleTracker");
    private static readonly AccessTools.FieldRef<TimedProbabilityCurve.Tracker, float> Time =
        AccessTools.FieldRefAccess<TimedProbabilityCurve.Tracker, float>("m_Time");
    private static readonly AccessTools.FieldRef<TimedProbabilityCurve.Tracker, float> Random =
        AccessTools.FieldRefAccess<TimedProbabilityCurve.Tracker, float>("m_Random");
    private static readonly AccessTools.FieldRef<TimedProbabilityCurve.Tracker, TimedProbabilityCurve> Curve =
        AccessTools.FieldRefAccess<TimedProbabilityCurve.Tracker, TimedProbabilityCurve>("m_Curve");
    private static readonly AccessTools.FieldRef<UnitAnimationManager, UnitAnimationActionHandle> MicroIdleHandle =
        AccessTools.FieldRefAccess<UnitAnimationManager, UnitAnimationActionHandle>("m_MicroIdleHandle");
    private static readonly AccessTools.FieldRef<UnitAnimationManager, UnitAnimationActionMicroIdle> MicroIdle =
        AccessTools.FieldRefAccess<UnitAnimationManager, UnitAnimationActionMicroIdle>("m_MicroIdle");
    private static readonly AccessTools.FieldRef<UnitAnimationManager, UnitAnimationActionVariantIdle> VariantIdle =
        AccessTools.FieldRefAccess<UnitAnimationManager, UnitAnimationActionVariantIdle>("m_VariantIdle");

    // The unit's tracker, or null (no animation manager or no tracker).
    public static State? Read(UnitEntityData u)
    {
        if (!(u.View?.AnimationManager is UnitAnimationManager um)) return null;
        var t = Tracker(um);
        if (t == null) return null;
        var c = Curve(t);
        var mi = MicroIdle(um);
        var vi = VariantIdle(um);
        var curve = c == null ? "none" : mi != null && ReferenceEquals(c, mi.TriggerProbability) ? "micro" : vi != null && ReferenceEquals(c, vi.RetriggerProbability) ? "variant" : "other";
        return new State { Time = Time(t), Random = Random(t), Curve = curve, Playing = !(MicroIdleHandle(um)?.IsFinished ?? true) };
    }

    // Observe's first tick, before it records the unit's tracker: a turn-based replay takes the recording's.
    public static void Apply(UnitEntityData u)
    {
        if (!Sched.Replay || !Sched.On) return;
        try
        {
            var now = Read(u);
            var has = Tape.TryGetValue(u.UniqueId, out var rec);
            string why = null;
            if (now.HasValue != has) why = now.HasValue ? "only the replay has a tracker" : "only the recording has a tracker";
            else if (has && (now.Value.Curve != rec.Curve || now.Value.Playing != rec.Playing))
                why = $"curve {now.Value.Curve}/{rec.Curve}, playing {now.Value.Playing}/{rec.Playing} (replay/recording)";
            if (why != null)
            {
                var id = u.UniqueId;
                Sched.Obs("idle0_unpinned", j => { j.Prop("u", id); j.Prop("why", why); });
                return;
            }
            if (!has) return;
            var t = Tracker((UnitAnimationManager)u.View.AnimationManager);
            Time(t) = rec.Time;
            Random(t) = rec.Random;
        }
        catch (Exception e) { Health.Failed("idle pin", e); }
    }
}

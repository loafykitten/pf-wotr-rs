using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Controllers;
using UnityEngine;

namespace WrathOracle;

// The time tape. TimeController.Tick computes its delta from Time.deltaTime, or from
// Time.unscaledDeltaTime on the first tick after it is activated (m_JustActivated), and
// TimeController.TickRealTime adds Time.unscaledDeltaTime to Player.RealTime. A fixed
// Time.captureDeltaTime makes deltaTime deterministic but not unscaledDeltaTime, which is
// wall-clock time, so the first tick after every mode activation (after a load, entering or
// leaving a dialogue...) leaks real time into game time. Recording keeps the game's own
// behaviour and records the raw values ("time".raw, "tick".unscaled_delta); replay feeds the
// recorded values back through these two call sites (a transpiler on both methods), so the
// replayed game sees exactly the recorded deltas. In fixture mode (a fixed capture delta set
// by "prepare" or a fixture window) the wall-clock reads at these two sites return the fixed
// delta instead, so even the ticks before a window (the reload) are deterministic.
internal static class OracleTime
{
    // Set by "prepare" and fixture windows, cleared by Fixture.Cleanup: the oracle owns the
    // frame delta while this is true (not merely "some capture delta is set").
    public static bool FixtureMode;

    private static List<float> _raw, _unscaled;
    private static int _rawPos, _unscaledPos;
    public static long Forced, Exhausted;

    public static void Load(List<float> raw, List<float> unscaled)
    {
        _raw = raw;
        _unscaled = unscaled;
        _rawPos = _unscaledPos = 0;
        _rawExhaustedAt = _unscaledExhaustedAt = -1;
        Forced = Exhausted = 0;
    }

    public static void Unload() { _raw = null; _unscaled = null; }

    // Exhaustion is counted once per tape position (a missing tick), not per read: the trace
    // prefix, the game and TickRealTime each read the tapes within one tick.
    private static int _rawExhaustedAt = -1, _unscaledExhaustedAt = -1;

    private static float NextRaw(float real)
    {
        if (_raw == null || !Trace.Active) return real;
        if (_rawPos >= _raw.Count)
        {
            if (_rawExhaustedAt != _rawPos) { Exhausted++; _rawExhaustedAt = _rawPos; }
            return real;
        }
        Forced++;
        return _raw[_rawPos];
    }

    // Called once per TimeController.Tick (after the delta was read), to advance the tape.
    public static void AfterTick() { if (_raw != null && Trace.Active) _rawPos++; }

    public static float DeltaTime() => NextRaw(Time.deltaTime);
    public static float UnscaledDeltaTime() => NextRaw(Unscaled());

    // With a fixed frame delta (fixture mode), the wall-clock delta is replaced by the fixed
    // one: the game sees the same values as if every frame had taken exactly that long, which
    // is a legal input for it, and the first tick after an activation stops depending on how
    // long the real frame took. Outside fixture mode: the real value.
    private static float Unscaled() => FixtureMode && Time.captureDeltaTime > 0f ? Time.captureDeltaTime : Time.unscaledDeltaTime;

    // TickRealTime runs before GameMode.Tick in Game.Tick: one per sim tick.
    public static float RealUnscaledDeltaTime()
    {
        if (_unscaled == null || !Trace.Active) return Unscaled();
        if (_unscaledPos >= _unscaled.Count)
        {
            if (_unscaledExhaustedAt != _unscaledPos) { Exhausted++; _unscaledExhaustedAt = _unscaledPos; }
            _unscaledPos++;
            return Unscaled();
        }
        Forced++;
        return _unscaled[_unscaledPos++];
    }

    private static IEnumerable<CodeInstruction> TickTranspiler(IEnumerable<CodeInstruction> instructions)
    {
        var d = AccessTools.PropertyGetter(typeof(Time), nameof(Time.deltaTime));
        var u = AccessTools.PropertyGetter(typeof(Time), nameof(Time.unscaledDeltaTime));
        foreach (var ins in instructions)
        {
            if (ins.opcode == OpCodes.Call && Equals(ins.operand, d)) ins.operand = AccessTools.Method(typeof(OracleTime), nameof(DeltaTime));
            else if (ins.opcode == OpCodes.Call && Equals(ins.operand, u)) ins.operand = AccessTools.Method(typeof(OracleTime), nameof(UnscaledDeltaTime));
            yield return ins;
        }
    }

    private static IEnumerable<CodeInstruction> RealTranspiler(IEnumerable<CodeInstruction> instructions)
    {
        var u = AccessTools.PropertyGetter(typeof(Time), nameof(Time.unscaledDeltaTime));
        foreach (var ins in instructions)
        {
            if (ins.opcode == OpCodes.Call && Equals(ins.operand, u)) ins.operand = AccessTools.Method(typeof(OracleTime), nameof(RealUnscaledDeltaTime));
            yield return ins;
        }
    }

    public static void Install(Harmony h)
    {
        h.Patch(AccessTools.Method(typeof(TimeController), nameof(TimeController.Tick)), transpiler: new HarmonyMethod(typeof(OracleTime), nameof(TickTranspiler)));
        h.Patch(AccessTools.Method(typeof(TimeController), nameof(TimeController.TickRealTime)), transpiler: new HarmonyMethod(typeof(OracleTime), nameof(RealTranspiler)));
    }
}

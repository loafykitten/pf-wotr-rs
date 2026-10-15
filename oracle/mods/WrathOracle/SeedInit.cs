using System;
using System.Globalization;
using HarmonyLib;
using Kingmaker;

namespace WrathOracle;

// The plan step `seedinit:X:Y:Z:W` (four 32-bit hex words, Unity's xorshift128 state as the trace's draw
// records print it in `st`): an input that sets UnityEngine.Random's state once, right before the turn-based
// window's next initiative batch, so that the batch's d20s are known in advance (oracle/tools/seedinit-search
// picks a state that makes two units tie). The step itself is recorded as an input (and checked in a replay
// like any other); it only arms. It fires in a prefix on UnitCombatPrepareController.Tick, at the first Tick
// after the arming that is about to roll initiative: the controller's own guard, evaluated with the same
// field reads (Player.IsInCombat, and a unit of State.Units, enumerated as the controller does, in combat
// with CombatState.Prepared false). The state is set before the controller's first RuleInitiativeRoll, so
// the batch's draws are the generator's next outputs from it. Firing writes a gate ("seedinit", the state),
// which a replay must reach at the recorded tick, and an rng_state note (Rng.SetState). In a replay the
// state is set too, though the draws come from the tape.
// The begin record declares the window's step ("seedinit": its argument, or null).
// Failures: the step outside a turn-based window, a second seedinit in a window, a step the begin record
// doesn't declare, or a window that ends with the step armed but not fired, are structural errors (the record run fails its health check), and in a
// replay also invalidate it. Field reads only; nothing the game evaluates is called.
internal static class SeedInit
{
    private static Rng.S4? _armed;
    private static bool _fired, _used;
    public static long Fired;
    // The window's seedinit step as the plan declares it (its argument), or null: the begin record's
    // "seedinit".
    public static string Declared;

    // The window's start (before the trace begins): the plan's steps up to the window's "end" declare its
    // seedinit (the last one; a second one fails when it runs, in Arm).
    public static void ResetWindow(System.Collections.Generic.IEnumerable<string> stepsAhead)
    {
        _armed = null;
        _fired = _used = false;
        Fired = 0;
        Declared = null;
        foreach (var step in stepsAhead)
        {
            if (step == "end") break;
            if (!step.StartsWith("seedinit:", StringComparison.Ordinal)) continue;
            Declared = step.Substring("seedinit:".Length);
        }
    }

    // The plan step: parse, record the input, arm.
    public static bool Arm(string arg, Action<string> fail)
    {
        var p = (arg ?? "").Split(':');
        var w = new uint[4];
        if (p.Length != 4 || !TryHex(p, w) || (w[0] | w[1] | w[2] | w[3]) == 0)
        {
            fail("seedinit: expected X:Y:Z:W, four 32-bit hex words, not all zero");
            return false;
        }
        if (!Trace.Active || !Sched.Window)
        {
            Trace.Structural("seedinit outside a turn-based window");
            fail("seedinit outside a turn-based window");
            return false;
        }
        if (_used)
        {
            Trace.Structural("a second seedinit in the window");
            fail("a second seedinit in the window");
            return false;
        }
        var s = new Rng.S4 { X = w[0], Y = w[1], Z = w[2], W = w[3] };
        if (Declared == null || !string.Equals(Declared, s.ToString(), StringComparison.OrdinalIgnoreCase))
        {
            Trace.Structural("a seedinit step the window's begin record doesn't declare");
            fail("a seedinit step the window's begin record doesn't declare");
            return false;
        }
        Driver.RecordInput("seedinit:" + s);
        _armed = s;
        _used = true;
        return true;
    }

    private static bool TryHex(string[] p, uint[] w)
    {
        for (var i = 0; i < 4; i++)
            if (p[i].Length != 8 || !uint.TryParse(p[i], NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture, out w[i])) return false;
        return true;
    }

    public static void Install(Harmony h)
    {
        try
        {
            var tick = AccessTools.Method(typeof(Kingmaker.Controllers.Combat.UnitCombatPrepareController), "Tick")
                       ?? throw new MissingMethodException("UnitCombatPrepareController.Tick");
            h.Patch(tick, prefix: new HarmonyMethod(typeof(SeedInit), nameof(TickPrefix)));
        }
        catch (Exception e) { Health.Failed("patch UnitCombatPrepareController.Tick (seedinit)", e); }
    }

    public static void TickPrefix()
    {
        if (_armed == null || _fired || !Sched.On) return;
        try
        {
            if (!AboutToRoll()) return;
            var s = _armed.Value;
            _fired = true;
            _armed = null;
            Rng.SetState(s, "seedinit");
            if (!Rng.Now().Equals(s))
            {
                Trace.Structural($"seedinit: the generator's state is {Rng.Now()} after setting {s}");
                if (Sched.Replay) Sched.Invalidate("seedinit: the state didn't take");
            }
            Fired++;
            Sched.Gate("seedinit", s.ToString());
        }
        catch (Exception e) { Trace.HookFailed("seedinit", e); }
    }

    // UnitCombatPrepareController.Tick's guard: Player.IsInCombat and a unit in combat not yet prepared
    // (its Units: State.Units.InCombat().CombatStates() where !Prepared), by the same reads.
    private static bool AboutToRoll()
    {
        var game = Game.Instance;
        if (!game.Player.IsInCombat) return false;
        foreach (var u in game.State.Units)
        {
            var cs = u.CombatState;
            if (cs != null && cs.IsInCombat && !cs.Prepared) return true;
        }
        return false;
    }

    // The window's end: an armed step that never fired fails the run.
    public static void CloseWindow()
    {
        if (_armed == null) return;
        Trace.Structural("seedinit armed but never fired: no initiative batch followed it in the window");
        if (Sched.Replay) Sched.Invalidate("seedinit armed but never fired");
        _armed = null;
    }
}

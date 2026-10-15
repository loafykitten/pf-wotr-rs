using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using Kingmaker;
using Kingmaker.Blueprints;
using Kingmaker.DialogSystem;
using Kingmaker.DialogSystem.Blueprints;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.EntitySystem.Stats;
using Kingmaker.RuleSystem;
using Kingmaker.RuleSystem.Rules;
using Kingmaker.UnitLogic;
using Kingmaker.UnitLogic.Commands;
using Kingmaker.UnitLogic.Commands.Base;
using UnityEngine;

namespace WrathOracle;

// Scripted inputs and fixture windows (WrathOracle 0.2). Plan steps:
//   fixture:NAME          begin recording fixture NAME: fixed frame delta, a fixed seed, trace on
//   replay:NAME[:N:+D]    begin replaying fixture NAME from its recorded trace (optionally
//                         perturbing draw N by D: the mutation test)
//   end                   end the current window (writes the trace)
//   state:LABEL           state dump (docs/oracle/state-dump-format.md)
//   wait:N                N frames (ticks once loaded)
//   move:x,y,z            main character walks to a point (UnitMoveTo); waits until it stops
//   teleport:x,y,z        main character placed at a point
//   dialog:DIALOG[:UNIT]  start a dialogue with the unit of blueprint UNIT (StartDialogWithUnit)
//   answer:N              choose answer N of the current cue, once the dialogue waits for one
//   answerbp:GUID         choose the shown answer with that blueprint GUID
//   waitdialog            until a dialogue shows answers;  waitnodialog: until none runs
//   dialogrun[:G1,G2,...] answer the running dialogue until it ends: each time it waits for an
//                         answer, the first shown answer whose GUID is in the list (each GUID
//                         used once), else answer 0; waits up to 3600 frames per answer
//   attack:UNIT           main character attacks the nearest living unit of blueprint UNIT
//   waitidle              until the main character runs no command
//   skillcheck:STAT:DC[:true|false]  a skill check by the main character (ensureSuccess optional)
//   rolldice:N:SIDES      RuleRollDice(N dSIDES) by the main character
//   d20reroll             RuleRollD20 with one take-best reroll (AddReroll)
//   addfact:GUID          add a fact (feature or buff) to the main character
//   tempmod:STAT:DC:B     skill check carrying a temporary +B stat modifier (AddTemporaryModifier)
//   savebonus:SAVE:DC:B   saving throw with a +B rule bonus (applied through a TemporaryModifiersScope)
//   seed:N                UnityEngine.Random.InitState(N)
//   seedinit:X:Y:Z:W      in a turn-based window: Random's state set to X:Y:Z:W (hex) right before the next
//                         initiative batch (UnitCombatPrepareController.Tick; SeedInit.cs)
//   run:N                 N frames, resuming the game whenever it pauses itself (auto-pause)
//   prepare               before a fixture's reload: fixed frame delta, Random.InitState(seed)
// Every input is recorded in the trace as an "input" record.
internal static class Driver
{
    public static float FixedDelta = 1f / 60f;
    private static bool Optional; // optional steps give up after 120 frames instead of 3600
    public static int Seed = 20261011;

    private static UnitEntityData Main_ => Game.Instance.Player.MainCharacter.Value;

    internal static void RecordInput(string step) => Input(step);

    private static void Input(string step, Action<Json> extra = null)
    {
        if (!Trace.Active) return;
        Sched.CheckInput(step); // turn-based replays: the recorded input, at the recorded tick
        Trace.Emit("input", j => { j.Prop("step", step); j.Prop("frame", Time.frameCount); extra?.Invoke(j); });
    }

    private static Vector3 ParseVec(string s)
    {
        var p = s.Split(',').Select(x => float.Parse(x, CultureInfo.InvariantCulture)).ToArray();
        return new Vector3(p[0], p[1], p[2]);
    }

    private static T Bp<T>(string guid) where T : BlueprintScriptableObject => ResourcesLibrary.TryGetBlueprint<T>(guid);

    private static UnitEntityData UnitOf(string blueprintGuid)
    {
        var main = Main_;
        return StateDump.AllUnits(Game.Instance)
            .Where(u => u.IsInGame && !u.State.IsDead && u.Blueprint != null && u.Blueprint.AssetGuid.ToString() == blueprintGuid)
            .OrderBy(u => main == null ? 0 : (u.Position - main.Position).sqrMagnitude)
            .FirstOrDefault();
    }

    private static IEnumerator WaitUntil(Func<bool> cond, int maxFrames, string what, Action<string> fail)
    {
        if (Optional) maxFrames = Math.Min(maxFrames, 120);
        for (var i = 0; i < maxFrames; i++)
        {
            if (cond()) yield break;
            yield return null;
        }
        if (!cond()) fail($"{what}: timed out after {maxFrames} frames");
    }

    private static bool DialogWaitsForAnswer()
    {
        var dc = Game.Instance.DialogController;
        return dc.Dialog != null && dc.CurrentCue != null && dc.Answers.Any() && !Hooks.IsCueScheduled(dc);
    }

    // Returns null if the step isn't a driver step.
    public static IEnumerator Step(string name, string arg, Runner runner, Action<string> fail, bool optional = false)
    {
        Optional = optional;
        switch (name)
        {
            case "wait":
                return Wait(int.Parse(arg ?? "1", CultureInfo.InvariantCulture));
            case "move":
                return Move(ParseVec(arg), fail);
            case "teleport":
                return Do(() =>
                {
                    var p = ParseVec(arg);
                    Input("teleport:" + arg);
                    Main_.Position = p;
                });
            case "dialog":
                return Do(() =>
                {
                    var parts = arg.Split(':');
                    var dialog = Bp<BlueprintDialog>(parts[0]) ?? throw new Exception("no dialog " + parts[0]);
                    var unit = parts.Length > 1 ? UnitOf(parts[1]) ?? throw new Exception("no unit " + parts[1]) : null;
                    Input("dialog:" + arg, j => { j.Key("unit"); Snap.UnitRef(j, unit); });
                    if (unit != null) Game.Instance.DialogController.StartDialogWithUnit(dialog, unit, Main_);
                    else Game.Instance.DialogController.StartDialogWithoutTarget(dialog, null, Main_);
                });
            case "waitdialog":
                return WaitUntil(DialogWaitsForAnswer, 3600, "waitdialog", fail);
            case "waitnodialog":
                return WaitUntil(() => Game.Instance.DialogController.Dialog == null && !Game.Instance.IsModeActive(Kingmaker.GameModes.GameModeType.Dialog), 3600, "waitnodialog", fail);
            case "answer":
                return Answer(int.Parse(arg, CultureInfo.InvariantCulture), null, fail);
            case "answerbp":
                return Answer(-1, arg, fail);
            case "dialogrun":
                return DialogRun(arg, fail);
            case "attack":
                return Attack(arg, fail);
            case "waitidle":
                return WaitUntil(() => Main_.Commands.Empty, 3600, "waitidle", fail);
            case "skillcheck":
                return Do(() =>
                {
                    var p = arg.Split(':');
                    var stat = (StatType)Enum.Parse(typeof(StatType), p[0]);
                    var dc = int.Parse(p[1], CultureInfo.InvariantCulture);
                    bool? ensure = p.Length > 2 ? bool.Parse(p[2]) : (bool?)null;
                    Input("skillcheck:" + arg);
                    var check = new RuleSkillCheck(Main_, stat, dc) { EnsureSuccess = ensure };
                    Rulebook.Trigger(check);
                });
            case "rolldice":
                return Do(() =>
                {
                    var p = arg.Split(':');
                    var n = int.Parse(p[0], CultureInfo.InvariantCulture);
                    var sides = (DiceType)Enum.Parse(typeof(DiceType), "D" + p[1]);
                    Input("rolldice:" + arg);
                    Rulebook.Trigger(new RuleRollDice(Main_, new DiceFormula(n, sides)));
                });
            case "d20reroll":
                return Do(() =>
                {
                    Input("d20reroll");
                    var r = new RuleRollD20(Main_);
                    r.AddReroll(1, takeBest: true, null);
                    Rulebook.Trigger(r);
                });
            case "tempmod":
                // A skill check carrying a temporary stat modifier (RulebookEvent.AddTemporaryModifier;
                // OnDidTrigger removes it): STAT:DC:BONUS
                return Do(() =>
                {
                    var p = arg.Split(':');
                    var stat = (StatType)Enum.Parse(typeof(StatType), p[0]);
                    var dc = int.Parse(p[1], CultureInfo.InvariantCulture);
                    var bonus = int.Parse(p[2], CultureInfo.InvariantCulture);
                    Input("tempmod:" + arg);
                    var check = new RuleSkillCheck(Main_, stat, dc);
                    check.AddTemporaryModifier(Main_.Stats.GetStat(stat).AddModifier(bonus, (Kingmaker.EntitySystem.EntityFact)null, Kingmaker.Enums.ModifierDescriptor.Morale));
                    Rulebook.Trigger(check);
                });
            case "savebonus":
                // A saving throw with a rule bonus, which RuleSavingThrow applies to the save
                // stat through a TemporaryModifiersScope: SAVE:DC:BONUS (SAVE = Fortitude/Reflex/Will)
                return Do(() =>
                {
                    var p = arg.Split(':');
                    var type = (Kingmaker.EntitySystem.Stats.SavingThrowType)Enum.Parse(typeof(Kingmaker.EntitySystem.Stats.SavingThrowType), p[0]);
                    var dc = int.Parse(p[1], CultureInfo.InvariantCulture);
                    var bonus = int.Parse(p[2], CultureInfo.InvariantCulture);
                    Input("savebonus:" + arg);
                    var save = new RuleSavingThrow(Main_, type, dc);
                    save.AddModifierCopy(new Modifier(bonus, Kingmaker.Enums.ModifierDescriptor.Morale));
                    Rulebook.Trigger(save);
                });
            case "addfact":
                return Do(() =>
                {
                    var bp = Bp<Kingmaker.Blueprints.Facts.BlueprintUnitFact>(arg) ?? throw new Exception("no fact " + arg);
                    Input("addfact:" + arg);
                    Main_.AddFact(bp);
                });
            case "run":
                return Run(int.Parse(arg ?? "1", CultureInfo.InvariantCulture));
            case "prepare":
                // Before a fixture's reload: fixed frame delta from now on, a known seed.
                return Do(() =>
                {
                    OracleTime.FixtureMode = true;
                    Time.captureDeltaTime = FixedDelta;
                    UnityEngine.Random.InitState(Seed);
                    // Player input is the fixture's from here (the reload and the wait before the window
                    // included), so the window opens on pointer, camera and UI state no hardware touched.
                    InputPin.Begin("prepare");
                    // A turn-based replay: the recording's native unit hashes are loaded now, before the reload, so
                    // a unit has the same value for its whole life (WallClock.UnitHash).
                    if (runner != null)
                    {
                        Sched.HashTape = null;
                        PhysicsPhase.SetTarget(null, 0);
                        PrePathSync.Disarm();
                        for (var k = runner.StepIndex + 1; k < runner.Plan.Steps.Count; k++)
                        {
                            var st = runner.Plan.Steps[k];
                            if (st == "end" || st.StartsWith("fixture:", StringComparison.Ordinal)) break;
                            if (!st.StartsWith("replay:", StringComparison.Ordinal)) continue;
                            if (runner.Plan.Steps.Skip(k + 1).TakeWhile(x => x != "end").Any(x => x.StartsWith("tbm:", StringComparison.Ordinal)))
                            {
                                var rec = System.IO.Path.Combine(Fixture.Dir(runner.Plan, st.Substring(7).Split(':')[0]), "record.trace.jsonl.gz");
                                Sched.LoadHashTape(rec);
                                // The recording's physics phase at its window's start, reproduced from the reload's end on.
                                PrePathSync.LoadTape(rec); // its pre-window path deliveries, reproduced from the reload's first idle frame
                                var head = Sched.ReadHeader(rec);
                                if (head?["fixed_phase0"] != null && head["frames_anchor_to_begin"] != null && (int)head["frames_anchor_to_begin"] >= 0)
                                    PhysicsPhase.SetTarget((double)head["fixed_phase0"], (int)head["frames_anchor_to_begin"]);
                            }
                            break;
                        }
                    }
                });
            case "seedinit":
                return Do(() => SeedInit.Arm(arg, fail));
            case "seed":
                return Do(() =>
                {
                    Input("seed:" + arg);
                    UnityEngine.Random.InitState(int.Parse(arg, CultureInfo.InvariantCulture));
                });
        }
        return TurnBased.Step(name, arg, runner, fail); // turn-based steps (TurnBased.cs), or null
    }

    private static IEnumerator Do(Action a)
    {
        a();
        yield break;
    }

    // N frames, unpausing whenever the game pauses itself (auto-pause at combat start, a
    // tutorial): the player's "resume" input, recorded each time.
    private static IEnumerator Run(int frames)
    {
        for (var i = 0; i < frames; i++)
        {
            if (Game.Instance.IsPaused)
            {
                Input("unpause");
                Game.Instance.IsPaused = false;
            }
            yield return null;
        }
    }

    private static IEnumerator Wait(int frames)
    {
        for (var i = 0; i < frames; i++) yield return null;
    }

    private static IEnumerator Move(Vector3 p, Action<string> fail)
    {
        var u = Main_;
        Input("move", j => j.Prop("to", p));
        u.Commands.Run(new UnitMoveTo(p, 0.3f));
        yield return null;
        for (var i = 0; i < 3600; i++)
        {
            if (u.Commands.Empty) break;
            yield return null;
        }
        if (!u.Commands.Empty) fail("move: still moving after 3600 frames");
        Input("move_done", j => j.Prop("at", u.Position));
    }

    private static IEnumerator Answer(int n, string guid, Action<string> fail)
    {
        for (var i = 0; i < (Optional ? 120 : 3600) && !DialogWaitsForAnswer(); i++) yield return null;
        var dc = Game.Instance.DialogController;
        var answers = dc.Answers.ToList();
        if (guid != null) n = answers.FindIndex(x => x.AssetGuid.ToString() == guid);
        if (n < 0) { fail($"answerbp:{guid}: not among the {answers.Count} answers shown ({string.Join(",", answers.Select(x => x.AssetGuid.ToString()))})"); yield break; }
        if (!DialogWaitsForAnswer() || n >= answers.Count)
        {
            fail($"answer:{n}: no such answer (dialogue {(dc.Dialog != null ? "running" : "not running")}, {answers.Count} answers)");
            yield break;
        }
        var a = answers[n];
        var manual = a.CharacterSelection.SelectionType == CharacterSelection.Type.Manual ? Main_ : null;
        Input("answer:" + n, j => { j.Prop("answer", Snap.Guid(a)); j.Prop("of", answers.Count); });
        dc.SelectAnswer(a, manual);
        yield return null;
    }

    private static bool DialogRunning() =>
        Game.Instance.DialogController.Dialog != null || Game.Instance.IsModeActive(Kingmaker.GameModes.GameModeType.Dialog);

    private static IEnumerator DialogRun(string arg, Action<string> fail)
    {
        var prefs = string.IsNullOrEmpty(arg) ? new List<string>() : arg.Split(',').ToList();
        var answered = 0;
        while (true)
        {
            for (var i = 0; i < 3600 && DialogRunning() && !DialogWaitsForAnswer(); i++) yield return null;
            if (!DialogRunning())
            {
                Input("dialogrun_end", j => j.Prop("answered", answered));
                yield break;
            }
            if (!DialogWaitsForAnswer()) { fail("dialogrun: timed out after 3600 frames"); yield break; }
            if (answered >= 200) { fail("dialogrun: 200 answers without the dialogue ending (a loop?)"); yield break; }
            var dc = Game.Instance.DialogController;
            var answers = dc.Answers.ToList();
            var n = 0;
            foreach (var p in prefs)
            {
                var k = answers.FindIndex(x => x.AssetGuid.ToString() == p);
                if (k >= 0) { n = k; prefs.Remove(p); break; }
            }
            var a = answers[n];
            var manual = a.CharacterSelection.SelectionType == CharacterSelection.Type.Manual ? Main_ : null;
            Input("answer:" + n, j => { j.Prop("answer", Snap.Guid(a)); j.Prop("of", answers.Count); j.Prop("via", "dialogrun"); });
            dc.SelectAnswer(a, manual);
            answered++;
            yield return null;
        }
    }

    private static IEnumerator Attack(string guid, Action<string> fail)
    {
        var target = UnitOf(guid);
        if (target == null) { fail("attack: no living unit of blueprint " + guid); yield break; }
        Input("attack:" + guid, j => { j.Key("target"); Snap.UnitRef(j, target); });
        Main_.Commands.Run(UnitAttack.CreateAttackCommand(Main_, target));
        yield return null;
    }
}

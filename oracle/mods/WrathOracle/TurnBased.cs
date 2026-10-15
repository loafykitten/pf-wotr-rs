using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Reflection;
using HarmonyLib;
using Kingmaker;
using Kingmaker.AI;
using Kingmaker.Blueprints;
using Kingmaker.Controllers.Clicks;
using Kingmaker.Controllers.Clicks.Handlers;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.PubSubSystem;
using Kingmaker.Settings;
using Kingmaker.TurnBasedMode;
using Kingmaker.UnitLogic;
using Kingmaker.UnitLogic.Commands;
using Kingmaker.UnitLogic.Commands.Base;
using TurnBased.Controllers;
using UnityEngine;

namespace WrathOracle;

// Turn-based combat (WrathOracle 0.3, Wrath-Rust POC-5): plan steps that drive turn-based
// fights through the game's own input paths, and trace records ("tb", "cmd", "ai") of the
// turn-based controller, unit commands and AI decisions.
//
// Plan steps:
//   tbm:on | tbm:off      the "enable turn-based" game setting (SetValueAndConfirm, as the
//                         options menu does; it raises HandleTurnBasedModeStateChanged). The
//                         original value is restored when the fixture window ends or the plan
//                         ends, so the player's setting is never left changed.
//   tbwaitcombat[:N]      until the party is in turn-based combat (N frames at most)
//   tbfight:POLICY[:N]    play the party's turns until combat ends (N frames at most, default
//                         30000), unpausing whenever the game pauses itself. On each turn of a
//                         directly controllable unit, POLICY decides (inputs recorded):
//                           POLICY = entry(,entry)*;  entry = WHO=WHAT
//                           WHO    = default | <unit UniqueId>
//                           WHAT   = nearest | end | focus/<id>/<id>/... (first living in the list,
//                                    else nearest) | cast/<ability guid> (that cantrip, from the
//                                    unit's spellbook, on the nearest enemy)
//                         An attack is issued as a click on the enemy: the target is hovered
//                         (TurnController.HandlePortraitHover, which computes the turn-based
//                         prediction and path), the click handler's command is created
//                         (ClickUnitHandler.CreateCommand) with the predicted path as its forced
//                         path, run, and the hover released. When the unit's command has ended
//                         and its turn is still waiting, the turn is ended as the UI's end-turn
//                         button does (TurnController.ForceToEnd).
//                         A cast is the action bar's spell slot (SelectedAbilityHandler.SetAbility,
//                         pointer mode Ability) and a click on the enemy: the target hovered as for an
//                         attack, the command ClickWithSelectedAbilityHandler makes for a spellbook
//                         spell (UnitUseAbility.CreateCastCommand, OnCastRequested) run with the
//                         predicted path, the pointer mode cleared. Only a cantrip known in one
//                         spellbook, unit-targeted, whose cast command is a plain UnitUseAbility.
//   tbend                 end the current turn (ForceToEnd), if it is a controllable unit's
internal static class TurnBased
{
    private static bool? _originalSetting;

    public static void ResetWindow() => _turnIndex = 0;

    // "tb", "cmd" and "ai" records are written only in windows that used a tbm step, so other
    // fixtures (the real-time certification ones) keep exactly the records they had.
    private static bool InWindow => _originalSetting.HasValue;

    public static void SetTurnBased(bool on)
    {
        var s = SettingsRoot.Game.TurnBased.EnableTurnBasedMode;
        if (!_originalSetting.HasValue) _originalSetting = s.GetValue();
        s.SetValueAndConfirm(on);
    }

    // Restores the player's setting (fixture end, plan end, cleanup). Idempotent. The saved
    // original is kept until a restore succeeds, so a failed one is retried by the next cleanup
    // (and fails the fixture through the health counters).
    public static void RestoreSetting()
    {
        if (!_originalSetting.HasValue) return;
        try
        {
            var s = SettingsRoot.Game.TurnBased.EnableTurnBasedMode;
            if (s.GetValue() != _originalSetting.Value) s.SetValueAndConfirm(_originalSetting.Value);
            _originalSetting = null;
        }
        catch (Exception e)
        {
            Health.Failed("restore turn-based setting", e);
        }
    }

    private static void Input(string step, Action<Json> extra = null)
    {
        if (!Trace.Active) return;
        Sched.CheckInput(step); // a replay must make exactly the recorded input, at the recorded tick
        Trace.Emit("input", j => { j.Prop("step", step); j.Prop("frame", Time.frameCount); extra?.Invoke(j); });
    }

    private static CombatController Cc => Game.Instance.TurnBasedCombatController;

    public static IEnumerator Step(string name, string arg, Runner runner, Action<string> fail)
    {
        switch (name)
        {
            case "tbm":
                return Do(() =>
                {
                    var on = arg == "on";
                    if (!on && arg != "off") throw new ArgumentException("tbm:on or tbm:off");
                    Input("tbm:" + arg);
                    SetTurnBased(on);
                });
            case "tbwaitcombat":
                return WaitCombat(int.Parse(arg ?? "3600", CultureInfo.InvariantCulture), fail);
            case "tbfight":
                {
                    var parts = (arg ?? "default=nearest").Split(':');
                    var max = parts.Length > 1 ? int.Parse(parts[1], CultureInfo.InvariantCulture) : 30000;
                    return Fight(Policy.Parse(parts[0]), max, fail);
                }
            case "idtrace":
                return IdTrace.Step(arg, runner);
            case "tbend":
                return Do(() =>
                {
                    var turn = Cc.CurrentTurn;
                    if (turn == null || !turn.Rider.IsDirectlyControllable) throw new Exception("tbend: not a controllable unit's turn");
                    EndTurn(turn);
                });
        }
        return null;
    }

    private static IEnumerator Do(Action a)
    {
        a();
        yield break;
    }

    private static void Unpause()
    {
        if (Game.Instance.IsPaused)
        {
            Input("unpause");
            Game.Instance.IsPaused = false;
        }
    }

    private static IEnumerator WaitCombat(int frames, Action<string> fail)
    {
        for (var i = 0; i < frames; i++)
        {
            if (Hooks.InTurnBasedCombat() && Cc.Initialized) yield break;
            Unpause();
            yield return null;
        }
        fail($"tbwaitcombat: no turn-based combat after {frames} frames");
    }

    private sealed class Policy
    {
        public readonly Dictionary<string, string> Rules = new Dictionary<string, string>();

        public static Policy Parse(string s)
        {
            var p = new Policy();
            foreach (var e in s.Split(new[] { ',' }, StringSplitOptions.RemoveEmptyEntries))
            {
                var kv = e.Split('=');
                if (kv.Length != 2) throw new ArgumentException("tbfight policy entry must be WHO=WHAT: " + e);
                var what = kv[1];
                if (what.StartsWith("cast/", StringComparison.Ordinal))
                {
                    if (CastGuid(what) == null) throw new ArgumentException("tbfight: cast/ takes one ability GUID (32 lower-case hex digits): " + what);
                }
                else if (what != "nearest" && what != "end" && !what.StartsWith("focus/", StringComparison.Ordinal))
                    throw new ArgumentException("tbfight: unknown action " + what);
                p.Rules[kv[0]] = what;
            }
            if (!p.Rules.ContainsKey("default")) p.Rules["default"] = "nearest";
            return p;
        }

        public string For(UnitEntityData u) => Rules.TryGetValue(u.UniqueId, out var w) ? w : Rules["default"];
    }

    // The ability GUID of a cast/<guid> action (32 lower-case hex digits, as Snap.Guid writes them), else null.
    private static string CastGuid(string what)
    {
        if (what == null || !what.StartsWith("cast/", StringComparison.Ordinal)) return null;
        var g = what.Substring(5);
        if (g.Length != 32) return null;
        foreach (var c in g)
            if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'))) return null;
        return g;
    }

    private static bool Alive(UnitEntityData u) => u != null && u.IsInGame && !u.State.IsDead && u.IsInCombat;

    private static UnitEntityData ChooseTarget(UnitEntityData unit, string what)
    {
        // Only enemies the player's pointer can be over (visible, targetable): a click needs the unit under it.
        var enemies = Cc.SortedUnits.Where(u => Alive(u) && IsEnemyPassive(unit, u) && InputPin.CanPoint(u)).ToList();
        if (what.StartsWith("focus/", StringComparison.Ordinal))
        {
            foreach (var id in what.Substring(6).Split('/'))
            {
                var t = enemies.FirstOrDefault(u => u.UniqueId == id);
                if (t != null) return t;
            }
        }
        // Nearest by straight-line distance; ties by UniqueId (ordinal), so the choice never
        // depends on enumeration order.
        return enemies.OrderBy(u => (u.Position - unit.Position).sqrMagnitude).ThenBy(u => u.UniqueId, StringComparer.Ordinal).FirstOrDefault();
    }

    private static readonly AccessTools.FieldRef<UnitEntityData, Kingmaker.UnitLogic.Groups.UnitGroup> CachedGroup =
        AccessTools.FieldRefAccess<UnitEntityData, Kingmaker.UnitLogic.Groups.UnitGroup>("m_Group");

    // UnitEntityData.IsEnemy through the units' cached groups (the Group getter materializes a missing group:
    // the recording's policy only reads). Combat units always have theirs; one without is a fixture error.
    private static bool IsEnemyPassive(UnitEntityData a, UnitEntityData b)
    {
        var ga = CachedGroup(a);
        var gb = CachedGroup(b);
        if (ga == null || ga.IsFake || ga.Disposed || gb == null || gb.IsFake || gb.Disposed)
            throw new InvalidOperationException($"tbfight: no materialized group for {a.UniqueId} or {b.UniqueId}");
        return ga.IsEnemy(gb);
    }

    private static void EndTurn(TurnController turn)
    {
        Input("tbend", j => { j.Key("unit"); Snap.UnitRef(j, turn.Rider); j.Prop("status", turn.Status.ToString()); j.Prop("round", Cc.RoundNumber); j.Prop("turn_index", _turnIndex); });
        turn.ForceToEnd();
    }

    // Turns started in this window (the identity of a turn in the recorded inputs).
    private static int _turnIndex;

    // The unit has nothing queued or running and no projectile in flight: the state in which
    // the game itself would auto-end its turn (TurnController.Tick) and in which an input is
    // safe to give. Queued commands (UnitCommands.Empty ignores the queue) and launched
    // projectiles are part of it.
    private static bool Idle(TurnController turn, UnitEntityData unit) =>
        unit.Commands.Empty && unit.Commands.Queue.Count == 0 && !turn.HasRunningCommands &&
        !Game.Instance.ProjectileController.HasLaunchedProjectile(unit, turn.Mount);

    private static IEnumerator Fight(Policy policy, int maxFrames, Action<string> fail) =>
        Sched.Replay ? ReplayFight(maxFrames, fail) : RecordFight(policy, maxFrames, fail);

    // Recording: the policy decides, every decision is an input record.
    private static IEnumerator RecordFight(Policy policy, int maxFrames, Action<string> fail)
    {
        TurnController handled = null; // the turn we already gave a command
        UnitCommand issued = null;
        var idleFrames = 0;
        for (var frame = 0; frame < maxFrames; frame++)
        {
            Unpause();
            var player = Game.Instance.Player;
            if (!player.IsInCombat && frame > 0)
            {
                Input("tbfight_done");
                yield break;
            }
            var turn = Cc.CurrentTurn;
            if (turn != null && turn.Rider != null && turn.Rider.IsDirectlyControllable &&
                (turn.Status == TurnController.TurnStatus.Preparing || turn.Status == TurnController.TurnStatus.Acting))
            {
                var unit = turn.Rider;
                if (handled != turn)
                {
                    if (Idle(turn, unit))
                    {
                        handled = turn;
                        issued = null;
                        idleFrames = 0;
                        var what = policy.For(unit);
                        var target = what == "end" ? null : ChooseTarget(unit, what);
                        if (target == null)
                        {
                            EndTurn(turn);
                        }
                        else
                        {
                            var cast = CastGuid(what);
                            issued = cast != null ? IssueCast(turn, unit, target, cast, what) : IssueAttack(turn, unit, target, what);
                            if (issued == null) EndTurn(turn);
                        }
                    }
                }
                else if (turn.Status == TurnController.TurnStatus.Acting && Idle(turn, unit) && (issued == null || issued.IsFinished))
                {
                    // The command is over and the turn still waits for the player (it has a
                    // move action left, say): end it as the end-turn button does. One idle frame
                    // first, so the controller's own auto-end gets its chance.
                    if (++idleFrames > 1) EndTurn(turn);
                }
                else idleFrames = 0;
            }
            yield return null;
        }
        fail($"tbfight: combat still running after {maxFrames} frames");
    }

    private static UnitEntityData FindUnit(string id) =>
        StateDump.AllUnits(Game.Instance).FirstOrDefault(u => u.UniqueId == id);

    private static string RefId(Newtonsoft.Json.Linq.JToken t) => t is Newtonsoft.Json.Linq.JObject o ? (string)o["$unit"] : null;

    // Replaying: the recorded inputs are made again, through the same entry points, in the
    // recorded order, each when the game is in the state the recording was in when it made it
    // (the right unit's turn, the unit idle, the target alive). The recording made an input at
    // the first moment it could; so does the replay, but not before the recorded tick (moved by
    // the offset built up so far, Sched.Offset) and, if the state is not reached within a few ticks of it, the
    // fixture is over (Sched.Invalidate): the replay never waits past other events, never
    // chooses another target, and every gate (Sched.Gate) between inputs is checked.
    private static IEnumerator ReplayFight(int maxFrames, Action<string> fail)
    {
        for (var frame = 0; frame < maxFrames; frame++)
        {
            if (Sched.Invalid != null) yield break;
            Unpause();
            while (Sched.Invalid == null && Sched.PeekInput(out var step, out var tick, out var rec))
            {
                var due = tick + Sched.Offset;
                if (Trace.Tick < due) break;
                var late = Trace.Tick > due + 40;
                if (step == "tbfight_done")
                {
                    if (Game.Instance.Player.IsInCombat)
                    {
                        if (late) { Sched.Invalidate($"recorded combat end at tick {tick}, but the party is still in combat at tick {Trace.Tick}"); yield break; }
                        break;
                    }
                    Input("tbfight_done");
                    yield break;
                }
                var turn = Cc.CurrentTurn;
                var unit = turn?.Rider;
                var wantUnit = RefId(rec["unit"]);
                var attack = step == "tbattack" || step == "tbattack_impossible" || step == "tbattack_rejected";
                var cast = step == "tbcast" || step == "tbcast_impossible" || step == "tbcast_rejected";
                if (!attack && !cast && step != "tbend") break; // an input of some other step: not ours to make
                // Not there yet: another unit's turn would be a different fight; no turn yet, or a
                // turn still scrolling or preparing, is the same fight a few ticks later.
                string why = null;
                if (turn == null) why = "no turn";
                else if (unit.UniqueId != wantUnit) why = $"turn of {unit.UniqueId}, recorded {wantUnit}";
                else if (!unit.IsDirectlyControllable) why = "unit not controllable";
                else if (turn.Status != TurnController.TurnStatus.Preparing && turn.Status != TurnController.TurnStatus.Acting) why = "turn status " + turn.Status;
                else if (!Idle(turn, unit)) why = "unit not idle";
                UnitEntityData target = null;
                if (why == null && (attack || cast))
                {
                    target = FindUnit(RefId(rec["target"]));
                    if (target == null) why = "target not found";
                    else if (!Alive(target)) why = "target not alive";
                }
                if (why != null)
                {
                    if (turn != null && unit.UniqueId != wantUnit && (turn.Status == TurnController.TurnStatus.Preparing || turn.Status == TurnController.TurnStatus.Acting) || late)
                        Sched.Invalidate($"recorded {step} by {wantUnit} at tick {tick} (due tick {due}), not possible at tick {Trace.Tick}: {why}");
                    break;
                }
                if (attack)
                {
                    var cmd = IssueAttack(turn, unit, target, (string)rec["policy"] ?? "replay");
                    // The outcome (a command, or the unit can't) is the recorded one if the next
                    // recorded input was this attempt; the input check has verified that.
                    if (cmd == null && step == "tbattack") { Sched.Invalidate($"recorded attack by {wantUnit} could not be issued at tick {Trace.Tick}"); yield break; }
                }
                else if (cast)
                {
                    // The same spell as recorded; its outcome is checked as an attack's is.
                    var guid = rec["ability"] is Newtonsoft.Json.Linq.JValue gv && gv.Type == Newtonsoft.Json.Linq.JTokenType.String ? (string)gv : null;
                    if (guid == null || CastGuid("cast/" + guid) == null) { Sched.Invalidate($"recorded {step} by {wantUnit} at tick {tick} has no valid ability"); yield break; }
                    var cmd = IssueCast(turn, unit, target, guid, (string)rec["policy"] ?? "replay");
                    if (cmd == null && step == "tbcast") { Sched.Invalidate($"recorded cast by {wantUnit} could not be issued at tick {Trace.Tick}"); yield break; }
                }
                else EndTurn(turn);
            }
            yield return null;
        }
        fail($"tbfight: combat still running after {maxFrames} frames");
    }

    private static UnitCommand IssueAttack(TurnController turn, UnitEntityData unit, UnitEntityData target, string what)
    {
        // The player's pointer goes to the target (InputPin: the game's own PointerController.Tick picks
        // the unit up, so PointerOn, the hover highlight and the click handler are the target's, as for a
        // mouse over it), then the portrait hover asks the turn for its predictions at once
        // (TurnController.HandlePortraitHover: UpdateSelectedClickHandler finds the unit handler from
        // PointerOn and SimulateClick computes the attack and its approach path, which the click's command
        // gets as its ForcedPath). Both are released on every path, exceptions included; the pointer is
        // then over nothing again.
        try
        {
            var miss = InputPin.PointAt(target);
            if (miss != null)
            {
                // A recording only targets units the pointer can be over (ChooseTarget); a replay whose pointer
                // can't reach the recorded target has left its recording.
                if (!Sched.Replay) throw new InvalidOperationException(miss);
                Sched.Invalidate($"recorded attack on {target.UniqueId} at tick {Trace.Tick}: {miss}");
                return null;
            }
            try
            {
                turn.HandlePortraitHover(target, true);
                return IssueHovered(unit, target, what);
            }
            finally
            {
                turn.HandlePortraitHover(target, false);
            }
        }
        finally
        {
            InputPin.PointAt(null);
        }
    }

    private static UnitCommand IssueHovered(UnitEntityData unit, UnitEntityData target, string what)
    {
        var path = PathVisualizer.Instance != null ? PathVisualizer.Instance.CurrentPathForUnit(unit.View) : null;
        var cmd = ClickUnitHandler.CreateCommand(unit, target.View);
        if (cmd is UnitAttack && !unit.CanAttack(u => u.GetThreatHand()?.Weapon))
        {
            Input("tbattack_impossible", j => { j.Key("unit"); Snap.UnitRef(j, unit); j.Key("target"); Snap.UnitRef(j, target); });
            return null;
        }
        // `UnitCommands.CanRunCommand`: a command the unit can't run is dropped silently, and
        // waiting for it would never end, so the turn is ended instead.
        if ((cmd.Type == UnitCommand.CommandType.Standard && unit.State.HasCondition(UnitCondition.CantUseStandardActions)) ||
            !unit.Descriptor.State.IsConscious)
        {
            Input("tbattack_rejected", j => { j.Key("unit"); Snap.UnitRef(j, unit); j.Key("target"); Snap.UnitRef(j, target); });
            return null;
        }
        if (path != null && path.vectorPath != null && path.vectorPath.Count > 0) cmd.ForcedPath = path;
        Input("tbattack", j =>
        {
            j.Key("unit"); Snap.UnitRef(j, unit);
            j.Key("target"); Snap.UnitRef(j, target);
            j.Prop("policy", what);
            j.Prop("round", Cc.RoundNumber);
            j.Prop("turn_index", _turnIndex);
            j.Prop("command", cmd.GetType().Name);
            j.Obj("at");
            j.Prop("from", unit.Position);
            j.Prop("target_at", target.Position);
            var pc = Game.Instance.DefaultPointerController;
            j.Prop("pointer_on", pc.PointerOn != null ? pc.PointerOn.name : null);
            j.Prop("pointer_handler", Traverse.Create(pc).Field("m_SimulateClickHandler").GetValue()?.GetType().Name);
            j.Prop("path_points", path?.vectorPath?.Count ?? -1);
            if (path?.vectorPath != null && path.vectorPath.Count > 0) j.Prop("path_end", path.vectorPath[path.vectorPath.Count - 1]);
            j.End();
        });
        if ((unit.IsEnemy(target) || target.Faction.Neutral))
        {
            var view = target.View;
            EventBus.RaiseEvent((IClickActionHandler h) => h.OnAttackRequested(unit, view));
        }
        unit.Commands.Run(cmd);
        CommandObs.DriverSetManualTarget(unit, target);
        return cmd;
    }

    // The spell of a cast/<guid> action: a cantrip (a known spell of level 0, cast without a slot, so the
    // action bar's spell slot holds the known spell itself) with that blueprint in the unit's spellbooks, the
    // first of its spellbook's level-0 list (a spell can be listed twice; the order is the spellbook's, the
    // same in a recording and its replays). Exactly one spellbook may know it; anything else is a fixture
    // error (thrown while recording, the fixture left while replaying). Levelled spells (memorized slots,
    // spontaneous conversions) take other action bar slots and aren't driven.
    private static Kingmaker.UnitLogic.Abilities.AbilityData FindSpell(UnitEntityData unit, string guid, out string error)
    {
        error = null;
        Kingmaker.UnitLogic.Abilities.AbilityData found = null;
        var books = 0;
        foreach (var book in unit.Descriptor.Spellbooks)
        {
            var a = book.GetKnownSpells(0).FirstOrDefault(s => s?.Blueprint != null && Snap.Guid(s.Blueprint) == guid);
            if (a == null) continue;
            books++;
            found = a;
        }
        if (books != 1) { error = $"cast/{guid}: {unit.UniqueId} knows it in {books} spellbooks, not one"; return null; }
        if (found.Spellbook == null || found.SourceItem != null) { error = $"cast/{guid}: not a spellbook spell of {unit.UniqueId}"; return null; }
        return found;
    }

    private static UnitCommand IssueCast(TurnController turn, UnitEntityData unit, UnitEntityData target, string guid, string what)
    {
        var spell = FindSpell(unit, guid, out var error);
        if (spell == null)
        {
            if (!Sched.Replay) throw new InvalidOperationException(error);
            Sched.Invalidate(error);
            return null;
        }
        var bp = spell.Blueprint;
        // ClickWithSelectedAbilityHandler.CreateCommand's other branches (custom move or attack, items, service
        // casters) and the special cast commands (touch delivery, magus spellstrike, kinetic blades) aren't driven.
        if (bp.GetComponent<Kingmaker.UnitLogic.Abilities.Components.AbilityCustomMove>() != null ||
            bp.GetComponent<Kingmaker.UnitLogic.Abilities.Components.AbilityCustomAttack>() != null ||
            bp.StickyTouch != null || spell.IsCastByServiceCaster || spell.TargetAnchor != Kingmaker.UnitLogic.Abilities.Blueprints.AbilityTargetAnchor.Unit)
        {
            var msg = $"cast/{guid}: the driver casts only plain unit-targeted spells";
            if (!Sched.Replay) throw new InvalidOperationException(msg);
            Sched.Invalidate(msg);
            return null;
        }
        var tw = new Kingmaker.Utility.TargetWrapper(target);
        // The action bar's slot is enabled only for an available spell, and the click handler refuses a target the
        // spell can't take (OnAbilityCastRefused): both end the turn here, as an input.
        if (!spell.IsAvailable || !spell.CanTarget(tw))
        {
            Input("tbcast_impossible", j => { j.Key("unit"); Snap.UnitRef(j, unit); j.Key("target"); Snap.UnitRef(j, target); j.Prop("ability", guid); j.Prop("policy", what); });
            return null;
        }
        var pc = Game.Instance.ClickEventsController;
        if (pc == null || pc != Game.Instance.DefaultPointerController || pc.Mode != PointerMode.Default)
        {
            var msg = $"cast/{guid}: the pointer isn't the default one in its default mode";
            if (!Sched.Replay) throw new InvalidOperationException(msg);
            Sched.Invalidate(msg);
            return null;
        }
        // The action bar's spell slot (MechanicActionBarSlotSpell.OnClick): the ability selected, the pointer in
        // ability mode. Released on every path: a pointer mode left behind would change every later pick.
        Game.Instance.SelectedAbilityHandler.SetAbility(spell);
        try
        {
            if (pc.Mode != PointerMode.Ability || Game.Instance.SelectedAbilityHandler.SelectedAbility != spell)
                throw new InvalidOperationException($"cast/{guid}: selecting the spell did not put the pointer in ability mode");
            var miss = InputPin.PointAt(target);
            if (miss != null)
            {
                if (!Sched.Replay) throw new InvalidOperationException(miss);
                Sched.Invalidate($"recorded cast on {target.UniqueId} at tick {Trace.Tick}: {miss}");
                return null;
            }
            try
            {
                turn.HandlePortraitHover(target, true);
                return IssueHoveredCast(unit, target, spell, tw, guid, what);
            }
            finally
            {
                turn.HandlePortraitHover(target, false);
            }
        }
        finally
        {
            InputPin.PointAt(null);
            if (pc.Mode != PointerMode.Default) pc.ClearPointerMode();
        }
    }

    private static UnitCommand IssueHoveredCast(UnitEntityData unit, UnitEntityData target, Kingmaker.UnitLogic.Abilities.AbilityData spell,
        Kingmaker.Utility.TargetWrapper tw, string guid, string what)
    {
        var path = PathVisualizer.Instance != null ? PathVisualizer.Instance.CurrentPathForUnit(unit.View) : null;
        var cmd = UnitUseAbility.CreateCastCommand(spell, tw);
        if (!(cmd is UnitUseAbility use) || !ReferenceEquals(use.Ability, spell))
            throw new InvalidOperationException($"cast/{guid}: the cast command is {cmd?.GetType().Name ?? "null"}, not a plain UnitUseAbility of the spell");
        // UnitCommands.CanRunCommand drops a command the unit can't run, silently (as for an attack).
        if ((cmd.Type == UnitCommand.CommandType.Standard && unit.State.HasCondition(UnitCondition.CantUseStandardActions)) ||
            !unit.Descriptor.State.IsConscious)
        {
            Input("tbcast_rejected", j => { j.Key("unit"); Snap.UnitRef(j, unit); j.Key("target"); Snap.UnitRef(j, target); j.Prop("ability", guid); j.Prop("policy", what); });
            return null;
        }
        if (path != null && path.vectorPath != null && path.vectorPath.Count > 0) cmd.ForcedPath = path;
        Input("tbcast", j =>
        {
            j.Key("unit"); Snap.UnitRef(j, unit);
            j.Key("target"); Snap.UnitRef(j, target);
            j.Prop("ability", guid);
            j.Prop("spellbook", Snap.Guid(spell.Spellbook.Blueprint));
            j.Prop("policy", what);
            j.Prop("round", Cc.RoundNumber);
            j.Prop("turn_index", _turnIndex);
            j.Prop("command", cmd.GetType().Name);
            j.Prop("type", (int)cmd.Type);
            j.Obj("at");
            j.Prop("from", unit.Position);
            j.Prop("target_at", target.Position);
            var pc = Game.Instance.DefaultPointerController;
            j.Prop("pointer_on", pc.PointerOn != null ? pc.PointerOn.name : null);
            j.Prop("pointer_mode", pc.Mode.ToString());
            j.Prop("pointer_handler", Traverse.Create(pc).Field("m_SimulateClickHandler").GetValue()?.GetType().Name);
            j.Prop("path_points", path?.vectorPath?.Count ?? -1);
            if (path?.vectorPath != null && path.vectorPath.Count > 0) j.Prop("path_end", path.vectorPath[path.vectorPath.Count - 1]);
            j.End();
        });
        EventBus.RaiseEvent((IClickActionHandler h) => h.OnCastRequested(spell, tw));
        unit.Commands.Run(cmd);
        return cmd;
    }

    // ---------------- trace records ----------------

    private static void UnitOrder(Json j, string key)
    {
        j.Arr(key);
        var cc = Cc;
        var units = Traverse.Create(cc).Field("m_Units").GetValue<List<CombatController.TBUnitInfo>>();
        if (units != null)
        {
            foreach (var i in units)
            {
                var u = i.Unit;
                j.Obj();
                j.Prop("unit", u?.UniqueId);
                j.Prop("initiative", u?.CombatState.Initiative ?? 0);
                j.Key("override"); if (i.InitiativeOverride.HasValue) j.W.WriteValue(i.InitiativeOverride.Value); else j.W.WriteNull();
                j.Prop("sequence", i.InitiativeSequence);
                j.Prop("processed", i.InitiativeProcessed);
                j.Prop("surprising", i.Surprising);
                j.Prop("surprised", i.Surprised);
                j.Prop("acting_in_surprise_round", Traverse.Create(i).Field("ActingInSurpriseRound").GetValue<bool>());
                j.End();
            }
        }
        j.EndArr();
    }

    private static void Tb(string ev, Action<Json> extra = null)
    {
        if (!Trace.Active || !InWindow) return;
        try
        {
            Trace.Emit("tb", j =>
            {
                j.Prop("ev", ev);
                j.Prop("round", Cc.RoundNumber);
                extra?.Invoke(j);
                j.Prop("game_time", Game.Instance.Player.GameTime.Ticks);
            });
        }
        catch (Exception e)
        {
            Trace.HookFailed("tb " + ev, e);
        }
    }

    public static void CombatStartPostfix(bool isPartyCombatStateChanged) =>
        Tb("combat_start", j => { j.Prop("party_combat_state_changed", isPartyCombatStateChanged); j.Prop("surprise_round", Traverse.Create(Cc).Field("m_HasSurpriseRound").GetValue<bool>()); UnitOrder(j, "units"); });

    public static void CombatEndPrefix() => Tb("combat_end");

    // StartRound returns without starting a round in some states (no units, nobody in combat):
    // the record is written only if the round number advanced.
    public static void StartRoundPrefix(out int __state) => __state = Cc.RoundNumber;

    public static void StartRoundPostfix(int __state)
    {
        if (Cc.RoundNumber != __state) Tb("round_start", j => UnitOrder(j, "units"));
    }

    public static void SortUnitsPostfix(string reason) => Tb("sort", j => { j.Prop("reason", reason); UnitOrder(j, "units"); });

    // The entry to CombatController.StartTurn: before its buff sweep of the awake units and
    // CurrentTurn.Start, so a replay knows where the sweep begins ("turn_start" is the postfix).
    public static void StartTurnPrefix(UnitEntityData unit) =>
        Tb("turn_start_begin", j => { j.Key("unit"); Snap.UnitRef(j, unit); });

    // CombatController.CurrentTurn set to null (the turn controller cleared): which unit stopped
    // being current, and when.
    public static void SetCurrentTurnPrefix(CombatController __instance, TurnController value)
    {
        var cur = __instance.CurrentTurn;
        if (value == null && cur != null) Tb("turn_clear", j => { j.Key("unit"); Snap.UnitRef(j, cur.Rider); });
    }

    // DropLootAndDestroyOnDeactivate's Observable.Timer callback (UniRx, on Unity's clock): the
    // frame the buff's removal comes from.
    public static void LootTimerPrefix(object __instance)
    {
        try
        {
            var buff = Traverse.Create(__instance).Field("buff").GetValue<Kingmaker.UnitLogic.Buffs.Buff>();
            Tb("loot_timer", j => { j.Prop("fact", buff?.UniqueId); j.Key("owner"); Snap.UnitRef(j, buff?.Owner?.Unit); });
        }
        catch (Exception e) { Trace.HookFailed("tb loot_timer", e); }
    }

    // "env" for a weapon attack's resolve (a projectile hit is resolved later than its roll):
    // both units' positions as it triggers, under "at".
    public static void ResolvePrefix(Kingmaker.RuleSystem.Rules.RuleAttackWithWeaponResolve __instance)
    {
        if (!Trace.Active || !InWindow) return;
        try
        {
            var a = __instance.Initiator;
            var t = __instance.Target;
            Trace.Emit("env", j =>
            {
                j.Prop("rule", "RuleAttackWithWeaponResolve");
                j.Key("initiator"); Snap.UnitRef(j, a);
                j.Key("target"); Snap.UnitRef(j, t);
                j.Obj("at");
                j.Prop("initiator_pos", a.Position);
                j.Prop("target_pos", t.Position);
                j.End();
            });
        }
        catch (Exception e) { Trace.HookFailed("env RuleAttackWithWeaponResolve", e); }
    }

    // "env" for a root flat-footed check (the UI's initiative tracker and tooltips, the AI): what
    // its OnTrigger reads besides the units' conditions, as it triggers. Inside another rule the
    // enclosing attack roll's env covers it.
    public static void FlatFootedPrefix(Kingmaker.RuleSystem.Rules.RuleCheckTargetFlatFooted __instance)
    {
        if (!Trace.Active || !InWindow || Rules.OpenRules != 1) return;
        try
        {
            var a = __instance.Initiator;
            var t = __instance.Target;
            Trace.Emit("env", j =>
            {
                j.Prop("rule", "RuleCheckTargetFlatFooted");
                j.Key("initiator"); Snap.UnitRef(j, a);
                j.Key("target"); Snap.UnitRef(j, t);
                var remembers = GroupObs.RemembersPassive(t, a);
                j.Key("target_remembers_initiator"); if (remembers.HasValue) j.W.WriteValue(remembers.Value); else j.W.WriteNull();
                WriteOpt(j, "target_can_act_in_combat", CanActInCombatPassive(t));
                WriteOpt(j, "concealment_initiator_from_target", ConcealmentPassive(t, a));
            });
        }
        catch (Exception e) { Trace.HookFailed("env RuleCheckTargetFlatFooted", e); }
    }

    public static void StartTurnPostfix(UnitEntityData unit)
    {
        if (Trace.Active && InWindow) _turnIndex++;
        StartTurnRecord(unit);
    }

    private static void StartTurnRecord(UnitEntityData unit) =>
        Tb("turn_start", j => { j.Key("unit"); Snap.UnitRef(j, unit); j.Prop("status", Cc.CurrentTurn?.Status.ToString()); });

    public static void TurnEndPostfix(TurnController __instance) =>
        Tb("turn_end", j => { j.Key("unit"); Snap.UnitRef(j, __instance.Rider); });

    private static void Cmd(string ev, UnitCommand c)
    {
        if (!Trace.Active || !InWindow || c == null) return;
        try
        {
            Trace.Emit("cmd", j =>
            {
                j.Prop("ev", ev);
                j.Prop("class", c.GetType().Name);
                j.Key("unit"); Snap.UnitRef(j, c.Executor);
                j.Key("target"); Snap.UnitRef(j, c.TargetUnit);
                j.Prop("type", (int)c.Type);
                j.Prop("ai_action", c.AiAction?.Blueprint != null ? Snap.Guid(c.AiAction.Blueprint) : null);
                if (ev == "end") j.Prop("result", (int)c.Result);
                j.Obj("at");
                j.Prop("pos", c.Executor != null ? c.Executor.Position : Vector3.zero);
                j.End();
            });
        }
        catch (Exception e)
        {
            Trace.HookFailed("cmd " + ev, e);
        }
    }

    public static void CommandStartPostfix(UnitCommand __instance)
    {
        if (__instance.IsStarted) Cmd("start", __instance);
    }

    public static void CommandEndedPrefix(UnitCommand __instance, out bool __state) => __state = __instance.IsFinished;

    public static void CommandEndedPostfix(UnitCommand __instance, bool __state)
    {
        if (!__state && __instance.IsFinished) Cmd("end", __instance);
    }

    public static void CommandRunPostfix(UnitCommand cmd) => Cmd("run", cmd);

    public static void SelectActionPostfix(UnitEntityData unit, bool __result)
    {
        if (!Trace.Active || !InWindow) return;
        try
        {
            Trace.Emit("ai", j =>
            {
                j.Key("unit"); Snap.UnitRef(j, unit);
                j.Prop("selected", __result);
            });
        }
        catch (Exception e)
        {
            Trace.HookFailed("ai", e);
        }
    }

    public static void Install(Harmony h)
    {
        void P(MethodBase m, string prefix = null, string postfix = null)
        {
            if (m == null) { Health.Failed("tb hook target missing", new Exception(prefix ?? postfix)); return; }
            try
            {
                h.Patch(m, prefix: prefix == null ? null : new HarmonyMethod(typeof(TurnBased), prefix),
                    postfix: postfix == null ? null : new HarmonyMethod(typeof(TurnBased), postfix));
            }
            catch (Exception e)
            {
                Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name, e);
            }
        }

        IdTrace.Install(h);
        var cc = typeof(CombatController);
        P(AccessTools.Method(cc, "HandleCombatStart"), postfix: nameof(CombatStartPostfix));
        P(AccessTools.Method(cc, "HandleCombatEnd"), prefix: nameof(CombatEndPrefix));
        try
        {
            h.Patch(AccessTools.Method(cc, "StartRound"), prefix: new HarmonyMethod(typeof(TurnBased), nameof(StartRoundPrefix)), postfix: new HarmonyMethod(typeof(TurnBased), nameof(StartRoundPostfix)));
        }
        catch (Exception e) { Health.Failed("patch StartRound", e); }
        P(AccessTools.Method(cc, "SortUnits"), postfix: nameof(SortUnitsPostfix));
        P(AccessTools.Method(cc, nameof(CombatController.StartTurn)), nameof(StartTurnPrefix), nameof(StartTurnPostfix));
        P(AccessTools.PropertySetter(cc, nameof(CombatController.CurrentTurn)), prefix: nameof(SetCurrentTurnPrefix));
        P(AccessTools.Method(typeof(Kingmaker.RuleSystem.Rules.RuleAttackWithWeaponResolve), "OnTrigger"), prefix: nameof(ResolvePrefix));
        {
            // The timer's lambda lives in a compiler-generated closure class.
            var lootType = typeof(Kingmaker.UnitLogic.FactLogic.DropLootAndDestroyOnDeactivate);
            var lambda = lootType.GetNestedTypes(BindingFlags.NonPublic)
                .SelectMany(t => t.GetMethods(BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public | BindingFlags.DeclaredOnly))
                .FirstOrDefault(m => m.Name.StartsWith("<OnActivate>", StringComparison.Ordinal));
            P(lambda, prefix: nameof(LootTimerPrefix));
        }
        P(AccessTools.Method(typeof(TurnController), "End"), postfix: nameof(TurnEndPostfix));
        var uc = typeof(UnitCommand);
        P(AccessTools.Method(uc, nameof(UnitCommand.Start)), postfix: nameof(CommandStartPostfix));
        P(AccessTools.Method(uc, "OnEnded"), nameof(CommandEndedPrefix), nameof(CommandEndedPostfix));
        P(AccessTools.Method(typeof(UnitCommands), nameof(UnitCommands.Run), new[] { typeof(UnitCommand) }), postfix: nameof(CommandRunPostfix));
        P(AccessTools.Method(typeof(AiBrainController), nameof(AiBrainController.SelectAction)), postfix: nameof(SelectActionPostfix));
        P(AccessTools.Method(typeof(Kingmaker.RuleSystem.Rules.RuleAttackRoll), nameof(Kingmaker.RuleSystem.Rules.RuleAttackRoll.OnTrigger)), prefix: nameof(AttackRollPrefix));
        P(AccessTools.Method(typeof(Kingmaker.RuleSystem.Rules.RuleCheckTargetFlatFooted), nameof(Kingmaker.RuleSystem.Rules.RuleCheckTargetFlatFooted.OnTrigger)), prefix: nameof(FlatFootedPrefix));
    }

    private static readonly AccessTools.FieldRef<Kingmaker.View.UnitEntityView, TimeSpan> StartGetUpTime =
        AccessTools.FieldRefAccess<Kingmaker.View.UnitEntityView, TimeSpan>("m_StartGetUpTime");

    // `UnitEntityView.IsGetUp` (RuleCalculateAC reads it when the posture isn't prone) without
    // the side-effecting `CombatController.IsInTurnBasedCombat()`; null without a view.
    private static readonly AccessTools.FieldRef<Kingmaker.UnitLogic.Parts.UnitPartConcealment, IList> ConcealmentEntries =
        AccessTools.FieldRefAccess<Kingmaker.UnitLogic.Parts.UnitPartConcealment, IList>("m_Concealments");
    private static readonly AccessTools.FieldRef<Kingmaker.UnitLogic.Parts.UnitPartConcealment, IList> BlindsightRanges =
        AccessTools.FieldRefAccess<Kingmaker.UnitLogic.Parts.UnitPartConcealment, IList>("m_BlindsightRanges");

    // `CombatState.CanActInCombat` without `CombatController.IsInTurnBasedCombat()` (which writes
    // `CanChangeWeaponSet` outside turn-based combat): the getter's own branches, with the oracle's
    // passive predicate for turn-based combat. A saddled unit's `GetRider()` resolves an entity
    // reference (which can create a proxy): null (not observed).
    private static bool? CanActInCombatPassive(UnitEntityData u)
    {
        var s = u.CombatState;
        if (!s.IsInCombat) return false;
        if (u.SaddledPart != null) return null;
        var waiting = s.Cooldown.Initiative > 0f || (Hooks.InTurnBasedCombat() && !s.HadTurnInTMBCombat);
        return !waiting || s.NotSurprised;
    }

    // `UnitPartConcealment.Calculate(initiator, target, attack)` only on its plain path, null otherwise
    // (not observed): blindsight ranges and distance-bound entries read `View.Corpulence` (size caches),
    // weapon-range entries and bad weather read the current rule's or the first weapon, and See
    // Invisibility or True Seeing on the initiator evaluate condition exceptions (which load blueprints).
    private static int? ConcealmentPassive(UnitEntityData initiator, UnitEntityData target, bool attack = false)
    {
        var st = initiator.Descriptor.State;
        if (st.HasCondition(Kingmaker.UnitLogic.UnitCondition.SeeInvisibility) || st.HasCondition(Kingmaker.UnitLogic.UnitCondition.TrueSeeing)) return null;
        var mine = initiator.Get<Kingmaker.UnitLogic.Parts.UnitPartConcealment>();
        var theirs = target.Get<Kingmaker.UnitLogic.Parts.UnitPartConcealment>();
        if (mine != null && !mine.IgnoreAll && BlindsightRanges(mine) != null) return null;
        var entries = theirs != null ? ConcealmentEntries(theirs) : null;
        if (entries != null)
            foreach (Kingmaker.UnitLogic.Parts.UnitPartConcealment.ConcealmentEntry e in entries)
                if (e.DistanceGreater.Meters > 0f || e.RangeType.HasValue) return null;
        if (Game.Instance.Player.Weather.ActualWeather >= Kingmaker.Blueprints.Root.BlueprintRoot.Instance.WeatherSettings.ConcealmentBeginsOn) return null;
        return (int)Kingmaker.UnitLogic.Parts.UnitPartConcealment.Calculate(initiator, target, attack);
    }

    private static void WriteOpt(Json j, string key, int? v) { j.Key(key); if (v.HasValue) j.W.WriteValue(v.Value); else j.W.WriteNull(); }
    private static void WriteOpt(Json j, string key, bool? v) { j.Key(key); if (v.HasValue) j.W.WriteValue(v.Value); else j.W.WriteNull(); }

    internal static bool? IsGetUpPassive(UnitEntityData u)
    {
        var view = u.View;
        if (view == null) return null;
        var am = view.AnimationManager;
        if (Hooks.InTurnBasedCombat() || Kingmaker.Armies.TacticalCombat.TacticalCombatHelper.IsActive)
            return am != null && am.IsStandUp;
        if (am == null || !am.IsStandUp)
            return Game.Instance.TimeController.GameTime - StartGetUpTime(view) < TimeSpan.FromSeconds(0.5);
        return true;
    }

    // "env": the controller state an attack roll reads besides rule fields (engagement and
    // flanking, the target's memory of the attacker, whether it can act in combat yet,
    // concealment both ways), written as the roll starts. Positions go under "at" (T1).
    public static void AttackRollPrefix(Kingmaker.RuleSystem.Rules.RuleAttackRoll __instance)
    {
        if (!Trace.Active || !InWindow) return;
        try
        {
            var a = __instance.Initiator;
            var t = __instance.Target;
            Trace.Emit("env", j =>
            {
                j.Prop("rule", "RuleAttackRoll");
                j.Key("initiator"); Snap.UnitRef(j, a);
                j.Key("target"); Snap.UnitRef(j, t);
                // `IsFlanked` asks `CombatController.IsInTurnBasedCombat()`, which writes
                // `CanChangeWeaponSet` when not in turn-based combat; it is read only when that
                // predicate (checked here from its inputs) holds and the call is passive.
                if (Hooks.InTurnBasedCombat()) j.Prop("target_flanked", t.CombatState.IsFlanked);
                else { j.Key("target_flanked"); j.W.WriteNull(); }
                j.Arr("target_engaged_by");
                foreach (var u in t.CombatState.EngagedBy.OrderBy(u => u.UniqueId, StringComparer.Ordinal)) j.W.WriteValue(u.UniqueId);
                j.EndArr();
                var remembers = GroupObs.RemembersPassive(t, a);
                j.Key("target_remembers_initiator"); if (remembers.HasValue) j.W.WriteValue(remembers.Value); else j.W.WriteNull();
                WriteOpt(j, "target_can_act_in_combat", CanActInCombatPassive(t));
                var getUp = IsGetUpPassive(t);
                j.Key("target_is_get_up"); if (getUp.HasValue) j.W.WriteValue(getUp.Value); else j.W.WriteNull();
                WriteOpt(j, "concealment_target_from_initiator", ConcealmentPassive(a, t, attack: true));
                WriteOpt(j, "concealment_initiator_from_target", ConcealmentPassive(t, a));
                j.Obj("at");
                j.Prop("initiator_pos", a.Position);
                j.Prop("target_pos", t.Position);
                j.End();
            });
        }
        catch (Exception e)
        {
            Trace.HookFailed("env RuleAttackRoll", e);
        }
    }
}

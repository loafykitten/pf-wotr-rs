using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using HarmonyLib;
using Kingmaker;
using Kingmaker.AreaLogic.Etudes;
using Kingmaker.AreaLogic.QuestSystem;
using Kingmaker.Blueprints;
using Kingmaker.Blueprints.Quests;
using Kingmaker.Controllers;
using Kingmaker.Controllers.Dialog;
using Kingmaker.DialogSystem.Blueprints;
using Kingmaker.EntitySystem;
using Kingmaker.EntitySystem.Entities;
using Kingmaker.EntitySystem.Stats;
using Kingmaker.GameModes;
using Kingmaker.RuleSystem;
using Kingmaker.UnitLogic;
using Kingmaker.UnitLogic.Alignments;
using UnityEngine;

namespace WrathOracle;

// Ticks and time, dialogue transitions, and persistent mutations, for the fixture trace.
internal static class Hooks
{
    // ---------------- ticks and time ----------------

    private static readonly HashSet<GameMode> ModesSeen = new HashSet<GameMode>();
    private static readonly AccessTools.FieldRef<GameMode, string[]> ModeNames = AccessTools.FieldRefAccess<GameMode, string[]>("m_NamesCache");

    public static void ResetWindow()
    {
        ModesSeen.Clear();
        Updating.Clear();
    }

    // GameMode.Tick runs once per Game.Tick that ticks (not while loading): one sim tick.
    public static void ModeTickPrefix(GameMode __instance)
    {
        if (!Trace.Active) return;
        try
        {
            Trace.Tick++;
            var gap = Rng.Checkpoint();
            if (ModesSeen.Add(__instance))
            {
                Trace.Emit("controllers", j =>
                {
                    j.Prop("mode", (int)__instance.Type);
                    j.Prop("mode_name", __instance.Type.ToString());
                    j.Arr("order");
                    foreach (var n in ModeNames(__instance) ?? new string[0]) j.W.WriteValue(n);
                    j.EndArr();
                });
            }
            Trace.Emit("tick", j =>
            {
                j.Prop("mode", (int)__instance.Type);
                j.Prop("frame", Time.frameCount);
                j.Prop("delta", Time.deltaTime);
                j.Prop("unscaled_delta", Time.captureDeltaTime > 0f ? Time.captureDeltaTime : Time.unscaledDeltaTime);
                j.Prop("wall_delta", WallClock.RealUnscaledDeltaTime());
                j.Prop("time_scale", Time.timeScale);
                j.Prop("capture_delta", Time.captureDeltaTime);
                j.Prop("rng_gap", gap);
            });
            Observe.Tick();
            DeathFxPin.Flush(); // death effect records held while a rule was open
            CamPin.TickStart(); // a recorded view change the replay missed
        }
        catch (Exception e)
        {
            Trace.HookFailed("tick", e);
        }
    }

    // TimeController.Tick assigns DeltaTime = min(PlayerTimeScale * DebugTimeScale / 12, raw)
    // while PlayerTimeScale holds the turn-based effective scale (restored afterwards); the
    // setter's prefix sees the scales actually used.
    private static bool _inTick, _haveEffective;
    private static float _effPlayer, _effDebug;

    public static void DeltaSetterPrefix(TimeController __instance)
    {
        if (!_inTick || _haveEffective) return;
        _effPlayer = __instance.PlayerTimeScale;
        _effDebug = __instance.DebugTimeScale;
        _haveEffective = true;
    }

    public struct TimeState
    {
        public bool JustActivated;
        public float Raw, PlayerTimeScale, DebugTimeScale;
    }

    public static void TimePrefix(TimeController __instance, out TimeState __state)
    {
        __state = default;
        _inTick = true;
        _haveEffective = false;
        if (!Trace.Active) return;
        __state.JustActivated = __instance.m_JustActivated;
        __state.Raw = __instance.m_JustActivated ? OracleTime.UnscaledDeltaTime() : OracleTime.DeltaTime();
        __state.PlayerTimeScale = __instance.PlayerTimeScale;
        __state.DebugTimeScale = __instance.DebugTimeScale;
    }

    public static void TimePostfix(TimeController __instance, TimeState __state)
    {
        OracleTime.AfterTick();
        _inTick = false;
        if (!Trace.Active) return;
        try
        {
            var game = Game.Instance;
            Trace.Emit("time", j =>
            {
                j.Prop("raw", __state.Raw);
                j.Prop("just_activated", __state.JustActivated);
                j.Prop("player_time_scale", __state.PlayerTimeScale);
                j.Prop("debug_time_scale", __state.DebugTimeScale);
                j.Prop("effective_player_time_scale", _haveEffective ? _effPlayer : __state.PlayerTimeScale);
                j.Prop("cap", (_haveEffective ? _effPlayer * _effDebug : __state.PlayerTimeScale * __state.DebugTimeScale) / 12f);
                j.Prop("delta", __instance.DeltaTime);
                j.Prop("game_delta", __instance.GameDeltaTime);
                j.Prop("game_time", game.Player.GameTime.Ticks);
                j.Prop("real_time", game.Player.RealTime.Ticks);
                j.Prop("turn_based", InTurnBasedCombat());
                j.Prop("time_scale_after", Time.timeScale);
            });
        }
        catch (Exception e)
        {
            Trace.HookFailed("time", e);
        }
    }

    // CombatController.IsInTurnBasedCombat() can set CanChangeWeaponSet; mirror its test.
    public static bool InTurnBasedCombat()
    {
        var game = Game.Instance;
        if (game?.Player == null) return false;
        return game.Player.IsInCombat && game.Player.IsTurnBasedModeOn() && game.CurrentMode != GameModeType.Cutscene;
    }

    // ---------------- dialogue ----------------

    private static readonly AccessTools.FieldRef<DialogController, bool> CueScheduled = AccessTools.FieldRefAccess<DialogController, bool>("m_CuePlayScheduled");
    private static readonly AccessTools.FieldRef<DialogController, BlueprintCueBase> CueToPlay = AccessTools.FieldRefAccess<DialogController, BlueprintCueBase>("m_CueToPlay");
    private static readonly AccessTools.FieldRef<DialogController, BlueprintCueBase> ContinueCue = AccessTools.FieldRefAccess<DialogController, BlueprintCueBase>("m_ContinueCue");
    private static readonly AccessTools.FieldRef<DialogController, List<SkillCheckResult>> SkillChecks = AccessTools.FieldRefAccess<DialogController, List<SkillCheckResult>>("m_SkillChecks");

    public static int DialogTransitions;

    public static bool IsCueScheduled(DialogController dc) => CueScheduled(dc);

    public static void StartDialogPrefix(BlueprintDialog dialog, UnitEntityData initiator, UnitEntityData unit)
    {
        if (!Trace.Active) return;
        Safe("dlg_request", j =>
        {
            j.Prop("dialog", Snap.Guid(dialog));
            j.Key("initiator"); Snap.UnitRef(j, initiator);
            j.Key("unit"); Snap.UnitRef(j, unit);
            j.Prop("rule", Rules.CurrentRuleId);
        });
    }

    public static void StartedPostfix(DialogController __instance)
    {
        if (!Trace.Active) return;
        Safe("dlg_started", j =>
        {
            j.Prop("dialog", Snap.Guid(__instance.Dialog));
            j.Key("first_speaker"); Snap.UnitRef(j, __instance.FirstSpeaker);
            j.Prop("scheduled_cue", Snap.Guid(CueScheduled(__instance) ? CueToPlay(__instance) : null));
        });
    }

    public static void SelectAnswerPrefix(DialogController __instance, BlueprintAnswer answer, UnitEntityData manualUnitSelection)
    {
        if (!Trace.Active) return;
        Safe("dlg_answer", j =>
        {
            j.Prop("answer", Snap.Guid(answer));
            j.Prop("index", __instance.Answers.ToList().IndexOf(answer));
            j.Prop("cue", Snap.Guid(__instance.CurrentCue));
            j.Key("manual_unit"); Snap.UnitRef(j, manualUnitSelection);
        });
    }

    public static void SelectAnswerPostfix(DialogController __instance)
    {
        if (!Trace.Active) return;
        Safe("dlg_answer_end", j =>
        {
            j.Prop("scheduled", CueScheduled(__instance));
            j.Prop("scheduled_cue", Snap.Guid(CueToPlay(__instance)));
            j.Key("acting_unit"); Snap.UnitRef(j, __instance.ActingUnit);
            j.Prop("dialog_active", __instance.Dialog != null);
        });
    }

    public static void PlayCuePrefix(BlueprintCueBase cue)
    {
        if (!Trace.Active) return;
        Safe("dlg_play", j => { j.Prop("cue", Snap.Guid(cue)); j.Prop("class", cue?.GetType().Name); });
    }

    public static void PlayCheckPrefix(BlueprintCheck check)
    {
        if (!Trace.Active) return;
        Safe("dlg_check", j =>
        {
            j.Prop("check", Snap.Guid(check));
            j.Prop("stat", (int)check.Type);
            j.Prop("authored_dc", check.DC);
            j.Prop("hidden", check.Hidden);
        });
    }

    public static void PlayCheckPostfix(DialogController __instance, BlueprintCheck check)
    {
        if (!Trace.Active) return;
        Safe("dlg_check_end", j =>
        {
            j.Prop("check", Snap.Guid(check));
            j.Prop("passed", __instance.LocalPassedChecks.Contains(check));
            j.Prop("failed", __instance.LocalFailedChecks.Contains(check));
        });
    }

    public static void DialogTickPrefix(DialogController __instance, out bool __state)
    {
        __state = Trace.Active && __instance.Dialog != null && CueScheduled(__instance);
    }

    // The completed transition: after the Tick that played the scheduled cue.
    public static void DialogTickPostfix(DialogController __instance, bool __state)
    {
        if (!__state || !Trace.Active) return;
        DialogTransitions++;
        Safe("dlg_transition", j =>
        {
            var d = Game.Instance.Player.Dialog;
            j.Prop("dialog", Snap.Guid(__instance.Dialog));
            j.Prop("cue", Snap.Guid(__instance.CurrentCue));
            j.Arr("answers");
            foreach (var a in __instance.Answers) j.W.WriteValue(Snap.Guid(a));
            j.EndArr();
            j.Prop("continue_cue", Snap.Guid(ContinueCue(__instance)));
            j.Prop("scheduled", CueScheduled(__instance));
            j.Key("acting_unit"); Snap.UnitRef(j, __instance.ActingUnit);
            j.Key("current_speaker"); Snap.UnitRef(j, __instance.CurrentSpeaker);
            j.Arr("skill_checks");
            foreach (var s in SkillChecks(__instance) ?? new List<SkillCheckResult>())
            {
                j.W.WriteStartObject();
                j.Key("unit"); Snap.UnitRef(j, s.ActingUnit);
                j.Prop("stat", (int)s.StatType);
                j.Prop("dc", s.DC);
                j.Prop("passed", s.Passed);
                j.Prop("roll", s.RollResult);
                j.Prop("d20", s.D20);
                j.Prop("stat_value", s.StatValue);
                j.W.WriteEndObject();
            }
            j.EndArr();
            j.Prop("local_shown_cues", __instance.LocalShownCues.Count);
            j.Prop("local_selected_answers", __instance.LocalSelectedAnswers.Count);
            j.Arr("local_passed_checks"); foreach (var c in __instance.LocalPassedChecks.Select(Snap.Guid).OrderBy(x => x, StringComparer.Ordinal)) j.W.WriteValue(c); j.EndArr();
            j.Arr("local_failed_checks"); foreach (var c in __instance.LocalFailedChecks.Select(Snap.Guid).OrderBy(x => x, StringComparer.Ordinal)) j.W.WriteValue(c); j.EndArr();
            j.Prop("global_shown_cues", d.ShownCues.Count);
            j.Prop("global_selected_answers", d.SelectedAnswers.Count);
            j.Prop("global_shown_dialogs", d.ShownDialogs.Count);
            j.Prop("alignment", (int)Game.Instance.Player.Alignment);
        });
        TextObs.WriteText(__instance);
    }

    public static void StopDialogPrefix(DialogController __instance)
    {
        if (!Trace.Active || __instance.Dialog == null) return;
        Safe("dlg_stop", j => j.Prop("dialog", Snap.Guid(__instance.Dialog)));
    }

    private static void Safe(string kind, Action<Json> body)
    {
        try { Trace.Emit(kind, body); }
        catch (Exception e) { Trace.HookFailed(kind, e); }
    }

    // ---------------- mutations ----------------

    // A mutation in Pause mode (no controller ticks there: UI or input code made it) carries its
    // caller chain (origin, as a root rule's), declared as paused_origins.
    private static void Mut(string what, Action<Json> body)
    {
        if (!Trace.Active || !Trace.OnMainThread) return;
        try
        {
            var origin = Game.Instance?.CurrentMode == GameModeType.Pause ? Rules.Origin() : null;
            Trace.Emit("mut", j =>
            {
                j.Prop("what", what);
                j.Prop("rule", Rules.CurrentRuleId);
                j.Prop("h", Rules.CurrentHandlerId);
                if (origin != null) j.Prop("origin", origin);
                body(j);
            });
        }
        catch (Exception e)
        {
            Trace.HookFailed("mut " + what, e);
        }
    }

    private static void StatId(Json j, ModifiableValue mv)
    {
        j.Prop("stat", (int)mv.Type);
        j.Prop("owner", mv.Owner?.Unit?.UniqueId);
    }

    public static void AddModifierPostfix(ModifiableValue __instance, ModifiableValue.Modifier mod) =>
        Mut("stat_mod_add", j => { StatId(j, __instance); j.Key("mod"); Snap.Modifier(j, mod); j.Prop("modified", __instance.ModifiedValue); });

    public static void RemoveModifierPostfix(ModifiableValue __instance, ModifiableValue.Modifier mod, bool __result)
    {
        if (!__result) return;
        Mut("stat_mod_remove", j => { StatId(j, __instance); j.Key("mod"); Snap.Modifier(j, mod); j.Prop("modified", __instance.ModifiedValue); });
    }

    private static readonly FieldInfo ScopeValue = AccessTools.Field(typeof(ModifiableValue.TemporaryModifiersScope), "m_ModifiableValue");
    private static readonly FieldInfo ScopeMods = AccessTools.Field(typeof(ModifiableValue.TemporaryModifiersScope), "m_Modifiers");

    public static void ScopeAddPostfix(object __instance) => ScopeMut("stat_temp_scope_add", __instance);
    public static void ScopeRemovePrefix(object __instance) => ScopeMut("stat_temp_scope_remove", __instance);

    private static void ScopeMut(string what, object scope) =>
        Mut(what, j =>
        {
            var mv = ScopeValue.GetValue(scope) as ModifiableValue;
            if (mv != null) StatId(j, mv);
            j.Arr("mods");
            if (ScopeMods.GetValue(scope) is List<ModifiableValue.Modifier> mods) foreach (var m in mods) Snap.Modifier(j, m);
            j.EndArr();
        });

    // Only the outermost UpdateValue of a stat reports (overrides call the base).
    private static readonly HashSet<ModifiableValue> Updating = new HashSet<ModifiableValue>(new RefEq<ModifiableValue>());

    public static void UpdateValuePrefix(ModifiableValue __instance, out int __state)
    {
        __state = Updating.Add(__instance) ? __instance.ModifiedValue : int.MinValue;
    }

    // Releases the outermost marker even when the game's UpdateValue throws.
    public static Exception UpdateValueFinalizer(ModifiableValue __instance, int __state, Exception __exception)
    {
        if (__exception != null && __state != int.MinValue) Updating.Remove(__instance);
        return __exception;
    }

    public static void UpdateValuePostfix(ModifiableValue __instance, int __state)
    {
        if (__state == int.MinValue) return;
        Updating.Remove(__instance);
        if (__instance.ModifiedValue == __state) return;
        Mut("stat_value", j => { StatId(j, __instance); j.Prop("old", __state); j.Prop("new", __instance.ModifiedValue); j.Prop("permanent", __instance.PermanentValue); });
    }

    public static void RuleBonusPostfix(RulebookEvent __instance, Kingmaker.RuleSystem.Rules.Modifier bonus) =>
        Mut("rule_bonus", j =>
        {
            j.Prop("target_rule", Rules.IdOf(__instance));
            j.Prop("value", bonus.Value);
            j.Prop("descriptor", (int)bonus.Descriptor);
            j.Prop("type", (int)bonus.Type);
            j.Prop("stat_type", (int)bonus.Stat);
            j.Key("fact"); Snap.FactRef(j, bonus.Fact);
        });

    public static void TempModifierPrefix(RulebookEvent __instance, ModifiableValue.Modifier modifier) =>
        Mut("rule_temp_mod", j =>
        {
            j.Prop("target_rule", Rules.IdOf(__instance));
            j.Prop("rule_triggered", __instance.IsTriggered);
            j.Prop("on_stat", modifier?.AppliedTo != null ? (int)modifier.AppliedTo.Type : -1);
            j.Prop("on_owner", modifier?.AppliedTo?.Owner?.Unit?.UniqueId);
            j.Key("mod"); Snap.Modifier(j, modifier);
        });

    public static void FactAttachPostfix(EntityFactsManager __instance, EntityFact fact) =>
        Mut("fact_attach", j => { j.Prop("owner", __instance.Owner?.UniqueId); FactBody(j, fact); });

    public static void FactDetachPrefix(EntityFactsManager __instance, EntityFact fact) =>
        Mut("fact_detach", j => { j.Prop("owner", __instance.Owner?.UniqueId); FactBody(j, fact); });

    private static void FactBody(Json j, EntityFact fact)
    {
        j.Key("fact"); Snap.FactRef(j, fact);
        j.Prop("class", fact?.GetType().Name);
        j.Prop("rank", fact?.GetRank() ?? 0);
        if (fact is Kingmaker.UnitLogic.Buffs.Buff b)
        {
            j.Prop("end_time", b.EndTime.Ticks);
            j.Prop("is_permanent", b.IsPermanent);
        }
    }

    public static void DamagePrefix(UnitDescriptor __instance, out int __state) => __state = __instance.Damage;

    public static void DamagePostfix(UnitDescriptor __instance, int __state)
    {
        if (__instance.Damage == __state) return;
        Mut("damage", j => { j.Prop("unit", __instance.Unit?.UniqueId); j.Prop("old", __state); j.Prop("new", __instance.Damage); });
    }

    public static void FlagPostfix(BlueprintUnlockableFlag flag, MethodBase __originalMethod) =>
        Mut("flag_" + __originalMethod.Name, j =>
        {
            j.Prop("flag", Snap.Guid(flag));
            var flags = Game.Instance.Player.UnlockableFlags;
            j.Prop("unlocked", flags.IsUnlocked(flag));
            j.Prop("value", flags.IsUnlocked(flag) ? flags.GetFlagValue(flag) : 0);
        });

    public static void EtudePostfix(BlueprintEtude bp, MethodBase __originalMethod) =>
        Mut("etude_" + __originalMethod.Name, j => j.Prop("etude", Snap.Guid(bp)));

    public static void ObjectivePostfix(QuestObjective __instance, QuestObjectiveState state) =>
        Mut("objective", j => { j.Prop("objective", Snap.Guid(__instance.Blueprint)); j.Prop("state", (int)state); });

    public static void XpPostfix(UnitProgressionData __instance, int exp) =>
        Mut("xp", j => { j.Prop("unit", __instance.Owner?.Unit?.UniqueId); j.Prop("gained", exp); j.Prop("total", __instance.Experience); });

    public static void AlignmentPostfix(UnitAlignment __instance, AlignmentShiftDirection direction, int value) =>
        Mut("alignment", j => { j.Prop("direction", (int)direction); j.Prop("value", value); j.Prop("now", (int)__instance.ValueRaw); });

    // ---------------- installation ----------------

    public static void Install(Harmony h)
    {
        void P(MethodBase m, string prefix = null, string postfix = null)
        {
            if (m == null) { Health.Failed("hook target missing", new Exception(prefix ?? postfix)); return; }
            try
            {
                h.Patch(m, prefix: prefix == null ? null : new HarmonyMethod(typeof(Hooks), prefix),
                    postfix: postfix == null ? null : new HarmonyMethod(typeof(Hooks), postfix));
            }
            catch (Exception e)
            {
                Health.Failed("patch " + m.DeclaringType?.Name + "." + m.Name, e);
            }
        }

        P(AccessTools.Method(typeof(GameMode), nameof(GameMode.Tick)), prefix: nameof(ModeTickPrefix));
        P(AccessTools.Method(typeof(TimeController), nameof(TimeController.Tick)), nameof(TimePrefix), nameof(TimePostfix));

        var dc = typeof(DialogController);
        P(AccessTools.Method(dc, "StartDialog"), prefix: nameof(StartDialogPrefix));
        P(AccessTools.Method(dc, "StartScheduledDialogImmediately"), postfix: nameof(StartedPostfix));
        P(AccessTools.Method(dc, nameof(DialogController.SelectAnswer)), nameof(SelectAnswerPrefix), nameof(SelectAnswerPostfix));
        P(AccessTools.Method(dc, "PlayCue"), prefix: nameof(PlayCuePrefix));
        P(AccessTools.Method(dc, "PlayCheck"), nameof(PlayCheckPrefix), nameof(PlayCheckPostfix));
        P(AccessTools.Method(dc, nameof(DialogController.Tick)), nameof(DialogTickPrefix), nameof(DialogTickPostfix));
        P(AccessTools.Method(dc, nameof(DialogController.StopDialog)), prefix: nameof(StopDialogPrefix));

        var mv = typeof(ModifiableValue);
        P(AccessTools.Method(mv, nameof(ModifiableValue.AddModifier), new[] { typeof(ModifiableValue.Modifier) }), postfix: nameof(AddModifierPostfix));
        P(AccessTools.Method(mv, nameof(ModifiableValue.RemoveModifier), new[] { typeof(ModifiableValue.Modifier) }), postfix: nameof(RemoveModifierPostfix));
        P(AccessTools.Method(typeof(ModifiableValue.TemporaryModifiersScope), "AddTemporaryModifiers"), postfix: nameof(ScopeAddPostfix));
        P(AccessTools.Method(typeof(ModifiableValue.TemporaryModifiersScope), "RemoveTemporaryModifiers"), prefix: nameof(ScopeRemovePrefix));
        // One patch call, so prefix, postfix and finalizer share __state.
        try
        {
            h.Patch(AccessTools.Method(mv, nameof(ModifiableValue.UpdateValue)), prefix: new HarmonyMethod(typeof(Hooks), nameof(UpdateValuePrefix)),
                postfix: new HarmonyMethod(typeof(Hooks), nameof(UpdateValuePostfix)), finalizer: new HarmonyMethod(typeof(Hooks), nameof(UpdateValueFinalizer)));
        }
        catch (Exception e) { Health.Failed("patch UpdateValue", e); }
        P(AccessTools.PropertySetter(typeof(TimeController), nameof(TimeController.DeltaTime)), prefix: nameof(DeltaSetterPrefix));

        P(AccessTools.Method(typeof(RulebookEvent), "AddModifier", new[] { typeof(Kingmaker.RuleSystem.Rules.Modifier) }), postfix: nameof(RuleBonusPostfix));
        P(AccessTools.Method(typeof(RulebookEvent), nameof(RulebookEvent.AddTemporaryModifier)), prefix: nameof(TempModifierPrefix));

        P(AccessTools.Method(typeof(EntityFactsManager), "DelegateOnFactDidAttach"), postfix: nameof(FactAttachPostfix));
        P(AccessTools.Method(typeof(EntityFactsManager), "DelegateOnFactWillDetach"), prefix: nameof(FactDetachPrefix));
        P(AccessTools.PropertySetter(typeof(UnitDescriptor), nameof(UnitDescriptor.Damage)), nameof(DamagePrefix), nameof(DamagePostfix));

        var uf = typeof(UnlockableFlagsManager);
        P(AccessTools.Method(uf, nameof(UnlockableFlagsManager.Lock)), postfix: nameof(FlagPostfix));
        P(AccessTools.Method(uf, nameof(UnlockableFlagsManager.Unlock)), postfix: nameof(FlagPostfix));
        P(AccessTools.Method(uf, nameof(UnlockableFlagsManager.SetFlagValue)), postfix: nameof(FlagPostfix));
        var es = typeof(EtudesSystem);
        P(AccessTools.Method(es, nameof(EtudesSystem.StartEtude)), postfix: nameof(EtudePostfix));
        P(AccessTools.Method(es, nameof(EtudesSystem.MarkEtudeCompleted)), postfix: nameof(EtudePostfix));
        P(AccessTools.Method(es, nameof(EtudesSystem.UnstartEtude)), postfix: nameof(EtudePostfix));
        P(AccessTools.Method(typeof(QuestObjective), "SetState"), postfix: nameof(ObjectivePostfix));
        P(AccessTools.Method(typeof(UnitProgressionData), nameof(UnitProgressionData.GainExperience)), postfix: nameof(XpPostfix));
        P(AccessTools.Method(typeof(UnitAlignment), nameof(UnitAlignment.Shift), new[] { typeof(AlignmentShiftDirection), typeof(int), typeof(IAlignmentShiftProvider) }), postfix: nameof(AlignmentPostfix));
    }
}

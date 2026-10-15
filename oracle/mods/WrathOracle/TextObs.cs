using System;
using System.Collections.Generic;
using System.Linq;
using HarmonyLib;
using Kingmaker.Controllers.Dialog;
using Kingmaker.DialogSystem.Blueprints;

namespace WrathOracle;

// The dialogue's text as the game builds it (declared as dialog_text):
//  - dlg_history: each line DialogController.AddHistoryEntry raises (IDialogHandler.HandleOnDialogHistory),
//    for the cue and then the answer SelectAnswer adds: which overload ("cue"/"answer"), its blueprint and
//    the line, taken from the first of the line's subscribers that receives it (the game log's handler, the
//    dialogue view model, the old dialogue UI). An answer with AddToHistory off raises nothing and writes
//    nothing; an entry whose line no subscriber received writes "line": null.
//  - dlg_text, right after each dlg_transition (Hooks.DialogTickPostfix): the current cue's DisplayText,
//    each shown answer's, and CurrentSpeakerName/CurrentSpeakerColor, read as the UI reads them when the
//    cue shows (nothing between PlayCue and the tick's end changes what they read).
//  - the begin record's dialog_speaker: DialogController's m_CustomSpeakerName and m_CustomSpeakerColor
//    when the window opens. Both live as long as the game process (Clear keeps them; a dialogue's start
//    sets the name, and the colour only when it starts with a speaker), so CurrentSpeakerColor for a cue
//    without a speaker can read a value from before the window.
// DisplayText is the LocalizedString's implicit conversion: the pack lookup and TextTemplateEngine.Process.
// It sets the string's ShouldProcess cache flag, which the UI's own read of the same text sets the same
// way, and the templates dialogue text uses read state without changing it.
internal static class TextObs
{
    internal static bool Installed;
    private static readonly AccessTools.FieldRef<DialogController, string> CustomName =
        AccessTools.FieldRefAccess<DialogController, string>("m_CustomSpeakerName");
    private static readonly AccessTools.FieldRef<DialogController, string> CustomColor =
        AccessTools.FieldRefAccess<DialogController, string>("m_CustomSpeakerColor");

    // Fixture.Begin's begin record.
    public static void WriteSpeakerState(Json j)
    {
        var dc = Kingmaker.Game.Instance?.DialogController;
        j.Obj();
        j.Prop("custom_name", dc != null ? CustomName(dc) : null);
        j.Prop("custom_color", dc != null ? CustomColor(dc) : null);
        j.Prop("controller", dc != null);
        j.End();
    }
    private static string _pendingKind, _pendingOf, _line;
    private static bool _pending, _seen;

    public static void Install(Harmony h)
    {
        var dc = typeof(DialogController);
        var cue = AccessTools.Method(dc, "AddHistoryEntry", new[] { typeof(BlueprintCue), typeof(string), typeof(string) })
            ?? throw new MissingMethodException("DialogController.AddHistoryEntry(BlueprintCue, string, string)");
        var answer = AccessTools.Method(dc, "AddHistoryEntry", new[] { typeof(BlueprintAnswer) })
            ?? throw new MissingMethodException("DialogController.AddHistoryEntry(BlueprintAnswer)");
        h.Patch(cue, prefix: new HarmonyMethod(typeof(TextObs), nameof(CueEntryPrefix)), postfix: new HarmonyMethod(typeof(TextObs), nameof(EntryPostfix)));
        h.Patch(answer, prefix: new HarmonyMethod(typeof(TextObs), nameof(AnswerEntryPrefix)), postfix: new HarmonyMethod(typeof(TextObs), nameof(EntryPostfix)));
        var subscribers = new[]
        {
            AccessTools.Method(AccessTools.Inner(typeof(Kingmaker.UI.Models.Log.Events.GameLogEventDialogHistory), "EventsHandler"), "HandleOnDialogHistory"),
            AccessTools.Method(typeof(Kingmaker.UI.MVVM._VM.Dialog.Dialog.DialogVM), "HandleOnDialogHistory"),
            AccessTools.Method(typeof(Kingmaker.UI.Dialog.DialogController), "HandleOnDialogHistory"),
        };
        foreach (var m in subscribers)
        {
            if (m == null) throw new MissingMethodException("a HandleOnDialogHistory subscriber");
            h.Patch(m, prefix: new HarmonyMethod(typeof(TextObs), nameof(LinePrefix)));
        }
        Installed = true;
    }

    public static void CueEntryPrefix(BlueprintCue cue) => Begin("cue", Snap.Guid(cue));

    public static void AnswerEntryPrefix(BlueprintAnswer answer)
    {
        // AddHistoryEntry(answer) raises a line only when the answer adds to the history.
        if (answer != null && answer.AddToHistory) Begin("answer", Snap.Guid(answer));
        else _pending = false;
    }

    private static void Begin(string kind, string of)
    {
        _pending = Installed && Trace.Active && Trace.OnMainThread;
        _pendingKind = kind;
        _pendingOf = of;
        _line = null;
        _seen = false;
    }

    public static void LinePrefix(string line)
    {
        if (!_pending || _seen) return;
        _line = line;
        _seen = true;
    }

    public static void EntryPostfix()
    {
        if (!_pending) return;
        _pending = false;
        if (!Trace.Active) return;
        var kind = _pendingKind; var of = _pendingOf; var line = _seen ? _line : null;
        try
        {
            Trace.Emit("dlg_history", j =>
            {
                j.Prop("kind", kind);
                j.Prop("of", of);
                j.Prop("line", line);
            });
        }
        catch (Exception e) { Trace.HookFailed("dlg_history", e); }
    }

    // Hooks.DialogTickPostfix, after its dlg_transition.
    public static void WriteText(DialogController dc)
    {
        if (!Installed || !Trace.Active) return;
        try
        {
            Trace.Emit("dlg_text", j =>
            {
                var cue = dc.CurrentCue;
                j.Prop("cue", Snap.Guid(cue));
                j.Prop("cue_text", cue?.DisplayText);
                j.Arr("answers");
                foreach (var a in dc.Answers.ToList())
                {
                    j.W.WriteStartObject();
                    j.Prop("answer", Snap.Guid(a));
                    j.Prop("text", a?.DisplayText);
                    j.W.WriteEndObject();
                }
                j.EndArr();
                j.Prop("speaker_name", dc.CurrentSpeakerName);
                j.Prop("speaker_color", dc.CurrentSpeakerColor);
            });
        }
        catch (Exception e) { Trace.HookFailed("dlg_text", e); }
    }
}

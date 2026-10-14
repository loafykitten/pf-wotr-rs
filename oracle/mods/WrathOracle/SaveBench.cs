using System;
using System.Collections;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Text.RegularExpressions;
using HarmonyLib;
using Kingmaker;
using Kingmaker.EntitySystem.Persistence;
using UnityEngine;

namespace WrathOracle;

// Timed manual save into a new slot named WR_<something> (never one of Ember's saves),
// then deletes that slot again through the game's SaveManager.DeleteSave.
// Times, from the frame Game.SaveGame is called (Stopwatch):
//   routine_end_s:  SaveManager.SaveRoutine finished (LoadingProcess idle again; the
//                   routine returns once the background commit has *started*);
//   commit_end_s:   ZipSaver.Save() returned on the background thread, i.e. the .zks is
//                   completely written (Harmony postfix);
//   total_s:        max of the two: the full save path including writing the .zks.
// Frame times over the save are recorded too (the save blocks the main thread in parts).
internal static class SaveBench
{
    private static readonly Regex OurSlot = new Regex(@"^Manual_\d+_WR_[A-Za-z0-9_]+\.zks$");

    public static IEnumerator Run(Plan plan, int index, Json j, Action<string> fail)
    {
        var game = Game.Instance;
        var sm = game.SaveManager;
        if (!sm.IsSaveAllowed(SaveInfo.SaveType.Manual))
        {
            fail("save: not allowed in the current state");
            yield break;
        }
        var info = sm.CreateNewSave($"{plan.SaveSlotName}_{index}");
        if (info.IsActuallySaved)
        {
            fail("save: CreateNewSave returned an existing save; refusing");
            yield break;
        }
        CommitProbe.Reset();
        WorkerProbe.Reset();
        SaveGuard.Allowed = info;
        var clock = FrameClock.Instance;
        clock.Arm();
        var sw = Stopwatch.StartNew();
        var startTs = Stopwatch.GetTimestamp();
        var frame0 = Time.frameCount;
        game.SaveGame(info);

        var lp = LoadingProcess.Instance;
        var started = false;
        while (sw.Elapsed.TotalSeconds < 5)
        {
            if (lp.IsLoadingInProcess) { started = true; break; }
            yield return null;
        }
        if (!started)
        {
            clock.Disarm();
            SaveGuard.Allowed = null;
            fail("save: the save never started (refused?)");
            yield break;
        }
        var routineDone = false;
        while (sw.Elapsed.TotalSeconds < 120)
        {
            if (!lp.IsLoadingInProcess) { routineDone = true; break; }
            yield return null;
        }
        var routineEnd = sw.Elapsed.TotalSeconds;
        var routineFrames = Time.frameCount - frame0;
        // The commit (ZipSaver.Save on the worker) and then the worker's bookkeeping: the
        // replacement SaveInfo returns to OperationState None and SaveManager to State.None.
        string path = null;
        var committed = false;
        var workerDone = false;
        SaveInfo listed = null;
        while (routineDone && sw.Elapsed.TotalSeconds < 120)
        {
            path = info.FolderName;
            if (path != null && !committed) committed = CommitProbe.CommittedAt(path, out _);
            if (committed)
            {
                listed = sm.FirstOrDefault(x => x.FolderName == path);
                // The worker task itself (SerializeAndSaveThread) must have completed: its
                // OperationState goes None before its last bookkeeping and write-scope release.
                if (listed != null && WorkerProbe.Completed && listed.OperationState == SaveInfo.StateType.None && sm.CurrentState == SaveManager.State.None)
                {
                    workerDone = true;
                    break;
                }
            }
            yield return null;
        }
        clock.Disarm();
        SaveGuard.Allowed = null;
        double commitEnd = -1;
        if (committed && CommitProbe.CommittedAt(path, out var ts)) commitEnd = (ts - startTs) / (double)Stopwatch.Frequency;
        var valid = routineDone && committed && workerDone;

        double maxFrame = 0;
        int over33 = 0, over50 = 0;
        for (var k = 1; k < clock.Count; k++)
        {
            var ms = clock.IntervalMs(k);
            if (ms > maxFrame) maxFrame = ms;
            if (ms >= 33.0) over33++;
            if (ms >= 50.0) over50++;
        }
        var existed = path != null && File.Exists(path);
        var size = existed ? new FileInfo(path).Length : -1;

        j.Obj();
        j.Prop("slot_name", info.Name);
        j.Prop("file", path != null ? Path.GetFileName(path) : null);
        j.Prop("file_bytes", size);
        j.Prop("routine_end_s", routineEnd);
        j.Prop("routine_frames", routineFrames);
        j.Prop("commit_end_s", commitEnd);
        j.Prop("valid", valid);
        j.Prop("routine_done", routineDone);
        j.Prop("committed", committed);
        j.Prop("worker_done", workerDone);
        if (valid) j.Prop("total_s", Math.Max(routineEnd, commitEnd)); else j.Null("total_s");
        j.Prop("frames_during_save", clock.Count);
        j.Prop("max_frame_ms", maxFrame);
        j.Prop("frames_over_33ms", over33);
        j.Prop("frames_over_50ms", over50);

        // Delete the slot we made, through the game, then check it's gone. Only once the
        // worker has finished with it; never anything that doesn't look like our slot.
        var deleted = false;
        string deleteNote = null;
        try
        {
            if (!workerDone)
            {
                deleteNote = "save did not finish (timeout); NOT deleted: " + path;
            }
            else if (path != null && OurSlot.IsMatch(Path.GetFileName(path)) && existed)
            {
                DeleteGuard.AllowedFile = Path.GetFileName(path);
                try { sm.DeleteSave(listed); }
                finally { DeleteGuard.AllowedFile = null; }
                if (File.Exists(path))
                {
                    File.Delete(path);
                    deleteNote = "deleted the file directly after DeleteSave left it";
                }
                deleted = !File.Exists(path);
            }
            else
            {
                deleteNote = $"not deleted: file {(existed ? "doesn't look like our slot" : "missing")}: {path}";
            }
        }
        catch (Exception e)
        {
            deleteNote = "delete failed: " + e.Message;
            Health.Failed("delete bench save", e);
        }
        j.Prop("deleted", deleted);
        j.Prop("delete_note", deleteNote);
        j.End();
        Main.Note($"save: {info.Name} -> {(path != null ? Path.GetFileName(path) : "?")}: routine {routineEnd:0.000} s, commit {commitEnd:0.000} s, max frame {maxFrame:0.0} ms; deleted={deleted} {deleteNote}");
        if (!valid) fail($"save: incomplete (routine {routineDone}, commit {committed}, worker {workerDone})");
        if (!deleted && existed) fail($"save: bench slot not deleted ({path}); delete it by hand");
    }
}

// While a plan runs, the only save allowed through SaveManager.SaveRoutine is the bench
// slot SaveBench registers; autosaves, quicksaves or scripted saves are skipped (an empty
// routine), so a live run can never overwrite one of Ember's slots.
[HarmonyPatch(typeof(SaveManager), nameof(SaveManager.SaveRoutine))]
internal static class SaveGuard
{
    public static volatile bool Active;
    public static SaveInfo Allowed;
    public static int Blocked;

    private static bool Prefix(SaveInfo saveInfo, ref System.Collections.Generic.IEnumerator<object> __result)
    {
        if (!Active || (saveInfo != null && ReferenceEquals(saveInfo, Allowed))) return true;
        Blocked++;
        Main.Note($"blocked a save request ({saveInfo?.Type} '{saveInfo?.Name}') while the oracle runs");
        __result = Enumerable.Empty<object>().GetEnumerator();
        return false;
    }
}

// While a plan runs, SaveManager.DeleteSave may only delete the bench slot SaveBench is
// deleting right now: the game itself can delete saves (e.g. GameOverIronmanController on an
// Ironman game over), which must never touch Ember's saves during a run. The name pattern
// alone isn't enough, since Ember's own fixture saves are named Manual_N_WR_* too.
[HarmonyPatch(typeof(SaveManager), nameof(SaveManager.DeleteSave))]
internal static class DeleteGuard
{
    private static readonly Regex OurSlot = new Regex(@"^Manual_\d+_WR_[A-Za-z0-9_]+\.zks$");
    public static int Blocked;
    public static string AllowedFile;

    private static bool Prefix(SaveInfo saveInfo)
    {
        if (!SaveGuard.Active) return true;
        var file = saveInfo?.FolderName != null ? Path.GetFileName(saveInfo.FolderName) : null;
        if (file != null && OurSlot.IsMatch(file) && file == AllowedFile) return true;
        Blocked++;
        Main.Note($"blocked deleting a save ('{file}') while the oracle runs");
        return false;
    }
}

// The save worker: SaveManager.SerializeAndSaveThread is async; its returned Task completes
// after the commit, bookkeeping and the write scope's release. Remember the latest one.
[HarmonyPatch(typeof(SaveManager), "SerializeAndSaveThread")]
internal static class WorkerProbe
{
    private static System.Threading.Tasks.Task _task;

    public static void Reset() => _task = null;

    public static bool Completed => _task != null && _task.IsCompleted;

    private static void Postfix(System.Threading.Tasks.Task __result) => _task = __result;
}

// ZipSaver.Save() is the commit that writes the .zks (SaveManager.SerializeAndSaveThread,
// on a worker thread). Record when it returns, per file.
[HarmonyPatch]
internal static class CommitProbe
{
    private static readonly object Lock = new object();
    private static string _path;
    private static long _ts;

    private static MethodBase TargetMethod() =>
        AccessTools.Method(AccessTools.TypeByName("Kingmaker.EntitySystem.Persistence.ZipSaver"), "Save");

    public static void Reset()
    {
        lock (Lock) { _path = null; _ts = 0; }
    }

    public static bool CommittedAt(string path, out long ts)
    {
        lock (Lock)
        {
            ts = _ts;
            return _path != null && _path == path;
        }
    }

    private static void Postfix(object __instance)
    {
        try
        {
            var ts = Stopwatch.GetTimestamp();
            var path = Traverse.Create(__instance).Property("FolderName").GetValue<string>();
            lock (Lock) { _path = path; _ts = ts; }
        }
        catch (Exception)
        {
            // never break saving
        }
    }
}

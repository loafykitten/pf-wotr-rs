using System;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Blueprints.Area;
using Kingmaker.EntitySystem.Persistence;

namespace WrathStats.Core;

// Labels loading spans with what started them. The recorder times the spans itself
// (LoadingProcess busy from start to finish); these prefixes only say why. They run
// before the game's code and never change its behaviour.
internal static class LoadReasons
{
    public const string LoadSave = "load_save";
    public const string AreaTransition = "area_transition";
    public const string Teleport = "teleport";
    public const string Save = "save";
    public const string NewGame = "new_game";
    public const string ToMainMenu = "to_main_menu";

    public static Action<string, string> OnReason; // (reason, saveType), set by the recorder

    // Entry points call each other within one frame (LoadNewGame -> LoadArea, and LoadArea
    // -> Teleport when the target is the current area). Within a frame the highest rank
    // labels the load.
    public static int Rank(string reason)
    {
        switch (reason)
        {
            case NewGame:
            case LoadSave:
            case ToMainMenu: return 3;
            case Teleport: return 2;
            case AreaTransition: return 1;
            default: return 0; // Save
        }
    }

    public static string SaveTypeName(SaveInfo.SaveType type)
    {
        switch (type)
        {
            case SaveInfo.SaveType.Manual: return "manual";
            case SaveInfo.SaveType.Quick: return "quick";
            case SaveInfo.SaveType.Auto: return "auto";
            case SaveInfo.SaveType.Remote: return "remote";
            case SaveInfo.SaveType.Bugreport: return "bugreport";
            case SaveInfo.SaveType.IronMan: return "ironman";
            case SaveInfo.SaveType.ForImport: return "for_import";
            default: return "other";
        }
    }

    internal static void Raise(string reason, string saveType = null)
    {
        try
        {
            OnReason?.Invoke(reason, saveType);
        }
        catch (Exception)
        {
            // Never let measurement break the game.
        }
    }
}

// Game.HandleQuit (called from Runner.OnApplicationQuit) starts a thread that kills the
// process about 20 ms later. Our own OnApplicationQuit may run after that, so the final
// flush and session-end note were lost on the arm64 player. Finish recording first.
internal static class QuitHook
{
    public static Action OnQuit; // set by the recorder while recording

    internal static void Raise()
    {
        try
        {
            OnQuit?.Invoke();
        }
        catch (Exception)
        {
            // Never let measurement break quitting.
        }
    }
}

[HarmonyPatch(typeof(Game), nameof(Game.HandleQuit))]
internal static class HandleQuitPatch
{
    private static void Prefix() => QuitHook.Raise();
}

[HarmonyPatch(typeof(Game), nameof(Game.LoadGame))]
internal static class LoadGamePatch
{
    private static void Prefix() => LoadReasons.Raise(LoadReasons.LoadSave);
}

[HarmonyPatch(typeof(Game), nameof(Game.LoadArea), typeof(BlueprintAreaEnterPoint), typeof(AutoSaveMode), typeof(Action))]
internal static class LoadAreaPatch
{
    private static void Prefix() => LoadReasons.Raise(LoadReasons.AreaTransition);
}

[HarmonyPatch(typeof(Game), nameof(Game.Teleport))]
internal static class TeleportPatch
{
    private static void Prefix() => LoadReasons.Raise(LoadReasons.Teleport);
}

[HarmonyPatch(typeof(Game), nameof(Game.SaveGame))]
internal static class SaveGamePatch
{
    private static void Prefix(SaveInfo saveInfo) =>
        LoadReasons.Raise(LoadReasons.Save, saveInfo != null ? LoadReasons.SaveTypeName(saveInfo.Type) : null);
}

[HarmonyPatch(typeof(Game), nameof(Game.LoadNewGame), typeof(BlueprintAreaPreset), typeof(SaveInfo))]
internal static class LoadNewGamePatch
{
    private static void Prefix() => LoadReasons.Raise(LoadReasons.NewGame);
}

[HarmonyPatch(typeof(Game), nameof(Game.ResetToMainMenu))]
internal static class ResetToMainMenuPatch
{
    private static void Prefix() => LoadReasons.Raise(LoadReasons.ToMainMenu);
}

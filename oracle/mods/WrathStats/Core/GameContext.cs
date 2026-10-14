using System;
using HarmonyLib;
using Kingmaker;
using Kingmaker.Blueprints.Area;
using Kingmaker.EntitySystem.Persistence;
using Kingmaker.GameModes;
using UnityEngine;

namespace WrathStats.Core;

// Reads game state for the recorder. UpdateFrame() runs every frame and must stay cheap
// and allocation-free; the rest is read once a second or on events. Never creates game
// singletons: Game.Instance and LoadingProcess.Instance both construct themselves on first
// access, so presence is checked without touching those getters.
internal sealed class GameContext
{
    private static readonly AccessTools.FieldRef<LoadingProcess> LoadingProcessInstance =
        AccessTools.StaticFieldRefAccess<LoadingProcess>(AccessTools.Field(typeof(LoadingProcess), "s_Instance"));

    public bool Loading;      // loading this frame or last frame
    public bool LoadingNow;   // a LoadingProcess is running right now
    public int LoadingTag = -1;
    public bool Menu;
    public bool Combat;
    public bool TurnBased;
    public bool Paused;
    public int ModeIndex = 255; // GameModeType.Index of the top game mode; 255 = no game
    public BlueprintArea Area;
    public string AreaName = "";
    public bool AreaChanged;  // set on the frame the loaded area changes
    public string PreviousAreaName = "";

    private bool _wasLoading;
    private BlueprintArea _lastArea;
    private Action<Exception> _onError;
    private bool _errorReported;

    public GameContext(Action<Exception> onError) => _onError = onError;

    public void UpdateFrame()
    {
        AreaChanged = false;
        try
        {
            if (!Game.HasInstance)
            {
                Reset(menu: true);
                return;
            }
            var game = Game.Instance;
            var lp = LoadingProcessInstance();
            LoadingNow = lp != null && lp.IsLoadingInProcess;
            LoadingTag = LoadingNow ? (int)lp.CurrentProcessTag : -1;
            Loading = LoadingNow || _wasLoading;
            _wasLoading = LoadingNow;

            var player = game.Player;
            Area = game.CurrentlyLoadedArea;
            Menu = !Loading && (player == null || Area == null);
            Combat = player != null && player.IsInCombat;
            // CombatController.IsInTurnBasedCombat() has a side effect, so mirror its check.
            TurnBased = Combat && player.IsTurnBasedModeOn() && game.CurrentMode != GameModeType.Cutscene;
            Paused = game.IsPaused;
            ModeIndex = game.CurrentMode?.Index ?? 255;

            // The area is briefly null while one unloads and the next loads; report only
            // arrivals in a real area, "from" being the last real one.
            if (!ReferenceEquals(Area, _lastArea))
            {
                _lastArea = Area;
                if (Area != null)
                {
                    PreviousAreaName = AreaName;
                    AreaName = Area.name ?? "";
                    AreaChanged = true;
                }
            }
        }
        catch (Exception e)
        {
            Reset(menu: false);
            ReportOnce(e);
        }
    }

    public string ModeName()
    {
        try
        {
            return Game.HasInstance ? Game.Instance.CurrentMode?.Name ?? "" : "";
        }
        catch (Exception e)
        {
            ReportOnce(e);
            return "";
        }
    }

    public void ReadSecond(ref SecondRecord r)
    {
        r.Area = AreaName;
        r.Mode = ModeName();
        r.Combat = Combat;
        r.TurnBased = TurnBased;
        r.Paused = Paused;
        r.TimeScale = Time.timeScale;
        try
        {
            if (!Game.HasInstance) return;
            var game = Game.Instance;
            r.AwakeUnits = game.State?.AwakeUnits?.Count ?? 0;
            // Not Player.Party: that getter rebuilds stale character lists and can repair
            // PartyCharacters. Reading the backing list is side-effect free.
            r.Party = game.Player?.PartyCharacters?.Count ?? 0;
            r.PlayerTimeScale = game.TimeController.PlayerTimeScale;
        }
        catch (Exception e)
        {
            ReportOnce(e);
        }
    }

    private void Reset(bool menu)
    {
        Loading = LoadingNow = _wasLoading = false;
        LoadingTag = -1;
        Menu = menu;
        Combat = TurnBased = Paused = false;
        ModeIndex = 255;
    }

    private void ReportOnce(Exception e)
    {
        if (_errorReported) return;
        _errorReported = true;
        _onError?.Invoke(e);
    }
}

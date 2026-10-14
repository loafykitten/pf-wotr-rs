using System;
using System.Linq;
using HarmonyLib;
using UnityEngine;
using UnityModManagerNet;
using WrathStats.Core;

namespace WrathStats;

public sealed class Settings : UnityModManager.ModSettings
{
    public bool Collect = true;
    public float HitchMs = 50f;
    public bool MarkerHotkey = true;

    public override void Save(UnityModManager.ModEntry modEntry) => Save(this, modEntry);
}

// UMM host for the recorder. Everything UMM-specific lives here; Core/ only needs Unity,
// Harmony and the game.
public static class Main
{
    private static UnityModManager.ModEntry _mod;
    private static Settings _settings;
    private static Harmony _harmony;
    private static Recorder _recorder;
    private static string _hitchText;

    public static bool Load(UnityModManager.ModEntry modEntry)
    {
        _mod = modEntry;
        _settings = UnityModManager.ModSettings.Load<Settings>(modEntry);
        _hitchText = _settings.HitchMs.ToString("0");
        modEntry.OnToggle = OnToggle;
        modEntry.OnGUI = OnGUI;
        modEntry.OnSaveGUI = entry => _settings.Save(entry);

        _harmony = new Harmony(modEntry.Info.Id);
        _harmony.PatchAll(typeof(Main).Assembly);

        var host = new GameObject("WrathStats");
        UnityEngine.Object.DontDestroyOnLoad(host);
        host.hideFlags = HideFlags.HideAndDontSave;
        _recorder = host.AddComponent<Recorder>();
        _recorder.Log = msg => modEntry.Logger.Log(msg);
        _recorder.LogError = msg => modEntry.Logger.Error(msg);
        return true;
    }

    private static bool OnToggle(UnityModManager.ModEntry modEntry, bool active)
    {
        if (active && _settings.Collect) Start();
        else _recorder.End();
        return true;
    }

    private static void Start()
    {
        try
        {
            _recorder.HitchMs = _settings.HitchMs;
            _recorder.MarkerHotkey = _settings.MarkerHotkey;
            var mods = UnityModManager.modEntries.Select(m => new ModInfo
            {
                Id = m.Info.Id,
                Version = m.Info.Version,
                Active = m.Active || m == _mod,
            }).ToList();
            _recorder.Begin(_mod.Info.Version, mods);
        }
        catch (Exception e)
        {
            _mod.Logger.Error($"could not start recording: {e}");
            _recorder.End();
        }
    }

    private static void OnGUI(UnityModManager.ModEntry modEntry)
    {
        var collect = GUILayout.Toggle(_settings.Collect, " Record performance stats");
        if (collect != _settings.Collect)
        {
            _settings.Collect = collect;
            if (collect && modEntry.Active) Start();
            else _recorder.End();
        }

        GUILayout.BeginHorizontal();
        GUILayout.Label("Log frames slower than (ms):", GUILayout.ExpandWidth(false));
        _hitchText = GUILayout.TextField(_hitchText, GUILayout.Width(60));
        if (float.TryParse(_hitchText, out var hitch) && hitch >= 1f) _settings.HitchMs = hitch;
        if (_recorder.Recording && Math.Abs(_settings.HitchMs - _recorder.HitchMs) > 0.01f)
            GUILayout.Label($"(this recording uses {_recorder.HitchMs:0} ms; the new value applies to the next one)");
        GUILayout.EndHorizontal();

        var hotkey = GUILayout.Toggle(_settings.MarkerHotkey, " Ctrl+Shift+M drops a marker into the log");
        if (hotkey != _settings.MarkerHotkey)
        {
            _settings.MarkerHotkey = hotkey;
            _recorder.MarkerHotkey = hotkey;
        }

        if (!_recorder.Recording)
        {
            GUILayout.Label("Not recording.");
            return;
        }

        GUILayout.Space(8);
        GUILayout.Label($"Last second: {_recorder.LastFps} frames, p50 {_recorder.LastP50:0.0} ms, " +
                        $"p95 {_recorder.LastP95:0.0} ms, max {_recorder.LastMax:0.0} ms");
        GUILayout.Label($"This session: {_recorder.Hitches} hitches, {_recorder.Loads} loads " +
                        $"(last {_recorder.LastLoadText()}), {_recorder.Markers} markers");
        GUILayout.Label($"Writing to: {_recorder.SessionDir}");
        GUILayout.BeginHorizontal();
        if (GUILayout.Button("Add marker", GUILayout.ExpandWidth(false))) _recorder.AddMarker();
        if (GUILayout.Button("Open folder", GUILayout.ExpandWidth(false))) Application.OpenURL("file://" + _recorder.SessionDir);
        GUILayout.EndHorizontal();
    }
}

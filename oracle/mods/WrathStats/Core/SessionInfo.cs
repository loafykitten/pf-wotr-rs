using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using Kingmaker;
using Kingmaker.GameModes;
using Kingmaker.Utility;
using UnityEngine;
using UnityEngine.Scripting;

namespace WrathStats.Core;

internal sealed class ModInfo
{
    public string Id;
    public string Version;
    public bool Active;
}

// Builds session.json once at session start. Allocation here is fine.
internal static class SessionInfo
{
    public static string Build(DateTime startUtc, string statsVersion, IEnumerable<ModInfo> mods, float hitchMs)
    {
        var j = new Json();
        j.Begin();
        j.Field("format", 2);
        j.Field("wrathstats_version", statsVersion);
        j.Field("start_utc", startUtc.ToString("o", CultureInfo.InvariantCulture));
        j.Field("hitch_threshold_ms", hitchMs);

        j.Field("game_version", Safe(GameVersion.GetVersion));
        j.Field("unity_version", Application.unityVersion);
        j.Field("data_path", Application.dataPath);
        j.Field("persistent_data_path", Safe(() => ApplicationPaths.persistentDataPath));

        j.Key("platform");
        j.Begin();
        var translated = NativeMac.ProcTranslated();
        var machine = NativeMac.SysctlString("hw.machine");
        // RuntimeInformation.ProcessArchitecture reports X64 even on Unity's native arm64
        // Mono, and hw.machine reports the process's view (x86_64 under Rosetta), so use that.
        j.Field("process_arch", machine == "arm64" ? "arm64" : machine == "x86_64" ? "x86_64" : machine);
        j.Field("proc_translated", translated);
        j.Field("rosetta", translated == 1);
        j.Field("hw_machine", machine);
        j.Field("cpu_brand", NativeMac.SysctlString("machdep.cpu.brand_string"));
        j.Field("hw_model", NativeMac.SysctlString("hw.model"));
        j.Field("os_version", NativeMac.SysctlString("kern.osproductversion"));
        j.Field("os", SystemInfo.operatingSystem);
        j.End();

        j.Key("system");
        j.Begin();
        j.Field("processor_type", SystemInfo.processorType);
        j.Field("processor_count", SystemInfo.processorCount);
        j.Field("system_memory_mb", SystemInfo.systemMemorySize);
        j.Field("graphics_device_name", SystemInfo.graphicsDeviceName);
        j.Field("graphics_device_type", SystemInfo.graphicsDeviceType.ToString());
        j.Field("graphics_device_version", SystemInfo.graphicsDeviceVersion);
        j.Field("graphics_memory_mb", SystemInfo.graphicsMemorySize);
        j.Field("graphics_multithreaded", SystemInfo.graphicsMultiThreaded);
        j.End();

        j.Key("display");
        j.Begin();
        var res = Screen.currentResolution;
        j.Field("screen_width", Screen.width);
        j.Field("screen_height", Screen.height);
        j.Field("current_resolution", $"{res.width}x{res.height}@{res.refreshRate}");
        j.Field("fullscreen_mode", Screen.fullScreenMode.ToString());
        j.Field("dpi", Screen.dpi);
        j.End();

        j.Key("quality");
        j.Begin();
        var level = QualitySettings.GetQualityLevel();
        j.Field("level", level);
        j.Field("level_name", level >= 0 && level < QualitySettings.names.Length ? QualitySettings.names[level] : null);
        j.Field("vsync_count", QualitySettings.vSyncCount);
        j.Field("target_frame_rate", Application.targetFrameRate);
        j.Field("anti_aliasing", QualitySettings.antiAliasing);
        j.Field("anisotropic_filtering", QualitySettings.anisotropicFiltering.ToString());
        j.Field("master_texture_limit", QualitySettings.masterTextureLimit);
        j.Field("shadows", QualitySettings.shadows.ToString());
        j.Field("shadow_resolution", QualitySettings.shadowResolution.ToString());
        j.Field("shadow_distance", QualitySettings.shadowDistance);
        j.Field("lod_bias", QualitySettings.lodBias);
        j.Field("maximum_lod_level", QualitySettings.maximumLODLevel);
        j.Field("pixel_light_count", QualitySettings.pixelLightCount);
        j.Field("soft_particles", QualitySettings.softParticles);
        j.Field("realtime_reflection_probes", QualitySettings.realtimeReflectionProbes);
        j.End();

        j.Key("gc");
        j.Begin();
        j.Field("mode", Safe(() => GarbageCollector.GCMode.ToString()));
        j.Field("incremental", GarbageCollector.isIncremental);
        j.Field("incremental_time_slice_ns", (long)GarbageCollector.incrementalTimeSliceNanoseconds);
        j.Field("max_generation", GC.MaxGeneration);
        j.End();

        // frames.bin stores GameModeType.Index per frame; this maps it back to names.
        j.Key("modes");
        j.Begin();
        foreach (var mode in GameModeType.All.OrderBy(m => m.Index))
            j.Field(mode.Index.ToString(CultureInfo.InvariantCulture), mode.Name);
        j.End();

        // On macOS Application.dataPath is the bundle's Contents/; boot.config is in Resources/Data.
        j.Field("boot_config", Safe(() =>
        {
            var mac = Path.Combine(Application.dataPath, "Resources", "Data", "boot.config");
            return File.ReadAllText(File.Exists(mac) ? mac : Path.Combine(Application.dataPath, "boot.config"));
        }));

        j.Key("pathviewer_installed");
        j.Raw(AppDomain.CurrentDomain.GetAssemblies().Any(a =>
            string.Equals(a.GetName().Name, "Pathviewer", StringComparison.OrdinalIgnoreCase)) ? "true" : "false");

        j.Key("mods");
        j.BeginArray();
        foreach (var m in mods)
        {
            j.Begin();
            j.Field("id", m.Id);
            j.Field("version", m.Version);
            j.Field("active", m.Active);
            j.End();
        }
        j.EndArray();

        j.End();
        return j.ToString();
    }

    private static string Safe(Func<string> f)
    {
        try
        {
            return f();
        }
        catch (Exception e)
        {
            return "error: " + e.GetType().Name;
        }
    }

    // Minimal pretty JSON writer for the header.
    private sealed class Json
    {
        private readonly StringBuilder _sb = new StringBuilder();
        private int _depth;
        private bool _needComma;

        public void Begin() => Open('{');
        public void End() => Close('}');
        public void BeginArray() => Open('[');
        public void EndArray() => Close(']');

        public void Key(string key)
        {
            Separator();
            Str(key);
            _sb.Append(": ");
            _needComma = false;
            _afterKey = true;
        }

        public void Field(string key, string value) { Key(key); Value(value == null ? "null" : Quote(value)); }
        public void Field(string key, bool value) { Key(key); Value(value ? "true" : "false"); }
        public void Field(string key, int value) { Key(key); Value(value.ToString(CultureInfo.InvariantCulture)); }
        public void Field(string key, long value) { Key(key); Value(value.ToString(CultureInfo.InvariantCulture)); }
        public void Field(string key, float value) { Key(key); Value(value.ToString("R", CultureInfo.InvariantCulture)); }
        public void Field(string key, int? value) { Key(key); Value(value.HasValue ? value.Value.ToString(CultureInfo.InvariantCulture) : "null"); }
        public void Raw(string json) => Value(json);

        public override string ToString() => _sb.Append('\n').ToString();

        private bool _afterKey;

        private void Open(char c)
        {
            Separator();
            _sb.Append(c);
            _depth++;
            _needComma = false;
            _afterKey = false;
        }

        private void Close(char c)
        {
            _depth--;
            _sb.Append('\n').Append(' ', _depth * 2).Append(c);
            _needComma = true;
        }

        private void Value(string json)
        {
            Separator();
            _sb.Append(json);
            _needComma = true;
        }

        private void Separator()
        {
            if (_afterKey)
            {
                _afterKey = false;
                return;
            }
            if (_needComma) _sb.Append(',');
            if (_depth > 0) _sb.Append('\n').Append(' ', _depth * 2);
        }

        private void Str(string s) => _sb.Append(Quote(s));

        private static string Quote(string s)
        {
            var sb = new StringBuilder(s.Length + 2).Append('"');
            foreach (var c in s)
            {
                switch (c)
                {
                    case '"': sb.Append("\\\""); break;
                    case '\\': sb.Append("\\\\"); break;
                    case '\n': sb.Append("\\n"); break;
                    case '\r': sb.Append("\\r"); break;
                    case '\t': sb.Append("\\t"); break;
                    default:
                        if (c < 0x20) sb.Append("\\u").Append(((int)c).ToString("x4"));
                        else sb.Append(c);
                        break;
                }
            }
            return sb.Append('"').ToString();
        }
    }
}

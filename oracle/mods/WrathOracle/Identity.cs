using System;
using System.IO;
using System.Linq;
using System.Reflection;
using Kingmaker;
using Kingmaker.QA;
using Kingmaker.Settings;
using Kingmaker.Utility;
using UnityEngine;
using UnityEngine.Rendering;
using UnityModManagerNet;

namespace WrathOracle;

// Build identity, recorded with every capture (Wrath-Rust PLAN: fixtures record the exact
// build). Hashes of the big files run once per run (blueprints pack 240 MB, zip 270 MB).
internal static class Identity
{
    private static readonly string[] Assemblies =
    {
        "Assembly-CSharp.dll", "Assembly-CSharp-firstpass.dll", "Owlcat.Runtime.Core.dll", "Owlcat.Runtime.Visual.dll",
        "Owlcat.Runtime.UI.dll", "Owlcat.SharedTypes.dll", "AstarPathfindingProject.dll", "DXTCompressor.dll",
        "Newtonsoft.Json.dll", "UnityEngine.CoreModule.dll", "0Harmony.dll", "UnityEngine.UIModule.dll",
    };

    public static void Write(Json j, Plan plan, string saveFile)
    {
        var contents = Application.dataPath;              // .../Wrath.app/Contents on macOS
        var appRoot = Path.GetDirectoryName(contents);     // .../Wrath.app
        var buildDir = Path.GetDirectoryName(appRoot);     // .../builds/arm64
        var managed = Path.Combine(contents, "Resources", "Data", "Managed");

        j.Obj("identity");
        j.Prop("oracle_version", Main.Mod.Info.Version);
        j.Prop("captured_utc", DateTime.UtcNow.ToString("o"));
        j.Prop("game_version", Safe(GameVersion.GetVersion));
        j.Prop("game_revision", Safe(() => GameRevision.Value));
        j.Prop("unity_version", Application.unityVersion);
        j.Prop("platform", Application.platform.ToString());
        j.Prop("app_path", appRoot);
        j.Prop("build_info", ReadText(Path.Combine(buildDir, "BUILD_INFO")));
        j.Prop("boot_config", ReadText(Path.Combine(contents, "Resources", "Data", "boot.config")));
        j.Prop("version_info", ReadText(Path.Combine(contents, "Resources", "Data", "StreamingAssets", "Version.info")));
        j.Prop("startup_json", ReadText(Path.Combine(appRoot, "startup.json")));

        j.Obj("sha256");
        foreach (var a in Assemblies)
        {
            var p = Path.Combine(managed, a);
            if (File.Exists(p)) j.Prop("Managed/" + a, Json.Sha256File(p));
        }
        j.Prop("Frameworks/UnityPlayer.dylib", Json.Sha256File(Path.Combine(contents, "Frameworks", "UnityPlayer.dylib")));
        j.Prop("Bundles/blueprints-pack.bbp", Json.Sha256File(Path.Combine(appRoot, "Bundles", "blueprints-pack.bbp")));
        j.Prop("blueprints.zip", Json.Sha256File(Path.Combine(appRoot, "blueprints.zip")));
        if (saveFile != null) j.Prop("save", Json.Sha256File(saveFile));
        j.End();
        j.Prop("save_file", saveFile);

        j.Arr("mods");
        foreach (var m in UnityModManager.modEntries)
        {
            j.Obj().Prop("id", m.Info.Id).Prop("version", m.Info.Version).Prop("active", m.Active || m == Main.Mod)
                .Prop("dll_sha256", Json.Sha256File(Path.Combine(m.Path, m.Info.AssemblyName ?? (m.Info.Id + ".dll")))).End();
        }
        j.EndArr();

        j.Obj("settings");
        WriteSettingsGroup(j, "difficulty", Safe(() => (object)SettingsRoot.Difficulty));
        WriteSettingsGroup(j, "graphics", Safe(() => (object)SettingsRoot.Graphics));
        j.End();

        j.Obj("display");
        j.Prop("screen_width", Screen.width);
        j.Prop("screen_height", Screen.height);
        j.Prop("current_resolution", Screen.currentResolution.ToString());
        j.Prop("refresh_rate", Screen.currentResolution.refreshRate);
        j.Prop("full_screen_mode", Screen.fullScreenMode.ToString());
        j.Prop("vsync_count", QualitySettings.vSyncCount);
        j.Prop("target_frame_rate", Application.targetFrameRate);
        j.Prop("quality_level", QualitySettings.GetQualityLevel());
        j.Prop("quality_name", QualitySettings.names[QualitySettings.GetQualityLevel()]);
        j.Prop("shadow_distance", QualitySettings.shadowDistance);
        j.Prop("lod_bias", QualitySettings.lodBias);
        j.Prop("anisotropic", QualitySettings.anisotropicFiltering.ToString());
        j.Prop("master_texture_limit", QualitySettings.masterTextureLimit);
        j.Prop("render_pipeline", UnityEngine.Rendering.GraphicsSettings.currentRenderPipeline != null ? UnityEngine.Rendering.GraphicsSettings.currentRenderPipeline.name : null);
        j.Prop("graphics_device", SystemInfo.graphicsDeviceName);
        j.Prop("graphics_api", SystemInfo.graphicsDeviceType.ToString());
        j.Prop("cpu", SystemInfo.processorType);
        j.Prop("system_memory_mb", SystemInfo.systemMemorySize);
        j.Prop("incremental_gc", UnityEngine.Scripting.GarbageCollector.isIncremental);
        j.End();

        j.Prop("plan", plan.RawJson);
        j.End();
    }

    // Settings groups hold SettingsEntity<T> fields with a Key and GetValue().
    private static void WriteSettingsGroup(Json j, string name, object group)
    {
        j.Obj(name);
        if (group != null)
        {
            foreach (var f in group.GetType().GetFields(BindingFlags.Instance | BindingFlags.Public))
            {
                try
                {
                    var e = f.GetValue(group);
                    if (!(e is ISettingsEntity)) continue;
                    var key = e.GetType().GetProperty("Key")?.GetValue(e) as string ?? f.Name;
                    var get = e.GetType().GetMethod("GetValue", Type.EmptyTypes);
                    j.Prop(key, get?.Invoke(e, null)?.ToString());
                }
                catch (Exception ex)
                {
                    Health.Failed("settings " + f.Name, ex);
                }
            }
        }
        j.End();
    }

    private static string ReadText(string path)
    {
        try { return File.Exists(path) ? File.ReadAllText(path) : null; } catch (Exception) { return null; }
    }

    private static T Safe<T>(Func<T> f) where T : class
    {
        try { return f(); } catch (Exception) { return null; }
    }
}
